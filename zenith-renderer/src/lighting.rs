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
pub struct ShadowSettings {
    pub enabled: bool,
    pub bias: f32,
    pub max_distance: f32,
}

impl Default for ShadowSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            bias: 0.001,
            max_distance: 10000.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LightingSettings {
    pub directional: DirectionalLight,
    pub sky_intensity: f32,
    pub shadows: ShadowSettings,
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
            shadows: ShadowSettings::default(),
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
                && [light.intensity, self.sky_intensity]
                    .iter().all(|value| value.is_finite() && *value >= 0.0)
                && (light.color * light.intensity).is_finite(),
            "lighting requires a nonzero finite direction and nonnegative finite color and intensities"
        );
        light.direction_to_light = light.direction_to_light.normalize();
        anyhow::ensure!(
            self.shadows.bias.is_finite()
                && self.shadows.bias > 0.0
                && self.shadows.max_distance.is_finite()
                && self.shadows.max_distance > self.shadows.bias,
            "shadows require a positive finite bias and a finite distance greater than the bias"
        );
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
    max_mip: f32,
    acceleration: u64,
    shadow_bias: f32,
    shadow_distance: f32,
}

pub struct DirectLightingRenderer {
    linear: Arc<Sampler>,
    pipeline: Arc<RasterPipeline>,
}
impl DirectLightingRenderer {
    pub const COLOR_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

    pub fn new(
        gpu: &Arc<Gpu>,
        [vertex, fragment]: [&Shader; 2],
        linear: Arc<Sampler>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            linear,
            pipeline: raster(
                gpu,
                vertex,
                fragment,
                &[Self::COLOR_FORMAT],
                vk::Format::UNDEFINED,
            )?,
        })
    }
    pub fn render(
        &self,
        builder: &mut RenderGraphBuilder<'_>,
        acceleration: Option<Arc<AccelerationStructure>>,
        scene: SceneTextures,
        ibl: IblResources,
        view_data: GpuViewData,
        settings: LightingSettings,
        debug_mode: u32,
        output: ImageId,
    ) -> anyhow::Result<()> {
        let desc = builder.image_desc(output)?;
        let pipeline = self.pipeline.clone();
        let linear = self.linear.clone();
        let extent = vk::Extent2D {
            width: desc.extent.width,
            height: desc.extent.height,
        };
        let mut uses = vec![
            scene.base_color.read(FRAGMENT_READ),
            scene.normal_mra.read(FRAGMENT_READ),
            scene.depth.read(FRAGMENT_READ),
            ibl.skybox.read(FRAGMENT_READ),
            ibl.sh.read(FRAGMENT_READ),
            ibl.specular.read(FRAGMENT_READ),
            ibl.brdf_lut.read(FRAGMENT_READ),
            output.write(COLOR_WRITE),
        ];
        if let Some(structure) = &acceleration {
            let id = builder.import_buffer(structure.storage().clone());
            uses.push(id.read(Access::AS_FRAGMENT_READ));
        }
        builder.pass("lighting", uses, move |ctx| {
            if let Some(structure) = &acceleration {
                ctx.commands.retain_acceleration_structure(structure)?;
            }
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
                max_mip: ibl.max_mip,
                acceleration: acceleration
                    .as_ref()
                    .map_or(0, |structure| structure.address().value()),
                shadow_bias: settings.shadows.bias,
                shadow_distance: settings.shadows.max_distance,
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
        })
    }
}
