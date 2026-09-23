use crate::helpers::*;
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;
use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::*;

#[derive(Clone, Copy, Debug)]
pub struct PostProcessingSettings {
    pub exposure: f32,
}

impl Default for PostProcessingSettings {
    fn default() -> Self {
        Self { exposure: 1.0 }
    }
}

impl PostProcessingSettings {
    pub(crate) fn validated(self) -> anyhow::Result<Self> {
        anyhow::ensure!(
            self.exposure.is_finite() && self.exposure >= 0.0,
            "post processing requires nonnegative finite exposure"
        );
        Ok(self)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    hdr_color: u32,
    exposure: f32,
    encode_srgb: u32,
    bypass_tonemapping: u32,
}

pub struct PostProcessingRenderer {
    vertex: Shader,
    fragment: Shader,
    pipeline: Option<(vk::Format, Arc<RasterPipeline>)>,
}

impl PostProcessingRenderer {
    pub fn new([vertex, fragment]: [Shader; 2]) -> Self {
        Self {
            vertex,
            fragment,
            pipeline: None,
        }
    }

    pub fn render(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        hdr_color: ImageId,
        output: ImageId,
        settings: PostProcessingSettings,
        bypass_tonemapping: bool,
    ) -> anyhow::Result<()> {
        let desc = builder.image_desc(output)?;
        anyhow::ensure!(
            builder.image_desc(hdr_color)?.extent == desc.extent,
            "post processing requires matching input and output extents"
        );
        if self
            .pipeline
            .as_ref()
            .is_none_or(|(format, _)| *format != desc.format)
        {
            self.pipeline = Some((
                desc.format,
                raster(
                    builder.gpu(),
                    &self.vertex,
                    &self.fragment,
                    &[desc.format],
                    vk::Format::UNDEFINED,
                )?,
            ));
        }
        let pipeline = self.pipeline.as_ref().unwrap().1.clone();
        let encode_srgb = !matches!(
            desc.format,
            vk::Format::B8G8R8A8_SRGB
                | vk::Format::R8G8B8A8_SRGB
                | vk::Format::A8B8G8R8_SRGB_PACK32
        );
        let extent = vk::Extent2D {
            width: desc.extent.width,
            height: desc.extent.height,
        };
        builder.pass(
            "post_processing",
            vec![hdr_color.read(FRAGMENT_READ), output.write(COLOR_WRITE)],
            move |ctx| {
                let data = Root {
                    hdr_color: ctx.sampled(hdr_color)?,
                    exposure: settings.exposure,
                    encode_srgb: u32::from(encode_srgb),
                    bypass_tonemapping: u32::from(bypass_tonemapping),
                };
                let root = ctx.arguments(&data)?;
                let target = ctx.view(output)?;
                ctx.commands.begin_rendering(
                    &[Attachment {
                        view: &target,
                        clear: Some([0.0, 0.0, 0.0, 1.0]),
                        store: true,
                        resolve: None,
                    }],
                    None,
                    extent,
                )?;
                viewport(ctx.commands, extent)?;
                unsafe {
                    ctx.commands
                        .draw(&pipeline, &root, &root, 0..3, 0..1, &[])?;
                }
                ctx.commands.end_rendering()
            },
        )
    }
}
