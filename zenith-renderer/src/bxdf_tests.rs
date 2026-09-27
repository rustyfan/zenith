use anyhow::{ensure, Result};
use bytemuck::{Pod, Zeroable};
use std::f64::consts::PI;
use zenith_rendergraph::{RenderGraphBuilder, ResourceCache};
use zenith_rhi::*;

fn fresnel(c: f64, eta: f64) -> f64 {
    let t = (1.0 - (1.0 - c * c) / (eta * eta)).max(0.0).sqrt();
    0.5 * (((c - eta * t) / (c + eta * t)).powi(2) + ((eta * c - t) / (eta * c + t)).powi(2))
}

fn longitudinal(ti: f64, to: f64, beta: f64, gaussian: bool) -> f64 {
    let variance = (beta * beta).max(0.0001);
    if gaussian {
        return (-((ti + to) * 0.5).powi(2) / (2.0 * variance)).exp()
            / ((2.0 * PI * variance).sqrt() * ((to - ti) * 0.5).cos().powi(2).max(1e-6));
    }
    // Independent numerical integration of the spherical Gaussian around the cone.
    let a = ti.cos().abs() * to.cos().abs() / variance;
    let b = ti.sin() * to.sin() / variance;
    let count = 512;
    let sum: f64 = (0..count)
        .map(|i| {
            let phi = PI * (i as f64 + 0.5) / count as f64;
            (a * phi.cos() - b - 1.0 / variance).exp()
        })
        .sum();
    sum / count as f64 / (variance * (1.0 - (-2.0 / variance).exp()))
}

fn gaussian_detector(phi: f64, beta: f64) -> f64 {
    let phi = (phi + PI).rem_euclid(2.0 * PI) - PI;
    (-2..=2)
        .map(|k| (-(phi + k as f64 * 2.0 * PI).powi(2) / (2.0 * beta * beta)).exp())
        .sum::<f64>()
        / (2.0 * PI).sqrt()
        / beta
}

fn hair_reference(h: [f32; 8], ti: f64, to: f64, phi: f64, gaussian: bool) -> [f64; 3] {
    let h = h.map(f64::from);
    let td = (to - ti) * 0.5;
    let cd = td.cos().max(1e-4);
    let eta_prime = (h[6] * h[6] - td.sin().powi(2)).sqrt() / cd;
    let cos_t = (1.0 - (td.sin() / h[6]).powi(2)).sqrt();
    let mut azimuth = [[0.0; 3]; 4];
    let count = 4096;
    for j in 0..count {
        let offset = 2.0 * (j as f64 + 0.5) / count as f64 - 1.0;
        let gi = offset.asin();
        let gt = (offset / eta_prime).asin();
        let f = fresnel(cd * gi.cos(), h[6]);
        for channel in 0..3 {
            let transmission = (-h[channel] * 2.0 * gt.cos() / cos_t).exp();
            let mut attenuation = f;
            for p in 0..3 {
                let shift = 2.0 * p as f64 * gt - 2.0 * gi + p as f64 * PI;
                azimuth[p][channel] +=
                    attenuation * gaussian_detector(phi - shift, h[4]) / count as f64;
                attenuation = if p == 0 {
                    (1.0 - f).powi(2) * transmission
                } else {
                    attenuation * f * transmission
                };
            }
            if !gaussian {
                azimuth[3][channel] +=
                    attenuation / (1.0 - f * transmission).max(1e-6) / (2.0 * PI) / count as f64;
            }
        }
    }
    let shifts = [-2.0 * h[5], h[5], 4.0 * h[5], 0.0];
    let widths = [h[3], h[3] * 0.5, h[3] * 2.0, h[3] * 2.0];
    std::array::from_fn(|c| {
        (0..4)
            .map(|p| azimuth[p][c] * longitudinal(ti, to - shifts[p], widths[p], gaussian))
            .sum()
    })
}

fn coat_reference(nv: f64, nl: f64, roughness: f64) -> f64 {
    let vh = (0.5 * (1.0 + nv * nl)).sqrt();
    let nh = (nv + nl) / (2.0 + 2.0 * nv * nl).sqrt();
    let a2 = roughness.powi(4);
    let d = a2 / (PI * (1.0 + nh * nh * (a2 - 1.0)).powi(2));
    let visibility =
        0.5 / (nl * (nv * nv * (1.0 - a2) + a2).sqrt() + nv * (nl * nl * (1.0 - a2) + a2).sqrt());
    (0.04 + 0.96 * (1.0 - vh).powi(5)) * d * visibility
}

#[test]
fn corrected_longitudinal_lobe_is_normalized_and_reciprocal() {
    for beta in [0.03, 0.1, 0.3, 0.7, 1.0] {
        for to in [-1.5, -0.7, 0.0, 0.7, 1.5] {
            let count = 8192;
            let integral: f64 = (0..count)
                .map(|i| {
                    let ti = (2.0 * (i as f64 + 0.5) / count as f64 - 1.0).asin();
                    longitudinal(ti, to, beta, false) * 2.0 / count as f64
                })
                .sum();
            assert!(
                (integral - 1.0).abs() < 0.003,
                "beta={beta} to={to} energy={integral}"
            );
            assert!(
                (longitudinal(0.3, to, beta, false) - longitudinal(to, 0.3, beta, false)).abs()
                    < 1e-10
            );
        }
    }
}

#[test]
fn hair_reference_wraps_azimuth_and_absorption_reduces_transmission() {
    let h = [0.0, 0.0, 0.0, 0.3, 0.35, 0.0, 1.55, 1.0];
    let a = hair_reference(h, 0.2, -0.2, PI - 1e-7, false);
    let b = hair_reference(h, 0.2, -0.2, -PI - 1e-7, false);
    for c in 0..3 {
        assert!((a[c] - b[c]).abs() < 1e-7);
    }
    let absorbing = hair_reference(
        [2.0, 3.0, 4.0, 0.3, 0.35, 0.0, 1.55, 1.0],
        0.2,
        -0.2,
        PI,
        false,
    );
    assert!(absorbing.iter().zip(a).all(|(x, y)| *x < y));
    let reciprocal = hair_reference(h, -0.2, 0.2, -PI + 1e-7, false);
    for c in 0..3 {
        assert!((a[c] - reciprocal[c]).abs() < 1e-7);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Case {
    hair: [f32; 8],
    angles: [f32; 4],
    surface: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    cases: GpuAddress,
    output: GpuAddress,
    count: u32,
    padding: u32,
}

#[test]
#[ignore = "requires Vulkan validation, Slang, and supported GPU features"]
fn bxdf_gpu_matches_independent_cpu_reference() -> Result<()> {
    std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
    let instance = Instance::new(&[], true)?;
    ensure!(
        instance.validation_enabled(),
        "Vulkan validation is required"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    {
        let descriptors = Descriptors::new(&gpu, 256, 16)?;
        let shader = gpu.compile_shader(
            "content/shaders/bxdf_validation.slang",
            "main",
            ShaderStage::Compute,
        )?;
        let pipeline = gpu.compute(&shader)?;
        let mut cases = Vec::new();
        for model in [0.0, 1.0] {
            for beta in [0.15, 0.3, 0.7] {
                for (ti, to, phi) in [
                    (0.0, 0.0, 0.0),
                    (0.2, -0.4, 3.14),
                    (-1.2, 1.1, -0.7),
                    (1.5, -1.4, 0.9),
                ] {
                    cases.push(Case {
                        hair: [0.1, 0.3, 0.8, beta, 0.3, 0.035, 1.55, 1.0],
                        angles: [ti, to, phi, model],
                        surface: [0.2 + beta, beta, beta, 0.0],
                    });
                }
            }
        }
        let input = gpu.allocate(
            std::mem::size_of_val(cases.as_slice()) as u64,
            MemoryDomain::Upload,
        )?;
        input.write(0, bytemuck::cast_slice(&cases))?;
        let readback = gpu.allocate(cases.len() as u64 * 16, MemoryDomain::Readback)?;
        let mut cache = ResourceCache::default();
        let mut graph = RenderGraphBuilder::new(&gpu, &descriptors, &mut cache)?;
        let input_id = graph.import_buffer(input);
        let output = graph.create_buffer(readback.size(), MemoryDomain::Device)?;
        let count = cases.len() as u32;
        graph.pass(
            "bxdf_reference",
            vec![
                input_id.read(Access::COMPUTE_READ),
                output.write(Access::COMPUTE_WRITE),
            ],
            move |ctx| {
                let input = ctx.buffer(input_id)?;
                let output = ctx.buffer(output)?;
                let root = ctx.arguments(&Root {
                    cases: input.address(),
                    output: output.address(),
                    count,
                    padding: 0,
                })?;
                unsafe {
                    ctx.commands.dispatch(
                        &pipeline,
                        &root,
                        [count.div_ceil(64), 1, 1],
                        &[input, output],
                    )
                }
            },
        )?;
        let destination = graph.import_buffer(readback.clone());
        graph.pass(
            "read_bxdf",
            vec![
                output.read(Access::COPY_READ),
                destination.write(Access::COPY_WRITE),
            ],
            move |ctx| {
                ctx.commands
                    .copy(&ctx.buffer(output)?, &ctx.buffer(destination)?)
            },
        )?;
        graph.pass(
            "host_bxdf",
            vec![destination.read(Access::HOST_READ)],
            |_| Ok(()),
        )?;
        graph.record()?.submit()?.wait(10_000_000_000)?;
        let mut bytes = vec![0; cases.len() * 16];
        readback.read(0, &mut bytes)?;
        for (index, (case, pixel)) in cases.iter().zip(bytes.chunks_exact(16)).enumerate() {
            let hair = hair_reference(
                case.hair,
                case.angles[0] as f64,
                case.angles[1] as f64,
                case.angles[2] as f64,
                case.angles[3] != 0.0,
            );
            let expected = [
                hair[0],
                hair[1],
                hair[2],
                coat_reference(
                    case.surface[0] as f64,
                    case.surface[1] as f64,
                    case.surface[2] as f64,
                ),
            ];
            for c in 0..4 {
                let actual = f32::from_le_bytes(pixel[c * 4..c * 4 + 4].try_into()?) as f64;
                ensure!(
                    actual.is_finite() && actual >= 0.0,
                    "nonfinite/negative BxDF at {index}:{c}"
                );
                ensure!(
                    (actual - expected[c]).abs() < 0.0001 + 0.015 * expected[c],
                    "case {index}:{c}: GPU={actual} CPU={}",
                    expected[c]
                );
            }
        }
    }
    drop(gpu);
    ensure!(
        instance.validation_errors().is_empty(),
        "{:?}",
        instance.validation_errors()
    );
    Ok(())
}
