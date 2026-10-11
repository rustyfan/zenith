use super::*;
use crate::{GraphicsMode, Instance};
use std::ffi::CString;

fn shader(stage: ShaderStage) -> Shader {
    Shader {
        request: None,
        dependencies: Vec::new(),
        source: None,
        initial_watch: None,
        code: vec![0x07230203],
        stage,
        entry: CString::new("main").unwrap(),
        heap_strides: (64, 64),
    }
}

#[test]
fn shader_groups_enforce_indices_and_stage_types() -> Result<()> {
    let shaders = [
        ShaderStage::RayGeneration,
        ShaderStage::Miss,
        ShaderStage::ClosestHit,
        ShaderStage::AnyHit,
        ShaderStage::Intersection,
        ShaderStage::Callable,
    ]
    .map(shader);
    let shaders = shaders.iter().collect::<Vec<_>>();
    for index in [0, 1, 5] {
        let info = RayTracingShaderGroup::General { shader: index }.create_info(&shaders)?;
        assert_eq!(info.ty, vk::RayTracingShaderGroupTypeKHR::GENERAL);
        assert_eq!(info.general_shader, index);
        assert_eq!(info.closest_hit_shader, vk::SHADER_UNUSED_KHR);
    }
    let triangles = RayTracingShaderGroup::Triangles {
        closest_hit: Some(2),
        any_hit: Some(3),
    }
    .create_info(&shaders)?;
    assert_eq!(
        triangles.ty,
        vk::RayTracingShaderGroupTypeKHR::TRIANGLES_HIT_GROUP
    );
    assert_eq!(triangles.closest_hit_shader, 2);
    assert_eq!(triangles.any_hit_shader, 3);
    assert_eq!(triangles.intersection_shader, vk::SHADER_UNUSED_KHR);
    let empty = RayTracingShaderGroup::Triangles {
        closest_hit: None,
        any_hit: None,
    }
    .create_info(&shaders)?;
    assert_eq!(empty.closest_hit_shader, vk::SHADER_UNUSED_KHR);
    assert_eq!(empty.any_hit_shader, vk::SHADER_UNUSED_KHR);
    let procedural = RayTracingShaderGroup::Procedural {
        intersection: 4,
        closest_hit: None,
        any_hit: Some(3),
    }
    .create_info(&shaders)?;
    assert_eq!(
        procedural.ty,
        vk::RayTracingShaderGroupTypeKHR::PROCEDURAL_HIT_GROUP
    );
    assert_eq!(procedural.intersection_shader, 4);
    assert_eq!(procedural.closest_hit_shader, vk::SHADER_UNUSED_KHR);
    for group in [
        RayTracingShaderGroup::General { shader: 2 },
        RayTracingShaderGroup::General { shader: u32::MAX },
        RayTracingShaderGroup::Triangles {
            closest_hit: Some(1),
            any_hit: None,
        },
        RayTracingShaderGroup::Triangles {
            closest_hit: None,
            any_hit: Some(2),
        },
        RayTracingShaderGroup::Triangles {
            closest_hit: Some(u32::MAX),
            any_hit: None,
        },
        RayTracingShaderGroup::Procedural {
            intersection: 0,
            closest_hit: None,
            any_hit: None,
        },
        RayTracingShaderGroup::Procedural {
            intersection: u32::MAX,
            closest_hit: None,
            any_hit: None,
        },
    ] {
        assert!(group.create_info(&shaders).is_err(), "{group:?}");
    }
    Ok(())
}

#[test]
fn pipeline_descriptions_require_raygen_and_respect_recursion_limits() -> Result<()> {
    let raygen = shader(ShaderStage::RayGeneration);
    let miss = shader(ShaderStage::Miss);
    let compute = shader(ShaderStage::Compute);
    let groups = [RayTracingShaderGroup::General { shader: 0 }];
    let shaders = [&raygen];
    let mut desc = RayTracingDesc {
        shaders: &shaders,
        groups: &groups,
        max_recursion_depth: 1,
    };
    desc.validate(1)?;
    desc.max_recursion_depth = 0;
    desc.validate(1)?;
    desc.max_recursion_depth = 2;
    assert!(desc.validate(1).is_err());
    for shaders in [vec![], vec![&miss], vec![&raygen, &compute]] {
        assert!(
            RayTracingDesc {
                shaders: &shaders,
                groups: &groups,
                max_recursion_depth: 1
            }
            .validate(1)
            .is_err()
        );
    }
    assert!(
        RayTracingDesc {
            shaders: &[&raygen],
            groups: &[],
            max_recursion_depth: 1
        }
        .validate(1)
        .is_err()
    );
    Ok(())
}

#[test]
fn pipeline_cache_keys_include_entries_stages_group_order_and_depth() {
    let raygen = shader(ShaderStage::RayGeneration);
    let miss = shader(ShaderStage::Miss);
    let shaders = [&raygen, &miss];
    let groups = [
        RayTracingShaderGroup::General { shader: 0 },
        RayTracingShaderGroup::General { shader: 1 },
    ];
    let key = RayTracingDesc {
        shaders: &shaders,
        groups: &groups,
        max_recursion_depth: 1,
    }
    .key();
    let mut alternate = shader(ShaderStage::RayGeneration);
    alternate.entry = CString::new("alternate").unwrap();
    let callable = shader(ShaderStage::Callable);
    for shaders in [[&alternate, &miss], [&raygen, &callable]] {
        assert!(
            key != RayTracingDesc {
                shaders: &shaders,
                groups: &groups,
                max_recursion_depth: 1
            }
            .key()
        );
    }
    assert!(
        key != RayTracingDesc {
            shaders: &shaders,
            groups: &[groups[1], groups[0]],
            max_recursion_depth: 1
        }
        .key()
    );
    assert!(
        key != RayTracingDesc {
            shaders: &shaders,
            groups: &groups,
            max_recursion_depth: 2
        }
        .key()
    );
}

#[test]
#[ignore = "requires Vulkan validation, Slang, descriptor heaps, and ray tracing pipeline support"]
fn ray_tracing_pipelines_compile_cache_and_release() -> Result<()> {
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
        let properties = gpu
            .ray_tracing_properties()
            .context("test requires ray tracing pipeline support")?;
        assert!(gpu.info.ray_tracing_pipeline);
        assert!(properties.shader_group_handle_size > 0 && properties.max_ray_recursion_depth >= 1);
        assert!(properties.p_next.is_null());
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/shaders/ray_tracing.slang");
        let shaders = [
            ("raygen", ShaderStage::RayGeneration),
            ("miss", ShaderStage::Miss),
            ("closest_hit", ShaderStage::ClosestHit),
            ("any_hit", ShaderStage::AnyHit),
            ("intersection", ShaderStage::Intersection),
            ("callable", ShaderStage::Callable),
        ]
        .map(|(entry, stage)| gpu.compile_shader(&path, entry, stage))
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
        let shaders = shaders.iter().collect::<Vec<_>>();
        let groups = [
            RayTracingShaderGroup::General { shader: 0 },
            RayTracingShaderGroup::General { shader: 1 },
            RayTracingShaderGroup::Triangles {
                closest_hit: Some(2),
                any_hit: Some(3),
            },
            RayTracingShaderGroup::Procedural {
                intersection: 4,
                closest_hit: Some(2),
                any_hit: Some(3),
            },
            RayTracingShaderGroup::General { shader: 5 },
        ];
        let desc = RayTracingDesc {
            shaders: &shaders,
            groups: &groups,
            max_recursion_depth: 1,
        };
        let pipeline = gpu.ray_tracing(&desc)?;
        let cached = gpu.ray_tracing(&desc)?;
        assert!(Arc::ptr_eq(&pipeline, &cached));
        assert_eq!(pipeline.shader_groups(), &groups);
        assert_eq!(pipeline.shader_stages().len(), 6);
        assert_eq!(pipeline.max_recursion_depth(), 1);
        assert_eq!(gpu.pipeline_count(), 1);
        let mut reordered_groups = groups;
        reordered_groups.swap(0, 1);
        let reordered = gpu.ray_tracing(&RayTracingDesc {
            groups: &reordered_groups,
            ..desc
        })?;
        assert!(!Arc::ptr_eq(&pipeline, &reordered));
        let depth = if properties.max_ray_recursion_depth >= 2 {
            2
        } else {
            0
        };
        let deeper = gpu.ray_tracing(&RayTracingDesc {
            max_recursion_depth: depth,
            ..desc
        })?;
        assert!(!Arc::ptr_eq(&pipeline, &deeper));
        assert_eq!(gpu.pipeline_count(), 3);
        let capture = Gpu::create(
            instance.clone(),
            Some(&gpu.info.name),
            None,
            GraphicsMode::Capture,
        )?;
        assert!(capture.ray_tracing_properties().is_none());
        assert!(
            capture
                .compile_shader(&path, "raygen", ShaderStage::RayGeneration)
                .is_err()
        );
        assert!(capture.ray_tracing(&desc).is_err());
        let weak = Arc::downgrade(&pipeline);
        drop((pipeline, cached, reordered, deeper));
        assert!(weak.upgrade().is_none());
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
