use anyhow::{ensure, Result};
use std::sync::Arc;
use zenith_rhi::*;

pub const VERTEX_READ: Access = Access {
    stages: vk::PipelineStageFlags2::VERTEX_SHADER,
    access: vk::AccessFlags2::SHADER_READ,
};
pub const FRAGMENT_READ: Access = Access {
    stages: vk::PipelineStageFlags2::FRAGMENT_SHADER,
    access: vk::AccessFlags2::SHADER_READ,
};
pub const INDEX_READ: Access = Access {
    stages: vk::PipelineStageFlags2::INDEX_INPUT,
    access: vk::AccessFlags2::INDEX_READ,
};
pub const COLOR_WRITE: Access = Access {
    stages: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
    access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
};
pub const DEPTH_WRITE: Access = Access {
    stages: vk::PipelineStageFlags2::from_raw(
        vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS.as_raw()
            | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS.as_raw(),
    ),
    access: vk::AccessFlags2::from_raw(
        vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ.as_raw()
            | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE.as_raw(),
    ),
};

pub fn upload_buffer(gpu: &Arc<Gpu>, commands: &mut Commands, bytes: &[u8]) -> Result<Arc<Memory>> {
    let staging = gpu.allocate(bytes.len() as u64, MemoryDomain::Upload)?;
    staging.write(0, bytes)?;
    let memory = gpu.allocate(bytes.len() as u64, MemoryDomain::Device)?;
    commands.copy(&staging.whole(), &memory.whole())?;
    Ok(memory)
}

pub fn upload_texture(
    gpu: &Arc<Gpu>,
    commands: &mut Commands,
    asset: &zenith_asset::texture::Texture,
) -> Result<Arc<Texture>> {
    let mut desc = TextureDesc::color(asset.width, asset.height, asset.format.to_vk());
    desc.usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST;
    desc.cube = asset.is_cubemap;
    desc.layers = if asset.is_cubemap { 6 } else { 1 };
    desc.mip_levels = asset.mip_levels;
    let mut regions = Vec::new();
    let mut offset = 0;
    for mip in 0..desc.mip_levels {
        let width = (asset.width >> mip).max(1);
        let height = (asset.height >> mip).max(1);
        regions.push(
            vk::BufferImageCopy::default()
                .buffer_offset(offset)
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                })
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(mip)
                        .layer_count(desc.layers),
                ),
        );
        offset += asset.format.data_size_in_bytes(width, height) as u64 * desc.layers as u64;
    }
    ensure!(
        offset == asset.pixels.len() as u64,
        "texture byte count differs from its mip/layer footprint"
    );
    let texture = gpu.texture(desc)?;
    let staging = gpu.allocate(offset, MemoryDomain::Upload)?;
    staging.write(0, &asset.pixels)?;
    unsafe {
        commands.initialize(&texture)?;
        commands.upload_image(&staging.whole(), &texture, &regions)?;
    }
    Ok(texture)
}

pub fn raster(
    gpu: &Arc<Gpu>,
    vertex: &Shader,
    fragment: &Shader,
    colors: &[vk::Format],
    depth: vk::Format,
) -> Result<Arc<RasterPipeline>> {
    gpu.raster(&RasterDesc {
        vertex,
        fragment,
        colors,
        depth,
        stencil: vk::Format::UNDEFINED,
        samples: vk::SampleCountFlags::TYPE_1,
        topology: vk::PrimitiveTopology::TRIANGLE_LIST,
        blend: &vec![Blend::default(); colors.len()],
        dynamic_blend: false,
    })
}

pub fn viewport(commands: &mut Commands, extent: vk::Extent2D) -> Result<()> {
    commands.viewport_scissor(
        vk::Viewport {
            x: 0.0,
            y: extent.height as f32,
            width: extent.width as f32,
            height: -(extent.height as f32),
            min_depth: 0.0,
            max_depth: 1.0,
        },
        vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent,
        },
    )
}
