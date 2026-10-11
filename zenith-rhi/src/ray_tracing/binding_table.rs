use super::{RayTracingPipeline, RayTracingShaderGroup};
use crate::memory::align_up;
use crate::{AccelerationStructure, Commands, Gpu, Memory, MemoryDomain, MemorySlice, ShaderStage};
use anyhow::{Context, Result, ensure};
use ash::vk;
use std::sync::Arc;

pub struct ShaderBindingTableDesc<'a> {
    pub raygen: u32,
    pub miss: &'a [u32],
    pub hit: &'a [u32],
    pub callable: &'a [u32],
}

#[derive(Clone, Copy)]
pub struct ShaderBindingTableRegions {
    pub raygen: vk::StridedDeviceAddressRegionKHR,
    pub miss: vk::StridedDeviceAddressRegionKHR,
    pub hit: vk::StridedDeviceAddressRegionKHR,
    pub callable: vk::StridedDeviceAddressRegionKHR,
}

pub struct ShaderBindingTable {
    pub(crate) current: parking_lot::RwLock<Arc<TableVersion>>,
    pub(crate) recipe: TableRecipe,
    pub(crate) generation: std::sync::atomic::AtomicU64,
}

pub(crate) struct TableVersion {
    pub pipeline: Arc<crate::shader_reload::PipelineVersion>,
    pub storage: Arc<Memory>,
    pub regions: ShaderBindingTableRegions,
}

pub(crate) struct TableRecipe {
    pub pipeline: Arc<RayTracingPipeline>,
    raygen: u32,
    miss: Vec<u32>,
    hit: Vec<u32>,
    callable: Vec<u32>,
}

impl TableRecipe {
    pub fn build(
        &self,
        version: &Arc<crate::shader_reload::PipelineVersion>,
    ) -> Result<Arc<TableVersion>> {
        self.pipeline.gpu.table_version(
            &self.pipeline,
            version,
            &ShaderBindingTableDesc {
                raygen: self.raygen,
                miss: &self.miss,
                hit: &self.hit,
                callable: &self.callable,
            },
        )
    }
}

struct TableLayout {
    offsets: [u64; 4],
    sizes: [u64; 4],
    stride: u64,
    size: u64,
}

fn table_layout(
    properties: &vk::PhysicalDeviceRayTracingPipelinePropertiesKHR<'_>,
    counts: [usize; 4],
) -> Result<TableLayout> {
    ensure!(
        counts[0] == 1,
        "SBT requires exactly one ray generation record"
    );
    ensure!(
        properties.shader_group_handle_size > 0,
        "empty shader group handle"
    );
    let stride = align_up(
        properties.shader_group_handle_size as u64,
        properties.shader_group_handle_alignment as u64,
    )?;
    ensure!(
        stride <= properties.max_shader_group_stride as u64,
        "SBT stride exceeds device limit"
    );
    let mut layout = TableLayout {
        offsets: [0; 4],
        sizes: [0; 4],
        stride,
        size: 0,
    };
    for (i, count) in counts.into_iter().enumerate() {
        if count == 0 {
            continue;
        }
        layout.offsets[i] = align_up(layout.size, properties.shader_group_base_alignment as u64)?;
        layout.sizes[i] = stride
            .checked_mul(count as u64)
            .context("SBT region size overflow")?;
        layout.size = layout.offsets[i]
            .checked_add(layout.sizes[i])
            .context("SBT size overflow")?;
    }
    ensure!(
        layout.size <= isize::MAX as u64,
        "SBT exceeds allocation limit"
    );
    Ok(layout)
}

fn validate_groups(
    records: [&[u32]; 4],
    groups: &[RayTracingShaderGroup],
    stages: &[ShaderStage],
) -> Result<()> {
    for (region, records) in records.into_iter().enumerate() {
        for &index in records {
            let group = groups
                .get(index as usize)
                .context("SBT group index is out of bounds")?;
            let valid = match *group {
                RayTracingShaderGroup::General { shader } => {
                    let expected = match region {
                        0 => ShaderStage::RayGeneration,
                        1 => ShaderStage::Miss,
                        3 => ShaderStage::Callable,
                        _ => anyhow::bail!("SBT hit record requires a hit group"),
                    };
                    stages[shader as usize] == expected
                }
                _ => region == 2,
            };
            ensure!(
                valid,
                "SBT group {index} has the wrong shader type for region {region}"
            );
        }
    }
    Ok(())
}

impl Gpu {
    pub fn shader_binding_table(
        self: &Arc<Self>,
        pipeline: &Arc<RayTracingPipeline>,
        desc: &ShaderBindingTableDesc<'_>,
    ) -> Result<Arc<ShaderBindingTable>> {
        let _scope = self.shader_reload_scope();
        let recipe = TableRecipe {
            pipeline: pipeline.clone(),
            raygen: desc.raygen,
            miss: desc.miss.to_vec(),
            hit: desc.hit.to_vec(),
            callable: desc.callable.to_vec(),
        };
        let version = recipe.build(&pipeline.current.read())?;
        let table = Arc::new(ShaderBindingTable {
            current: parking_lot::RwLock::new(version),
            recipe,
            generation: std::sync::atomic::AtomicU64::new(pipeline.generation()),
        });
        self.reload_service()
            .track(crate::shader_reload::Target::Table(Arc::downgrade(&table)));
        Ok(table)
    }

    fn table_version(
        self: &Arc<Self>,
        pipeline: &Arc<RayTracingPipeline>,
        version: &Arc<crate::shader_reload::PipelineVersion>,
        desc: &ShaderBindingTableDesc<'_>,
    ) -> Result<Arc<TableVersion>> {
        ensure!(
            Arc::ptr_eq(&pipeline.gpu, self),
            "ray tracing pipeline belongs to another device"
        );
        let api = self
            .ray_tracing
            .as_ref()
            .context("ray tracing pipelines are unavailable")?;
        let properties = self.ray_tracing_properties().unwrap();
        let raygen = [desc.raygen];
        let records = [&raygen[..], desc.miss, desc.hit, desc.callable];
        validate_groups(records, &pipeline.groups, &pipeline.stages)?;
        let layout = table_layout(properties, records.map(|records| records.len()))?;
        let handle_size = properties.shader_group_handle_size as usize;
        let data_size = handle_size
            .checked_mul(pipeline.groups.len())
            .context("shader group handle size overflow")?;
        let handles = unsafe {
            api.get_ray_tracing_shader_group_handles(
                version.raw,
                0,
                pipeline.groups.len() as u32,
                data_size,
            )?
        };
        let storage = self.allocate_buffer(
            layout.size,
            MemoryDomain::Upload,
            vk::BufferUsageFlags::SHADER_BINDING_TABLE_KHR,
            properties.shader_group_base_alignment as u64,
        )?;
        let base = storage.address().value();
        ensure!(
            base % properties.shader_group_base_alignment as u64 == 0,
            "unaligned SBT allocation"
        );
        let mut data = vec![0u8; layout.size as usize];
        let mut regions = [vk::StridedDeviceAddressRegionKHR::default(); 4];
        for (i, records) in records.into_iter().enumerate() {
            if records.is_empty() {
                continue;
            }
            regions[i] = vk::StridedDeviceAddressRegionKHR {
                device_address: base
                    .checked_add(layout.offsets[i])
                    .context("SBT address overflow")?,
                stride: layout.stride,
                size: layout.sizes[i],
            };
            for (record, &group) in records.iter().enumerate() {
                let source = group as usize * handle_size;
                let destination = (layout.offsets[i] + record as u64 * layout.stride) as usize;
                data[destination..destination + handle_size]
                    .copy_from_slice(&handles[source..source + handle_size]);
            }
        }
        storage.write(0, &data)?;
        storage.set_debug_name("RayTracing/ShaderBindingTable")?;
        let [raygen, miss, hit, callable] = regions;
        Ok(Arc::new(TableVersion {
            pipeline: version.clone(),
            storage,
            regions: ShaderBindingTableRegions {
                raygen,
                miss,
                hit,
                callable,
            },
        }))
    }
}

impl ShaderBindingTable {
    pub fn storage(&self) -> Arc<Memory> {
        self.current.read().storage.clone()
    }

    pub fn regions(&self) -> ShaderBindingTableRegions {
        self.current.read().regions
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Acquire)
    }
}

fn validate_extent(
    extent: [u32; 3],
    limits: &vk::PhysicalDeviceLimits,
    max_invocations: u32,
) -> Result<()> {
    let mut count = 1u64;
    for (i, dimension) in extent.into_iter().enumerate() {
        let limit = limits.max_compute_work_group_count[i] as u64
            * limits.max_compute_work_group_size[i] as u64;
        ensure!(
            dimension as u64 <= limit,
            "ray dispatch dimension exceeds device limit"
        );
        count = count
            .checked_mul(dimension as u64)
            .context("ray dispatch size overflow")?;
    }
    ensure!(
        count <= max_invocations as u64,
        "ray dispatch exceeds invocation limit"
    );
    Ok(())
}

impl Commands {
    pub unsafe fn trace_rays(
        &mut self,
        table: &Arc<ShaderBindingTable>,
        root: &MemorySlice,
        extent: [u32; 3],
        reachable: &[MemorySlice],
        acceleration: &[Arc<AccelerationStructure>],
    ) -> Result<()> {
        let handle = table.clone();
        let table = table.current.read().clone();
        ensure!(
            self.rendering.is_none() && Arc::ptr_eq(&table.pipeline.gpu, &self.gpu),
            "invalid ray tracing device or rendering scope"
        );
        ensure!(
            root.address().value() % 8 == 0 && root.size() >= 8,
            "unaligned or empty ray tracing arguments"
        );
        let properties = self
            .gpu
            .ray_tracing_properties()
            .context("ray tracing pipelines are unavailable")?;
        validate_extent(
            extent,
            &self.gpu.limits,
            properties.max_ray_dispatch_invocation_count,
        )?;
        self.retain(root)?;
        self.retain(&table.storage.whole())?;
        for memory in reachable {
            self.retain(memory)?;
        }
        for structure in acceleration {
            self.retain_acceleration_structure(structure)?;
        }
        self.retained.push(handle);
        self.retained.push(table.clone());
        self.push_root(root.address().value(), 0);
        let regions = &table.regions;
        unsafe {
            self.gpu.raw.cmd_bind_pipeline(
                self.raw,
                vk::PipelineBindPoint::RAY_TRACING_KHR,
                table.pipeline.raw,
            );
            self.gpu.ray_tracing.as_ref().unwrap().cmd_trace_rays(
                self.raw,
                &regions.raygen,
                &regions.miss,
                &regions.hit,
                &regions.callable,
                extent[0],
                extent[1],
                extent[2],
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
