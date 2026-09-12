use anyhow::{Result, ensure};
use std::sync::Arc;
use zenith_core::log;
use zenith_rhi::*;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Root {
    vertices: GpuAddress,
    first_instance: u32,
    scale: f32,
    first_color: [f32; 4],
    second_color: [f32; 4],
}

pub fn run(gpu: &Arc<Gpu>) -> Result<()> {
    let vertices = gpu.allocate(48, MemoryDomain::Upload)?;
    vertices.write(
        0,
        bytemuck::cast_slice(&[
            [0.0f32, 0.0],
            [0.0, 0.0],
            [-1.0, -1.0],
            [1.0, -1.0],
            [1.0, 1.0],
            [-1.0, 1.0],
        ]),
    )?;
    let indices = gpu.allocate(12, MemoryDomain::Upload)?;
    indices.write(0, bytemuck::cast_slice(&[0u16, 1, 2, 0, 2, 3]))?;
    let records = gpu.allocate(16, MemoryDomain::Upload)?;
    records.write(0, bytemuck::cast_slice(&[3u32, 1, 0, 0]))?;
    let mut arguments = Arguments::new(gpu, 1024)?;
    let root_data = Root {
        vertices: vertices.address(),
        first_instance: 3,
        scale: 0.5,
        first_color: [1.0, 0.0, 0.0, 1.0],
        second_color: [0.0, 1.0, 0.0, 1.0],
    };
    let instanced = arguments.push(&root_data)?;
    let red = arguments.push(&Root {
        first_instance: 0,
        scale: 1.0,
        ..root_data
    })?;
    let blue = arguments.push(&Root {
        first_instance: 0,
        scale: 1.0,
        first_color: [0.0, 0.0, 1.0, 1.0],
        ..root_data
    })?;
    let vertex = gpu.compile_shader(
        "zenith-rhi/tests/shaders/draw_arguments.slang",
        "indexed",
        ShaderStage::Vertex,
    )?;
    let procedural = gpu.compile_shader(
        "zenith-rhi/tests/shaders/draw_arguments.slang",
        "procedural",
        ShaderStage::Vertex,
    )?;
    let fragment = gpu.compile_shader(
        "zenith-rhi/tests/shaders/draw_arguments.slang",
        "fragmentMain",
        ShaderStage::Fragment,
    )?;
    let colors = [vk::Format::R8G8B8A8_UNORM];
    let desc = RasterDesc {
        vertex: &vertex,
        fragment: &fragment,
        colors: &colors,
        depth: vk::Format::UNDEFINED,
        stencil: vk::Format::UNDEFINED,
        samples: vk::SampleCountFlags::TYPE_1,
        topology: vk::PrimitiveTopology::TRIANGLE_LIST,
        blend: &[Blend::default()],
        dynamic_blend: false,
    };
    let pipeline = gpu.raster(&desc)?;
    let procedural_pipeline = gpu.raster(&RasterDesc {
        vertex: &procedural,
        ..desc
    })?;
    let depth_pipeline = gpu.raster(&RasterDesc {
        depth: vk::Format::D32_SFLOAT,
        ..desc
    })?;
    let depth = gpu.texture(TextureDesc {
        usage: vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        ..TextureDesc::color(8, 8, vk::Format::D32_SFLOAT)
    })?;
    let depth_view = depth.full_view()?;
    for mode in 0..3 {
        let texture = gpu.texture(TextureDesc::color(8, 8, colors[0]))?;
        let view = texture.full_view()?;
        let readback = gpu.allocate(256, MemoryDomain::Readback)?;
        let mut commands = gpu.commands()?;
        unsafe {
            commands.initialize(&texture)?;
            if mode == 2 {
                commands.initialize(&depth)?;
            }
        }
        commands.begin_rendering(
            &[Attachment {
                view: &view,
                clear: Some([0.0; 4]),
                store: true,
                resolve: None,
            }],
            (mode == 2).then_some(DepthAttachment {
                view: &depth_view,
                clear: Some((1.0, 0)),
                store: true,
            }),
            vk::Extent2D {
                width: 8,
                height: 8,
            },
        )?;
        ensure!(
            commands
                .viewport_scissor(
                    vk::Viewport {
                        x: f32::NAN,
                        y: 0.0,
                        width: 8.0,
                        height: 8.0,
                        min_depth: 0.0,
                        max_depth: 1.0
                    },
                    vk::Rect2D::default()
                )
                .is_err(),
            "NaN viewport accepted"
        );
        unsafe {
            match mode {
                0 => commands.draw_indexed(
                    &pipeline,
                    &instanced,
                    &instanced,
                    &indices.whole(),
                    vk::IndexType::UINT16,
                    2,
                    3..5,
                    &[vertices.whole()],
                )?,
                1 => commands.draw_indirect(
                    &procedural_pipeline,
                    &blue,
                    &blue,
                    None,
                    &records.whole(),
                    16,
                    IndirectCount::Fixed(1),
                    &[],
                )?,
                _ => {
                    let state = RasterState {
                        depth_test: true,
                        depth_write: true,
                        depth_compare: vk::CompareOp::LESS_OR_EQUAL,
                        ..Default::default()
                    };
                    commands.raster_state(state)?;
                    commands.draw_indexed(
                        &depth_pipeline,
                        &red,
                        &red,
                        &indices.whole(),
                        vk::IndexType::UINT16,
                        2,
                        0..1,
                        &[vertices.whole()],
                    )?;
                    commands.raster_state(RasterState {
                        cull: vk::CullModeFlags::FRONT_AND_BACK,
                        ..state
                    })?;
                    commands.draw_indexed(
                        &depth_pipeline,
                        &blue,
                        &blue,
                        &indices.whole(),
                        vk::IndexType::UINT16,
                        2,
                        0..1,
                        &[vertices.whole()],
                    )?;
                    commands.raster_state(RasterState {
                        depth_bias: Some((65536.0, 0.0)),
                        ..state
                    })?;
                    commands.draw_indexed(
                        &depth_pipeline,
                        &blue,
                        &blue,
                        &indices.whole(),
                        vk::IndexType::UINT16,
                        2,
                        0..1,
                        &[vertices.whole()],
                    )?;
                    commands.viewport_scissor(
                        vk::Viewport {
                            x: 0.0,
                            y: 0.0,
                            width: 8.0,
                            height: 8.0,
                            min_depth: 0.0,
                            max_depth: 1.0,
                        },
                        vk::Rect2D {
                            offset: vk::Offset2D::default(),
                            extent: vk::Extent2D {
                                width: 4,
                                height: 8,
                            },
                        },
                    )?;
                    commands.raster_state(RasterState {
                        depth_bias: Some((-65536.0, 0.0)),
                        ..state
                    })?;
                    commands.draw_indexed(
                        &depth_pipeline,
                        &blue,
                        &blue,
                        &indices.whole(),
                        vk::IndexType::UINT16,
                        2,
                        0..1,
                        &[vertices.whole()],
                    )?;
                }
            }
        }
        commands.end_rendering()?;
        commands.barrier(Access::ALL, Access::COPY_READ)?;
        let region = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(texture.desc().extent);
        unsafe {
            commands.read_image(&texture, &readback.whole(), &[region])?;
        }
        commands.barrier(Access::COPY_WRITE, Access::HOST_READ)?;
        commands.submit()?.wait(10_000_000_000)?;
        let mut pixels = [0u8; 256];
        readback.read(0, &mut pixels)?;
        for (i, pixel) in pixels.chunks_exact(4).enumerate() {
            let expected = match mode {
                0 if i % 8 < 4 => [255, 0, 0, 255],
                0 => [0, 255, 0, 255],
                2 if i % 8 >= 4 => [255, 0, 0, 255],
                _ => [0, 0, 255, 255],
            };
            ensure!(pixel == expected, "draw mode {mode} pixel {i}: {pixel:?}");
        }
    }
    log::info!(
        "PASS: base vertex, nonzero first instance, instanced vertex pulling, nonindexed indirect drawing, culling and signed depth bias"
    );
    Ok(())
}
