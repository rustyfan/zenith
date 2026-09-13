use crate::{defer_shading::SceneTextures, helpers::*, ibl::IblResources, world::GpuViewData};
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use std::sync::Arc;
use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::*;

#[derive(Clone, Copy, Debug)]
pub struct DirectionalLight {
    pub direction_to_light: Vec3,
    pub color: Vec3,
    pub intensity: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct LightingSettings {
    pub directional: DirectionalLight,
    pub sky_intensity: f32,
    pub exposure: f32,
}

impl Default for LightingSettings {
    fn default() -> Self {
        Self {
            directional: DirectionalLight {
                direction_to_light: Vec3::new(0.5, -0.5, 1.0).normalize(),
                color: Vec3::ONE,
                intensity: 1.0,
            },
            sky_intensity: 1.0,
            exposure: 1.0,
        }
    }
}

impl LightingSettings {
    pub(crate) fn validated(mut self) -> anyhow::Result<Self> {
        let light = &mut self.directional;
        anyhow::ensure!(
            light.direction_to_light.is_finite()
                && light.direction_to_light.length_squared().is_finite()
                && light.direction_to_light.length_squared() > 1e-12
                && light.color.is_finite()
                && light.color.min_element() >= 0.0
                && [light.intensity, self.sky_intensity, self.exposure]
                    .iter().all(|value| value.is_finite() && *value >= 0.0)
                && (light.color * light.intensity).is_finite(),
            "lighting requires a nonzero finite direction and nonnegative finite color, intensities, and exposure"
        );
        light.direction_to_light = light.direction_to_light.normalize();
        Ok(self)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    view: GpuAddress,
    sh: GpuAddress,
    base_color: u32,
    normal_mra: u32,
    depth: u32,
    skybox: u32,
    specular: u32,
    brdf_lut: u32,
    linear_sampler: u32,
    debug_mode: u32,
    direction_to_light: [f32; 4],
    light_radiance: [f32; 4],
    sky_intensity: f32,
    exposure: f32,
    max_mip: f32,
    encode_srgb: u32,
}

pub struct DirectLightingRenderer {
    vertex: Shader,
    fragment: Shader,
    linear: Arc<Sampler>,
    pipeline: Option<(vk::Format, Arc<RasterPipeline>)>,
}
impl DirectLightingRenderer {
    pub fn new([vertex, fragment]: [Shader; 2], linear: Arc<Sampler>) -> Self {
        Self {
            vertex,
            fragment,
            linear,
            pipeline: None,
        }
    }
    pub fn render(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        scene: SceneTextures,
        ibl: IblResources,
        view_data: GpuViewData,
        settings: LightingSettings,
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
            "lighting",
            vec![
                scene.base_color.read(FRAGMENT_READ),
                scene.normal_mra.read(FRAGMENT_READ),
                scene.depth.read(FRAGMENT_READ),
                ibl.skybox.read(FRAGMENT_READ),
                ibl.sh.read(FRAGMENT_READ),
                ibl.specular.read(FRAGMENT_READ),
                ibl.brdf_lut.read(FRAGMENT_READ),
                output.write(COLOR_WRITE),
            ],
            move |ctx| {
                let view = ctx.arguments(&view_data)?;
                let sh = ctx.buffer(ibl.sh)?;
                let data = Root {
                    view: view.address(),
                    sh: sh.address(),
                    base_color: ctx.sampled(scene.base_color)?,
                    normal_mra: ctx.sampled(scene.normal_mra)?,
                    depth: ctx.sampled(scene.depth)?,
                    skybox: ctx.sampled(ibl.skybox)?,
                    specular: ctx.sampled(ibl.specular)?,
                    brdf_lut: ctx.sampled(ibl.brdf_lut)?,
                    linear_sampler: ctx.sampler(&linear)?,
                    debug_mode,
                    direction_to_light: settings
                        .directional
                        .direction_to_light
                        .extend(0.0)
                        .to_array(),
                    light_radiance: (settings.directional.color * settings.directional.intensity)
                        .extend(0.0)
                        .to_array(),
                    sky_intensity: settings.sky_intensity,
                    exposure: settings.exposure,
                    max_mip: ibl.max_mip,
                    encode_srgb: u32::from(encode_srgb),
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
