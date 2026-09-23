use crate::{Gpu, Instance, Memory, MemoryDomain, vk};
use anyhow::{Result, ensure};
use ash::vk::TaggedStructure;
use std::{ffi::CStr, sync::Arc};

#[derive(Debug, Default)]
pub struct CooperativeCapabilities {
    pub extensions: Vec<String>,
    pub float16: bool,
    pub storage16: bool,
    pub memory_model: bool,
    pub vector: bool,
    pub vector_training: bool,
    pub vector_stages: vk::ShaderStageFlags,
    pub vector_max_components: u32,
    pub vector_types: Vec<vk::CooperativeVectorPropertiesNV<'static>>,
    pub matrix: bool,
    pub matrix_stages: vk::ShaderStageFlags,
    pub matrix_types: Vec<vk::CooperativeMatrixPropertiesKHR<'static>>,
    pub matrix2: vk::PhysicalDeviceCooperativeMatrix2FeaturesNV<'static>,
    pub matrix2_properties: vk::PhysicalDeviceCooperativeMatrix2PropertiesNV<'static>,
    pub flexible_types: Vec<vk::CooperativeMatrixFlexibleDimensionsPropertiesNV<'static>>,
    pub decode_vector: bool,
    pub subgroup_size: u32,
    pub max_shared_memory: u32,
}

impl CooperativeCapabilities {
    pub fn matrix2_f16(&self, m: u32, n: u32, k: u32, threads: u32) -> bool {
        self.matrix && self.float16 && self.storage16 && self.memory_model
            && self.matrix_stages.contains(vk::ShaderStageFlags::COMPUTE)
            && self.matrix2.cooperative_matrix_workgroup_scope != 0
            && self.matrix2.cooperative_matrix_flexible_dimensions != 0
            && threads <= self.matrix2_properties.cooperative_matrix_workgroup_scope_max_workgroup_size
            && m.max(n).max(k) <= self.matrix2_properties.cooperative_matrix_flexible_dimensions_max_dimension
            && self.max_shared_memory.saturating_sub(self.matrix2_properties.cooperative_matrix_workgroup_scope_reserved_shared_memory) >= 3072
            && self.flexible_types.iter().any(|p| {
                p.scope == vk::ScopeKHR::WORKGROUP && p.workgroup_invocations == threads
                    && p.m_granularity != 0 && m % p.m_granularity == 0
                    && p.n_granularity != 0 && n % p.n_granularity == 0
                    && p.k_granularity != 0 && k % p.k_granularity == 0
                    && p.a_type == vk::ComponentTypeKHR::FLOAT16
                    && p.b_type == vk::ComponentTypeKHR::FLOAT16
                    && p.c_type == vk::ComponentTypeKHR::FLOAT32
                    && p.result_type == vk::ComponentTypeKHR::FLOAT32
                    && p.saturating_accumulation == vk::FALSE
            })
    }
    pub fn vector_f16(&self, stage: vk::ShaderStageFlags) -> bool {
        self.vector && self.float16 && self.storage16 && self.memory_model
            && self.vector_stages.contains(stage)
            && self.vector_max_components >= 32
            && self.vector_types.iter().any(|p| {
                p.input_type == vk::ComponentTypeKHR::FLOAT16
                    && p.input_interpretation == vk::ComponentTypeKHR::FLOAT16
                    && p.matrix_interpretation == vk::ComponentTypeKHR::FLOAT16
                    && p.bias_interpretation == vk::ComponentTypeKHR::FLOAT16
                    && p.result_type == vk::ComponentTypeKHR::FLOAT16
            })
    }

    pub fn matrix_f16(&self, m: u32, n: u32, k: u32) -> bool {
        self.matrix && self.float16 && self.storage16 && self.memory_model
            && self.matrix_stages.contains(vk::ShaderStageFlags::COMPUTE)
            && self.matrix_types.iter().any(|p| {
                p.m_size == m && p.n_size == n && p.k_size == k
                    && p.scope == vk::ScopeKHR::SUBGROUP
                    && p.a_type == vk::ComponentTypeKHR::FLOAT16
                    && p.b_type == vk::ComponentTypeKHR::FLOAT16
                    && p.c_type == vk::ComponentTypeKHR::FLOAT32
                    && p.result_type == vk::ComponentTypeKHR::FLOAT32
                    && p.saturating_accumulation == vk::FALSE
            })
    }
}

impl Instance {
    pub(crate) fn cooperative_capabilities(
        &self,
        physical: vk::PhysicalDevice,
        extensions: &[vk::ExtensionProperties],
    ) -> Result<CooperativeCapabilities> {
        let has = |name: &CStr| extensions.iter().any(|e| unsafe {
            CStr::from_ptr(e.extension_name.as_ptr()) == name
        });
        let mut f11 = vk::PhysicalDeviceVulkan11Features::default();
        let mut f12 = vk::PhysicalDeviceVulkan12Features::default();
        let mut vector = vk::PhysicalDeviceCooperativeVectorFeaturesNV::default();
        let mut matrix = vk::PhysicalDeviceCooperativeMatrixFeaturesKHR::default();
        let mut matrix2 = vk::PhysicalDeviceCooperativeMatrix2FeaturesNV::default();
        let mut decode = vk::PhysicalDeviceCooperativeMatrixDecodeVectorFeaturesNV::default();
        let mut features = vk::PhysicalDeviceFeatures2::default().push(&mut f11).push(&mut f12);
        if has(ash::nv::cooperative_vector::NAME) { features = features.push(&mut vector); }
        if has(ash::khr::cooperative_matrix::NAME) { features = features.push(&mut matrix); }
        if has(ash::nv::cooperative_matrix2::NAME) { features = features.push(&mut matrix2); }
        if has(ash::nv::cooperative_matrix_decode_vector::NAME) { features = features.push(&mut decode); }
        unsafe { self.raw.get_physical_device_features2(physical, &mut features); }
        let mut vector_props = vk::PhysicalDeviceCooperativeVectorPropertiesNV::default();
        let mut matrix_props = vk::PhysicalDeviceCooperativeMatrixPropertiesKHR::default();
        let mut matrix2_props = vk::PhysicalDeviceCooperativeMatrix2PropertiesNV::default();
        let mut subgroup = vk::PhysicalDeviceSubgroupProperties::default();
        let mut properties = vk::PhysicalDeviceProperties2::default().push(&mut subgroup);
        if has(ash::nv::cooperative_vector::NAME) { properties = properties.push(&mut vector_props); }
        if has(ash::khr::cooperative_matrix::NAME) { properties = properties.push(&mut matrix_props); }
        if has(ash::nv::cooperative_matrix2::NAME) { properties = properties.push(&mut matrix2_props); }
        unsafe { self.raw.get_physical_device_properties2(physical, &mut properties); }
        let max_shared_memory = properties.properties.limits.max_compute_shared_memory_size;
        matrix2.p_next = std::ptr::null_mut();
        matrix2_props.p_next = std::ptr::null_mut();
        let mut caps = CooperativeCapabilities {
            extensions: extensions.iter().filter_map(|e| {
                let name = unsafe { CStr::from_ptr(e.extension_name.as_ptr()) }.to_string_lossy();
                ["cooperative", "float8", "bfloat16", "long_vector"].iter()
                    .any(|s| name.contains(s)).then(|| name.into_owned())
            }).collect(),
            float16: f12.shader_float16 != 0,
            storage16: f11.storage_buffer16_bit_access != 0,
            memory_model: f12.vulkan_memory_model != 0,
            vector: vector.cooperative_vector != 0,
            vector_training: vector.cooperative_vector_training != 0,
            vector_stages: vector_props.cooperative_vector_supported_stages,
            vector_max_components: vector_props.max_cooperative_vector_components,
            matrix: matrix.cooperative_matrix != 0,
            matrix_stages: matrix_props.cooperative_matrix_supported_stages,
            matrix2,
            matrix2_properties: matrix2_props,
            decode_vector: decode.cooperative_matrix_decode_vector != 0,
            subgroup_size: subgroup.subgroup_size,
            max_shared_memory,
            ..Default::default()
        };
        if caps.vector {
            let api = ash::nv::cooperative_vector::Instance::load(&self.entry, &self.raw);
            caps.vector_types = enumerate(|count, data| unsafe {
                (api.fp().get_physical_device_cooperative_vector_properties_nv)(physical, count, data)
            })?;
        }
        if caps.matrix {
            let api = ash::khr::cooperative_matrix::Instance::load(&self.entry, &self.raw);
            caps.matrix_types = enumerate(|count, data| unsafe {
                (api.fp().get_physical_device_cooperative_matrix_properties_khr)(physical, count, data)
            })?;
        }
        if caps.matrix2.cooperative_matrix_flexible_dimensions != 0 {
            let api = ash::nv::cooperative_matrix2::Instance::load(&self.entry, &self.raw);
            caps.flexible_types = enumerate(|count, data| unsafe {
                (api.fp().get_physical_device_cooperative_matrix_flexible_dimensions_properties_nv)(physical, count, data)
            })?;
        }
        Ok(caps)
    }
}

fn enumerate<T: Default + Clone>(mut query: impl FnMut(*mut u32, *mut T) -> vk::Result) -> Result<Vec<T>> {
    for _ in 0..4 {
        let mut count = 0;
        query(&mut count, std::ptr::null_mut()).result()?;
        let mut properties = vec![T::default(); count as usize];
        if count == 0 { return Ok(properties); }
        let result = query(&mut count, properties.as_mut_ptr());
        if result == vk::Result::INCOMPLETE { continue; }
        result.result()?;
        properties.truncate(count as usize);
        return Ok(properties);
    }
    anyhow::bail!("cooperative property enumeration did not stabilize")
}

impl Gpu {
    pub fn cooperative_vector_weights(self: &Arc<Self>, weights: &[u16], rows: u32, columns: u32) -> Result<Arc<Memory>> {
        ensure!(self.info.cooperative.vector_f16(vk::ShaderStageFlags::COMPUTE), "FP16 cooperative vectors unavailable");
        ensure!(rows > 0 && columns > 0 && rows.checked_mul(columns).map(|n| n as usize) == Some(weights.len()), "invalid cooperative weight dimensions");
        let api = ash::nv::cooperative_vector::Device::load(&self.instance.raw, &self.raw);
        let mut size = 0;
        let mut info = vk::ConvertCooperativeVectorMatrixInfoNV::default()
            .src_size(std::mem::size_of_val(weights))
            .src_data(vk::DeviceOrHostAddressConstKHR { host_address: weights.as_ptr().cast() })
            .src_component_type(vk::ComponentTypeKHR::FLOAT16)
            .dst_component_type(vk::ComponentTypeKHR::FLOAT16)
            .num_rows(rows).num_columns(columns)
            .src_layout(vk::CooperativeVectorMatrixLayoutNV::ROW_MAJOR)
            .src_stride(columns as usize * 2)
            .dst_layout(vk::CooperativeVectorMatrixLayoutNV::INFERENCING_OPTIMAL);
        info.p_dst_size = &mut size;
        unsafe { api.convert_cooperative_vector_matrix(&info)?; }
        ensure!(size > 0, "empty converted cooperative matrix");
        let mut converted = vec![0u8; size];
        info.dst_data = vk::DeviceOrHostAddressKHR { host_address: converted.as_mut_ptr().cast() };
        unsafe { api.convert_cooperative_vector_matrix(&info)?; }
        let staging = self.allocate(size as u64, MemoryDomain::Upload)?;
        staging.write(0, &converted[..size])?;
        let memory = self.allocate_buffer(size as u64, MemoryDomain::Device,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST, 64)?;
        let mut commands = self.commands()?;
        commands.copy(&staging.whole(), &memory.whole())?;
        commands.barrier(crate::Access::COPY_WRITE, crate::Access::ALL)?;
        commands.submit()?.wait(10_000_000_000)?;
        Ok(memory)
    }
}
