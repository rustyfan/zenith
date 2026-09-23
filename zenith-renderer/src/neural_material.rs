mod model;

use crate::helpers::*;
use anyhow::{Result, ensure};
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;
use zenith_rendergraph::{BufferId, ImageId, RenderGraphBuilder};
use zenith_rhi::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum NeuralBackend {
    Scalar = 0,
    CooperativeVector = 1,
    CooperativeMatrix = 2,
    CooperativeMatrix2 = 3,
}

impl NeuralBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Scalar => "Scalar FP32",
            Self::CooperativeVector => "Cooperative vector FP16",
            Self::CooperativeMatrix => "Cooperative matrix KHR",
            Self::CooperativeMatrix2 => "Cooperative matrix2 NV",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NeuralMaterialSettings {
    pub backend: NeuralBackend,
    pub rotation: f32,
    pub exposure: f32,
    pub error_view: bool,
    pub flat_view: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct InferenceRoot {
    weights: GpuAddress,
    half_weights: GpuAddress,
    packed: [GpuAddress; 3],
    output: GpuAddress,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DisplayRoot {
    material: GpuAddress,
    width: u32,
    height: u32,
    atlas_width: u32,
    atlas_height: u32,
    rotation: f32,
    exposure: f32,
    error_view: u32,
    flat_view: u32,
    encode_srgb: u32,
    padding: u32,
}

pub struct NeuralMaterialRenderer {
    weights: Vec<f32>,
    canonical: Arc<Memory>,
    half_weights: Option<Arc<Memory>>,
    packed: Vec<Arc<Memory>>,
    kernels: Vec<(NeuralBackend, Arc<ComputePipeline>)>,
    vertex: Shader,
    fragment: Shader,
    pipeline: Option<(vk::Format, Arc<RasterPipeline>)>,
    pub settings: NeuralMaterialSettings,
}

impl NeuralMaterialRenderer {
    pub fn new(gpu: &Arc<Gpu>) -> Result<Self> {
        let weights = model::decode(include_bytes!("../../content/neural_material.bin"))?;
        let caps = &gpu.info.cooperative;
        let half: Vec<u16> = weights.iter().map(|w| half::f16::from_f32(*w).to_bits()).collect();
        let mut commands = gpu.commands()?;
        let canonical = upload_buffer(gpu, &mut commands, bytemuck::cast_slice(&weights))?;
        let half_weights = if caps.float16 && caps.storage16 && caps.memory_model {
            Some(upload_buffer(gpu, &mut commands, bytemuck::cast_slice(&half))?)
        } else { None };
        commands.barrier(Access::COPY_WRITE, Access::ALL)?;
        commands.submit()?.wait(10_000_000_000)?;
        let mut packed = Vec::new();
        let mut variants = vec![(NeuralBackend::Scalar, vec![])];
        if caps.vector_f16(vk::ShaderStageFlags::COMPUTE) {
            for &(inputs, outputs, matrix, _) in &model::LAYERS {
                packed.push(gpu.cooperative_vector_weights(&half[matrix..matrix + inputs * outputs], outputs as u32, inputs as u32)?);
            }
            variants.push((NeuralBackend::CooperativeVector, vec!["spvCooperativeVectorNV"]));
        }
        if caps.matrix_f16(16, 16, 16) && caps.max_shared_memory >= 3072 {
            variants.push((NeuralBackend::CooperativeMatrix, vec!["spvCooperativeMatrixKHR"]));
        }
        if [(16, 32, 16), (16, 32, 32), (16, 16, 32)].iter().all(|&(m, n, k)| caps.matrix2_f16(m, n, k, 32)) {
            variants.push((NeuralBackend::CooperativeMatrix2, vec!["spvCooperativeMatrixKHR", "SPV_NV_cooperative_matrix2"]));
        }
        let mut kernels = Vec::new();
        for (backend, capabilities) in variants {
            let group = if backend == NeuralBackend::CooperativeMatrix { caps.subgroup_size } else { 32 };
            let backend_define = (backend as u32).to_string();
            let group_define = group.to_string();
            let shader = gpu.compile_shader_with_options("content/shaders/neural_material_inference.slang", "main", ShaderStage::Compute,
                ShaderOptions { capabilities: &capabilities, defines: &[("BACKEND", &backend_define), ("GROUP_SIZE", &group_define)] })?;
            kernels.push((backend, gpu.compute(&shader)?));
        }
        let backend = if kernels.iter().any(|(b, _)| *b == NeuralBackend::CooperativeVector) {
            NeuralBackend::CooperativeVector
        } else { kernels.last().unwrap().0 };
        let [vertex, fragment] = gpu.compile_shaders([
            ("content/shaders/screen_quad.slang", "vsmain", ShaderStage::Vertex),
            ("content/shaders/neural_material.slang", "main", ShaderStage::Fragment),
        ])?;
        Ok(Self { weights, canonical, half_weights, packed, kernels, vertex, fragment, pipeline: None,
            settings: NeuralMaterialSettings { backend, rotation: 0.0, exposure: 1.2, error_view: false, flat_view: false } })
    }

    pub fn backends(&self) -> impl Iterator<Item = NeuralBackend> + '_ {
        self.kernels.iter().map(|(backend, _)| *backend)
    }

    pub fn reference(uv: [f32; 2]) -> [f32; 4] { model::reference(uv) }

    pub fn evaluate(&self, uv: [f32; 2]) -> [f32; 4] { model::evaluate(&self.weights, uv) }

    pub fn reconstruct(&self, builder: &mut RenderGraphBuilder<'_>, width: u32, height: u32) -> Result<BufferId> {
        let count = width.checked_mul(height).filter(|n| *n > 0 && *n <= 1024 * 1024).ok_or_else(|| anyhow::anyhow!("neural material extent must contain 1 to 1048576 texels"))?;
        let backend = self.settings.backend;
        let pipeline = self.kernels.iter().find(|(b, _)| *b == backend).ok_or_else(|| anyhow::anyhow!("{} unavailable", backend.label()))?.1.clone();
        let output = builder.create_buffer(count as u64 * 16, MemoryDomain::Device)?;
        let canonical = builder.import_buffer(self.canonical.clone());
        let half = builder.import_buffer(self.half_weights.as_ref().unwrap_or(&self.canonical).clone());
        let mut packed = [half; 3];
        for (slot, memory) in packed.iter_mut().zip(&self.packed) { *slot = builder.import_buffer(memory.clone()); }
        let mut uses = vec![output.write(Access::COMPUTE_WRITE), canonical.read(Access::COMPUTE_READ), half.read(Access::COMPUTE_READ)];
        if backend == NeuralBackend::CooperativeVector { uses.extend(packed.iter().map(|id| id.read(Access::COMPUTE_READ))); }
        builder.pass(format!("neural_material_{}", backend as u32), uses, move |ctx| {
            let output = ctx.buffer(output)?;
            let canonical = ctx.buffer(canonical)?;
            let half = ctx.buffer(half)?;
            let mut reachable = vec![output.clone(), canonical.clone(), half.clone()];
            let mut addresses = [half.address(); 3];
            if backend == NeuralBackend::CooperativeVector {
                for (address, id) in addresses.iter_mut().zip(packed) { let memory = ctx.buffer(id)?; *address = memory.address(); reachable.push(memory); }
            }
            let root = ctx.arguments(&InferenceRoot { weights: canonical.address(), half_weights: half.address(), packed: addresses, output: output.address(), width, height })?;
            let samples = if matches!(backend, NeuralBackend::CooperativeMatrix | NeuralBackend::CooperativeMatrix2) { 16 } else { 64 };
            unsafe { ctx.commands.dispatch(&pipeline, &root, [count.div_ceil(samples), 1, 1], &reachable) }
        })?;
        Ok(output)
    }

    pub fn render(&mut self, builder: &mut RenderGraphBuilder<'_>, output: ImageId) -> Result<()> {
        ensure!(self.settings.rotation.is_finite() && self.settings.exposure.is_finite() && self.settings.exposure >= 0.0, "invalid neural material display settings");
        let desc = builder.image_desc(output)?;
        let extent = vk::Extent2D { width: desc.extent.width, height: desc.extent.height };
        if self.pipeline.as_ref().is_none_or(|(format, _)| *format != desc.format) {
            self.pipeline = Some((desc.format, raster(builder.gpu(), &self.vertex, &self.fragment, &[desc.format], vk::Format::UNDEFINED)?));
        }
        let material = self.reconstruct(builder, 256, 256)?;
        let pipeline = self.pipeline.as_ref().unwrap().1.clone();
        let settings = self.settings;
        let encode_srgb = !matches!(desc.format, vk::Format::B8G8R8A8_SRGB | vk::Format::R8G8B8A8_SRGB | vk::Format::A8B8G8R8_SRGB_PACK32);
        builder.pass("neural_material_display", vec![material.read(FRAGMENT_READ), output.write(COLOR_WRITE)], move |ctx| {
            let material = ctx.buffer(material)?;
            let root = ctx.arguments(&DisplayRoot { material: material.address(), width: extent.width, height: extent.height, atlas_width: 256, atlas_height: 256,
                rotation: settings.rotation, exposure: settings.exposure, error_view: settings.error_view.into(), flat_view: settings.flat_view.into(), encode_srgb: encode_srgb.into(), padding: 0 })?;
            let view = ctx.view(output)?;
            ctx.commands.begin_rendering(&[Attachment { view: &view, clear: Some([0.0, 0.0, 0.0, 1.0]), store: true, resolve: None }], None, extent)?;
            viewport(ctx.commands, extent)?;
            unsafe { ctx.commands.draw(&pipeline, &root, &root, 0..3, 0..1, &[material])?; }
            ctx.commands.end_rendering()
        })
    }
}
