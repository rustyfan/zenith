use crate::{
    ambient_occlusion::AmbientOcclusionSettings,
    defer_shading::SceneTextures,
    helpers::*,
    ibl::IblResources,
    world::{DebugMode, GpuViewData},
};
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
    pub multiple_scattering: bool,
    pub shadows: ShadowSettings,
    pub ambient_occlusion: AmbientOcclusionSettings,
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
            multiple_scattering: true,
            shadows: ShadowSettings::default(),
            ambient_occlusion: AmbientOcclusionSettings::default(),
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
        self.ambient_occlusion.validate()?;
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
    shadows_enabled: u32,
    global_illumination: u32,
    brdf_average: u32,
    _padding: u32,
}

pub struct DirectLightingRenderer {
    linear: Arc<Sampler>,
    pipelines: [Arc<RasterPipeline>; 4],
}
impl DirectLightingRenderer {
    pub const COLOR_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

    pub fn new(
        gpu: &Arc<Gpu>,
        [vertex, fragment]: [&Shader; 2],
        linear: Arc<Sampler>,
    ) -> anyhow::Result<Self> {
        let desc = RasterDesc {
            vertex,
            fragment,
            colors: &[Self::COLOR_FORMAT],
            depth: vk::Format::UNDEFINED,
            stencil: vk::Format::UNDEFINED,
            samples: vk::SampleCountFlags::TYPE_1,
            topology: vk::PrimitiveTopology::TRIANGLE_LIST,
            blend: &[Blend::default()],
            dynamic_blend: false,
        };
        let pipeline = |furnace, multiple_scattering| {
            gpu.raster_specialized(&desc, &[], &[(0, furnace), (1, multiple_scattering)])
        };
        Ok(Self {
            linear,
            pipelines: [
                pipeline(0, 0)?,
                pipeline(0, 1)?,
                pipeline(1, 0)?,
                pipeline(1, 1)?,
            ],
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
        let furnace = debug_mode & DebugMode::WHITE_FURNACE.bits() != 0;
        let pipeline = self.pipelines
            [usize::from(furnace) * 2 + usize::from(settings.multiple_scattering)]
        .clone();
        let linear = self.linear.clone();
        let extent = vk::Extent2D {
            width: desc.extent.width,
            height: desc.extent.height,
        };
        let mut uses = vec![
            scene.base_color.read(FRAGMENT_READ),
            scene.normal_mra.read(FRAGMENT_READ),
            scene.depth.read(FRAGMENT_READ),
            scene.global_illumination.read(FRAGMENT_READ),
            ibl.skybox.read(FRAGMENT_READ),
            ibl.sh.read(FRAGMENT_READ),
            ibl.specular.read(FRAGMENT_READ),
            ibl.brdf_lut.read(FRAGMENT_READ),
            ibl.brdf_average.read(FRAGMENT_READ),
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
                shadows_enabled: u32::from(settings.shadows.enabled),
                global_illumination: ctx.sampled(scene.global_illumination)?,
                brdf_average: ctx.sampled(ibl.brdf_average)?,
                _padding: 0,
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
