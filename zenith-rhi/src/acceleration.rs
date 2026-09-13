use super::{Access, Commands, GpuAddress, Memory, MemoryDomain, MemorySlice};
use anyhow::{Context, Result, ensure};
use ash::vk;
use bytemuck::{Pod, Zeroable};
use std::{collections::HashSet, sync::Arc};

pub struct TriangleGeometry {
    pub vertices: MemorySlice,
    pub indices: MemorySlice,
    pub vertex_count: u32,
    pub vertex_stride: u64,
}

#[derive(Clone)]
pub struct AccelerationInstance {
    pub blas: Arc<AccelerationStructure>,
    pub transform: [[f32; 4]; 3],
}

pub struct AccelerationStructure {
    raw: vk::AccelerationStructureKHR,
    kind: vk::AccelerationStructureTypeKHR,
    storage: Arc<Memory>,
    address: GpuAddress,
    children: Vec<Arc<AccelerationStructure>>,
}

impl AccelerationStructure {
    pub fn address(&self) -> GpuAddress {
        self.address
    }
    pub fn storage(&self) -> &Arc<Memory> {
        &self.storage
    }
}

impl Drop for AccelerationStructure {
    fn drop(&mut self) {
        unsafe {
            self.storage
                .gpu
                .acceleration
                .destroy_acceleration_structure(self.raw, None);
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuInstance {
    transform: [[f32; 4]; 3],
    index_mask: u32,
    offset_flags: u32,
    address: u64,
}

impl Commands {
    pub fn retain_acceleration_structure(
        &mut self,
        structure: &Arc<AccelerationStructure>,
    ) -> Result<()> {
        self.retain(&structure.storage.whole())?;
        for child in &structure.children {
            self.retain_acceleration_structure(child)?;
        }
        self.retained.push(structure.clone());
        Ok(())
    }

    pub unsafe fn build_blas(
        &mut self,
        mesh: &TriangleGeometry,
    ) -> Result<Arc<AccelerationStructure>> {
        ensure!(
            self.rendering.is_none(),
            "acceleration build inside rendering"
        );
        for input in [&mesh.vertices, &mesh.indices] {
            ensure!(
                Arc::ptr_eq(&input.memory.gpu, &self.gpu)
                    && input.memory.usage.contains(
                        vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
                    )
                    && input.address().value() % 4 == 0,
                "invalid acceleration build input"
            );
        }
        ensure!(
            mesh.vertex_count > 0
                && mesh.vertex_stride >= 12
                && mesh.vertex_stride <= u32::MAX as u64
                && mesh.vertex_stride % 4 == 0
                && (mesh.vertex_count as u64 - 1)
                    .checked_mul(mesh.vertex_stride)
                    .and_then(|size| size.checked_add(12))
                    .is_some_and(|size| size <= mesh.vertices.size())
                && mesh.indices.size() % 12 == 0,
            "invalid triangle geometry layout"
        );
        let count = u32::try_from(mesh.indices.size() / 12).context("too many triangles")?;
        ensure!(
            count > 0 && count as u64 <= self.gpu.acceleration_properties.max_primitive_count,
            "triangle count exceeds device limit"
        );
        let format = vk::Format::R32G32B32_SFLOAT;
        let properties = unsafe {
            self.gpu
                .instance
                .raw
                .get_physical_device_format_properties(self.gpu.physical, format)
        };
        ensure!(
            properties
                .buffer_features
                .contains(vk::FormatFeatureFlags::ACCELERATION_STRUCTURE_VERTEX_BUFFER_KHR),
            "float3 acceleration vertices are unsupported"
        );
        let triangles = vk::AccelerationStructureGeometryTrianglesDataKHR::default()
            .vertex_format(format)
            .vertex_data(vk::DeviceOrHostAddressConstKHR {
                device_address: mesh.vertices.address().value(),
            })
            .vertex_stride(mesh.vertex_stride)
            .max_vertex(mesh.vertex_count - 1)
            .index_type(vk::IndexType::UINT32)
            .index_data(vk::DeviceOrHostAddressConstKHR {
                device_address: mesh.indices.address().value(),
            });
        let geometry = vk::AccelerationStructureGeometryKHR::default()
            .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
            .flags(vk::GeometryFlagsKHR::OPAQUE)
            .geometry(vk::AccelerationStructureGeometryDataKHR { triangles });
        self.retain(&mesh.vertices)?;
        self.retain(&mesh.indices)?;
        self.barrier(Access::COPY_WRITE, Access::AS_BUILD_INPUT)?;
        self.build_acceleration(
            vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL,
            geometry,
            count,
            Vec::new(),
        )
    }

    pub unsafe fn build_tlas(
        &mut self,
        instances: &[AccelerationInstance],
    ) -> Result<Arc<AccelerationStructure>> {
        ensure!(
            self.rendering.is_none(),
            "acceleration build inside rendering"
        );
        ensure!(
            !instances.is_empty()
                && instances.len() as u64 <= self.gpu.acceleration_properties.max_instance_count
                && instances.len() <= 0x1000000,
            "invalid acceleration instance count"
        );
        let mut data = Vec::with_capacity(instances.len());
        let mut children = Vec::new();
        let mut seen = HashSet::new();
        for (index, instance) in instances.iter().enumerate() {
            ensure!(
                instance.blas.kind == vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL
                    && Arc::ptr_eq(&instance.blas.storage.gpu, &self.gpu)
                    && instance.transform.iter().flatten().all(|v| v.is_finite()),
                "invalid acceleration instance"
            );
            let m = instance.transform;
            let determinant = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
            ensure!(
                determinant.is_finite() && determinant != 0.0,
                "singular acceleration instance transform"
            );
            data.push(GpuInstance {
                transform: m,
                index_mask: index as u32 | 0xff000000,
                offset_flags: vk::GeometryInstanceFlagsKHR::TRIANGLE_FACING_CULL_DISABLE.as_raw()
                    << 24,
                address: instance.blas.address().value(),
            });
            if seen.insert(instance.blas.address().value()) {
                children.push(instance.blas.clone());
            }
        }
        let bytes = bytemuck::cast_slice(&data);
        let input = self
            .gpu
            .allocate(bytes.len() as u64, MemoryDomain::Upload)?;
        input.write(0, bytes)?;
        self.retain(&input.whole())?;
        let instances_data = vk::AccelerationStructureGeometryInstancesDataKHR::default()
            .array_of_pointers(false)
            .data(vk::DeviceOrHostAddressConstKHR {
                device_address: input.address().value(),
            });
        let geometry = vk::AccelerationStructureGeometryKHR::default()
            .geometry_type(vk::GeometryTypeKHR::INSTANCES)
            .geometry(vk::AccelerationStructureGeometryDataKHR {
                instances: instances_data,
            });
        self.barrier(Access::AS_BUILD_WRITE, Access::AS_BUILD_READ)?;
        self.build_acceleration(
            vk::AccelerationStructureTypeKHR::TOP_LEVEL,
            geometry,
            instances.len() as u32,
            children,
        )
    }

    fn build_acceleration(
        &mut self,
        kind: vk::AccelerationStructureTypeKHR,
        geometry: vk::AccelerationStructureGeometryKHR<'_>,
        count: u32,
        children: Vec<Arc<AccelerationStructure>>,
    ) -> Result<Arc<AccelerationStructure>> {
        let geometries = [geometry];
        let mut build = vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(kind)
            .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
            .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
            .geometries(&geometries);
        let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
        unsafe {
            self.gpu
                .acceleration
                .get_acceleration_structure_build_sizes(
                    vk::AccelerationStructureBuildTypeKHR::DEVICE,
                    &build,
                    Some(&[count]),
                    &mut sizes,
                );
        }
        let storage = self.gpu.allocate_buffer(
            sizes.acceleration_structure_size,
            MemoryDomain::Device,
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR,
            256,
        )?;
        let scratch = self.gpu.allocate_buffer(
            sizes.build_scratch_size.max(1),
            MemoryDomain::Device,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            self.gpu
                .acceleration_properties
                .min_acceleration_structure_scratch_offset_alignment as u64,
        )?;
        let raw = unsafe {
            self.gpu.acceleration.create_acceleration_structure(
                &vk::AccelerationStructureCreateInfoKHR::default()
                    .buffer(storage.raw)
                    .size(sizes.acceleration_structure_size)
                    .ty(kind),
                None,
            )?
        };
        let address = unsafe {
            self.gpu
                .acceleration
                .get_acceleration_structure_device_address(
                    &vk::AccelerationStructureDeviceAddressInfoKHR::default()
                        .acceleration_structure(raw),
                )
        };
        let structure = Arc::new(AccelerationStructure {
            raw,
            kind,
            storage,
            address: GpuAddress(address),
            children,
        });
        ensure!(
            address != 0 && address % 256 == 0,
            "invalid acceleration structure address"
        );
        self.retain_acceleration_structure(&structure)?;
        self.retain(&scratch.whole())?;
        build = build
            .dst_acceleration_structure(raw)
            .scratch_data(vk::DeviceOrHostAddressKHR {
                device_address: scratch.address().value(),
            });
        let ranges = [vk::AccelerationStructureBuildRangeInfoKHR::default().primitive_count(count)];
        unsafe {
            self.gpu.acceleration.cmd_build_acceleration_structures(
                self.raw,
                &[build],
                &[Some(&ranges)],
            );
        }
        Ok(structure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Arguments, Descriptors, Gpu, Instance, ShaderStage};

    #[test]
    #[ignore = "requires Vulkan validation, Slang, and ray query support"]
    fn ray_queries_hit_miss_and_retain_acceleration_structures() -> Result<()> {
        let _ = zenith_core::log::initialize(zenith_core::log::LevelFilter::Info);
        let instance = Instance::new(&[], true)?;
        ensure!(
            instance.validation_enabled(),
            "test requires Vulkan validation"
        );
        let gpu = Gpu::new(
            instance.clone(),
            std::env::var("ZENITH_ADAPTER").ok().as_deref(),
        )?;
        {
            let descriptors = Descriptors::new(&gpu, 32, 8)?;
            let shader = gpu.compile_shader(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/shaders/ray_query.slang"),
                "main",
                ShaderStage::Compute,
            )?;
            let pipeline = gpu.compute(&shader)?;
            let vertices = gpu.allocate(36, MemoryDomain::Upload)?;
            vertices.write(
                0,
                bytemuck::cast_slice(&[[-1.0f32, 2.0, -1.0], [1.0, 2.0, -1.0], [0.0, 2.0, 1.0]]),
            )?;
            let indices = gpu.allocate(12, MemoryDomain::Upload)?;
            indices.write(0, bytemuck::cast_slice(&[0u32, 1, 2]))?;
            let mut commands = gpu.commands()?;
            let blas = unsafe {
                commands.build_blas(&TriangleGeometry {
                    vertices: vertices.whole(),
                    indices: indices.whole(),
                    vertex_count: 3,
                    vertex_stride: 12,
                })?
            };
            let tlas = unsafe {
                commands.build_tlas(&[AccelerationInstance {
                    blas: blas.clone(),
                    transform: [
                        [1.0, 0.0, 0.0, 0.0],
                        [0.0, 1.0, 0.0, 0.0],
                        [0.0, 0.0, 1.0, 0.0],
                    ],
                }])?
            };
            let weak_blas = Arc::downgrade(&blas);
            let weak_tlas = Arc::downgrade(&tlas);
            let result = gpu.allocate(12, MemoryDomain::Readback)?;
            result.write(0, bytemuck::cast_slice(&[99u32; 3]))?;
            #[repr(C)]
            #[derive(Clone, Copy, Pod, Zeroable)]
            struct Root {
                scene: GpuAddress,
                results: GpuAddress,
            }
            let mut arguments = Arguments::new(&gpu, 256)?;
            let root = arguments.push(&Root {
                scene: tlas.address(),
                results: result.address(),
            })?;
            commands.barrier(
                Access::AS_BUILD_WRITE,
                Access {
                    stages: vk::PipelineStageFlags2::COMPUTE_SHADER,
                    access: vk::AccessFlags2::ACCELERATION_STRUCTURE_READ_KHR,
                },
            )?;
            commands.bind_descriptors(&descriptors)?;
            commands.retain_acceleration_structure(&tlas)?;
            unsafe {
                commands.dispatch(&pipeline, &root, [3, 1, 1], &[result.whole()])?;
            }
            commands.barrier(Access::COMPUTE_WRITE, Access::HOST_READ)?;
            drop((blas, tlas, vertices, indices, arguments, root));
            let mut submission = commands.submit()?;
            assert!(weak_blas.upgrade().is_some());
            assert!(weak_tlas.upgrade().is_some());
            submission.wait(10_000_000_000)?;
            let mut values = [0u32; 3];
            result.read(0, bytemuck::cast_slice_mut(&mut values))?;
            assert_eq!(values, [1, 0, 1]);
            assert!(weak_blas.upgrade().is_none());
            assert!(weak_tlas.upgrade().is_none());
        }
        gpu.wait_idle()?;
        drop(gpu);
        ensure!(
            instance.validation_errors().is_empty(),
            "{:?}",
            instance.validation_errors()
        );
        Ok(())
    }
}
