use crate::{defer_shading::SceneTextures, helpers::*, world::GpuViewData};
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;
use zenith_rendergraph::{BufferId, ImageId, RenderGraphBuilder};
use zenith_rhi::*;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    view: GpuAddress,
    sh: GpuAddress,
    base_color: u32,
    normal_mra: u32,
    depth: u32,
    skybox: u32,
    linear_sampler: u32,
    nearest_sampler: u32,
    debug_mode: u32,
    padding: u32,
}

pub struct DirectLightingRenderer {
    vertex: Shader,
    fragment: Shader,
    linear: Arc<Sampler>,
    nearest: Arc<Sampler>,
    pipeline: Option<(vk::Format, Arc<RasterPipeline>)>,
}
impl DirectLightingRenderer {
    pub fn new(
        [vertex, fragment]: [Shader; 2],
        linear: Arc<Sampler>,
        nearest: Arc<Sampler>,
    ) -> Self {
        Self {
            vertex,
            fragment,
            linear,
            nearest,
            pipeline: None,
        }
    }
    pub fn render(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        scene: SceneTextures,
        skybox: ImageId,
        sh: BufferId,
        view_data: GpuViewData,
        debug_mode: u32,
        output: ImageId,
    ) -> anyhow::Result<()> {
        let desc = builder.image_desc(output)?;
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
        let linear = self.linear.clone();
        let nearest = self.nearest.clone();
        let extent = vk::Extent2D {
            width: desc.extent.width,
            height: desc.extent.height,
        };
        builder.pass(
            "lighting",
            vec![
                scene.base_color.read(FRAGMENT_READ),
                scene.normal_mra.read(FRAGMENT_READ),
                scene.depth.read(FRAGMENT_READ),
                skybox.read(FRAGMENT_READ),
                sh.read(FRAGMENT_READ),
                output.write(COLOR_WRITE),
            ],
            move |ctx| {
                let view = ctx.arguments(&view_data)?;
                let sh = ctx.buffer(sh)?;
                let data = Root {
                    view: view.address(),
                    sh: sh.address(),
                    base_color: ctx.sampled(scene.base_color)?,
                    normal_mra: ctx.sampled(scene.normal_mra)?,
                    depth: ctx.sampled(scene.depth)?,
                    skybox: ctx.sampled(skybox)?,
                    linear_sampler: ctx.sampler(&linear)?,
                    nearest_sampler: ctx.sampler(&nearest)?,
                    debug_mode,
                    padding: 0,
                };
                let root = ctx.arguments(&data)?;
                let target = ctx.view(output)?;
                ctx.commands.begin_rendering(
                    &[Attachment {
                        view: &target,
                        clear: Some([0.02, 0.02, 0.02, 1.0]),
                        store: true,
                        resolve: None,
                    }],
                    None,
                    extent,
                )?;
                viewport(ctx.commands, extent)?;
                unsafe {
                    ctx.commands
                        .draw(&pipeline, &root, &root, 0..3, 0..1, &[view, sh])?;
                }
                ctx.commands.end_rendering()
            },
        )
    }
}
