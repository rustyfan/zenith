use crate::{defer_shading::SceneTextures, helpers::*, world::GpuViewData};
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;
use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::*;

#[derive(Clone, Copy, Debug)]
pub struct AmbientOcclusionSettings {
    pub enabled: bool,
    pub radius: f32,
    pub bias: f32,
    pub samples: u32,
}

impl Default for AmbientOcclusionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            radius: 1.0,
            bias: 0.001,
            samples: 8,
        }
    }
}

impl AmbientOcclusionSettings {
    pub(crate) fn validate(self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.bias.is_finite()
                && self.bias > 0.0
                && self.radius.is_finite()
                && self.radius > self.bias
                && (1..=32).contains(&self.samples),
            "ambient occlusion requires a positive finite bias, a finite radius greater than the bias, and 1 to 32 samples"
        );
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    view: GpuAddress,
    acceleration: GpuAddress,
    normal_mra: u32,
    depth: u32,
    samples: u32,
    radius: f32,
    bias: f32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct UpsampleRoot {
    ao: u32,
    depth: u32,
}

pub(crate) struct AmbientOcclusionRenderer {
    pipeline: Arc<RasterPipeline>,
    upsample_pipeline: Arc<RasterPipeline>,
}

impl AmbientOcclusionRenderer {
    pub fn new(gpu: &Arc<Gpu>, [vertex, fragment, upsample]: [&Shader; 3]) -> anyhow::Result<Self> {
        Ok(Self {
            pipeline: raster(
                gpu,
                vertex,
                fragment,
                &[SceneTextures::GLOBAL_ILLUMINATION_FORMAT],
                vk::Format::UNDEFINED,
            )?,
            upsample_pipeline: raster(
                gpu,
                vertex,
                upsample,
                &[SceneTextures::GLOBAL_ILLUMINATION_FORMAT],
                vk::Format::UNDEFINED,
            )?,
        })
    }

    pub fn render(
        &self,
        builder: &mut RenderGraphBuilder<'_>,
        acceleration: Option<Arc<AccelerationStructure>>,
        scene: &SceneTextures,
        view_data: GpuViewData,
        settings: AmbientOcclusionSettings,
    ) -> anyhow::Result<()> {
        let output = scene.global_illumination;
        let acceleration = acceleration.filter(|_| settings.enabled);
        let half_ao = if acceleration.is_some() {
            let mut desc = builder.image_desc(output)?;
            desc.extent.width = desc.extent.width.div_ceil(2);
            desc.extent.height = desc.extent.height.div_ceil(2);
            Some(builder.create_image(desc)?)
        } else {
            None
        };
        let trace_output = half_ao.unwrap_or(output);
        let desc = builder.image_desc(trace_output)?;
        let extent = vk::Extent2D {
            width: desc.extent.width,
            height: desc.extent.height,
        };
        let mut uses = vec![trace_output.write(COLOR_WRITE)];
        if let Some(structure) = &acceleration {
            let id = builder.import_buffer(structure.storage().clone());
            uses.extend([
                id.read(Access::AS_FRAGMENT_READ),
                scene.depth.read(FRAGMENT_READ),
                scene.normal_mra.read(FRAGMENT_READ),
            ]);
        }
        let (depth, normal_mra) = (scene.depth, scene.normal_mra);
        let pipeline = self.pipeline.clone();
        builder.pass("ambient_occlusion", uses, move |ctx| {
            let target = ctx.view(trace_output)?;
            ctx.commands.begin_rendering(
                &[Attachment {
                    view: &target,
                    clear: Some([1.0; 4]),
                    store: true,
                    resolve: None,
                }],
                None,
                extent,
            )?;
            if let Some(structure) = &acceleration {
                ctx.commands.retain_acceleration_structure(structure)?;
                let view = ctx.arguments(&view_data)?;
                let data = Root {
                    view: view.address(),
                    acceleration: structure.address(),
                    normal_mra: ctx.sampled(normal_mra)?,
                    depth: ctx.sampled(depth)?,
                    samples: settings.samples,
                    radius: settings.radius,
                    bias: settings.bias,
                    padding: 0,
                };
                let root = ctx.arguments(&data)?;
                viewport(ctx.commands, extent)?;
                unsafe {
                    ctx.commands
                        .draw(&pipeline, &root, &root, 0..3, 0..1, &[view])?;
                }
            }
            ctx.commands.end_rendering()
        })?;
        if let Some(ao) = half_ao {
            self.upsample(builder, ao, depth, output)?;
        }
        Ok(())
    }

    pub(crate) fn upsample(
        &self,
        builder: &mut RenderGraphBuilder<'_>,
        ao: ImageId,
        depth: ImageId,
        output: ImageId,
    ) -> anyhow::Result<()> {
        let desc = builder.image_desc(output)?;
        let half = builder.image_desc(ao)?;
        anyhow::ensure!(
            builder.image_desc(depth)?.extent == desc.extent
                && half.extent.width == desc.extent.width.div_ceil(2)
                && half.extent.height == desc.extent.height.div_ceil(2),
            "AO upsampling requires half-resolution AO and full-resolution depth"
        );
        let extent = vk::Extent2D {
            width: desc.extent.width,
            height: desc.extent.height,
        };
        let pipeline = self.upsample_pipeline.clone();
        builder.pass(
            "ao_upsample",
            vec![
                ao.read(FRAGMENT_READ),
                depth.read(FRAGMENT_READ),
                output.write(COLOR_WRITE),
            ],
            move |ctx| {
                let data = UpsampleRoot {
                    ao: ctx.sampled(ao)?,
                    depth: ctx.sampled(depth)?,
                };
                let root = ctx.arguments(&data)?;
                let target = ctx.view(output)?;
                ctx.commands.begin_rendering(
                    &[Attachment {
                        view: &target,
                        clear: Some([1.0; 4]),
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
