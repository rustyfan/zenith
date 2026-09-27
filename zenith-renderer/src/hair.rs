use crate::{
    helpers::*,
    ibl::IblResources,
    lighting::LightingSettings,
    shadows::{SceneAcceleration, instance_transform},
    world::{GpuMesh, GpuViewData},
};
use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;
use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::*;

#[derive(Clone, Copy, Debug)]
pub struct HairSettings {
    pub lobes: u32,
    pub gaussian_reference: bool,
    pub reference_quadrature: bool,
    pub environment_samples: u32,
    pub self_shadows: bool,
}
impl Default for HairSettings {
    fn default() -> Self {
        Self {
            lobes: 15,
            gaussian_reference: false,
            reference_quadrature: false,
            environment_samples: 16,
            self_shadows: true,
        }
    }
}
impl HairSettings {
    pub(crate) fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.lobes <= 15 && (1..=512).contains(&self.environment_samples),
            "invalid hair settings"
        );
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct HairShadow {
    pub acceleration: Arc<AccelerationStructure>,
    pub materials: Arc<Memory>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    view: GpuAddress,
    vertices: GpuAddress,
    model: [f32; 16],
    hair: [f32; 8],
    direction: [f32; 4],
    radiance: [f32; 4],
    opaque_acceleration: u64,
    hair_acceleration: u64,
    shadow_materials: u64,
    skybox: u32,
    sampler: u32,
    sky_intensity: f32,
    bias: f32,
    distance: f32,
    lobes: u32,
    scattering_model: u32,
    quality: u32,
    environment_samples: u32,
    debug_mode: u32,
    depth: u32,
    paired_ribbons: u32,
    viewport: [f32; 2],
}

pub(crate) struct HairRenderer {
    pipeline: Arc<RasterPipeline>,
    composite: Arc<RasterPipeline>,
    sampler: Arc<Sampler>,
    acceleration: SceneAcceleration,
    shadow_materials: Option<(Vec<[f32; 4]>, Arc<Memory>)>,
}
impl HairRenderer {
    pub fn new(gpu: &Arc<Gpu>, sampler: Arc<Sampler>) -> Result<Self> {
        let [vertex, fragment, quad, composite] = gpu.compile_shaders([
            ("content/shaders/hair.slang", "vsmain", ShaderStage::Vertex),
            (
                "content/shaders/hair.slang",
                "psmain",
                ShaderStage::Fragment,
            ),
            (
                "content/shaders/screen_quad.slang",
                "vsmain",
                ShaderStage::Vertex,
            ),
            (
                "content/shaders/hair_composite.slang",
                "main",
                ShaderStage::Fragment,
            ),
        ])?;
        let pipeline = gpu.raster(&RasterDesc {
            vertex: &vertex,
            fragment: &fragment,
            colors: &[vk::Format::R16G16B16A16_SFLOAT, vk::Format::R16_SFLOAT],
            depth: vk::Format::UNDEFINED,
            stencil: vk::Format::UNDEFINED,
            samples: vk::SampleCountFlags::TYPE_1,
            topology: vk::PrimitiveTopology::TRIANGLE_LIST,
            blend: &[
                Blend {
                    enabled: true,
                    destination_color: vk::BlendFactor::ONE,
                    destination_alpha: vk::BlendFactor::ONE,
                    ..Default::default()
                },
                Blend {
                    enabled: true,
                    destination_color: vk::BlendFactor::ONE,
                    destination_alpha: vk::BlendFactor::ONE,
                    ..Default::default()
                },
            ],
            dynamic_blend: false,
        })?;
        Ok(Self {
            pipeline,
            composite: raster(
                gpu,
                &quad,
                &composite,
                &[vk::Format::R16G16B16A16_SFLOAT],
                vk::Format::UNDEFINED,
            )?,
            sampler,
            acceleration: SceneAcceleration::default(),
            shadow_materials: None,
        })
    }

    pub fn shadows(&mut self, gpu: &Arc<Gpu>, meshes: &[&GpuMesh]) -> Result<Option<HairShadow>> {
        if gpu.graphics_mode() == GraphicsMode::Capture {
            return Ok(None);
        }
        let instances = meshes
            .iter()
            .map(|mesh| AccelerationInstance {
                blas: mesh
                    .geometry
                    .blas
                    .clone()
                    .expect("full mode mesh acceleration"),
                transform: instance_transform(mesh.model),
            })
            .collect();
        let Some(acceleration) = self.acceleration.prepare(gpu, instances)? else {
            return Ok(None);
        };
        let data: Vec<[f32; 4]> = meshes
            .iter()
            .map(|mesh| {
                let hair = mesh.hair.unwrap();
                [
                    hair.absorption[0],
                    hair.absorption[1],
                    hair.absorption[2],
                    hair.opacity,
                ]
            })
            .collect();
        if self
            .shadow_materials
            .as_ref()
            .is_none_or(|(previous, _)| *previous != data)
        {
            let materials = gpu.allocate(
                std::mem::size_of_val(data.as_slice()) as u64,
                MemoryDomain::Upload,
            )?;
            materials.write(0, bytemuck::cast_slice(&data))?;
            self.shadow_materials = Some((data, materials));
        }
        Ok(Some(HairShadow {
            acceleration,
            materials: self.shadow_materials.as_ref().unwrap().1.clone(),
        }))
    }

    pub fn render(
        &self,
        builder: &mut RenderGraphBuilder<'_>,
        meshes: &[&GpuMesh],
        opaque: Option<Arc<AccelerationStructure>>,
        shadow: Option<HairShadow>,
        ibl: IblResources,
        view_data: GpuViewData,
        lighting: LightingSettings,
        settings: HairSettings,
        debug_mode: u32,
        depth: ImageId,
        background: ImageId,
    ) -> Result<ImageId> {
        let desc = builder.image_desc(background)?;
        let extent = vk::Extent2D {
            width: desc.extent.width,
            height: desc.extent.height,
        };
        let accumulation = builder.create_image(TextureDesc::color(
            extent.width,
            extent.height,
            vk::Format::R16G16B16A16_SFLOAT,
        ))?;
        let revealage = builder.create_image(TextureDesc::color(
            extent.width,
            extent.height,
            vk::Format::R16_SFLOAT,
        ))?;
        let mut uses = vec![
            accumulation.write(COLOR_WRITE),
            revealage.write(COLOR_WRITE),
            depth.read(FRAGMENT_READ),
            ibl.skybox.read(FRAGMENT_READ),
        ];
        if let Some(opaque) = &opaque {
            let id = builder.import_buffer(opaque.storage().clone());
            uses.push(id.read(Access::AS_FRAGMENT_READ));
        }
        let shadow_buffer = if let Some(shadow) = &shadow {
            let structure = builder.import_buffer(shadow.acceleration.storage().clone());
            let buffer = builder.import_buffer(shadow.materials.clone());
            uses.extend([
                structure.read(Access::AS_FRAGMENT_READ),
                buffer.read(FRAGMENT_READ),
            ]);
            Some(buffer)
        } else {
            None
        };
        let mut draws = Vec::new();
        for mesh in meshes {
            let vertices = builder.import_buffer(mesh.geometry.vertices.clone());
            let indices = builder.import_buffer(mesh.geometry.indices.clone());
            uses.extend([vertices.read(VERTEX_READ), indices.read(INDEX_READ)]);
            draws.push((
                vertices,
                indices,
                mesh.model,
                mesh.hair.unwrap(),
                mesh.geometry.paired_hair_ribbons,
            ));
        }
        let pipeline = self.pipeline.clone();
        let sampler = self.sampler.clone();
        builder.pass("hair_scattering", uses, move |ctx| {
            if let Some(opaque) = &opaque {
                ctx.commands.retain_acceleration_structure(opaque)?;
            }
            if let Some(shadow) = &shadow {
                ctx.commands
                    .retain_acceleration_structure(&shadow.acceleration)?;
            }
            let materials = shadow_buffer.map(|id| ctx.buffer(id)).transpose()?;
            let view = ctx.arguments(&view_data)?;
            let accumulation = ctx.view(accumulation)?;
            let revealage = ctx.view(revealage)?;
            let sampler = ctx.sampler(&sampler)?;
            ctx.commands.begin_rendering(
                &[
                    Attachment {
                        view: &accumulation,
                        clear: Some([0.0; 4]),
                        store: true,
                        resolve: None,
                    },
                    Attachment {
                        view: &revealage,
                        clear: Some([0.0; 4]),
                        store: true,
                        resolve: None,
                    },
                ],
                None,
                extent,
            )?;
            viewport(ctx.commands, extent)?;
            ctx.commands.raster_state(RasterState {
                cull: vk::CullModeFlags::NONE,
                depth_test: false,
                depth_write: false,
                ..Default::default()
            })?;
            for (vertices, indices, model, hair, paired_ribbons) in draws {
                let vertices = ctx.buffer(vertices)?;
                let indices = ctx.buffer(indices)?;
                let data = Root {
                    view: view.address(),
                    vertices: vertices.address(),
                    model,
                    hair: [
                        hair.absorption[0],
                        hair.absorption[1],
                        hair.absorption[2],
                        hair.longitudinal_roughness,
                        hair.azimuthal_roughness,
                        hair.cuticle_tilt,
                        hair.ior,
                        hair.opacity,
                    ],
                    direction: lighting
                        .directional
                        .direction_to_light
                        .extend(0.0)
                        .to_array(),
                    radiance: (lighting.directional.color * lighting.directional.intensity)
                        .extend(0.0)
                        .to_array(),
                    opaque_acceleration: opaque.as_ref().map_or(0, |a| a.address().value()),
                    hair_acceleration: shadow
                        .as_ref()
                        .map_or(0, |s| s.acceleration.address().value()),
                    shadow_materials: materials.as_ref().map_or(0, |m| m.address().value()),
                    skybox: ctx.sampled(ibl.skybox)?,
                    sampler,
                    sky_intensity: lighting.sky_intensity,
                    bias: lighting.shadows.bias,
                    distance: lighting.shadows.max_distance,
                    lobes: settings.lobes,
                    scattering_model: u32::from(settings.gaussian_reference),
                    quality: u32::from(settings.reference_quadrature),
                    environment_samples: settings.environment_samples,
                    debug_mode,
                    depth: ctx.sampled(depth)?,
                    paired_ribbons: u32::from(paired_ribbons),
                    viewport: [extent.width as f32, extent.height as f32],
                };
                let root = ctx.arguments(&data)?;
                let mut keep = vec![vertices, view.clone()];
                keep.extend(materials.clone());
                unsafe {
                    ctx.commands.draw_indexed(
                        &pipeline,
                        &root,
                        &root,
                        &indices,
                        vk::IndexType::UINT32,
                        0,
                        0..1,
                        &keep,
                    )?;
                }
            }
            ctx.commands.end_rendering()
        })?;
        let output = builder.create_image(desc)?;
        let pipeline = self.composite.clone();
        builder.pass(
            "hair_composite",
            vec![
                background.read(FRAGMENT_READ),
                accumulation.read(FRAGMENT_READ),
                revealage.read(FRAGMENT_READ),
                output.write(COLOR_WRITE),
            ],
            move |ctx| {
                let data = [
                    ctx.sampled(background)?,
                    ctx.sampled(accumulation)?,
                    ctx.sampled(revealage)?,
                ];
                let root = ctx.arguments(&data)?;
                let target = ctx.view(output)?;
                ctx.commands.begin_rendering(
                    &[Attachment {
                        view: &target,
                        clear: Some([0.0; 4]),
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
        )?;
        Ok(output)
    }
}
