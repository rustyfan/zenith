use ash::vk;
use std::sync::Arc;
use zenith_core::log;
use zenith_rhi::*;

pub fn run(gpu: &Arc<Gpu>) -> anyhow::Result<()> {
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    #[repr(C)]
    struct Root {
        color0: [f32; 4],
        color1: [f32; 4],
        depth: f32,
    }
    let mut arguments = Arguments::new(gpu, 4096)?;
    let red = arguments.push(&Root {
        color0: [1.0, 0.0, 0.0, 1.0],
        color1: [0.0, 1.0, 0.0, 1.0],
        depth: 0.5,
    })?;
    let rejected = arguments.push(&Root {
        color0: [0.0, 1.0, 0.0, 1.0],
        color1: [1.0, 0.0, 0.0, 1.0],
        depth: 0.75,
    })?;
    let blue = arguments.push(&Root {
        color0: [0.0, 0.0, 1.0, 1.0],
        color1: [0.0, 0.0, 1.0, 1.0],
        depth: 0.25,
    })?;
    let white = arguments.push(&Root {
        color0: [1.0, 1.0, 1.0, 0.5],
        color1: [1.0, 1.0, 1.0, 0.5],
        depth: 0.0,
    })?;
    let format = vk::Format::R8G8B8A8_UNORM;
    let make_msaa = || {
        gpu.texture(TextureDesc {
            samples: vk::SampleCountFlags::TYPE_4,
            ..TextureDesc::color(8, 8, format)
        })
    };
    let a = make_msaa()?;
    let b = make_msaa()?;
    let ra = gpu.texture(TextureDesc::color(8, 8, format))?;
    let rb = gpu.texture(TextureDesc::color(8, 8, format))?;
    let depth = gpu.texture(TextureDesc {
        format: vk::Format::D32_SFLOAT_S8_UINT,
        usage: vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        samples: vk::SampleCountFlags::TYPE_4,
        ..TextureDesc::color(8, 8, vk::Format::D32_SFLOAT_S8_UINT)
    })?;
    let av = a.full_view()?;
    let bv = b.full_view()?;
    let rav = ra.full_view()?;
    let rbv = rb.full_view()?;
    let dv = depth.full_view()?;
    let vertex = gpu.compile_shader(
        "zenith-rhi/tests/shaders/raster_states.slang",
        "vertexMain",
        ShaderStage::Vertex,
    )?;
    let fragment = gpu.compile_shader(
        "zenith-rhi/tests/shaders/raster_states.slang",
        "fragmentMain",
        ShaderStage::Fragment,
    )?;
    let pipeline = gpu.raster_specialized(
        &RasterDesc {
            vertex: &vertex,
            fragment: &fragment,
            colors: &[format, format],
            depth: depth.desc().format,
            stencil: depth.desc().format,
            samples: vk::SampleCountFlags::TYPE_4,
            topology: vk::PrimitiveTopology::TRIANGLE_LIST,
            blend: &[Blend::default(); 2],
            dynamic_blend: true,
        },
        &[(0, 1.0f32.to_bits())],
        &[(1, 1.0f32.to_bits())],
    )?;
    let count = gpu.pipeline_count();
    let mut commands = gpu.commands()?;
    for image in [&a, &b, &ra, &rb, &depth] {
        unsafe {
            commands.initialize(image)?;
        }
    }
    commands.begin_rendering(
        &[
            Attachment {
                view: &av,
                clear: Some([0.0; 4]),
                store: false,
                resolve: Some(&rav),
            },
            Attachment {
                view: &bv,
                clear: Some([0.0; 4]),
                store: false,
                resolve: Some(&rbv),
            },
        ],
        Some(DepthAttachment {
            view: &dv,
            clear: Some((1.0, 0)),
            store: false,
        }),
        vk::Extent2D {
            width: 8,
            height: 8,
        },
    )?;
    let stencil = vk::StencilOpState::default()
        .compare_op(vk::CompareOp::ALWAYS)
        .pass_op(vk::StencilOp::REPLACE)
        .fail_op(vk::StencilOp::KEEP)
        .depth_fail_op(vk::StencilOp::KEEP)
        .compare_mask(0xff)
        .write_mask(0xff)
        .reference(1);
    commands.raster_state(RasterState {
        depth_test: true,
        depth_write: true,
        depth_compare: vk::CompareOp::LESS,
        stencil: Some((stencil, stencil)),
        ..Default::default()
    })?;
    unsafe {
        commands.draw(&pipeline, &red, &red, 0..3, 0..1, &[])?;
        commands.draw(&pipeline, &rejected, &rejected, 0..3, 0..1, &[])?;
    }
    let stencil = vk::StencilOpState {
        compare_op: vk::CompareOp::EQUAL,
        pass_op: vk::StencilOp::KEEP,
        write_mask: 0,
        ..stencil
    };
    commands.raster_state(RasterState {
        depth_test: true,
        depth_write: true,
        depth_compare: vk::CompareOp::LESS,
        stencil: Some((stencil, stencil)),
        ..Default::default()
    })?;
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
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D {
                width: 4,
                height: 8,
            },
        },
    )?;
    unsafe {
        commands.draw(&pipeline, &blue, &blue, 0..3, 0..1, &[])?;
    }
    commands.raster_state(RasterState::default())?;
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
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D {
                width: 8,
                height: 8,
            },
        },
    )?;
    let blend = Blend {
        enabled: true,
        source_color: vk::BlendFactor::SRC_ALPHA,
        destination_color: vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        ..Default::default()
    };
    commands.blend(&[blend, blend])?;
    unsafe {
        commands.draw(&pipeline, &white, &white, 0..3, 0..1, &[])?;
    }
    commands.end_rendering()?;
    anyhow::ensure!(
        gpu.pipeline_count() == count,
        "state setters created pipelines"
    );
    commands.barrier(Access::ALL, Access::COPY_READ)?;
    let readback = gpu.allocate(512, MemoryDomain::Readback)?;
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
        commands.read_image(&ra, &readback.slice(0..256)?, &[region])?;
        commands.read_image(&rb, &readback.slice(256..512)?, &[region])?;
    }
    commands.barrier(Access::COPY_WRITE, Access::HOST_READ)?;
    commands.submit()?.wait(10_000_000_000)?;
    let mut bytes = [0u8; 512];
    readback.read(0, &mut bytes)?;
    for (i, pixel) in bytes.chunks_exact(4).enumerate() {
        let left = i % 8 < 4;
        let second = i >= 64;
        let expected = if left {
            [128i16, 128, 255, 128]
        } else if second {
            [128, 255, 128, 128]
        } else {
            [255, 128, 128, 128]
        };
        for c in 0..4 {
            anyhow::ensure!(
                (pixel[c] as i16 - expected[c]).abs() <= 1,
                "MRT/depth/stencil/blend/resolve pixel {i}: {pixel:?}, expected {expected:?}"
            );
        }
    }
    log::info!(
        "PASS: MRT, depth rejection, stencil selection, scissor, dynamic blend without pipeline creation, 4x MSAA resolves"
    );
    Ok(())
}
