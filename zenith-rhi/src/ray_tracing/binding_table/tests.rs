use super::*;
use crate::{
    AccelerationInstance, Access, Arguments, Descriptors, GraphicsMode, Instance, RayTracingDesc,
    TriangleGeometry,
};
use bytemuck::{Pod, Zeroable};

#[test]
fn binding_table_layout_respects_alignment_empty_regions_and_limits() -> Result<()> {
    let mut properties = vk::PhysicalDeviceRayTracingPipelinePropertiesKHR::default()
        .shader_group_handle_size(24)
        .shader_group_handle_alignment(32)
        .shader_group_base_alignment(64)
        .max_shader_group_stride(64);
    let layout = table_layout(&properties, [1, 3, 2, 1])?;
    assert_eq!(layout.stride, 32);
    assert_eq!(layout.offsets, [0, 64, 192, 256]);
    assert_eq!(layout.sizes, [32, 96, 64, 32]);
    assert_eq!(layout.size, 288);
    let empty = table_layout(&properties, [1, 0, 0, 0])?;
    assert_eq!(empty.offsets, [0; 4]);
    assert_eq!(empty.sizes, [32, 0, 0, 0]);
    assert_eq!(empty.size, 32);
    assert!(table_layout(&properties, [0, 1, 1, 1]).is_err());
    assert!(table_layout(&properties, [2, 1, 1, 1]).is_err());
    assert!(table_layout(&properties, [1, usize::MAX, 0, 0]).is_err());
    properties.max_shader_group_stride = 16;
    assert!(table_layout(&properties, [1, 1, 1, 1]).is_err());
    properties.max_shader_group_stride = 64;
    properties.shader_group_handle_alignment = 3;
    assert!(table_layout(&properties, [1, 1, 1, 1]).is_err());
    properties.shader_group_handle_alignment = 32;
    properties.shader_group_base_alignment = 0;
    assert!(table_layout(&properties, [1, 1, 1, 1]).is_err());
    Ok(())
}

#[test]
fn binding_table_records_require_matching_shader_groups() -> Result<()> {
    let stages = [
        ShaderStage::Miss,
        ShaderStage::RayGeneration,
        ShaderStage::Callable,
    ];
    let groups = [
        RayTracingShaderGroup::General { shader: 0 },
        RayTracingShaderGroup::Triangles {
            closest_hit: None,
            any_hit: None,
        },
        RayTracingShaderGroup::General { shader: 1 },
        RayTracingShaderGroup::General { shader: 2 },
        RayTracingShaderGroup::Procedural {
            intersection: 0,
            closest_hit: None,
            any_hit: None,
        },
    ];
    validate_groups([&[2], &[0, 0], &[1, 4], &[3]], &groups, &stages)?;
    validate_groups([&[2], &[], &[], &[]], &groups, &stages)?;
    for records in [
        [&[0][..], &[][..], &[][..], &[][..]],
        [&[2][..], &[1][..], &[][..], &[][..]],
        [&[2][..], &[2][..], &[][..], &[][..]],
        [&[2][..], &[][..], &[0][..], &[][..]],
        [&[2][..], &[][..], &[][..], &[0][..]],
        [&[2][..], &[][..], &[][..], &[1][..]],
        [&[u32::MAX][..], &[][..], &[][..], &[][..]],
    ] {
        assert!(validate_groups(records, &groups, &stages).is_err());
    }
    Ok(())
}

#[test]
fn ray_dispatch_checks_axis_invocation_and_overflow_limits() -> Result<()> {
    let mut limits = vk::PhysicalDeviceLimits::default();
    limits.max_compute_work_group_count = [2, 3, 4];
    limits.max_compute_work_group_size = [8, 4, 2];
    validate_extent([16, 12, 8], &limits, 1536)?;
    validate_extent([0, 12, 8], &limits, 1)?;
    for extent in [[17, 1, 1], [1, 13, 1], [1, 1, 9], [16, 12, 8]] {
        assert!(validate_extent(extent, &limits, 1535).is_err());
    }
    limits.max_compute_work_group_count = [u32::MAX; 3];
    limits.max_compute_work_group_size = [u32::MAX; 3];
    assert!(validate_extent([u32::MAX; 3], &limits, u32::MAX).is_err());
    Ok(())
}

#[test]
#[ignore = "requires Vulkan validation, Slang, descriptor heaps, and ray tracing pipeline support"]
fn trace_rays_selects_records_and_retains_resources_until_completion() -> Result<()> {
    let _ = zenith_core::log::initialize(zenith_core::log::LevelFilter::Info);
    let instance = Instance::new(&[], true)?;
    ensure!(
        instance.validation_enabled(),
        "test requires Vulkan validation"
    );
    let gpu = Gpu::create(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
        None,
        GraphicsMode::Full,
    )?;
    {
        let descriptors = Descriptors::new(&gpu, 32, 8)?;
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/trace_rays.slang");
        let shaders = [
            ("raygen", ShaderStage::RayGeneration),
            ("miss_zero", ShaderStage::Miss),
            ("miss_one", ShaderStage::Miss),
            ("hit_zero", ShaderStage::ClosestHit),
            ("hit_one", ShaderStage::ClosestHit),
            ("callable", ShaderStage::Callable),
            ("raygen_empty", ShaderStage::RayGeneration),
        ]
        .map(|(entry, stage)| gpu.compile_shader(&path, entry, stage))
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
        let shaders = shaders.iter().collect::<Vec<_>>();
        let groups = [
            RayTracingShaderGroup::General { shader: 2 },
            RayTracingShaderGroup::Triangles {
                closest_hit: Some(4),
                any_hit: None,
            },
            RayTracingShaderGroup::General { shader: 0 },
            RayTracingShaderGroup::General { shader: 5 },
            RayTracingShaderGroup::General { shader: 1 },
            RayTracingShaderGroup::Triangles {
                closest_hit: Some(3),
                any_hit: None,
            },
            RayTracingShaderGroup::General { shader: 6 },
        ];
        let pipeline = gpu.ray_tracing(&RayTracingDesc {
            shaders: &shaders,
            groups: &groups,
            max_recursion_depth: 1,
        })?;
        let desc = ShaderBindingTableDesc {
            raygen: 2,
            miss: &[4, 0],
            hit: &[5, 1],
            callable: &[3],
        };
        let table = gpu.shader_binding_table(&pipeline, &desc)?;
        let no_callable = gpu.shader_binding_table(
            &pipeline,
            &ShaderBindingTableDesc {
                callable: &[],
                ..desc
            },
        )?;
        let empty = gpu.shader_binding_table(
            &pipeline,
            &ShaderBindingTableDesc {
                raygen: 6,
                miss: &[],
                hit: &[],
                callable: &[],
            },
        )?;
        assert!(
            gpu.shader_binding_table(&pipeline, &ShaderBindingTableDesc { raygen: 0, ..desc })
                .is_err()
        );
        let properties = gpu.ray_tracing_properties().unwrap();
        for region in [
            table.regions().raygen,
            table.regions().miss,
            table.regions().hit,
            table.regions().callable,
        ] {
            assert_eq!(
                region.device_address % properties.shader_group_base_alignment as u64,
                0
            );
            assert_eq!(
                region.stride % properties.shader_group_handle_alignment as u64,
                0
            );
            assert!(region.stride <= properties.max_shader_group_stride as u64);
        }
        assert_eq!(table.regions().raygen.size, table.regions().raygen.stride);
        assert_eq!(no_callable.regions().callable.device_address, 0);
        assert_eq!(no_callable.regions().callable.stride, 0);
        assert_eq!(no_callable.regions().callable.size, 0);
        assert_eq!(empty.regions().miss.device_address, 0);
        assert_eq!(empty.regions().hit.device_address, 0);
        let weak_pipeline = Arc::downgrade(&pipeline);
        let weak_table = Arc::downgrade(&table);
        let weak_storage = Arc::downgrade(&table.storage());
        let vertices = gpu.allocate(36, MemoryDomain::Upload)?;
        vertices.write(
            0,
            bytemuck::cast_slice(&[[-1.0f32, 2.0, -1.0], [1.0, 2.0, -1.0], [0.0, 2.0, 1.0]]),
        )?;
        let indices = gpu.allocate(12, MemoryDomain::Upload)?;
        indices.write(0, bytemuck::cast_slice(&[0u32, 1, 2]))?;
        let mut build = gpu.commands()?;
        let blas = unsafe {
            build.build_blas(&TriangleGeometry {
                vertices: vertices.whole(),
                indices: indices.whole(),
                vertex_count: 3,
                vertex_stride: 12,
            })?
        };
        let tlas = unsafe {
            build.build_tlas(&[AccelerationInstance {
                blas: blas.clone(),
                transform: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                ],
            }])?
        };
        build.barrier(Access::AS_BUILD_WRITE, Access::AS_RAY_TRACING_READ)?;
        build.submit()?.wait(10_000_000_000)?;
        drop((vertices, indices));
        let weak_blas = Arc::downgrade(&blas);
        let weak_tlas = Arc::downgrade(&tlas);
        let result = gpu.allocate(72, MemoryDomain::Readback)?;
        result.write(0, bytemuck::cast_slice(&[99u32; 18]))?;
        #[repr(C)]
        #[derive(Clone, Copy, Pod, Zeroable)]
        struct Root {
            scene: crate::GpuAddress,
            results: crate::GpuAddress,
            offset: u32,
            call_enabled: u32,
            addend: u32,
            padding: u32,
        }
        let mut arguments = Arguments::new(&gpu, 1024)?;
        let mut commands = gpu.commands()?;
        commands.bind_descriptors(&descriptors)?;
        let mut root = arguments.push(&Root {
            scene: tlas.address(),
            results: result.address(),
            offset: 0,
            call_enabled: 1,
            addend: 5,
            padding: 0,
        })?;
        unsafe {
            assert!(
                commands
                    .trace_rays(&table, &root.slice(1..root.size())?, [1, 1, 1], &[], &[])
                    .is_err()
            );
            assert!(
                commands
                    .trace_rays(&table, &root.slice(0..4)?, [1, 1, 1], &[], &[])
                    .is_err()
            );
            assert!(
                commands
                    .trace_rays(&table, &root, [u32::MAX; 3], &[], &[])
                    .is_err()
            );
            commands.trace_rays(&table, &root, [2, 2, 2], &[result.whole()], &[tlas.clone()])?;
        }
        assert!(table.storage().write(0, &[0]).is_err());
        commands.barrier(Access::RAY_TRACING_WRITE, Access::RAY_TRACING_WRITE)?;
        root = arguments.push(&Root {
            scene: tlas.address(),
            results: result.address(),
            offset: 8,
            call_enabled: 0,
            addend: 5,
            padding: 0,
        })?;
        unsafe {
            commands.trace_rays(
                &no_callable,
                &root,
                [2, 2, 2],
                &[result.whole()],
                &[tlas.clone()],
            )?;
        }
        commands.barrier(Access::RAY_TRACING_WRITE, Access::RAY_TRACING_WRITE)?;
        root = arguments.push(&Root {
            scene: tlas.address(),
            results: result.address(),
            offset: 16,
            call_enabled: 0,
            addend: 5,
            padding: 0,
        })?;
        unsafe {
            commands.trace_rays(&empty, &root, [2, 1, 1], &[result.whole()], &[])?;
        }
        commands.barrier(Access::RAY_TRACING_WRITE, Access::HOST_READ)?;
        let weak_arguments = Arc::downgrade(&root.memory);
        drop((
            blas,
            tlas,
            pipeline,
            table,
            no_callable,
            empty,
            arguments,
            root,
        ));
        let mut submission = commands.submit()?;
        assert!(weak_pipeline.upgrade().is_some());
        assert!(weak_table.upgrade().is_some());
        assert!(weak_storage.upgrade().is_some());
        assert!(weak_arguments.upgrade().is_some());
        assert!(weak_blas.upgrade().is_some());
        assert!(weak_tlas.upgrade().is_some());
        submission.wait(10_000_000_000)?;
        let mut values = [0u32; 18];
        result.read(0, bytemuck::cast_slice_mut(&mut values))?;
        assert_eq!(
            values,
            [
                22, 112, 32, 212, 22, 112, 32, 212, 15, 105, 25, 205, 15, 105, 25, 205, 5, 6
            ]
        );
        assert!(weak_pipeline.upgrade().is_none());
        assert!(weak_table.upgrade().is_none());
        assert!(weak_storage.upgrade().is_none());
        assert!(weak_arguments.upgrade().is_none());
        assert!(weak_blas.upgrade().is_none());
        assert!(weak_tlas.upgrade().is_none());
        assert_eq!(gpu.pipeline_count(), 0);
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
