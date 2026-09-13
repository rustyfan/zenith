use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::{vk, TextureDesc};

pub struct SceneTextures {
    pub base_color: ImageId,
    pub normal_mra: ImageId,
    pub depth: ImageId,
}
impl SceneTextures {
    pub const COLOR_FORMATS: [vk::Format; 2] =
        [vk::Format::R8G8B8A8_UNORM, vk::Format::R16G16B16A16_UNORM];

    pub fn new(
        builder: &mut RenderGraphBuilder<'_>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Self> {
        let mut depth = TextureDesc::color(width, height, vk::Format::D32_SFLOAT);
        depth.usage = vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
        Ok(Self {
            base_color: builder.create_image(TextureDesc::color(
                width,
                height,
                Self::COLOR_FORMATS[0],
            ))?,
            normal_mra: builder.create_image(TextureDesc::color(
                width,
                height,
                Self::COLOR_FORMATS[1],
            ))?,
            depth: builder.create_image(depth)?,
        })
    }
}
