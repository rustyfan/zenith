use anyhow::{Result, ensure};
use std::sync::Arc;
use zenith_core::log;
use zenith_rhi::*;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Root {
    images: [u32; 9],
    samplers: [u32; 2],
    output: u32,
    result: GpuAddress,
}

fn color(index: usize) -> [f32; 4] {
    [(index + 1) as f32 / 16.0, 0.25, 0.5, 1.0]
}

pub fn run(gpu: &Arc<Gpu>) -> Result<()> {
    let shader = gpu.compile_shader(
        "zenith-rhi/tests/shaders/direct_heaps.slang",
        "main",
        ShaderStage::Compute,
    )?;
    let pipeline = gpu.compute(&shader)?;
    let mut commands = gpu.commands()?;
    let shapes = [
        (vk::ImageType::TYPE_1D, 1, 1, 1, false),
        (vk::ImageType::TYPE_1D, 1, 1, 2, false),
        (vk::ImageType::TYPE_2D, 2, 1, 1, false),
        (vk::ImageType::TYPE_2D, 2, 1, 1, false),
        (vk::ImageType::TYPE_2D, 2, 1, 2, false),
        (vk::ImageType::TYPE_3D, 2, 2, 1, false),
        (vk::ImageType::TYPE_2D, 2, 1, 6, true),
        (vk::ImageType::TYPE_2D, 2, 1, 12, true),
        (vk::ImageType::TYPE_2D, 2, 1, 1, false),
    ];
    let mut textures = Vec::new();
    for (index, (kind, height, depth, layers, cube)) in shapes.into_iter().enumerate() {
        let texture = gpu.texture(TextureDesc {
            kind,
            extent: vk::Extent3D {
                width: 2,
                height,
                depth,
            },
            layers,
            cube,
            usage: vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
            ..TextureDesc::color(
                2,
                height,
                if index == 8 {
                    vk::Format::D32_SFLOAT
                } else {
                    vk::Format::R32G32B32A32_SFLOAT
                },
            )
        })?;
        unsafe {
            commands.initialize(&texture)?;
        }
        if index == 8 {
            commands.clear_depth(&texture, 0.75, 0)?;
        } else {
            commands.clear_color(&texture, color(index))?;
        }
        textures.push(texture);
    }
    commands.barrier(Access::COPY_WRITE, Access::COMPUTE_READ)?;
    let mut outputs = Vec::new();
    let mut tables = Vec::new();
    let mut arguments = Arguments::new(gpu, 1024)?;
    for capacity in [32, 64] {
        let table = Descriptors::new(gpu, capacity, 8)?;
        let mut padding = Vec::new();
        for _ in 0..capacity / 16 {
            padding.push(table.image(&textures[0].full_view()?, false)?);
        }
        let mut bindings = Vec::new();
        for texture in &textures {
            bindings.push(table.image(&texture.full_view()?, false)?);
        }
        let unused_sampler = table.sampler(vk::Filter::NEAREST, vk::SamplerAddressMode::REPEAT)?;
        let edge = table.sampler(vk::Filter::NEAREST, vk::SamplerAddressMode::CLAMP_TO_EDGE)?;
        let border = table.sampler(vk::Filter::NEAREST, vk::SamplerAddressMode::CLAMP_TO_BORDER)?;
        ensure!(
            edge.index() > 0 && border.index() > 0,
            "sampler stride fixture requires nonzero slots"
        );
        let output = gpu.texture(TextureDesc {
            usage: vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC,
            ..TextureDesc::color(64, 8, vk::Format::R32G32B32A32_SFLOAT)
        })?;
        let output_binding = table.image(&output.full_view()?, true)?;
        let result = gpu.allocate(64 * 8 * 16, MemoryDomain::Readback)?;
        let image_result = gpu.allocate(result.size(), MemoryDomain::Readback)?;
        let copied = table.copy_image(&mut commands, &bindings[3])?;
        let mut images = std::array::from_fn(|i| bindings[i].index());
        images[3] = copied.index();
        let root = arguments.push(&Root {
            images,
            samplers: [edge.index(), border.index()],
            output: output_binding.index(),
            result: result.address(),
        })?;
        commands.bind_descriptors(&table)?;
        for binding in &bindings {
            commands.retain_image(binding)?;
        }
        commands.retain_image(&output_binding)?;
        commands.retain_sampler(&edge)?;
        commands.retain_sampler(&border)?;
        commands.barrier(
            Access::COPY_WRITE,
            Access {
                stages: vk::PipelineStageFlags2::COMPUTE_SHADER,
                access: vk::AccessFlags2::RESOURCE_HEAP_READ_EXT,
            },
        )?;
        unsafe {
            commands.initialize(&output)?;
            commands.dispatch(&pipeline, &root, [1, 1, 1], &[result.whole()])?;
        }
        commands.barrier(Access::COMPUTE_WRITE, Access::COPY_READ)?;
        let region = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(vk::Extent3D {
                width: 64,
                height: 8,
                depth: 1,
            });
        unsafe {
            commands.read_image(&output, &image_result.whole(), &[region])?;
        }
        ensure!(
            Arc::ptr_eq(&pipeline, &gpu.compute(&shader)?),
            "heap capacity changed pipeline identity"
        );
        drop((
            bindings,
            output_binding,
            copied,
            edge,
            border,
            unused_sampler,
            padding,
        ));
        ensure!(
            table.available_images() == capacity as usize - 11,
            "image descriptor retired before completion"
        );
        tables.push((table, capacity));
        outputs.push((result, image_result));
    }
    commands.barrier(Access::ALL, Access::HOST_READ)?;
    commands.submit()?.wait(10_000_000_000)?;
    for (table, capacity) in tables {
        ensure!(
            table.available_images() == capacity as usize,
            "image descriptor did not retire"
        );
    }
    for (result, image_result) in outputs {
        let mut values = vec![[0.0f32; 4]; 64 * 8];
        let mut pixels = values.clone();
        result.read(0, bytemuck::cast_slice_mut(&mut values))?;
        image_result.read(0, bytemuck::cast_slice_mut(&mut pixels))?;
        for lane in 0..64 {
            let expected = [
                if lane & 2 == 0 {
                    color(2 + (lane & 1))
                } else {
                    [0.0; 4]
                },
                color(0),
                color(1),
                color(4),
                color(5),
                color(6),
                color(7),
                [0.75; 4],
            ];
            for (kind, value) in expected.into_iter().enumerate() {
                ensure!(
                    values[lane * 8 + kind] == value,
                    "direct heap mismatch: lane={lane}, type={kind}, actual={:?}, expected={value:?}",
                    values[lane * 8 + kind]
                );
                ensure!(
                    pixels[kind * 64 + lane] == value,
                    "direct storage heap mismatch: lane={lane}, type={kind}"
                );
            }
        }
    }
    log::info!(
        "PASS: direct heaps with strides {:?}, divergent image/sampler indices, 1D/2D/3D/arrays/cubes/depth/storage, heap rebinding, GPU descriptor copy and retirement",
        gpu.descriptor_strides()?
    );
    Ok(())
}
