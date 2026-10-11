use super::pipeline_cache::{CachedPipeline, PipelineKey};
use super::shader::root_mapping;
use super::{Gpu, Shader, ShaderStage};
use anyhow::{Context, Result, ensure};
use ash::{vk, vk::TaggedStructure};
use std::sync::Arc;

mod binding_table;
pub use binding_table::{ShaderBindingTable, ShaderBindingTableDesc, ShaderBindingTableRegions};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RayTracingShaderGroup {
    General {
        shader: u32,
    },
    Triangles {
        closest_hit: Option<u32>,
        any_hit: Option<u32>,
    },
    Procedural {
        intersection: u32,
        closest_hit: Option<u32>,
        any_hit: Option<u32>,
    },
}

pub struct RayTracingDesc<'a> {
    pub shaders: &'a [&'a Shader],
    pub groups: &'a [RayTracingShaderGroup],
    pub max_recursion_depth: u32,
}

pub struct RayTracingPipeline {
    pub(crate) gpu: Arc<Gpu>,
    pub(crate) raw: vk::Pipeline,
    groups: Vec<RayTracingShaderGroup>,
    stages: Vec<ShaderStage>,
    max_recursion_depth: u32,
}

fn shader_index(index: Option<u32>, stage: ShaderStage, shaders: &[&Shader]) -> Result<u32> {
    let Some(index) = index else {
        return Ok(vk::SHADER_UNUSED_KHR);
    };
    ensure!(
        shaders
            .get(index as usize)
            .context("shader group index is out of bounds")?
            .stage
            == stage,
        "shader group index {index} requires {stage:?}"
    );
    Ok(index)
}

impl RayTracingShaderGroup {
    fn create_info(
        self,
        shaders: &[&Shader],
    ) -> Result<vk::RayTracingShaderGroupCreateInfoKHR<'static>> {
        let info = vk::RayTracingShaderGroupCreateInfoKHR::default()
            .general_shader(vk::SHADER_UNUSED_KHR)
            .closest_hit_shader(vk::SHADER_UNUSED_KHR)
            .any_hit_shader(vk::SHADER_UNUSED_KHR)
            .intersection_shader(vk::SHADER_UNUSED_KHR);
        Ok(match self {
            Self::General { shader } => {
                let stage = shaders
                    .get(shader as usize)
                    .context("shader group index is out of bounds")?
                    .stage;
                ensure!(
                    matches!(
                        stage,
                        ShaderStage::RayGeneration | ShaderStage::Miss | ShaderStage::Callable
                    ),
                    "general shader group requires ray generation, miss, or callable"
                );
                info.ty(vk::RayTracingShaderGroupTypeKHR::GENERAL)
                    .general_shader(shader)
            }
            Self::Triangles {
                closest_hit,
                any_hit,
            } => info
                .ty(vk::RayTracingShaderGroupTypeKHR::TRIANGLES_HIT_GROUP)
                .closest_hit_shader(shader_index(closest_hit, ShaderStage::ClosestHit, shaders)?)
                .any_hit_shader(shader_index(any_hit, ShaderStage::AnyHit, shaders)?),
            Self::Procedural {
                intersection,
                closest_hit,
                any_hit,
            } => info
                .ty(vk::RayTracingShaderGroupTypeKHR::PROCEDURAL_HIT_GROUP)
                .intersection_shader(shader_index(
                    Some(intersection),
                    ShaderStage::Intersection,
                    shaders,
                )?)
                .closest_hit_shader(shader_index(closest_hit, ShaderStage::ClosestHit, shaders)?)
                .any_hit_shader(shader_index(any_hit, ShaderStage::AnyHit, shaders)?),
        })
    }
}

impl RayTracingDesc<'_> {
    fn validate(
        &self,
        max_recursion_depth: u32,
    ) -> Result<Vec<vk::RayTracingShaderGroupCreateInfoKHR<'static>>> {
        ensure!(
            !self.shaders.is_empty()
                && self.shaders.len() <= u32::MAX as usize
                && !self.groups.is_empty()
                && self.groups.len() <= u32::MAX as usize,
            "ray tracing requires nonempty shader and group arrays within Vulkan limits"
        );
        ensure!(
            self.max_recursion_depth <= max_recursion_depth,
            "ray tracing recursion depth exceeds device limit"
        );
        ensure!(
            self.shaders
                .iter()
                .all(|shader| shader.stage.ray_tracing_flag().is_some()),
            "ray tracing pipeline requires ray tracing shaders"
        );
        let groups = self
            .groups
            .iter()
            .map(|group| group.create_info(self.shaders))
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            self.groups.iter().any(|group| matches!(*group,
                RayTracingShaderGroup::General { shader } if self.shaders[shader as usize].stage == ShaderStage::RayGeneration)),
            "ray tracing pipeline requires a ray generation group"
        );
        Ok(groups)
    }

    fn key(&self) -> PipelineKey {
        PipelineKey::RayTracing {
            shaders: self
                .shaders
                .iter()
                .map(|shader| (shader.stage, shader.entry.clone(), shader.code.clone()))
                .collect(),
            groups: self.groups.to_vec(),
            max_recursion_depth: self.max_recursion_depth,
        }
    }
}

impl Gpu {
    pub fn ray_tracing(
        self: &Arc<Self>,
        desc: &RayTracingDesc<'_>,
    ) -> Result<Arc<RayTracingPipeline>> {
        let api = self
            .ray_tracing
            .as_ref()
            .context("ray tracing pipelines are unavailable on this device or in capture mode")?;
        let properties = self.ray_tracing_properties().unwrap();
        let groups = desc.validate(properties.max_ray_recursion_depth)?;
        let strides = self.descriptor_strides()?;
        ensure!(
            desc.shaders
                .iter()
                .all(|shader| shader.heap_strides == strides),
            "shader descriptor strides do not match the device"
        );
        let key = desc.key();
        let mut cache = self.pipelines.lock();
        if let Some(CachedPipeline::RayTracing(weak)) = cache.entries.get(&key) {
            if let Some(pipeline) = weak.upgrade() {
                return Ok(pipeline);
            }
        }
        cache.collect();
        let mut modules = Vec::with_capacity(desc.shaders.len());
        for shader in desc.shaders {
            match unsafe {
                self.raw.create_shader_module(
                    &vk::ShaderModuleCreateInfo::default().code(&shader.code),
                    None,
                )
            } {
                Ok(module) => modules.push(module),
                Err(error) => {
                    for module in modules {
                        unsafe {
                            self.raw.destroy_shader_module(module, None);
                        }
                    }
                    return Err(error.into());
                }
            }
        }
        let mappings = [root_mapping(ShaderStage::RayGeneration)];
        let mut mapping_infos = vec![
            vk::ShaderDescriptorSetAndBindingMappingInfoEXT::default()
                .mappings(&mappings);
            desc.shaders.len()
        ];
        let stages = desc
            .shaders
            .iter()
            .zip(&modules)
            .zip(&mut mapping_infos)
            .map(|((shader, module), mapping)| {
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(shader.stage.ray_tracing_flag().unwrap())
                    .module(*module)
                    .name(&shader.entry)
                    .push(mapping)
            })
            .collect::<Vec<_>>();
        let mut flags = vk::PipelineCreateFlags2CreateInfo::default()
            .flags(vk::PipelineCreateFlags2::DESCRIPTOR_HEAP_EXT);
        let info = vk::RayTracingPipelineCreateInfoKHR::default()
            .stages(&stages)
            .groups(&groups)
            .max_pipeline_ray_recursion_depth(desc.max_recursion_depth)
            .push(&mut flags);
        let result = unsafe {
            api.create_ray_tracing_pipelines(
                vk::DeferredOperationKHR::null(),
                vk::PipelineCache::null(),
                &[info],
                None,
            )
        };
        for module in modules {
            unsafe {
                self.raw.destroy_shader_module(module, None);
            }
        }
        match result {
            Ok(pipelines) => {
                let pipeline = Arc::new(RayTracingPipeline {
                    gpu: self.clone(),
                    raw: pipelines[0],
                    groups: desc.groups.to_vec(),
                    stages: desc.shaders.iter().map(|shader| shader.stage).collect(),
                    max_recursion_depth: desc.max_recursion_depth,
                });
                self.name_object(
                    pipeline.raw,
                    &format!("RayTracing/{}", desc.shaders[0].entry.to_string_lossy()),
                )?;
                cache
                    .entries
                    .insert(key, CachedPipeline::RayTracing(Arc::downgrade(&pipeline)));
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

impl RayTracingPipeline {
    pub fn shader_groups(&self) -> &[RayTracingShaderGroup] {
        &self.groups
    }
    pub fn shader_stages(&self) -> &[ShaderStage] {
        &self.stages
    }
    pub fn max_recursion_depth(&self) -> u32 {
        self.max_recursion_depth
    }
}

impl Drop for RayTracingPipeline {
    fn drop(&mut self) {
        unsafe {
            self.gpu.raw.destroy_pipeline(self.raw, None);
        }
    }
}

#[cfg(test)]
mod tests;
