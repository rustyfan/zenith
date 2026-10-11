use crate::gpu_assets::Geometry;
use crate::{ibl::IblResources, lighting::LightingSettings, world::GpuViewData};
use anyhow::{ensure, Result};
use bytemuck::{Pod, Zeroable};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc, Weak,
};
use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RenderMode {
    #[default]
    Raster,
    PathTracing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathTracingSettings {
    pub target_spp: u32,
    pub samples_per_frame: u32,
    pub max_bounces: u32,
}

impl Default for PathTracingSettings {
    fn default() -> Self {
        Self {
            target_spp: 256,
            samples_per_frame: 1,
            max_bounces: 8,
        }
    }
}

impl PathTracingSettings {
    pub(crate) fn validate(self) -> Result<()> {
        ensure!(
            self.target_spp > 0 && self.samples_per_frame > 0,
            "path tracing requires positive SPP and samples per frame"
        );
        PathTracingBatch {
            max_bounces: self.max_bounces,
            ..Default::default()
        }
        .validate()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PathTracingBatch {
    pub first_sample: u32,
    pub sample_count: u32,
    pub max_bounces: u32,
}

impl Default for PathTracingBatch {
    fn default() -> Self {
        Self {
            first_sample: 0,
            sample_count: 1,
            max_bounces: 8,
        }
    }
}

impl PathTracingBatch {
    pub(crate) fn validate(self) -> Result<()> {
        ensure!(
            self.sample_count > 0
                && self
                    .first_sample
                    .checked_add(self.sample_count - 1)
                    .is_some(),
            "invalid path tracing sample range"
        );
        ensure!(
            (1..=8).contains(&self.max_bounces),
            "path tracing requires 1 to 8 bounces"
        );
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PathTracingRoot {
    view: GpuAddress,
    instances: u64,
    acceleration: u64,
    output: u32,
    skybox: u32,
    brdf_lut: u32,
    brdf_average: u32,
    sampler: u32,
    first_sample: u32,
    sample_count: u32,
    max_bounces: u32,
    direction_to_light: [f32; 4],
    light_radiance: [f32; 4],
    sky_intensity: f32,
    ray_bias: f32,
    multiple_scattering: u32,
    accumulate: u32,
}

#[derive(PartialEq)]
struct AccumulationParameters {
    view: GpuViewData,
    direction_to_light: [f32; 3],
    light_radiance: [f32; 3],
    sky_intensity: f32,
    ray_bias: f32,
    multiple_scattering: bool,
    max_bounces: u32,
}

struct Accumulation {
    texture: Arc<Texture>,
    scene: Arc<PathTracingScene>,
    skybox: Arc<Texture>,
    parameters: AccumulationParameters,
    spp: Arc<AtomicU32>,
    pending: Weak<()>,
}

pub(crate) struct PathTracingRenderer {
    table: Arc<ShaderBindingTable>,
    sampler: Arc<Sampler>,
    accumulation: Option<Accumulation>,
}

impl PathTracingRenderer {
    pub const COLOR_FORMAT: vk::Format = vk::Format::R32G32B32A32_SFLOAT;

    pub fn new(gpu: &Arc<Gpu>, descriptors: &Arc<Descriptors>) -> Result<Self> {
        let shaders = gpu.compile_shaders([
            (
                "content/shaders/path_tracing.slang",
                "raygen",
                ShaderStage::RayGeneration,
            ),
            (
                "content/shaders/path_tracing.slang",
                "miss",
                ShaderStage::Miss,
            ),
            (
                "content/shaders/path_tracing.slang",
                "closest_hit",
                ShaderStage::ClosestHit,
            ),
        ])?;
        let pipeline = gpu.ray_tracing(&RayTracingDesc {
            shaders: &[&shaders[0], &shaders[1], &shaders[2]],
            groups: &[
                RayTracingShaderGroup::General { shader: 0 },
                RayTracingShaderGroup::General { shader: 1 },
                RayTracingShaderGroup::Triangles {
                    closest_hit: Some(2),
                    any_hit: None,
                },
            ],
            max_recursion_depth: 1,
        })?;
        Ok(Self {
            accumulation: None,
            table: gpu.shader_binding_table(
                &pipeline,
                &ShaderBindingTableDesc {
                    raygen: 0,
                    miss: &[1],
                    hit: &[2],
                    callable: &[],
                },
            )?,
            sampler: descriptors
                .sampler(vk::Filter::LINEAR, vk::SamplerAddressMode::CLAMP_TO_EDGE)?,
        })
    }

    pub fn render(
        &self,
        builder: &mut RenderGraphBuilder<'_>,
        scene: Arc<PathTracingScene>,
        ibl: IblResources,
        view: GpuViewData,
        lighting: LightingSettings,
        batch: PathTracingBatch,
        output: ImageId,
        accumulate: bool,
    ) -> Result<()> {
        batch.validate()?;
        let desc = builder.image_desc(output)?;
        ensure!(
            desc.format == Self::COLOR_FORMAT
                && desc.kind == vk::ImageType::TYPE_2D
                && desc.layers == 1
                && desc.samples == vk::SampleCountFlags::TYPE_1,
            "invalid path tracing output"
        );
        let mut uses = vec![
            if accumulate && batch.first_sample > 0 {
                output.read_write(Access {
                    stages: Access::RAY_TRACING_WRITE.stages,
                    access: Access::RAY_TRACING_READ.access | Access::RAY_TRACING_WRITE.access,
                })
            } else {
                output.write(Access::RAY_TRACING_WRITE)
            },
            ibl.skybox.read(Access::RAY_TRACING_READ),
            ibl.brdf_lut.read(Access::RAY_TRACING_READ),
            ibl.brdf_average.read(Access::RAY_TRACING_READ),
            builder
                .import_buffer(self.table.storage().clone())
                .read(Access::RAY_TRACING_READ),
        ];
        if let Some(buffer) = &scene.buffer {
            uses.push(
                builder
                    .import_buffer(buffer.clone())
                    .read(Access::RAY_TRACING_READ),
            );
        }
        if let Some(acceleration) = &scene.acceleration {
            uses.push(
                builder
                    .import_buffer(acceleration.storage().clone())
                    .read(Access::AS_RAY_TRACING_READ),
            );
        }
        for instance in &scene.instances {
            uses.extend([
                builder
                    .import_buffer(instance.geometry.vertices.clone())
                    .read(Access::RAY_TRACING_READ),
                builder
                    .import_buffer(instance.geometry.indices.clone())
                    .read(Access::RAY_TRACING_READ),
            ]);
            for texture in instance.textures.iter().flatten() {
                uses.push(
                    builder
                        .import_sampled(texture.clone())?
                        .read(Access::RAY_TRACING_READ),
                );
            }
        }
        let table = self.table.clone();
        let sampler = self.sampler.clone();
        builder.pass("path_tracing", uses, move |ctx| {
            scene.retain(ctx.commands)?;
            let view = ctx.arguments(&view)?;
            let data = PathTracingRoot {
                view: view.address(),
                instances: scene
                    .buffer
                    .as_ref()
                    .map_or(0, |buffer| buffer.address().value()),
                acceleration: scene
                    .acceleration
                    .as_ref()
                    .map_or(0, |acceleration| acceleration.address().value()),
                output: ctx.storage(output)?,
                skybox: ctx.sampled(ibl.skybox)?,
                brdf_lut: ctx.sampled(ibl.brdf_lut)?,
                brdf_average: ctx.sampled(ibl.brdf_average)?,
                sampler: ctx.sampler(&sampler)?,
                first_sample: batch.first_sample,
                sample_count: batch.sample_count,
                max_bounces: batch.max_bounces,
                direction_to_light: lighting
                    .directional
                    .direction_to_light
                    .extend(0.0)
                    .to_array(),
                light_radiance: (lighting.directional.color * lighting.directional.intensity)
                    .extend(0.0)
                    .to_array(),
                sky_intensity: lighting.sky_intensity,
                ray_bias: lighting.shadows.bias,
                multiple_scattering: u32::from(lighting.multiple_scattering),
                accumulate: u32::from(accumulate),
            };
            let root = ctx.arguments(&data)?;
            unsafe {
                ctx.commands.trace_rays(
                    &table,
                    &root,
                    [desc.extent.width, desc.extent.height, 1],
                    &[view],
                    &[],
                )
            }
        })
    }

    pub fn reset(&mut self) {
        self.accumulation = None;
    }

    pub fn spp(&self) -> u32 {
        self.accumulation
            .as_ref()
            .map_or(0, |state| state.spp.load(Ordering::Relaxed))
    }

    pub fn render_progressive(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        scene: Arc<PathTracingScene>,
        ibl: IblResources,
        view: GpuViewData,
        lighting: LightingSettings,
        settings: PathTracingSettings,
        width: u32,
        height: u32,
    ) -> Result<ImageId> {
        settings.validate()?;
        ensure!(
            self.accumulation
                .as_ref()
                .is_none_or(|state| state.pending.upgrade().is_none()),
            "submit or drop the previous progressive frame before building another"
        );
        let skybox = builder.export_image(ibl.skybox)?;
        let parameters = AccumulationParameters {
            view,
            direction_to_light: lighting.directional.direction_to_light.to_array(),
            light_radiance: (lighting.directional.color * lighting.directional.intensity)
                .to_array(),
            sky_intensity: lighting.sky_intensity,
            ray_bias: lighting.shadows.bias,
            multiple_scattering: lighting.multiple_scattering,
            max_bounces: settings.max_bounces,
        };
        let mut desc = TextureDesc::color(width, height, Self::COLOR_FORMAT);
        desc.usage = vk::ImageUsageFlags::STORAGE
            | vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_SRC;
        if self.accumulation.as_ref().is_none_or(|state| {
            state.texture.desc() != desc
                || state.parameters != parameters
                || !Arc::ptr_eq(&state.scene, &scene)
                || !Arc::ptr_eq(&state.skybox, &skybox)
        }) {
            let texture = match &self.accumulation {
                Some(state) if state.texture.desc() == desc => state.texture.clone(),
                _ => builder.gpu().texture(desc)?,
            };
            texture.set_debug_name("PathTracing/Accumulation")?;
            self.accumulation = Some(Accumulation {
                texture,
                scene: scene.clone(),
                skybox,
                parameters,
                spp: Arc::new(AtomicU32::new(0)),
                pending: Weak::new(),
            });
        }
        let state = self.accumulation.as_ref().unwrap();
        let output = builder.import_image(state.texture.clone())?;
        let first_sample = state.spp.load(Ordering::Relaxed);
        let sample_count = settings
            .samples_per_frame
            .min(settings.target_spp.saturating_sub(first_sample));
        if sample_count > 0 {
            if first_sample == 0 {
                builder.pass(
                    "initialize_path_tracing",
                    vec![output.write(Access::RAY_TRACING_WRITE)],
                    move |ctx| unsafe {
                        ctx.commands.transition(
                            &ctx.image(output)?,
                            vk::ImageLayout::UNDEFINED,
                            vk::ImageLayout::GENERAL,
                            Access::ALL,
                            Access::RAY_TRACING_WRITE,
                        )
                    },
                )?;
            }
            self.render(
                builder,
                scene,
                ibl,
                view,
                lighting,
                PathTracingBatch {
                    first_sample,
                    sample_count,
                    max_bounces: settings.max_bounces,
                },
                output,
                true,
            )?;
        }
        let spp = state.spp.clone();
        let pending = Arc::new(());
        self.accumulation.as_mut().unwrap().pending = Arc::downgrade(&pending);
        builder.pass("path_tracing_progress", Vec::new(), move |ctx| {
            ctx.commands.on_submit(move || {
                spp.store(first_sample + sample_count, Ordering::Relaxed);
                drop(pending);
            });
            Ok(())
        })?;
        Ok(output)
    }
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuPathTracingInstance {
    pub vertices: GpuAddress,
    pub indices: GpuAddress,
    pub model: [f32; 16],
    pub normal_matrix: [f32; 16],
    pub base_color: [f32; 4],
    pub base_texture: u32,
    pub mra_texture: u32,
    pub normal_texture: u32,
    pub sampler: u32,
    pub metallic: f32,
    pub roughness: f32,
    pub tangent_sign: f32,
    pub padding: u32,
}

pub(crate) struct PathTracingInstance {
    pub geometry: Arc<Geometry>,
    pub textures: [Option<Arc<ImageBinding>>; 3],
    pub data: GpuPathTracingInstance,
}

pub struct PathTracingScene {
    instances: Vec<PathTracingInstance>,
    buffer: Option<Arc<Memory>>,
    acceleration: Option<Arc<AccelerationStructure>>,
    sampler: Arc<Sampler>,
}

impl PathTracingScene {
    pub(crate) fn new(
        gpu: &Arc<Gpu>,
        instances: Vec<PathTracingInstance>,
        acceleration: Option<Arc<AccelerationStructure>>,
        sampler: Arc<Sampler>,
    ) -> Result<Arc<Self>> {
        let buffer = if instances.is_empty() {
            None
        } else {
            let data: Vec<_> = instances.iter().map(|instance| instance.data).collect();
            let bytes = bytemuck::cast_slice(&data);
            let memory = gpu.allocate(bytes.len() as u64, MemoryDomain::Upload)?;
            memory.write(0, bytes)?;
            memory.set_debug_name("PathTracing/Instances")?;
            Some(memory)
        };
        Ok(Arc::new(Self {
            instances,
            buffer,
            acceleration,
            sampler,
        }))
    }

    pub(crate) fn matches(&self, instances: &[PathTracingInstance]) -> bool {
        self.instances.len() == instances.len()
            && self
                .instances
                .iter()
                .zip(instances)
                .all(|(a, b)| a.data == b.data && Arc::ptr_eq(&a.geometry, &b.geometry))
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    pub fn instance_buffer(&self) -> Option<&Arc<Memory>> {
        self.buffer.as_ref()
    }

    pub fn acceleration(&self) -> Option<&Arc<AccelerationStructure>> {
        self.acceleration.as_ref()
    }

    pub fn retain(&self, commands: &mut Commands) -> Result<()> {
        commands.retain_sampler(&self.sampler)?;
        if let Some(buffer) = &self.buffer {
            commands.retain(&buffer.whole())?;
        }
        if let Some(acceleration) = &self.acceleration {
            commands.retain_acceleration_structure(acceleration)?;
        }
        for instance in &self.instances {
            commands.retain(&instance.geometry.vertices.whole())?;
            commands.retain(&instance.geometry.indices.whole())?;
            for texture in instance.textures.iter().flatten() {
                commands.retain_image(texture)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
