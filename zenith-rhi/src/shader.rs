use super::Gpu;
use super::pipeline_cache::{CachedPipeline, PipelineKey};
use anyhow::{Context, Result, ensure};
use ash::{vk, vk::TaggedStructure};
use std::ffi::CString;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
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
        let debug = match std::env::var("ZENITH_SHADER_DEBUG") {
            Ok(value) if value == "1" => true,
            Ok(value) if value == "0" => false,
            Err(std::env::VarError::NotPresent) => cfg!(debug_assertions),
            _ => anyhow::bail!("ZENITH_SHADER_DEBUG must be 0 or 1"),
        };
        compile_shader(path.as_ref(), entry, stage, heap_strides, debug)
    }
}

fn compile_shader(
    path: &Path,
    entry: &str,
    stage: ShaderStage,
    heap_strides: (u64, u64),
    debug: bool,
) -> Result<Shader> {
    let path = path
        .canonicalize()
        .with_context(|| format!("shader source {}", path.display()))?;
    let entry_name = CString::new(entry).context("shader entry contains a NUL byte")?;
    ensure!(!entry.is_empty(), "shader entry is empty");
    for stride in [heap_strides.0, heap_strides.1] {
        ensure!(
            stride > 0 && stride <= i32::MAX as u64,
            "invalid shader descriptor stride"
        );
    }
    let compiler = std::env::var_os("ZENITH_SLANGC")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("SLANG_DIR").map(|dir| {
                PathBuf::from(dir).join("bin").join(if cfg!(windows) {
                    "slangc.exe"
                } else {
                    "slangc"
                })
            })
        })
        .unwrap_or_else(|| PathBuf::from("slangc"));
    let stage_name = match stage {
        ShaderStage::Vertex => "vertex",
        ShaderStage::Fragment => "fragment",
        ShaderStage::Compute => "compute",
    };
    let mut command = Command::new(compiler);
    command
        .arg(&path)
        .args(["-entry", entry, "-stage", stage_name])
        .args(["-target", "spirv", "-profile", "spirv_1_6"])
        .args(["-capability", "spvDescriptorHeapEXT+nonuniformqualifier"])
        .arg("-spirv-resource-heap-stride")
        .arg(heap_strides.0.to_string())
        .arg("-spirv-sampler-heap-stride")
        .arg(heap_strides.1.to_string())
        .args([
            "-matrix-layout-column-major",
            "-fvk-use-scalar-layout",
            "-fvk-use-entrypoint-name",
        ])
        .args(["-warnings-as-errors", "38006"])
        .args(if debug {
            ["-O0", "-g3"]
        } else {
            ["-O3", "-g0"]
        })
        .arg("-I")
        .arg(path.parent().context("shader source has no parent")?)
        .args(["-o", "-"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    zenith_core::log::debug!("Shader compiler: {command:?}");
    let output = command.output().with_context(|| {
        format!("start Slang compiler; set ZENITH_SLANGC, SLANG_DIR or PATH: {command:?}")
    })?;
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    ensure!(
        output.status.success(),
        "Slang failed ({}): {command:?}\n{diagnostics}",
        output.status
    );
    if !diagnostics.trim().is_empty() {
        zenith_core::log::warn!("Slang {} ({entry}):\n{diagnostics}", path.display());
    }
    let code = ash::util::read_spv(&mut Cursor::new(&output.stdout))
        .with_context(|| format!("invalid Slang SPIR-V: {command:?}"))?;
    ensure!(code.len() >= 5, "truncated Slang SPIR-V header");
    if let Some(directory) = std::env::var_os("ZENITH_SHADER_DUMP_DIR") {
        let directory = PathBuf::from(directory);
        std::fs::create_dir_all(&directory)?;
        let mut hash = DefaultHasher::new();
        (&path, entry, stage, heap_strides, debug).hash(&mut hash);
        let output_path = directory.join(format!("shader-{:016x}.spv", hash.finish()));
        std::fs::write(&output_path, &output.stdout)?;
        std::fs::write(
            output_path.with_extension("txt"),
            format!("{command:?}\n{diagnostics}"),
        )?;
        zenith_core::log::debug!("Shader dump: {}", output_path.display());
    }
    Ok(Shader {
        code,
        stage,
        entry: entry_name,
        heap_strides,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the Slang SDK"]
    fn slang_profiles_and_diagnostics() -> Result<()> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/transform.slang");
        let debug = compile_shader(&path, "transform", ShaderStage::Compute, (64, 64), true)?;
        let release = compile_shader(&path, "transform", ShaderStage::Compute, (64, 64), false)?;
        assert_eq!(debug.entry.to_str()?, "transform");
        assert_ne!(debug.code, release.code);
        let debug_bytes = bytemuck::cast_slice::<u32, u8>(&debug.code);
        assert!(
            debug_bytes
                .windows(b"transform.slang".len())
                .any(|w| w == b"transform.slang")
        );
        let missing = compile_shader(
            &path,
            "missing_entry",
            ShaderStage::Compute,
            (64, 64),
            false,
        )
        .err()
        .expect("missing entry must fail")
        .to_string();
        assert!(missing.contains("missing_entry") && missing.contains("transform.slang"));
        let mismatch = compile_shader(&path, "transform", ShaderStage::Fragment, (64, 64), false)
            .err()
            .expect("wrong stage must fail")
            .to_string();
        assert!(mismatch.contains("38006"));
        Ok(())
    }
}
