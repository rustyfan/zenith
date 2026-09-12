use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::{vk, TextureDesc};

pub struct SceneTextures {
    pub base_color: ImageId,
    pub normal_mra: ImageId,
    pub depth: ImageId,
}
impl SceneTextures {
    pub fn new(
        builder: &mut RenderGraphBuilder<'_>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Self> {
        let color = TextureDesc::color(width, height, vk::Format::R8G8B8A8_UNORM);
        let mut depth = TextureDesc::color(width, height, vk::Format::D32_SFLOAT);
        depth.usage = vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
        Ok(Self {
            base_color: builder.create_image(color)?,
            normal_mra: builder.create_image(color)?,
            depth: builder.create_image(depth)?,
        })
    }
}
