use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::{vk, TextureDesc};

pub struct SceneTextures {
    pub base_color: ImageId,
    pub normal_mra: ImageId,
    pub depth: ImageId,
    pub global_illumination: ImageId,
}
impl SceneTextures {
    pub const COLOR_FORMATS: [vk::Format; 2] =
        [vk::Format::R8G8B8A8_UNORM, vk::Format::R16G16B16A16_UNORM];
    pub const GLOBAL_ILLUMINATION_FORMAT: vk::Format = vk::Format::R16_SFLOAT;

    pub fn new(
        builder: &mut RenderGraphBuilder<'_>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Self> {
        let mut depth = TextureDesc::color(width, height, vk::Format::D32_SFLOAT);
        depth.usage = vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
        let mut global_illumination =
            TextureDesc::color(width, height, Self::GLOBAL_ILLUMINATION_FORMAT);
        global_illumination.usage =
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
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
            global_illumination: builder.create_image(global_illumination)?,
        })
    }
}
