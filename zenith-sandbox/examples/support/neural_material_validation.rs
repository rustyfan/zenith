use anyhow::{Result, ensure};
use std::io::Write;
use zenith::{
    core::log,
    renderer::NeuralMaterialRenderer,
    rendergraph::{RenderGraphBuilder, ResourceCache},
    rhi::{Access, Descriptors, Gpu, Instance, MemoryDomain, TextureDesc, vk},
};

pub fn run() -> Result<()> {
    log::initialize(log::LevelFilter::Info)?;
    let instance = Instance::new(&[], true)?;
    ensure!(
        instance.validation_enabled(),
        "neural material validation requires Vulkan validation layers"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    let descriptors = Descriptors::new(&gpu, 256, 16)?;
    let mut renderer = NeuralMaterialRenderer::new(&gpu)?;
    let backends: Vec<_> = renderer.backends().collect();
    let directory = std::path::PathBuf::from("target/neural-material");
    std::fs::create_dir_all(&directory)?;
    let mut report = std::fs::File::create(directory.join("validation.csv"))?;
    writeln!(
        report,
        "backend,width,height,cpu_rmse,reference_rmse,max_cpu_error,gpu_ms"
    )?;
    let mut cache = ResourceCache::default();
    for backend in backends {
        renderer.settings.backend = backend;
        for (width, height) in [(1u32, 1u32), (19, 7), (256, 256)] {
            let readback =
                gpu.allocate(width as u64 * height as u64 * 16, MemoryDomain::Readback)?;
            let mut graph = RenderGraphBuilder::new(&gpu, &descriptors, &mut cache)?;
            let output = renderer.reconstruct(&mut graph, width, height)?;
            let destination = graph.import_buffer(readback.clone());
            graph.pass(
                "readback",
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
                "host",
                vec![destination.read(Access::HOST_READ)],
                |_| Ok(()),
            )?;
            let (commands, timings) = graph.record_profiled(true)?;
            commands.submit()?.wait(10_000_000_000)?;
            let milliseconds = timings
                .unwrap()
                .read()?
                .into_iter()
                .find(|(name, _)| name.starts_with("neural_material_"))
                .unwrap()
                .1;
            let mut bytes = vec![0u8; (width * height * 16) as usize];
            readback.read(0, &mut bytes)?;
            let mut cpu_error = 0.0f64;
            let mut reference_error = 0.0f64;
            let mut maximum = 0.0f32;
            for (index, pixel) in bytes.chunks_exact(16).enumerate() {
                let uv = [
                    ((index as u32 % width) as f32 + 0.5) / width as f32,
                    ((index as u32 / width) as f32 + 0.5) / height as f32,
                ];
                let expected = renderer.evaluate(uv);
                let reference = NeuralMaterialRenderer::reference(uv);
                for channel in 0..4 {
                    let actual =
                        f32::from_le_bytes(pixel[channel * 4..channel * 4 + 4].try_into()?);
                    ensure!(
                        actual.is_finite() && (0.0..=1.0).contains(&actual),
                        "invalid output at {index}:{channel}"
                    );
                    let error = (actual - expected[channel]).abs();
                    maximum = maximum.max(error);
                    cpu_error += (error as f64).powi(2);
                    reference_error += ((actual - reference[channel]) as f64).powi(2);
                }
            }
            let samples = f64::from(width * height * 4);
            let cpu_rmse = (cpu_error / samples).sqrt();
            let reference_rmse = (reference_error / samples).sqrt();
            println!(
                "{} {width}x{height}: CPU RMSE {cpu_rmse:.6}, max {maximum:.6}, reference RMSE {reference_rmse:.6}, {milliseconds:.4} ms",
                backend.label()
            );
            writeln!(
                report,
                "{backend:?},{width},{height},{cpu_rmse},{reference_rmse},{maximum},{milliseconds}"
            )?;
            ensure!(
                cpu_rmse < 0.003 && maximum < 0.025,
                "{} differs from CPU reference",
                backend.label()
            );
            ensure!(
                reference_rmse < 0.035,
                "material reconstruction quality regressed"
            );
        }
        let (width, height) = (1280, 720);
        let readback = gpu.allocate(width as u64 * height as u64 * 4, MemoryDomain::Readback)?;
        let mut graph = RenderGraphBuilder::new(&gpu, &descriptors, &mut cache)?;
        let mut desc = TextureDesc::color(width, height, vk::Format::R8G8B8A8_UNORM);
        desc.usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC;
        let image = graph.create_image(desc)?;
        renderer.render(&mut graph, image)?;
        let destination = graph.import_buffer(readback.clone());
        graph.pass(
            "capture",
            vec![
                image.read(Access::COPY_READ),
                destination.write(Access::COPY_WRITE),
            ],
            move |ctx| {
                let texture = ctx.image(image)?;
                let buffer = ctx.buffer(destination)?;
                unsafe {
                    ctx.commands.read_image(
                        &texture,
                        &buffer,
                        &[vk::BufferImageCopy::default()
                            .image_subresource(
                                vk::ImageSubresourceLayers::default()
                                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                                    .layer_count(1),
                            )
                            .image_extent(vk::Extent3D {
                                width,
                                height,
                                depth: 1,
                            })],
                    )
                }
            },
        )?;
        graph.pass(
            "capture_host",
            vec![destination.read(Access::HOST_READ)],
            |_| Ok(()),
        )?;
        graph.record()?.submit()?.wait(10_000_000_000)?;
        let mut pixels = vec![0; (width * height * 4) as usize];
        readback.read(0, &mut pixels)?;
        save_bmp(
            &directory.join(format!("{backend:?}.bmp")),
            width,
            height,
            pixels,
        )?;
    }
    drop(renderer);
    drop(cache);
    drop(descriptors);
    drop(gpu);
    ensure!(
        instance.validation_errors().is_empty(),
        "Vulkan validation errors: {:?}",
        instance.validation_errors()
    );
    println!(
        "Neural material validation passed. Captures: {}",
        directory.display()
    );
    Ok(())
}

fn save_bmp(path: &std::path::Path, width: u32, height: u32, mut pixels: Vec<u8>) -> Result<()> {
    let mut header = [0u8; 54];
    header[..2].copy_from_slice(b"BM");
    header[2..6].copy_from_slice(&(54 + pixels.len() as u32).to_le_bytes());
    header[10..14].copy_from_slice(&54u32.to_le_bytes());
    header[14..18].copy_from_slice(&40u32.to_le_bytes());
    header[18..22].copy_from_slice(&width.to_le_bytes());
    header[22..26].copy_from_slice(&(-(height as i32)).to_le_bytes());
    header[26..28].copy_from_slice(&1u16.to_le_bytes());
    header[28..30].copy_from_slice(&32u16.to_le_bytes());
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let mut file = std::fs::File::create(path)?;
    file.write_all(&header)?;
    file.write_all(&pixels)?;
    Ok(())
}
