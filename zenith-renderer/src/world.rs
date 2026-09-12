use crate::{
    defer_shading::SceneTextures, helpers::*, ibl::ImageBasedLightingRenderer,
    lighting::DirectLightingRenderer,
};
use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use std::sync::Arc;
use zenith_asset::{
    material::Material,
    mesh::{Mesh, Scene},
    AssetHandle,
};
use zenith_core::camera::{Camera, ViewData, WORLD_SPACE_UP};
use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::*;

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct DebugMode: u32 {
        const DIFFUSE_SH = 1 << 0;
    }
}

struct GpuMesh {
    vertices: Arc<Memory>,
    indices: Arc<Memory>,
    model: [f32; 16],
    base_color: [f32; 4],
    textures: [Option<Arc<ImageBinding>>; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    view: GpuAddress,
    vertices: GpuAddress,
    model: [f32; 16],
    base_color: [f32; 4],
    base_texture: u32,
    mra_texture: u32,
    normal_texture: u32,
    sampler: u32,
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
    pipeline: Arc<RasterPipeline>,
    meshes: Vec<GpuMesh>,
    sampler: Arc<Sampler>,
    debug_mode: DebugMode,
    lighting: DirectLightingRenderer,
    ibl: ImageBasedLightingRenderer,
}

impl WorldRenderer {
    pub fn new(
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        _width: u32,
        _height: u32,
    ) -> Result<Self> {
        let vertex = gpu.compile_shader(
            "content/shaders/defer_shading.slang",
            "vsmain",
            ShaderStage::Vertex,
        )?;
        let fragment = gpu.compile_shader(
            "content/shaders/defer_shading.slang",
            "psmain",
            ShaderStage::Fragment,
        )?;
        let sampler = descriptors.sampler(vk::Filter::LINEAR, vk::SamplerAddressMode::REPEAT)?;
        let linear_clamp =
            descriptors.sampler(vk::Filter::LINEAR, vk::SamplerAddressMode::CLAMP_TO_EDGE)?;
        let nearest_clamp =
            descriptors.sampler(vk::Filter::NEAREST, vk::SamplerAddressMode::CLAMP_TO_EDGE)?;
        Ok(Self {
            pipeline: raster(
                gpu,
                &vertex,
                &fragment,
                &[vk::Format::R8G8B8A8_UNORM; 2],
                vk::Format::D32_SFLOAT,
            )?,
            meshes: Vec::new(),
            sampler,
            debug_mode: DebugMode::empty(),
            lighting: DirectLightingRenderer::new(gpu, linear_clamp.clone(), nearest_clamp)?,
            ibl: ImageBasedLightingRenderer::new(gpu, descriptors, linear_clamp)?,
        })
    }
    pub fn resize(&mut self, _width: u32, _height: u32) {}
    pub fn set_debug_mode(&mut self, debug_mode: DebugMode) {
        self.debug_mode = debug_mode;
    }
    pub fn set_skybox(
        &mut self,
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        texture: &zenith_asset::texture::Texture,
    ) -> Result<()> {
        self.ibl.set_skybox(gpu, descriptors, texture)
    }
    pub fn add_scene(
        &mut self,
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        scene: AssetHandle<Scene>,
    ) -> Result<()> {
        let scene = scene.get().context("scene is not loaded")?;
        let mut commands = gpu.commands()?;
        let mut meshes = Vec::new();
        let mut textures = std::collections::HashMap::new();
        for url in scene.iter() {
            let handle = AssetHandle::<Mesh>::new(url.clone());
            let mesh = handle.get().context("mesh is not loaded")?;
            let material_handle = mesh
                .material
                .as_ref()
                .map(|url| AssetHandle::<Material>::new(url.clone()));
            let Some(material_handle) = material_handle else {
                continue;
            };
            let material = material_handle.get().context("material is not loaded")?;
            let mut bindings = [None, None, None];
            for (slot, url) in bindings.iter_mut().zip([
                &material.base_color_tex,
                &material.mra_tex,
                &material.normal_tex,
            ]) {
                if let Some(url) = url {
                    if !textures.contains_key(url) {
                        let handle =
                            AssetHandle::<zenith_asset::texture::Texture>::new(url.clone());
                        let asset = handle.get().context("material texture is not loaded")?;
                        let texture = upload_texture(gpu, &mut commands, &asset)?;
                        textures.insert(
                            url.clone(),
                            descriptors.image(&texture.full_view()?, false)?,
                        );
                    }
                    *slot = textures.get(url).cloned();
                }
            }
            meshes.push(GpuMesh {
                vertices: upload_buffer(gpu, &mut commands, mesh.vertices_bytes())?,
                indices: upload_buffer(gpu, &mut commands, mesh.indices_bytes())?,
                model: Mat4::from_axis_angle(WORLD_SPACE_UP, 90.0f32.to_radians()).to_cols_array(),
                base_color: material.base_color,
                textures: bindings,
            });
        }
        commands.barrier(Access::COPY_WRITE, Access::ALL)?;
        commands.submit()?.wait(10_000_000_000)?;
        self.meshes.extend(meshes);
        Ok(())
    }
    pub fn render(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        camera: &Camera,
        output: ImageId,
    ) -> Result<()> {
        let sh = self.ibl.render(builder)?;
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
        for mesh in &self.meshes {
            let vertices = builder.import_buffer(mesh.vertices.clone());
            let indices = builder.import_buffer(mesh.indices.clone());
            uses.extend([vertices.read(VERTEX_READ), indices.read(INDEX_READ)]);
            let mut images = [None; 3];
            for (slot, binding) in images.iter_mut().zip(&mesh.textures) {
                if let Some(binding) = binding {
                    let id = builder.import_sampled(binding.clone())?;
                    uses.push(id.read(FRAGMENT_READ));
                    *slot = Some(id);
                }
            }
            draws.push((vertices, indices, mesh.model, mesh.base_color, images));
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
            ctx.commands.raster_state(RasterState {
                cull: vk::CullModeFlags::BACK,
                depth_test: true,
                depth_write: true,
                depth_compare: vk::CompareOp::GREATER_OR_EQUAL,
                ..Default::default()
            })?;
            for (vertices, indices, model, base_color, images) in draws {
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
                    base_color,
                    base_texture: texture_indices[0],
                    mra_texture: texture_indices[1],
                    normal_texture: texture_indices[2],
                    sampler,
                })?;
                unsafe {
                    ctx.commands.draw_indexed(
                        &pipeline,
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
        let skybox = builder.import_sampled(self.ibl.skybox.clone())?;
        self.lighting.render(
            builder,
            scene,
            skybox,
            sh,
            view_data,
            self.debug_mode.bits(),
            output,
        )
    }
}
