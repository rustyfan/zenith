use super::Gpu;
use super::pipeline_cache::{CachedPipeline, PipelineKey};
use anyhow::{Context, Result, ensure};
use ash::{vk, vk::TaggedStructure};
use shader_slang::{CompilerOptions, ComponentType, GlobalSession, SessionDesc, TargetDesc};
use std::ffi::CString;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}

pub struct Shader {
    pub(crate) code: Vec<u32>,
    pub(crate) stage: ShaderStage,
    pub(crate) entry: CString,
    pub(crate) heap_strides: (u64, u64),
}

pub(crate) struct Specialization {
    pub constants: Vec<(u32, u32)>,
    entries: Vec<vk::SpecializationMapEntry>,
    values: Vec<u32>,
}
impl Specialization {
    pub fn new(constants: &[(u32, u32)]) -> Result<Self> {
        ensure!(
            constants.len() <= u32::MAX as usize / 4,
            "too many specialization constants"
        );
        let mut constants = constants.to_vec();
        constants.sort_unstable_by_key(|v| v.0);
        ensure!(
            constants.windows(2).all(|p| p[0].0 != p[1].0),
            "duplicate specialization id"
        );
        let entries = constants
            .iter()
            .enumerate()
            .map(|(index, (id, _))| {
                vk::SpecializationMapEntry::default()
                    .constant_id(*id)
                    .offset(index as u32 * 4)
                    .size(4)
            })
            .collect();
        let values = constants.iter().map(|(_, value)| *value).collect();
        Ok(Self {
            constants,
            entries,
            values,
        })
    }
    pub fn info(&self) -> vk::SpecializationInfo<'_> {
        vk::SpecializationInfo::default()
            .map_entries(&self.entries)
            .data(bytemuck::cast_slice(&self.values))
    }
}

impl Shader {
    pub fn stage(&self) -> ShaderStage {
        self.stage
    }
}

impl Gpu {
    pub fn compile_shader(
        &self,
        path: impl AsRef<Path>,
        entry: &str,
        stage: ShaderStage,
    ) -> Result<Shader> {
        let heap_strides = self.descriptor_strides()?;
        let path = path.as_ref().canonicalize()?;
        let source = std::fs::read_to_string(&path)?;
        let global = GlobalSession::new().context("create Slang global session")?;
        let options = CompilerOptions::default()
            .target(shader_slang::CompileTarget::Spirv)
            .capability(global.find_capability("spvDescriptorHeapEXT"))
            .capability(global.find_capability("nonuniformqualifier"))
            .spirv_resource_heap_stride(
                i32::try_from(heap_strides.0)
                    .context("image descriptor stride exceeds Slang limit")?,
            )
            .spirv_sampler_heap_stride(
                i32::try_from(heap_strides.1)
                    .context("sampler descriptor stride exceeds Slang limit")?,
            )
            .matrix_layout_column(true)
            .glsl_force_scalar_layout(true)
            .optimization(shader_slang::OptimizationLevel::Maximal);
        let target = TargetDesc::default()
            .format(shader_slang::CompileTarget::Spirv)
            .profile(global.find_profile("spirv_1_6"))
            .options(&options);
        let directory = CString::new(
            path.parent()
                .unwrap()
                .to_str()
                .context("shader directory is not UTF-8")?,
        )?;
        let search = [directory.as_ptr()];
        let session = global
            .create_session(
                &SessionDesc::default()
                    .targets(std::slice::from_ref(&target))
                    .search_paths(&search),
            )
            .context("create Slang session")?;
        let module = session
            .load_module_from_source_string(
                path.file_stem().unwrap().to_str().unwrap(),
                path.to_str().unwrap(),
                &source,
            )
            .map_err(|e| anyhow::anyhow!("Slang {}: {e:?}", path.display()))?;
        let ep = module
            .find_entry_point_by_name(entry)
            .with_context(|| format!("Slang entry point {entry} missing"))?;
        let components = [ComponentType::from(module), ComponentType::from(ep)];
        let linked = session
            .create_composite_component_type(&components)
            .map_err(|e| anyhow::anyhow!("Slang composite: {e:?}"))?
            .link()
            .map_err(|e| anyhow::anyhow!("Slang link: {e:?}"))?;
        let layout = linked
            .layout(0)
            .map_err(|e| anyhow::anyhow!("Slang entry layout: {e:?}"))?;
        let expected = match stage {
            ShaderStage::Vertex => shader_slang::Stage::Vertex,
            ShaderStage::Fragment => shader_slang::Stage::Fragment,
            ShaderStage::Compute => shader_slang::Stage::Compute,
        };
        ensure!(
            layout
                .entry_points()
                .next()
                .is_some_and(|entry| entry.stage() == expected),
            "shader stage does not match requested stage"
        );
        let code = linked
            .entry_point_code(0, 0)
            .map_err(|e| anyhow::anyhow!("Slang SPIR-V: {e:?}"))?;
        ensure!(code.as_slice().len() % 4 == 0, "invalid SPIR-V length");
        let code = code
            .as_slice()
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        Ok(Shader {
            heap_strides,
            code,
            stage,
            entry: CString::new("main")?,
        })
    }
}

pub struct ComputePipeline {
    pub(crate) gpu: Arc<Gpu>,
    pub(crate) raw: vk::Pipeline,
}

pub(crate) fn root_mapping(stage: ShaderStage) -> vk::DescriptorSetAndBindingMappingEXT<'static> {
    vk::DescriptorSetAndBindingMappingEXT::default()
        .descriptor_set(0)
        .first_binding(0)
        .binding_count(1)
        .resource_mask(vk::SpirvResourceTypeFlagsEXT::UNIFORM_BUFFER)
        .source(vk::DescriptorMappingSourceEXT::PUSH_ADDRESS)
        .source_data(vk::DescriptorMappingSourceDataEXT {
            push_address_offset: if stage == ShaderStage::Fragment { 8 } else { 0 },
        })
}

impl Gpu {
    pub fn compute(self: &Arc<Self>, shader: &Shader) -> Result<Arc<ComputePipeline>> {
        self.compute_specialized(shader, &[])
    }

    pub fn compute_specialized(
        self: &Arc<Self>,
        shader: &Shader,
        specialization: &[(u32, u32)],
    ) -> Result<Arc<ComputePipeline>> {
        ensure!(
            shader.stage == ShaderStage::Compute,
            "compute pipeline requires a compute shader"
        );
        let specialization = Specialization::new(specialization)?;
        ensure!(
            shader.heap_strides == self.descriptor_strides()?,
            "shader descriptor strides do not match the device"
        );
        let key = PipelineKey::Compute {
            code: shader.code.clone(),
            specialization: specialization.constants.clone(),
        };
        let mut cache = self.pipelines.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(CachedPipeline::Compute(weak)) = cache.entries.get(&key) {
            if let Some(pipeline) = weak.upgrade() {
                return Ok(pipeline);
            }
        }
        cache.collect();
        let module = unsafe {
            self.raw.create_shader_module(
                &vk::ShaderModuleCreateInfo::default().code(&shader.code),
                None,
            )?
        };
        let mappings = [root_mapping(ShaderStage::Compute)];
        let mut mapping =
            vk::ShaderDescriptorSetAndBindingMappingInfoEXT::default().mappings(&mappings);
        let spec = specialization.info();
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .module(module)
            .name(&shader.entry)
            .stage(vk::ShaderStageFlags::COMPUTE)
            .specialization_info(&spec)
            .push(&mut mapping);
        let mut flags = vk::PipelineCreateFlags2CreateInfo::default()
            .flags(vk::PipelineCreateFlags2::DESCRIPTOR_HEAP_EXT);
        let info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .push(&mut flags);
        let result = unsafe {
            self.raw
                .create_compute_pipelines(vk::PipelineCache::null(), &[info], None)
        };
        unsafe {
            self.raw.destroy_shader_module(module, None);
        }
        match result {
            Ok(pipelines) => {
                let pipeline = Arc::new(ComputePipeline {
                    gpu: self.clone(),
                    raw: pipelines[0],
                });
                cache
                    .entries
                    .insert(key, CachedPipeline::Compute(Arc::downgrade(&pipeline)));
                Ok(pipeline)
            }
            Err((pipelines, error)) => {
                for pipeline in pipelines {
                    unsafe {
                        self.raw.destroy_pipeline(pipeline, None);
                    }
                }
                Err(error.into())
            }
        }
    }
}

impl Drop for ComputePipeline {
    fn drop(&mut self) {
        unsafe {
            self.gpu.raw.destroy_pipeline(self.raw, None);
        }
    }
}
