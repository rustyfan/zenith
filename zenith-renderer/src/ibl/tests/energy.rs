use super::*;

fn lookup(row: &[f32], mu: f64, channel: usize) -> f64 {
    let x = (mu.sqrt() * (row.len() / 2) as f64 - 0.5).clamp(0.0, (row.len() / 2 - 1) as f64);
    let lower = x.floor() as usize;
    let upper = (lower + 1).min(row.len() / 2 - 1);
    let fraction = x.fract();
    f64::from(row[lower * 2 + channel]) * (1.0 - fraction)
        + f64::from(row[upper * 2 + channel]) * fraction
}

pub(super) fn check_tables(directional: &[f32], average: &[f32]) {
    let size = BRDF_LUT_SIZE as usize;
    assert_eq!(average.len(), size * 2);
    let mut maximum = 0.0f32;
    for (y, row) in directional.chunks_exact(size * 2).enumerate() {
        for sample in row.chunks_exact(2) {
            maximum = maximum.max(sample[0] + sample[1]);
            assert!(sample.iter().all(|x| x.is_finite() && *x >= 0.0));
            assert!(
                sample[0] + sample[1] <= 1.000001,
                "LUT energy at row {y}: {sample:?}"
            );
        }
        for channel in 0..2 {
            let expected: f64 = (0..4096)
                .map(|i| {
                    let mu = (i as f64 + 0.5) / 4096.0;
                    2.0 * mu * lookup(row, mu, channel) / 4096.0
                })
                .sum();
            assert!((f64::from(average[y * 2 + channel]) - expected).abs() < 2e-6);
        }
    }
    let endpoint = &directional[directional.len() - 2..];
    assert!((endpoint[0] + endpoint[1] - (1.0 - 2.0f32.ln())).abs() < 0.006);
    zenith_core::log::info!("BRDF LUT maximum directional albedo: {maximum:.6}");
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct Case {
    material: [f32; 4],
    geometry: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    cases: GpuAddress,
    output: GpuAddress,
    directional: u32,
    average: u32,
    sampler: u32,
    samples: u32,
}

#[test]
#[ignore = "requires Vulkan validation, Slang, and supported GPU features"]
fn microfacet_energy_is_conserved_and_reciprocal() -> Result<()> {
    std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
    let _ = zenith_core::log::initialize(zenith_core::log::LevelFilter::Info);
    let instance = Instance::new(&[], true)?;
    anyhow::ensure!(
        instance.validation_enabled(),
        "BRDF test requires Vulkan validation"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    {
        let descriptors = Descriptors::new(&gpu, 512, 32)?;
        let mut ibl = create_ibl(&gpu, &descriptors)?;
        let mut cases = Vec::new();
        for material in [
            [1.0, 1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0, 0.0],
            [0.95, 0.64, 0.54, 1.0],
            [1.0, 0.0, 0.0, 0.5],
            [0.0, 0.0, 0.0, 1.0],
            [0.5, 0.5, 0.5, 0.0],
        ] {
            for roughness in [0.0, 0.1, 0.25, 0.35, 0.5, 0.75, 0.9, 1.0] {
                for nov in [0.001, 0.00390625, 0.02, 0.1, 0.5, 1.0] {
                    cases.push(Case {
                        material,
                        geometry: [roughness, nov, 0.37, 2.4],
                    });
                }
            }
        }
        for geometry in [
            [0.0, 0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, std::f32::consts::PI],
        ] {
            cases.push(Case {
                material: [1.0; 4],
                geometry,
            });
        }
        let count = cases.len();
        let input = gpu.allocate(
            std::mem::size_of_val(cases.as_slice()) as u64,
            MemoryDomain::Upload,
        )?;
        input.write(0, bytemuck::cast_slice(&cases))?;
        let readback = gpu.allocate((count * 20 * 4) as u64, MemoryDomain::Readback)?;
        let shader = gpu.compile_shader(
            "content/shaders/brdf_validation.slang",
            "main",
            ShaderStage::Compute,
        )?;
        let pipeline = gpu.compute(&shader)?;
        let mut cache = ResourceCache::default();
        let mut builder = RenderGraphBuilder::new(&gpu, &descriptors, &mut cache)?;
        let resources = ibl.render(&mut builder)?;
        let input = builder.import_buffer(input);
        let output = builder.create_buffer(readback.size(), MemoryDomain::Device)?;
        let sampler = ibl.sampler.clone();
        builder.pass(
            "integrate_microfacet_test",
            vec![
                input.read(Access::COMPUTE_READ),
                output.write(Access::COMPUTE_WRITE),
                resources.brdf_lut.read(Access::COMPUTE_READ),
                resources.brdf_average.read(Access::COMPUTE_READ),
            ],
            move |ctx| {
                let input = ctx.buffer(input)?;
                let output = ctx.buffer(output)?;
                let data = Root {
                    cases: input.address(),
                    output: output.address(),
                    directional: ctx.sampled(resources.brdf_lut)?,
                    average: ctx.sampled(resources.brdf_average)?,
                    sampler: ctx.sampler(&sampler)?,
                    samples: 262144,
                };
                let root = ctx.arguments(&data)?;
                unsafe {
                    ctx.commands
                        .dispatch(&pipeline, &root, [count as u32, 1, 1], &[input, output])
                }
            },
        )?;
        let destination = builder.import_buffer(readback.clone());
        builder.pass(
            "read_microfacet_test",
            vec![
                output.read(Access::COPY_READ),
                destination.write(Access::COPY_WRITE),
            ],
            move |ctx| {
                ctx.commands
                    .copy(&ctx.buffer(output)?, &ctx.buffer(destination)?)
            },
        )?;
        builder.record()?.submit()?.wait(60_000_000_000)?;
        let mut result = vec![0.0f32; count * 20];
        readback.read(0, bytemuck::cast_slice_mut(&mut result))?;
        let mut maximum_furnace_error = 0.0f32;
        let mut maximum_ibl_error = 0.0f32;
        for (case, values) in cases.iter().zip(result.chunks_exact(20)) {
            assert!(
                values.iter().all(|x| x.is_finite() && *x >= 0.0),
                "{case:?}: {values:?}"
            );
            for c in 0..3 {
                assert!(
                    (values[8 + c] - values[12 + c]).abs() <= 1e-5 * values[8 + c].max(1.0),
                    "reciprocity: {case:?}: {values:?}"
                );
                if case.geometry[1] == 0.0 || case.geometry[2] == 0.0 {
                    assert_eq!(values[8 + c], 0.0);
                }
                if case.geometry[1] == 0.0 {
                    continue;
                }
                assert!(values[c] <= 1.01, "excess energy: {case:?}: {values:?}");
                assert!(
                    values[4 + c] <= 1.00001,
                    "excess IBL energy: {case:?}: {values:?}"
                );
                assert!(
                    (values[c] - values[4 + c]).abs() < 0.01,
                    "direct/IBL integral: {case:?}: {values:?}"
                );
                if case.material == [0.95, 0.64, 0.54, 1.0] && case.geometry[..2] == [1.0, 1.0] {
                    // Independent alpha=1 quadrature, Eavg = 4/3 * (1 - ln(2)), with the squared Fresnel correction.
                    let expected = [0.8797628, 0.3965970, 0.2997605][c];
                    assert!(
                        (values[c] - expected).abs() < 0.01,
                        "colored multiple scattering: {case:?}: {values:?}"
                    );
                }
                if case.material[..3] == [1.0; 3] {
                    maximum_furnace_error = maximum_furnace_error.max((values[c] - 1.0).abs());
                    maximum_ibl_error = maximum_ibl_error.max((values[4 + c] - 1.0).abs());
                    assert!(
                        (values[c] - 1.0).abs() < 0.01,
                        "furnace: {case:?}: {values:?}"
                    );
                    assert!(
                        (values[4 + c] - 1.0).abs() < 1e-5,
                        "IBL furnace: {case:?}: {values:?}"
                    );
                }
            }
        }
        zenith_core::log::info!("{} BRDF cases: max furnace error {maximum_furnace_error:.6}, IBL {maximum_ibl_error:.6}", cases.len());
    }
    gpu.wait_idle()?;
    drop(gpu);
    anyhow::ensure!(
        instance.validation_errors().is_empty(),
        "BRDF validation: {:?}",
        instance.validation_errors()
    );
    Ok(())
}
