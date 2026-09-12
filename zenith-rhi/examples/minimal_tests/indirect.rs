use ash::vk;
use std::sync::Arc;
use zenith_core::log;
use zenith_rhi::*;

pub fn run(gpu: &Arc<Gpu>) -> anyhow::Result<()> {
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    #[repr(C)]
    struct Draw {
        offset: f32,
        tint: [f32; 4],
    }
    let vertices = gpu.allocate(32, MemoryDomain::Upload)?;
    vertices.write(
        0,
        bytemuck::cast_slice(&[[-1.0f32, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]]),
    )?;
    let indices = gpu.allocate(24, MemoryDomain::Upload)?;
    indices.write(0, bytemuck::cast_slice(&[0u32, 1, 2, 0, 2, 3]))?;
    let draws = gpu.allocate(40, MemoryDomain::Upload)?;
    draws.write(
        0,
        bytemuck::cast_slice(&[
            Draw {
                offset: -0.5,
                tint: [1.0, 0.0, 0.0, 1.0],
            },
            Draw {
                offset: 0.5,
                tint: [0.0, 1.0, 0.0, 1.0],
            },
        ]),
    )?;
    let records = gpu.allocate(40, MemoryDomain::Device)?;
    let count = gpu.allocate(4, MemoryDomain::Device)?;
    let mut arguments = Arguments::new(gpu, 2048)?;
    let generator = arguments.push(&[records.address().value(), count.address().value()])?;
    let root = arguments.push(&[vertices.address().value(), draws.address().value()])?;
    let generate = gpu.compile_shader(
        "zenith-rhi/tests/shaders/generate_indirect.slang",
        "generate",
        ShaderStage::Compute,
    )?;
    let generator_pipeline = gpu.compute_specialized(&generate, &[(0, 2)])?;
    anyhow::ensure!(
        Arc::ptr_eq(
            &generator_pipeline,
            &gpu.compute_specialized(&generate, &[(0, 2)])?
        ),
        "pipeline cache missed"
    );
    anyhow::ensure!(
        !Arc::ptr_eq(&generator_pipeline, &gpu.compute(&generate)?),
        "specialization did not separate pipeline variants"
    );
    let vertex = gpu.compile_shader(
        "zenith-rhi/tests/shaders/indirect.slang",
        "vertexMain",
        ShaderStage::Vertex,
    )?;
    let fragment = gpu.compile_shader(
        "zenith-rhi/tests/shaders/indirect.slang",
        "fragmentMain",
        ShaderStage::Fragment,
    )?;
    let desc = RasterDesc {
        vertex: &vertex,
        fragment: &fragment,
        colors: &[vk::Format::R8G8B8A8_UNORM],
        depth: vk::Format::UNDEFINED,
        stencil: vk::Format::UNDEFINED,
        samples: vk::SampleCountFlags::TYPE_1,
        topology: vk::PrimitiveTopology::TRIANGLE_LIST,
        dynamic_blend: false,
        blend: &[Blend::default()],
    };
    let pipeline = gpu.raster(&desc)?;
    anyhow::ensure!(
        Arc::ptr_eq(&pipeline, &gpu.raster(&desc)?),
        "raster pipeline cache missed"
    );
    let image = gpu.texture(TextureDesc::color(8, 8, vk::Format::R8G8B8A8_UNORM))?;
    let view = image.full_view()?;
    let readback = gpu.allocate(256, MemoryDomain::Readback)?;
    for (generated, max) in [(0, 2), (1, 2), (2, 0), (2, 1), (2, 2)] {
        let generator_pipeline = gpu.compute_specialized(&generate, &[(0, generated)])?;
        let mut commands = gpu.commands()?;
        unsafe {
            commands.initialize(&image)?;
            commands.dispatch(
                &generator_pipeline,
                &generator,
                [1, 1, 1],
                &[records.whole(), count.whole()],
            )?;
        }
        let dependency = commands.signal_dependency(Access::COMPUTE_WRITE, Access::INDIRECT)?;
        let extra_root = arguments.push(&[1u64, 2])?;
        anyhow::ensure!(
            extra_root.address().value() > root.address().value(),
            "argument arena overwrote live data"
        );
        commands.wait_dependency(dependency)?;
        commands.begin_rendering(
            &[Attachment {
                view: &view,
                clear: Some([0.0; 4]),
                store: true,
                resolve: None,
            }],
            None,
            vk::Extent2D {
                width: 8,
                height: 8,
            },
        )?;
        unsafe {
            commands.draw_indirect(
                &pipeline,
                &root,
                &root,
                Some((&indices.whole(), vk::IndexType::UINT32)),
                &records.whole(),
                20,
                IndirectCount::Gpu {
                    memory: &count.whole(),
                    max,
                },
                &[vertices.whole(), draws.whole()],
            )?;
        }
        commands.end_rendering()?;
        commands.barrier(Access::ALL, Access::COPY_READ)?;
        let region = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(vk::Extent3D {
                width: 8,
                height: 8,
                depth: 1,
            });
        unsafe {
            commands.read_image(&image, &readback.whole(), &[region])?;
        }
        commands.barrier(Access::COPY_WRITE, Access::HOST_READ)?;
        commands.submit()?.wait(10_000_000_000)?;
        let mut pixels = [0u8; 256];
        readback.read(0, &mut pixels)?;
        for (i, pixel) in pixels.chunks_exact(4).enumerate() {
            let visible = generated.min(max);
            let expected = if visible == 0 || (visible == 1 && i % 8 >= 4) {
                [0; 4]
            } else if i % 8 < 4 {
                [255, 0, 0, 255]
            } else {
                [0, 255, 0, 255]
            };
            anyhow::ensure!(pixel == expected, "indirect draw {i}: {pixel:?}");
        }
    }
    log::info!(
        "PASS: GPU-generated indexed multi-draw/count, per-draw fragment data, specialization/cache, split barrier and live argument arena append"
    );
    Ok(())
}
