use super::*;
use crate::WorldRenderer;
use anyhow::{ensure, Context};
use glam::{Mat4, Vec3};
use zenith_asset::{
    material::{ClearCoat, Hair, Material, ShadingModel},
    mesh::{Mesh, MeshInstance, Scene, SceneNode, Vertex},
    texture::{Texture as CpuTexture, TextureFormat},
    AssetServer, CpuRetention, MemorySource,
};

mod integration;
mod progressive;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Ray {
    origin: [f32; 3],
    direction: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    scene: u64,
    instances: u64,
    rays: GpuAddress,
    output: GpuAddress,
}

fn mesh(uv_scale: f32, handedness: f32) -> Mesh {
    let vertices = [
        ([1.0, 1.0, -1.0], [1.0, 0.0]),
        ([0.0, 1.0, 1.0], [0.5, 1.0]),
        ([-1.0, 1.0, -1.0], [0.0, 0.0]),
        ([5.0, 1.0, -1.0], [1.0, 0.0]),
        ([4.0, 1.0, 1.0], [0.5, 1.0]),
        ([3.0, 1.0, -1.0], [0.0, 0.0]),
    ]
    .into_iter()
    .map(|(position, uv)| Vertex {
        position,
        normal: [0.0, -1.0, 0.0],
        tex_coord: [uv[0] * uv_scale, uv[1] * uv_scale],
        tangent: if handedness == 0.0 {
            [0.0; 4]
        } else {
            [1.0, 0.0, 0.0, handedness]
        },
    })
    .collect();
    Mesh::new(vertices, vec![5, 3, 4, 2, 0, 1])
}

fn scene(instances: Vec<MeshInstance>) -> Scene {
    Scene {
        nodes: vec![SceneNode {
            source_index: 0,
            parent: None,
            transform: Mat4::IDENTITY.to_cols_array(),
        }],
        instances,
    }
}

fn srgb(value: u8) -> f32 {
    let value = value as f32 / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn close(actual: f32, expected: f32) {
    assert!(
        actual.is_finite() && (actual - expected).abs() < 0.0005,
        "actual {actual}, expected {expected}"
    );
}

#[test]
#[ignore = "requires Vulkan validation, Slang, descriptor heaps, and ray tracing pipeline support"]
fn opaque_hits_reconstruct_materials_and_preserve_scene_snapshots() -> Result<()> {
    std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
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
        let descriptors = Descriptors::new(&gpu, 512, 32)?;
        let assets = AssetServer::builder()
            .source(MemorySource::default())
            .with_builtin_assets()
            .cpu_retention::<Mesh>(CpuRetention::ReleaseAfterUpload)
            .cpu_retention::<CpuTexture>(CpuRetention::ReleaseAfterUpload)
            .build()?;
        let texture = |pixel: [u8; 4], format| {
            assets.add(CpuTexture {
                width: 2,
                height: 2,
                mip_levels: 2,
                is_cubemap: false,
                format,
                pixels: pixel
                    .repeat(4)
                    .into_iter()
                    .chain([255, 0, 0, 255])
                    .collect(),
            })
        };
        let base = texture([128, 64, 192, 255], TextureFormat::Rgba8Srgb);
        let mra = texture([255, 128, 64, 255], TextureFormat::Rgba8Unorm);
        let normal = texture([204, 179, 255, 255], TextureFormat::Rgba8Unorm);
        let material = assets.add(Material {
            base_color: [0.5, 0.75, 0.25, 0.3],
            metallic: 0.8,
            roughness: 0.6,
            base_color_tex: Some(base.clone()),
            mra_tex: Some(mra.clone()),
            normal_tex: Some(normal.clone()),
            ..Default::default()
        });
        let shared = assets.add(mesh(1.0, 1.0));
        let reversed = assets.add(mesh(0.000001, -1.0));
        let missing = assets.add(mesh(1.0, 0.0));
        let models = [
            Mat4::from_translation(Vec3::new(0.0, 2.0, 0.0)),
            Mat4::from_translation(Vec3::new(12.0, 2.0, 0.0))
                * Mat4::from_rotation_z(0.4)
                * Mat4::from_scale(Vec3::new(-2.0, 0.5, 1.5)),
            Mat4::from_translation(Vec3::new(24.0, 2.0, 0.0))
                * Mat4::from_rotation_x(0.3)
                * Mat4::from_scale(Vec3::new(1.5, 2.0, 0.7)),
            Mat4::from_translation(Vec3::new(36.0, 2.0, 0.0)),
        ];
        let opaque: Vec<_> = models
            .iter()
            .zip([
                shared.clone(),
                shared.clone(),
                reversed.clone(),
                missing.clone(),
            ])
            .map(|(model, mesh)| MeshInstance {
                node: 0,
                mesh,
                material: material.clone(),
                transform: model.to_cols_array(),
            })
            .collect();
        let coat = assets.add(Material {
            shading_model: ShadingModel::ClearCoat,
            clearcoat: ClearCoat {
                weight: 0.5,
                ..Default::default()
            },
            ..Default::default()
        });
        let hair = assets.add(Material {
            shading_model: ShadingModel::Hair,
            hair: Some(Hair::default()),
            ..Default::default()
        });
        let excluded = |x, material| MeshInstance {
            node: 0,
            mesh: shared.clone(),
            material,
            transform: Mat4::from_translation(Vec3::new(x, 2.0, 0.0)).to_cols_array(),
        };
        let mixed = assets.add(scene(vec![
            excluded(48.0, coat),
            opaque[0].clone(),
            excluded(60.0, hair),
            opaque[1].clone(),
            opaque[2].clone(),
            opaque[3].clone(),
        ]));
        let hidden = assets.add(scene(vec![excluded(72.0, material.clone())]));
        let mut renderer = WorldRenderer::new(&gpu, &descriptors, 16, 16)?;
        renderer.add_scene(&gpu, &descriptors, &mixed)?;
        renderer.add_scene(&gpu, &descriptors, &hidden)?;
        renderer.set_scene_visible(1, false)?;
        let snapshot = renderer.prepare_path_tracing_scene()?;
        assert_eq!(snapshot.instance_count(), 4);
        assert!(Arc::ptr_eq(
            &snapshot,
            &renderer.prepare_path_tracing_scene()?
        ));
        assert!(shared.get().is_none() && reversed.get().is_none() && missing.get().is_none());
        assert!(base.get().is_none() && mra.get().is_none() && normal.get().is_none());
        assert!(Arc::ptr_eq(
            &snapshot.instances[0].geometry,
            &snapshot.instances[1].geometry
        ));
        let shaders = gpu.compile_shaders([
            (
                "content/shaders/path_tracing_validation.slang",
                "raygen",
                ShaderStage::RayGeneration,
            ),
            (
                "content/shaders/path_tracing_validation.slang",
                "miss",
                ShaderStage::Miss,
            ),
            (
                "content/shaders/path_tracing_validation.slang",
                "closest_hit",
                ShaderStage::ClosestHit,
            ),
        ])?;
        let pipeline = gpu.ray_tracing(&RayTracingDesc {
            shaders: &[&shaders[0], &shaders[1], &shaders[2]],
            groups: &[
                RayTracingShaderGroup::General { shader: 0 },
                RayTracingShaderGroup::General { shader: 1 },
                RayTracingShaderGroup::Triangles {
                    closest_hit: Some(2),
                    any_hit: None,
                },
            ],
            max_recursion_depth: 1,
        })?;
        let table = gpu.shader_binding_table(
            &pipeline,
            &ShaderBindingTableDesc {
                raygen: 0,
                miss: &[1],
                hit: &[2],
                callable: &[],
            },
        )?;
        let mut rays = Vec::new();
        let mut expected = Vec::new();
        let normal_ts = {
            let x = 204.0 / 255.0 * 2.0 - 1.0;
            let y = 179.0 / 255.0 * 2.0 - 1.0;
            Vec3::new(x, y, (1.0f32 - x * x - y * y).sqrt()).normalize()
        };
        for (i, model) in models.iter().enumerate() {
            let n = model
                .inverse()
                .transpose()
                .transform_vector3(-Vec3::Y)
                .normalize();
            let t = model.transform_vector3(Vec3::X).normalize();
            let t = (t - n * n.dot(t)).normalize();
            let sign = if i == 2 { -1.0 } else { 1.0 }
                * if model.determinant() < 0.0 { -1.0 } else { 1.0 };
            let mapped = if i == 3 {
                n
            } else {
                (t * normal_ts.x + n.cross(t) * sign * normal_ts.y + n * normal_ts.z).normalize()
            };
            for (primitive, back) in [(1, false), (1, true), (0, false)] {
                let p = model.transform_point3(Vec3::new(
                    if primitive == 0 { 4.1 } else { 0.1 },
                    1.0,
                    0.0,
                ));
                let facing = if back { -1.0 } else { 1.0 };
                rays.push(Ray {
                    origin: (p + n * facing * 3.0).to_array(),
                    direction: (-n * facing).to_array(),
                });
                let uv_scale = if i == 2 { 0.000001 } else { 1.0 };
                expected.push((
                    p,
                    mapped * facing,
                    n * facing,
                    [0.55 * uv_scale, 0.5 * uv_scale],
                    i,
                    primitive,
                ));
            }
        }
        for x in [48.0, 60.0, 72.0, 100.0] {
            rays.push(Ray {
                origin: [x, 0.0, 0.0],
                direction: [0.0, 1.0, 0.0],
            });
        }
        let input = gpu.allocate(
            std::mem::size_of_val(rays.as_slice()) as u64,
            MemoryDomain::Upload,
        )?;
        input.write(0, bytemuck::cast_slice(&rays))?;
        let result = gpu.allocate(rays.len() as u64 * 80, MemoryDomain::Readback)?;
        let mut arguments = Arguments::new(&gpu, 1024)?;
        let root = arguments.push(&Root {
            scene: snapshot
                .acceleration()
                .context("expected TLAS")?
                .address()
                .value(),
            instances: snapshot
                .instance_buffer()
                .context("expected instance table")?
                .address()
                .value(),
            rays: input.address(),
            output: result.address(),
        })?;
        let mut commands = gpu.commands()?;
        commands.bind_descriptors(&descriptors)?;
        snapshot.retain(&mut commands)?;
        unsafe {
            commands.trace_rays(
                &table,
                &root,
                [rays.len() as u32, 1, 1],
                &[input.whole(), result.whole()],
                &[],
            )?;
        }
        commands.barrier(Access::RAY_TRACING_WRITE, Access::HOST_READ)?;
        commands.submit()?.wait(10_000_000_000)?;
        let mut values = vec![[0.0f32; 4]; rays.len() * 5];
        result.read(0, bytemuck::cast_slice_mut(&mut values))?;
        for (row, &(p, n, ng, uv, index, primitive)) in values.chunks_exact(5).zip(&expected) {
            close(row[0][3], 1.0);
            for c in 0..3 {
                close(row[0][c], p[c]);
                close(row[1][c], n[c]);
                close(row[2][c], ng[c]);
            }
            close(row[1][3], 0.8 * 64.0 / 255.0);
            close(row[2][3], 0.6 * 128.0 / 255.0);
            for (c, factor, pixel) in [(0, 0.5, 128), (1, 0.75, 64), (2, 0.25, 192)] {
                close(row[3][c], factor * srgb(pixel));
            }
            close(row[3][3], uv[0]);
            close(row[4][0], uv[1]);
            close(row[4][1], index as f32);
            close(row[4][2], primitive as f32);
            close(row[4][3], 3.0);
        }
        for row in values[expected.len() * 5..].chunks_exact(5) {
            assert_eq!(row, &[[0.0; 4]; 5]);
        }
        let replacement = assets.add(Material {
            base_color: [0.2, 0.3, 0.4, 1.0],
            metallic: 0.0,
            roughness: 0.7,
            ..Default::default()
        });
        let replacement = assets.add(scene(
            opaque
                .into_iter()
                .map(|mut instance| {
                    instance.material = replacement.clone();
                    instance
                })
                .collect(),
        ));
        renderer.add_scene(&gpu, &descriptors, &replacement)?;
        renderer.set_scene_visible(0, false)?;
        let changed = renderer.prepare_path_tracing_scene()?;
        assert!(!Arc::ptr_eq(&snapshot, &changed));
        assert!(Arc::ptr_eq(
            snapshot.acceleration().unwrap(),
            changed.acceleration().unwrap()
        ));
        assert!(!Arc::ptr_eq(
            snapshot.instance_buffer().unwrap(),
            changed.instance_buffer().unwrap()
        ));
        assert_eq!(changed.instances[0].data.base_color, [0.2, 0.3, 0.4, 1.0]);
        renderer.set_scene_visible(2, false)?;
        let empty = renderer.prepare_path_tracing_scene()?;
        assert_eq!(empty.instance_count(), 0);
        assert!(empty.acceleration().is_none() && empty.instance_buffer().is_none());
        assert!(Arc::ptr_eq(&empty, &renderer.prepare_path_tracing_scene()?));
        let changed_root = arguments.push(&Root {
            scene: changed.acceleration().unwrap().address().value(),
            instances: changed.instance_buffer().unwrap().address().value(),
            rays: input.address(),
            output: result.address(),
        })?;
        let empty_root = arguments.push(&Root {
            scene: 0,
            instances: 0,
            rays: input.address(),
            output: result
                .slice((3 * 5 * std::mem::size_of::<[f32; 4]>()) as u64..result.size())?
                .address(),
        })?;
        let mut commands = gpu.commands()?;
        commands.bind_descriptors(&descriptors)?;
        changed.retain(&mut commands)?;
        empty.retain(&mut commands)?;
        unsafe {
            commands.trace_rays(
                &table,
                &changed_root,
                [3, 1, 1],
                &[input.whole(), result.whole()],
                &[],
            )?;
            commands.trace_rays(
                &table,
                &empty_root,
                [1, 1, 1],
                &[input.whole(), result.whole()],
                &[],
            )?;
        }
        commands.barrier(Access::RAY_TRACING_WRITE, Access::HOST_READ)?;
        commands.submit()?.wait(10_000_000_000)?;
        result.read(0, bytemuck::cast_slice_mut(&mut values))?;
        for (i, row) in values[..15].chunks_exact(5).enumerate() {
            close(row[0][3], 1.0);
            let expected_normal = if i == 1 { Vec3::Y } else { -Vec3::Y };
            for c in 0..3 {
                close(row[1][c], expected_normal[c]);
                close(row[3][c], [0.2, 0.3, 0.4][c]);
            }
            close(row[1][3], 0.0);
            close(row[2][3], 0.7);
        }
        assert_eq!(&values[15..20], &[[0.0; 4]; 5]);
        drop((changed_root, empty_root));
        let weak_vertices = Arc::downgrade(&snapshot.instances[0].geometry.vertices);
        let weak_texture = Arc::downgrade(snapshot.instances[0].textures[0].as_ref().unwrap());
        let weak_table = Arc::downgrade(snapshot.instance_buffer().unwrap());
        let weak_tlas = Arc::downgrade(snapshot.acceleration().unwrap());
        let mut commands = gpu.commands()?;
        commands.bind_descriptors(&descriptors)?;
        snapshot.retain(&mut commands)?;
        unsafe {
            commands.trace_rays(
                &table,
                &root,
                [1, 1, 1],
                &[input.whole(), result.whole()],
                &[],
            )?;
        }
        commands.barrier(Access::RAY_TRACING_WRITE, Access::HOST_READ)?;
        drop((renderer, snapshot, changed, empty, arguments, root));
        let mut submission = commands.submit()?;
        assert!(
            weak_vertices.upgrade().is_some()
                && weak_texture.upgrade().is_some()
                && weak_table.upgrade().is_some()
                && weak_tlas.upgrade().is_some()
        );
        submission.wait(10_000_000_000)?;
        assert!(
            weak_vertices.upgrade().is_none()
                && weak_texture.upgrade().is_none()
                && weak_table.upgrade().is_none()
                && weak_tlas.upgrade().is_none()
        );
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
