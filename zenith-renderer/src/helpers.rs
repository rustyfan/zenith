use anyhow::Result;
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
