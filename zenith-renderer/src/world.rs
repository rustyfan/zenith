use crate::{
    ambient_occlusion::AmbientOcclusionRenderer,
    defer_shading::SceneTextures,
    gpu_assets::{AssetUploadStats, Geometry, GpuAssets, UploadTicket},
    helpers::*,
    ibl::ImageBasedLightingRenderer,
    lighting::{DirectLightingRenderer, LightingSettings},
    post_processing::{PostProcessingRenderer, PostProcessingSettings},
    shadows::{instance_transform, SceneAcceleration},
};
use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use std::sync::Arc;
use zenith_asset::{
    mesh::Scene, texture::Texture as CpuTexture, Asset, AssetError, AssetServer, ErrorKind, Handle,
    LoadState, Revision,
};
use zenith_core::camera::{Camera, ViewData};
use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::*;

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct DebugMode: u32 {
        const DIFFUSE_SH = 1 << 0;
        const AMBIENT_OCCLUSION = 1 << 1;
        const WHITE_FURNACE = 1 << 2;
    }
}

struct GpuMesh {
    geometry: Arc<Geometry>,
    model: [f32; 16],
    normal_matrix: [f32; 16],
    mirrored: bool,
    base_color: [f32; 4],
    metallic: f32,
    roughness: f32,
    textures: [Option<Arc<ImageBinding>>; 3],
}
struct SceneEntry {
    handle: Handle<Scene>,
    revision: Option<Revision>,
    meshes: Vec<GpuMesh>,
    stream: StreamState,
    visible: bool,
}
struct SkyboxEntry {
    handle: Handle<CpuTexture>,
    revision: Option<Revision>,
    stream: StreamState,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneStatus {
    Waiting,
    Uploading,
    Ready { revision: Revision },
    Failed { message: String },
}
#[derive(Default)]
struct StreamState {
    failure: Option<(Option<Revision>, String)>,
    restoring: bool,
}
impl StreamState {
    fn fail<T: Asset>(&mut self, handle: &Handle<T>, error: impl std::fmt::Display) {
        let revision = handle.revision();
        if self
            .failure
            .as_ref()
            .is_none_or(|(failed, _)| *failed != revision)
        {
            let message = error.to_string();
            zenith_core::log::error!("Asset {:?}: {message}", handle.address());
            self.failure = Some((revision, message));
        }
    }
    fn needs_update<T: Asset>(&mut self, handle: &Handle<T>, current: Option<Revision>) -> bool {
        if matches!(
            handle.state(),
            LoadState::Loading | LoadState::Reloading { .. }
        ) {
            self.failure = None;
            return false;
        }
        if let Some(error) = handle.last_error() {
            self.fail(handle, error);
            return false;
        }
        if self
            .failure
            .as_ref()
            .is_some_and(|(revision, _)| *revision == handle.revision())
        {
            return false;
        }
        self.failure = None;
        current.is_none() || current != handle.revision()
    }
    fn restore<T: zenith_asset::CookedAsset>(
        &mut self,
        server: Option<&AssetServer>,
        handle: &Handle<T>,
    ) {
        let result = if self.restoring {
            Err(anyhow::anyhow!(
                "required CPU data remains unavailable after restoration"
            ))
        } else {
            restore(server, handle)
        };
        match result {
            Ok(()) => self.restoring = true,
            Err(error) => self.fail(handle, error),
        }
    }
}
fn restore<T: zenith_asset::CookedAsset>(
    server: Option<&AssetServer>,
    handle: &Handle<T>,
) -> Result<()> {
    server.context("CPU data is unavailable; bind its AssetServer with with_asset_server or reload it explicitly")?.reload(handle)?;
    Ok(())
}
enum Prepared {
    Scene {
        index: usize,
        revision: Revision,
        meshes: Vec<GpuMesh>,
    },
    Skybox {
        handle: Handle<CpuTexture>,
        revision: Revision,
        binding: Arc<ImageBinding>,
    },
}
struct Pending {
    ticket: UploadTicket,
    value: Prepared,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    view: GpuAddress,
    vertices: GpuAddress,
    model: [f32; 16],
    normal_matrix: [f32; 16],
    base_color: [f32; 4],
    base_texture: u32,
    mra_texture: u32,
    normal_texture: u32,
    sampler: u32,
    metallic: f32,
    roughness: f32,
    tangent_sign: f32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuViewData {
    view_proj: [f32; 16],
    inv_view_proj: [f32; 16],
    view: [f32; 16],
    inv_view: [f32; 16],
    proj: [f32; 16],
    inv_proj: [f32; 16],
    position: [f32; 4],
}
impl GpuViewData {
    fn new(view: &ViewData) -> Self {
        Self {
            view_proj: view.view_proj.to_cols_array(),
            inv_view_proj: view.inv_view_proj.to_cols_array(),
            view: view.view.to_cols_array(),
            inv_view: view.inv_view.inverse().to_cols_array(),
            proj: view.proj.to_cols_array(),
            inv_proj: view.inv_proj.inverse().to_cols_array(),
            position: view.position.to_array(),
        }
    }
}

pub struct WorldRenderer {
    gpu: Arc<Gpu>,
    pipeline: Option<Arc<RasterPipeline>>,
    geometry_job: Option<std::thread::JoinHandle<Result<Arc<RasterPipeline>>>>,
    scenes: Vec<SceneEntry>,
    skybox: Option<SkyboxEntry>,
    assets: GpuAssets,
    server: Option<AssetServer>,
    pending: Option<Pending>,
    sampler: Arc<Sampler>,
    debug_mode: DebugMode,
    lighting: DirectLightingRenderer,
    ambient_occlusion: AmbientOcclusionRenderer,
    lighting_settings: LightingSettings,
    post_processing: PostProcessingRenderer,
    post_processing_settings: PostProcessingSettings,
    acceleration: SceneAcceleration,
    ibl: ImageBasedLightingRenderer,
}

impl WorldRenderer {
    pub fn new(
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        _width: u32,
        _height: u32,
    ) -> Result<Self> {
        let [vertex, fragment, ao, ao_upsample, post_processing, ibl, specular, lut, average] = gpu
            .compile_shaders([
                (
                    "content/shaders/screen_quad.slang",
                    "vsmain",
                    ShaderStage::Vertex,
                ),
                (
                    "content/shaders/lighting.slang",
                    "main",
                    ShaderStage::Fragment,
                ),
                (
                    "content/shaders/ambient_occlusion.slang",
                    "main",
                    ShaderStage::Fragment,
                ),
                (
                    "content/shaders/ao_upsample.slang",
                    "main",
                    ShaderStage::Fragment,
                ),
                (
                    "content/shaders/post_processing.slang",
                    "main",
                    ShaderStage::Fragment,
                ),
                (
                    "content/shaders/ibl_diffuse.slang",
                    "main",
                    ShaderStage::Compute,
                ),
                (
                    "content/shaders/ibl_specular.slang",
                    "main",
                    ShaderStage::Compute,
                ),
                (
                    "content/shaders/brdf_lut.slang",
                    "main",
                    ShaderStage::Compute,
                ),
                (
                    "content/shaders/brdf_average.slang",
                    "main",
                    ShaderStage::Compute,
                ),
            ])?;
        let sampler = descriptors.sampler(vk::Filter::LINEAR, vk::SamplerAddressMode::REPEAT)?;
        let linear_clamp =
            descriptors.sampler(vk::Filter::LINEAR, vk::SamplerAddressMode::CLAMP_TO_EDGE)?;
        Ok(Self {
            gpu: gpu.clone(),
            pipeline: None,
            geometry_job: None,
            scenes: Vec::new(),
            skybox: None,
            assets: GpuAssets::new(gpu, descriptors),
            server: None,
            pending: None,
            sampler,
            debug_mode: DebugMode::empty(),
            lighting_settings: LightingSettings::default(),
            acceleration: SceneAcceleration::default(),
            lighting: DirectLightingRenderer::new(gpu, [&vertex, &fragment], linear_clamp.clone())?,
            ambient_occlusion: AmbientOcclusionRenderer::new(gpu, [&vertex, &ao, &ao_upsample])?,
            post_processing: PostProcessingRenderer::new([vertex, post_processing]),
            post_processing_settings: PostProcessingSettings::default(),
            ibl: ImageBasedLightingRenderer::new(
                gpu,
                descriptors,
                linear_clamp,
                [&ibl, &specular, &lut, &average],
            )?,
        })
    }
    pub fn resize(&mut self, _width: u32, _height: u32) {}
    fn geometry_ready(&mut self, wait: bool) -> Result<bool> {
        if self.pipeline.is_some() {
            return Ok(true);
        }
        if self.geometry_job.is_none() {
            let gpu = self.gpu.clone();
            self.geometry_job = Some(
                std::thread::Builder::new()
                    .name("zenith-geometry".into())
                    .spawn(move || {
                        let [vertex, fragment] = gpu.compile_shaders([
                            (
                                "content/shaders/defer_shading.slang",
                                "vsmain",
                                ShaderStage::Vertex,
                            ),
                            (
                                "content/shaders/defer_shading.slang",
                                "psmain",
                                ShaderStage::Fragment,
                            ),
                        ])?;
                        raster(
                            &gpu,
                            &vertex,
                            &fragment,
                            &SceneTextures::COLOR_FORMATS,
                            vk::Format::D32_SFLOAT,
                        )
                    })?,
            );
        }
        if !wait && !self.geometry_job.as_ref().unwrap().is_finished() {
            return Ok(false);
        }
        self.pipeline = Some(
            self.geometry_job
                .take()
                .unwrap()
                .join()
                .map_err(|_| anyhow::anyhow!("geometry pipeline worker panicked"))??,
        );
        Ok(true)
    }
    pub fn with_asset_server(mut self, server: &AssetServer) -> Self {
        self.server = Some(server.clone());
        self
    }
    pub fn set_staging_cache_budget(&mut self, bytes: u64) {
        self.assets.set_staging_cache_budget(bytes);
    }
    pub fn scene_status(&self, index: usize) -> Option<SceneStatus> {
        let entry = self.scenes.get(index)?;
        if self.pending.as_ref().is_some_and(|pending| matches!(pending.value, Prepared::Scene { index: pending_index, .. } if index == pending_index)) {
            return Some(SceneStatus::Uploading);
        }
        if let Some((_, message)) = &entry.stream.failure {
            return Some(SceneStatus::Failed {
                message: message.clone(),
            });
        }
        if let Some(error) = entry.handle.last_error() {
            return Some(SceneStatus::Failed {
                message: error.to_string(),
            });
        }
        Some(match entry.revision {
            Some(revision)
                if Some(revision) == entry.handle.revision()
                    && !matches!(entry.handle.state(), LoadState::Reloading { .. }) =>
            {
                SceneStatus::Ready { revision }
            }
            _ => SceneStatus::Waiting,
        })
    }
    pub fn retry_scene(&mut self, index: usize) -> Result<()> {
        let entry = self.scenes.get_mut(index).context("invalid scene index")?;
        restore(self.server.as_ref(), &entry.handle)?;
        entry.stream = StreamState::default();
        Ok(())
    }
    pub fn set_debug_mode(&mut self, debug_mode: DebugMode) {
        self.debug_mode = debug_mode;
    }
    pub fn lighting_settings(&self) -> LightingSettings {
        self.lighting_settings
    }
    pub fn set_lighting(&mut self, settings: LightingSettings) -> Result<()> {
        self.lighting_settings = settings.validated()?;
        Ok(())
    }
    pub fn post_processing_settings(&self) -> PostProcessingSettings {
        self.post_processing_settings
    }
    pub fn set_post_processing(&mut self, settings: PostProcessingSettings) -> Result<()> {
        self.post_processing_settings = settings.validated()?;
        Ok(())
    }
    pub fn upload_stats(&self) -> AssetUploadStats {
        self.assets.stats()
    }

    pub fn queue_scene(&mut self, scene: &Handle<Scene>) -> usize {
        let index = self.scenes.len();
        self.scenes.push(SceneEntry {
            handle: scene.clone(),
            revision: None,
            meshes: Vec::new(),
            stream: StreamState::default(),
            visible: true,
        });
        index
    }

    pub fn set_scene_visible(&mut self, index: usize, visible: bool) -> Result<()> {
        self.scenes
            .get_mut(index)
            .context("invalid scene index")?
            .visible = visible;
        Ok(())
    }

    pub fn queue_skybox(&mut self, texture: &Handle<CpuTexture>) {
        self.skybox = Some(SkyboxEntry {
            handle: texture.clone(),
            revision: None,
            stream: StreamState::default(),
        });
    }

    pub fn add_scene(
        &mut self,
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        scene: &Handle<Scene>,
    ) -> Result<()> {
        self.assets.check_device(gpu, descriptors)?;
        self.complete_pending(true)?;
        if !matches!(scene.state(), LoadState::CpuReleased { .. }) {
            scene.wait()?;
        }
        let index = self.queue_scene(scene);
        let prepared = (|| {
            self.geometry_ready(true)?;
            if let Some(pending) = self.prepare_scene(index)? {
                return Ok(pending);
            }
            restore(self.server.as_ref(), scene)?;
            scene.wait()?;
            self.prepare_scene(index)?
                .context("scene CPU data could not be restored")
        })();
        match prepared {
            Ok(pending) => self.pending = Some(pending),
            Err(error) => {
                self.scenes.pop();
                return Err(error);
            }
        }
        self.complete_pending(true)
    }

    pub fn set_skybox(
        &mut self,
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        texture: &Handle<CpuTexture>,
    ) -> Result<()> {
        self.assets.check_device(gpu, descriptors)?;
        self.complete_pending(true)?;
        if !matches!(texture.state(), LoadState::CpuReleased { .. }) {
            texture.wait()?;
        }
        let pending = if let Some(pending) = self.prepare_skybox(texture)? {
            pending
        } else {
            restore(self.server.as_ref(), texture)?;
            texture.wait()?;
            self.prepare_skybox(texture)?
                .context("skybox CPU data could not be restored")?
        };
        self.queue_skybox(texture);
        self.pending = Some(pending);
        self.complete_pending(true)
    }

    fn prepare_scene(&mut self, index: usize) -> Result<Option<Pending>> {
        let handle = self.scenes[index].handle.clone();
        handle.with_snapshot(|snapshot| {
            let Some(scene) = snapshot else {
                return Ok(None);
            };
            for instance in &scene.instances {
                if Mat4::from_cols_array(&instance.transform)
                    .determinant()
                    .abs()
                    < 1e-12
                {
                    continue;
                }
                let Some(material) = instance.material.get() else {
                    return Ok(None);
                };
                if self.assets.cached_mesh(&instance.mesh).is_none() {
                    let Some(mesh) = instance.mesh.get() else {
                        return Ok(None);
                    };
                    mesh.validate()?;
                }
                for texture in [
                    &material.base_color_tex,
                    &material.mra_tex,
                    &material.normal_tex,
                ]
                .into_iter()
                .flatten()
                {
                    if self.assets.cached_texture(texture).is_none() {
                        let Some(texture) = texture.get() else {
                            return Ok(None);
                        };
                        texture.validate()?;
                    }
                }
            }
            let mut upload = self.assets.begin()?;
            let mut meshes = Vec::with_capacity(scene.instances.len());
            for instance in &scene.instances {
                let model = Mat4::from_cols_array(&instance.transform);
                let determinant = model.determinant();
                if determinant.abs() < 1e-12 {
                    continue;
                }
                let material = instance
                    .material
                    .snapshot()
                    .context("material is not loaded")?;
                let mut textures = [None, None, None];
                for (slot, texture) in textures.iter_mut().zip([
                    &material.base_color_tex,
                    &material.mra_tex,
                    &material.normal_tex,
                ]) {
                    if let Some(texture) = texture {
                        *slot = Some(self.assets.texture(&mut upload, texture)?);
                    }
                }
                meshes.push(GpuMesh {
                    geometry: self.assets.mesh(&mut upload, &instance.mesh)?,
                    model: model.to_cols_array(),
                    normal_matrix: model.inverse().transpose().to_cols_array(),
                    mirrored: determinant < 0.0,
                    base_color: material.base_color,
                    metallic: material.metallic,
                    roughness: material.roughness,
                    textures,
                });
                upload.consumed(&instance.material, &material);
            }
            upload.consumed(&handle, &scene);
            Ok(Some(Pending {
                ticket: self.assets.submit(upload)?,
                value: Prepared::Scene {
                    index,
                    revision: scene.revision,
                    meshes,
                },
            }))
        })
    }

    fn prepare_skybox(&mut self, handle: &Handle<CpuTexture>) -> Result<Option<Pending>> {
        handle.with_snapshot(|snapshot| {
            let cube = if let Some(binding) = self.assets.cached_texture(handle) {
                binding.view().texture().desc().cube
            } else if let Some(texture) = snapshot {
                texture.validate()?;
                texture.is_cubemap
            } else {
                return Ok(None);
            };
            if !cube {
                return Err(
                    AssetError::new(ErrorKind::InvalidData, "skybox must be a cubemap").into(),
                );
            }
            let mut upload = self.assets.begin()?;
            let binding = self.assets.texture(&mut upload, handle)?;
            Ok(Some(Pending {
                ticket: self.assets.submit(upload)?,
                value: Prepared::Skybox {
                    handle: handle.clone(),
                    revision: handle.revision().context("skybox has no revision")?,
                    binding,
                },
            }))
        })
    }

    fn complete_pending(&mut self, wait: bool) -> Result<()> {
        let Some(pending) = &mut self.pending else {
            return Ok(());
        };
        if wait {
            pending.ticket.wait()?;
        } else if !pending.ticket.poll()? {
            return Ok(());
        }
        let mut pending = self.pending.take().unwrap();
        let accepted = match pending.value {
            Prepared::Scene {
                index,
                revision,
                meshes,
            } => {
                self.scenes[index].meshes = meshes;
                self.scenes[index].revision = Some(revision);
                self.scenes[index].stream = StreamState::default();
                true
            }
            Prepared::Skybox {
                handle,
                revision,
                binding,
            } => {
                if let Some(skybox) = self
                    .skybox
                    .as_mut()
                    .filter(|skybox| skybox.handle == handle)
                {
                    self.ibl.set_skybox(binding)?;
                    skybox.revision = Some(revision);
                    skybox.stream = StreamState::default();
                    true
                } else {
                    false
                }
            }
        };
        if accepted {
            pending.ticket.release_cpu();
        }
        self.assets.recycle(pending.ticket);
        Ok(())
    }

    fn update_assets(&mut self) -> Result<()> {
        self.complete_pending(false)?;
        if self.pending.is_some() {
            return Ok(());
        }
        for index in 0..self.scenes.len() {
            let entry = &mut self.scenes[index];
            if !entry.stream.needs_update(&entry.handle, entry.revision) {
                continue;
            }
            if !self.geometry_ready(false)? {
                continue;
            }
            match self.prepare_scene(index) {
                Ok(Some(pending)) => {
                    self.pending = Some(pending);
                    return Ok(());
                }
                Ok(None) => {
                    let entry = &mut self.scenes[index];
                    entry.stream.restore(self.server.as_ref(), &entry.handle);
                }
                Err(error) if error.downcast_ref::<AssetError>().is_some() => {
                    let entry = &mut self.scenes[index];
                    entry.stream.fail(&entry.handle, error);
                }
                Err(error) => return Err(error),
            }
        }
        let skybox = self.skybox.as_mut().and_then(|skybox| {
            skybox
                .stream
                .needs_update(&skybox.handle, skybox.revision)
                .then(|| skybox.handle.clone())
        });
        if let Some(handle) = skybox {
            match self.prepare_skybox(&handle) {
                Ok(Some(pending)) => self.pending = Some(pending),
                Ok(None) => self
                    .skybox
                    .as_mut()
                    .unwrap()
                    .stream
                    .restore(self.server.as_ref(), &handle),
                Err(error) if error.downcast_ref::<AssetError>().is_some() => {
                    self.skybox.as_mut().unwrap().stream.fail(&handle, error)
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    pub fn render(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        camera: &Camera,
        output: ImageId,
    ) -> Result<()> {
        self.update_assets()?;
        let instances = if self.lighting_settings.shadows.enabled
            || self.lighting_settings.ambient_occlusion.enabled
        {
            self.scenes
                .iter()
                .filter(|scene| scene.visible)
                .flat_map(|scene| &scene.meshes)
                .map(|mesh| AccelerationInstance {
                    blas: mesh.geometry.blas.clone(),
                    transform: instance_transform(mesh.model),
                })
                .collect()
        } else {
            Vec::new()
        };
        let acceleration = self.acceleration.prepare(builder.gpu(), instances)?;
        let ibl = self.ibl.render(builder)?;
        let desc = builder.image_desc(output)?;
        let extent = vk::Extent2D {
            width: desc.extent.width,
            height: desc.extent.height,
        };
        let scene = SceneTextures::new(builder, extent.width, extent.height)?;
        let mut uses = vec![
            scene.base_color.write(COLOR_WRITE),
            scene.normal_mra.write(COLOR_WRITE),
            scene.depth.write(DEPTH_WRITE),
        ];
        let mut draws = Vec::new();
        for mesh in self
            .scenes
            .iter()
            .filter(|scene| scene.visible)
            .flat_map(|scene| &scene.meshes)
        {
            let vertices = builder.import_buffer(mesh.geometry.vertices.clone());
            let indices = builder.import_buffer(mesh.geometry.indices.clone());
            uses.extend([vertices.read(VERTEX_READ), indices.read(INDEX_READ)]);
            let mut images = [None; 3];
            for (slot, binding) in images.iter_mut().zip(&mesh.textures) {
                if let Some(binding) = binding {
                    let id = builder.import_sampled(binding.clone())?;
                    uses.push(id.read(FRAGMENT_READ));
                    *slot = Some(id);
                }
            }
            draws.push((
                vertices,
                indices,
                mesh.model,
                mesh.normal_matrix,
                mesh.mirrored,
                mesh.base_color,
                mesh.metallic,
                mesh.roughness,
                images,
            ));
        }
        let view_data = GpuViewData::new(&camera.view_data());
        let pipeline = self.pipeline.clone();
        let sampler = self.sampler.clone();
        let (base, nmr, depth) = (scene.base_color, scene.normal_mra, scene.depth);
        builder.pass("gbuffer", uses, move |ctx| {
            let view = ctx.arguments(&view_data)?;
            let base = ctx.view(base)?;
            let nmr = ctx.view(nmr)?;
            let depth = ctx.view(depth)?;
            let sampler = ctx.sampler(&sampler)?;
            ctx.commands.begin_rendering(
                &[
                    Attachment {
                        view: &base,
                        clear: Some([0.05, 0.05, 0.05, 1.0]),
                        store: true,
                        resolve: None,
                    },
                    Attachment {
                        view: &nmr,
                        clear: Some([0.5, 0.5, 1.0, 1.0]),
                        store: true,
                        resolve: None,
                    },
                ],
                Some(DepthAttachment {
                    view: &depth,
                    clear: Some((0.0, 0)),
                    store: true,
                }),
                extent,
            )?;
            viewport(ctx.commands, extent)?;
            for (
                vertices,
                indices,
                model,
                normal_matrix,
                mirrored,
                base_color,
                metallic,
                roughness,
                images,
            ) in draws
            {
                ctx.commands.raster_state(RasterState {
                    cull: vk::CullModeFlags::BACK,
                    front: if mirrored {
                        vk::FrontFace::CLOCKWISE
                    } else {
                        vk::FrontFace::COUNTER_CLOCKWISE
                    },
                    depth_test: true,
                    depth_write: true,
                    depth_compare: vk::CompareOp::GREATER_OR_EQUAL,
                    ..Default::default()
                })?;
                let vertices = ctx.buffer(vertices)?;
                let indices = ctx.buffer(indices)?;
                let mut texture_indices = [u32::MAX; 3];
                for (slot, image) in texture_indices.iter_mut().zip(images) {
                    if let Some(image) = image {
                        *slot = ctx.sampled(image)?;
                    }
                }
                let root = ctx.arguments(&Root {
                    view: view.address(),
                    vertices: vertices.address(),
                    model,
                    normal_matrix,
                    base_color,
                    base_texture: texture_indices[0],
                    mra_texture: texture_indices[1],
                    normal_texture: texture_indices[2],
                    sampler,
                    metallic,
                    roughness,
                    tangent_sign: if mirrored { -1.0 } else { 1.0 },
                    padding: 0,
                })?;
                unsafe {
                    ctx.commands.draw_indexed(
                        pipeline
                            .as_ref()
                            .context("geometry pipeline is not ready")?,
                        &root,
                        &root,
                        &indices,
                        vk::IndexType::UINT32,
                        0,
                        0..1,
                        &[vertices, view.clone()],
                    )?;
                }
            }
            ctx.commands.end_rendering()
        })?;
        let mut hdr_desc = TextureDesc::color(
            extent.width,
            extent.height,
            DirectLightingRenderer::COLOR_FORMAT,
        );
        hdr_desc.usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
        let hdr_color = builder.create_image(hdr_desc)?;
        self.ambient_occlusion.render(
            builder,
            acceleration.clone(),
            &scene,
            view_data,
            self.lighting_settings.ambient_occlusion,
        )?;
        self.lighting.render(
            builder,
            acceleration,
            scene,
            ibl,
            view_data,
            self.lighting_settings,
            self.debug_mode.bits(),
            hdr_color,
        )?;
        self.post_processing.render(
            builder,
            hdr_color,
            output,
            self.post_processing_settings,
            self.debug_mode
                .intersects(DebugMode::AMBIENT_OCCLUSION | DebugMode::WHITE_FURNACE),
        )
    }
}

impl Drop for WorldRenderer {
    fn drop(&mut self) {
        if let Some(job) = self.geometry_job.take() {
            let _ = job.join();
        }
    }
}

#[cfg(test)]
mod tests {
    mod ambient_occlusion;
    mod lighting;
    mod post_processing;
    mod shadows;
    mod streaming;
    use super::*;
    use std::time::{Duration, Instant};
    use zenith_asset::{
        material::{Material, MaterialData},
        mesh::{Mesh, MeshInstanceData, SceneData, SceneNode, Vertex},
        texture::TextureFormat,
        AssetServer, ImportContext, Importer, MemorySource,
    };
    use zenith_rendergraph::ResourceCache;

    struct TestScene;
    impl Importer for TestScene {
        type Settings = ();
        type Output = Scene;
        const KEY: &'static str = "test.scene";
        const VERSION: u32 = 1;
        fn extensions(&self) -> &[&str] {
            &["testscene"]
        }
        fn import(
            &self,
            bytes: &[u8],
            _: &(),
            ctx: &mut ImportContext<'_>,
        ) -> zenith_asset::Result<SceneData> {
            let texture = ctx.emit::<CpuTexture>(
                "color",
                CpuTexture {
                    width: 1,
                    height: 1,
                    format: TextureFormat::Rgba8Unorm,
                    pixels: vec![bytes[0], 32, 32, 255],
                    is_cubemap: false,
                    mip_levels: 1,
                },
            )?;
            let material = ctx.emit::<Material>(
                "material",
                MaterialData {
                    base_color_tex: Some(texture),
                    metallic: 0.0,
                    ..Default::default()
                },
            )?;
            let mesh = ctx.emit::<Mesh>(
                "mesh",
                Mesh::new(
                    vec![
                        Vertex {
                            position: [-0.8, 2.0, -0.8],
                            normal: [0.0, -1.0, 0.0],
                            tex_coord: [0.0; 2],
                            tangent: [0.0; 4],
                        },
                        Vertex {
                            position: [0.8, 2.0, -0.8],
                            normal: [0.0, -1.0, 0.0],
                            tex_coord: [0.0; 2],
                            tangent: [0.0; 4],
                        },
                        Vertex {
                            position: [0.0, 2.0, 0.8],
                            normal: [0.0, -1.0, 0.0],
                            tex_coord: [0.0; 2],
                            tangent: [0.0; 4],
                        },
                    ],
                    vec![0, 1, 2],
                ),
            )?;
            let identity = Mat4::IDENTITY.to_cols_array();
            Ok(SceneData {
                nodes: vec![SceneNode {
                    source_index: 0,
                    parent: None,
                    transform: identity,
                }],
                instances: vec![MeshInstanceData {
                    node: 0,
                    mesh,
                    material,
                    transform: identity,
                }],
            })
        }
    }
    fn frame(
        renderer: &mut WorldRenderer,
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        cache: &mut ResourceCache,
    ) -> Result<Vec<u8>> {
        frame_format(
            renderer,
            gpu,
            descriptors,
            cache,
            vk::Format::R8G8B8A8_UNORM,
        )
    }
    fn frame_format(
        renderer: &mut WorldRenderer,
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        cache: &mut ResourceCache,
        format: vk::Format,
    ) -> Result<Vec<u8>> {
        frame_extent(renderer, gpu, descriptors, cache, format, [64, 64])
    }
    fn frame_extent(
        renderer: &mut WorldRenderer,
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        cache: &mut ResourceCache,
        format: vk::Format,
        [width, height]: [u32; 2],
    ) -> Result<Vec<u8>> {
        let readback = gpu.allocate(u64::from(width * height * 4), MemoryDomain::Readback)?;
        let mut builder = RenderGraphBuilder::new(gpu, descriptors, cache)?;
        let mut desc = TextureDesc::color(width, height, format);
        desc.usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC;
        let output = builder.create_image(desc)?;
        renderer.render(&mut builder, &Camera::default(), output)?;
        let destination = builder.import_buffer(readback.clone());
        builder.pass(
            "readback",
            vec![
                output.read(Access::COPY_READ),
                destination.write(Access::COPY_WRITE),
            ],
            move |ctx| {
                let region = vk::BufferImageCopy::default()
                    .image_extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    );
                unsafe {
                    ctx.commands.read_image(
                        &ctx.image(output)?,
                        &ctx.buffer(destination)?,
                        &[region],
                    )
                }
            },
        )?;
        builder.pass(
            "host",
            vec![destination.read(Access::HOST_READ)],
            |_| Ok(()),
        )?;
        builder.record()?.submit()?.wait(10_000_000_000)?;
        let mut pixels = vec![0; (width * height * 4) as usize];
        readback.read(0, &mut pixels)?;
        Ok(pixels)
    }
    #[test]
    #[ignore = "requires Vulkan validation, Slang, and supported GPU features"]
    fn scene_cache_streams_revisions_at_frame_boundaries() -> Result<()> {
        std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
        let instance = Instance::new(&[], true)?;
        anyhow::ensure!(
            instance.validation_enabled(),
            "scene test requires Vulkan validation"
        );
        let gpu = Gpu::new(
            instance.clone(),
            std::env::var("ZENITH_ADAPTER").ok().as_deref(),
        )?;
        {
            let descriptors = Descriptors::new(&gpu, 512, 32)?;
            let source = Arc::new(MemorySource::default());
            source.insert("scene.testscene", vec![50])?;
            let assets = AssetServer::builder()
                .source(source.clone())
                .with_builtin_assets()
                .register_importer(TestScene)
                .build()?;
            let scene = assets.load_blocking::<Scene>("scene.testscene")?;
            let sky = assets.add(CpuTexture {
                width: 1,
                height: 1,
                mip_levels: 1,
                is_cubemap: true,
                format: TextureFormat::Rgba8Unorm,
                pixels: vec![255; 24],
            });
            let mut renderer = WorldRenderer::new(&gpu, &descriptors, 64, 64)?;
            renderer.add_scene(&gpu, &descriptors, &scene)?;
            renderer.set_skybox(&gpu, &descriptors, &sky)?;
            let counts = renderer.upload_stats();
            renderer.add_scene(&gpu, &descriptors, &scene)?;
            assert_eq!(renderer.upload_stats().bytes, counts.bytes);
            assert!(Arc::ptr_eq(
                &renderer.scenes[0].meshes[0].geometry,
                &renderer.scenes[1].meshes[0].geometry
            ));
            let mut cache = ResourceCache::default();
            let before = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
            assert!(before.chunks_exact(4).any(|pixel| pixel[0] < 250));
            source.insert("scene.testscene", vec![220])?;
            assets.reload(&scene)?;
            scene.wait()?;
            let revision = scene.snapshot().unwrap().revision;
            let started = Instant::now();
            while renderer.scenes.iter().any(|s| s.revision != Some(revision)) {
                anyhow::ensure!(
                    started.elapsed() < Duration::from_secs(10),
                    "scene streaming timed out"
                );
                frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
            }
            let after = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
            assert_ne!(before, after);
            assert_eq!(renderer.upload_stats().meshes, 1);
            assert_eq!(renderer.upload_stats().textures, 3);
            assert_eq!(renderer.upload_stats().staging_allocations, 1);
        }
        gpu.wait_idle()?;
        drop(gpu);
        anyhow::ensure!(
            instance.validation_errors().is_empty(),
            "scene validation errors: {:?}",
            instance.validation_errors()
        );
        Ok(())
    }
}
