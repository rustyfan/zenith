use ash::vk;
use std::sync::Arc;
use zenith_core::log;
use zenith_rhi::*;

pub fn run(gpu: &Arc<Gpu>) -> anyhow::Result<()> {
    padded(gpu)?;
    let shapes = [
        (vk::ImageType::TYPE_1D, 7, 1, 1, 1, false),
        (vk::ImageType::TYPE_1D, 7, 1, 1, 3, false),
        (vk::ImageType::TYPE_2D, 7, 5, 1, 1, false),
        (vk::ImageType::TYPE_2D, 7, 5, 1, 3, false),
        (vk::ImageType::TYPE_2D, 7, 7, 1, 6, true),
        (vk::ImageType::TYPE_2D, 7, 7, 1, 12, true),
        (vk::ImageType::TYPE_3D, 7, 5, 3, 1, false),
    ];
    let table = Descriptors::new(gpu, 32, 4)?;
    for (kind, width, height, depth, layers, cube) in shapes {
        let desc = TextureDesc {
            kind,
            extent: vk::Extent3D {
                width,
                height,
                depth,
            },
            layers,
            cube,
            mip_levels: 3,
            ..TextureDesc::color(width, height, vk::Format::R8G8B8A8_UNORM)
        };
        let source = gpu.texture(desc)?;
        let destination = gpu.texture(desc)?;
        let view = source.full_view()?;
        let _descriptor = table.image(&view, false)?;
        anyhow::ensure!(
            source
                .view(
                    view_type(kind),
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(4)
                        .layer_count(1)
                )
                .is_err(),
            "invalid mip range accepted"
        );
        let mut bytes = Vec::new();
        let mut regions = Vec::new();
        for mip in 0..3 {
            let e = vk::Extent3D {
                width: (width >> mip).max(1),
                height: (height >> mip).max(1),
                depth: (depth >> mip).max(1),
            };
            regions.push(
                vk::BufferImageCopy::default()
                    .buffer_offset(bytes.len() as u64)
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .mip_level(mip)
                            .layer_count(layers),
                    )
                    .image_extent(e),
            );
            let count = (e.width * e.height * e.depth * layers * 4) as usize;
            bytes.extend((0..count).map(|i| ((i * 17 + mip as usize * 23) % 256) as u8));
        }
        let upload = gpu.allocate(bytes.len() as u64, MemoryDomain::Upload)?;
        upload.write(0, &bytes)?;
        let readback = gpu.allocate(bytes.len() as u64, MemoryDomain::Readback)?;
        let mut commands = gpu.commands()?;
        unsafe {
            commands.initialize(&source)?;
            commands.initialize(&destination)?;
            anyhow::ensure!(
                commands
                    .upload_image(&upload.slice(0..4)?, &source, &regions)
                    .is_err(),
                "undersized image upload accepted"
            );
            commands.upload_image(&upload.whole(), &source, &regions)?;
        }
        commands.barrier(Access::COPY_WRITE, Access::COPY_READ)?;
        commands.copy_texture(&source, &destination)?;
        commands.barrier(Access::COPY_WRITE, Access::COPY_READ)?;
        unsafe {
            commands.read_image(&destination, &readback.whole(), &regions)?;
        }
        commands.barrier(Access::COPY_WRITE, Access::HOST_READ)?;
        commands.submit()?.wait(10_000_000_000)?;
        let mut result = vec![0; bytes.len()];
        readback.read(0, &mut result)?;
        anyhow::ensure!(
            result == bytes,
            "image copy mismatch: {kind:?}, layers={layers}, cube={cube}"
        );
    }
    for format in [vk::Format::BC7_UNORM_BLOCK, vk::Format::D32_SFLOAT] {
        let mut desc = TextureDesc::color(7, 5, format);
        desc.usage = vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_SRC
            | vk::ImageUsageFlags::TRANSFER_DST;
        let texture = gpu.texture(desc)?;
        let size = if format == vk::Format::BC7_UNORM_BLOCK {
            64
        } else {
            140
        };
        let data: Vec<u8> = if format == vk::Format::D32_SFLOAT {
            bytemuck::cast_slice(&vec![0.5f32; 35]).to_vec()
        } else {
            (0..size).map(|i| i as u8).collect()
        };
        let upload = gpu.allocate(size, MemoryDomain::Upload)?;
        upload.write(0, &data)?;
        let readback = gpu.allocate(size, MemoryDomain::Readback)?;
        let region = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(desc.aspect())
                    .layer_count(1),
            )
            .image_extent(desc.extent);
        let mut commands = gpu.commands()?;
        unsafe {
            commands.initialize(&texture)?;
            commands.upload_image(&upload.whole(), &texture, &[region])?;
        }
        commands.barrier(Access::COPY_WRITE, Access::COPY_READ)?;
        unsafe {
            commands.read_image(&texture, &readback.whole(), &[region])?;
        }
        commands.barrier(Access::COPY_WRITE, Access::HOST_READ)?;
        commands.submit()?.wait(10_000_000_000)?;
        let mut actual = vec![0; size as usize];
        readback.read(0, &mut actual)?;
        anyhow::ensure!(actual == data, "compressed/depth copy mismatch");
    }
    log::info!(
        "PASS: 1D/2D/3D, arrays/cubes/cube arrays, mip chains, odd extents, BC7 and depth copy/readback, invalid copy/view ranges"
    );
    Ok(())
}

fn padded(gpu: &Arc<Gpu>) -> anyhow::Result<()> {
    let texture = gpu.texture(TextureDesc::color(3, 2, vk::Format::R8G8B8A8_UNORM))?;
    let upload = gpu.allocate(64, MemoryDomain::Upload)?;
    let readback = gpu.allocate(64, MemoryDomain::Readback)?;
    let mut source = [0u8; 64];
    let mut expected = [0u8; 64];
    for row in 0..2 {
        for x in 0..12 {
            source[12 + row * 28 + x] = (row * 12 + x + 1) as u8;
            expected[8 + row * 20 + x] = (row * 12 + x + 1) as u8;
        }
    }
    upload.write(0, &source)?;
    readback.write(0, &[0u8; 64])?;
    let region = vk::BufferImageCopy::default()
        .buffer_offset(12)
        .buffer_row_length(7)
        .buffer_image_height(4)
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_extent(texture.desc().extent);
    let mut commands = gpu.commands()?;
    unsafe {
        commands.initialize(&texture)?;
        anyhow::ensure!(
            commands
                .upload_image(&upload.whole(), &texture, &[region.buffer_row_length(2)])
                .is_err(),
            "short row pitch accepted"
        );
        commands.upload_image(&upload.whole(), &texture, &[region])?;
    }
    commands.barrier(Access::COPY_WRITE, Access::COPY_READ)?;
    unsafe {
        commands.read_image(
            &texture,
            &readback.whole(),
            &[region
                .buffer_offset(8)
                .buffer_row_length(5)
                .buffer_image_height(2)],
        )?;
    }
    commands.barrier(Access::COPY_WRITE, Access::HOST_READ)?;
    commands.submit()?.wait(10_000_000_000)?;
    let mut actual = [0u8; 64];
    readback.read(0, &mut actual)?;
    anyhow::ensure!(actual == expected, "padded image upload/readback mismatch");
    Ok(())
}

fn view_type(kind: vk::ImageType) -> vk::ImageViewType {
    match kind {
        vk::ImageType::TYPE_1D => vk::ImageViewType::TYPE_1D,
        vk::ImageType::TYPE_2D => vk::ImageViewType::TYPE_2D,
        _ => vk::ImageViewType::TYPE_3D,
    }
}
