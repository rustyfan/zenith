use super::*;
use glam::Vec3;
use zenith_asset::{mesh::MeshInstance, CpuRetention};

fn quad(z: f32) -> Mesh {
    Mesh::new(
        [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .into_iter()
            .map(|(x, dz)| Vertex {
                position: [x, 0.0, z + dz],
                normal: [0.0, -1.0, 0.0],
                tex_coord: [0.0; 2],
                tangent: [0.0; 4],
            })
            .collect(),
        vec![0, 1, 2, 0, 2, 3],
    )
}

fn node() -> SceneNode {
    SceneNode {
        source_index: 0,
        parent: None,
        transform: Mat4::IDENTITY.to_cols_array(),
    }
}

fn caster_transform() -> Mat4 {
    Mat4::from_translation(Vec3::new(6.0, 1.0, 0.0)) * Mat4::from_scale(Vec3::new(1.0, 1.0, 0.6))
}

struct ShadowCaster;
impl Importer for ShadowCaster {
    type Settings = ();
    type Output = Scene;
    const KEY: &'static str = "test.shadow-caster";
    const VERSION: u32 = 1;
    fn extensions(&self) -> &[&str] {
        &["caster"]
    }
    fn import(
        &self,
        bytes: &[u8],
        _: &(),
        ctx: &mut ImportContext<'_>,
    ) -> zenith_asset::Result<SceneData> {
        let mesh = ctx.emit::<Mesh>("mesh", quad(bytes[0] as f32 * 8.0))?;
        let material = ctx.emit::<Material>("material", MaterialData::default())?;
        Ok(SceneData {
            nodes: vec![node()],
            instances: vec![MeshInstanceData {
                node: 0,
                mesh,
                material,
                transform: caster_transform().to_cols_array(),
            }],
        })
    }
}

fn center(pixels: &[u8]) -> [u8; 3] {
    pixels[(32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 3]
        .try_into()
        .unwrap()
}

#[test]
#[ignore = "requires Vulkan validation, Slang, and ray query support"]
fn directional_shadows_follow_offscreen_casters_and_streamed_geometry() -> Result<()> {
    std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
    let _ = zenith_core::log::initialize(zenith_core::log::LevelFilter::Info);
    let instance = Instance::new(&[], true)?;
    anyhow::ensure!(
        instance.validation_enabled(),
        "test requires Vulkan validation"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    {
        let descriptors = Descriptors::new(&gpu, 512, 32)?;
        let source = Arc::new(MemorySource::default());
        source.insert("shadow.caster", vec![0])?;
        let assets = AssetServer::builder()
            .source(source.clone())
            .with_builtin_assets()
            .register_importer(ShadowCaster)
            .cpu_retention::<Mesh>(CpuRetention::ReleaseAfterUpload)
            .build()?;
        let receiver = assets.add(Scene {
            nodes: vec![node()],
            instances: vec![MeshInstance {
                node: 0,
                mesh: assets.add(quad(0.0)),
                material: assets.add(Material {
                    base_color: [0.8, 0.5, 0.2, 1.0],
                    metallic: 0.0,
                    roughness: 0.6,
                    emissive: [0.0; 3],
                    base_color_tex: None,
                    mra_tex: None,
                    normal_tex: None,
                    emissive_tex: None,
                }),
                transform: (Mat4::from_translation(Vec3::new(0.0, 4.0, 0.0))
                    * Mat4::from_scale(Vec3::new(3.0, 1.0, 1.5)))
                .to_cols_array(),
            }],
        });
        let mut renderer =
            WorldRenderer::new(&gpu, &descriptors, 64, 64)?.with_asset_server(&assets);
        let mut settings = renderer.lighting_settings();
        settings.directional.direction_to_light = Vec3::new(2.0, -1.0, 0.0);
        settings.sky_intensity = 0.0;
        renderer.set_lighting(settings)?;
        let mut cache = ResourceCache::default();
        let empty = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
        renderer.add_scene(&gpu, &descriptors, &receiver)?;
        let lit = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
        assert!(center(&lit)[0] > 80, "{:?}", center(&lit));
        let caster = assets.load_blocking::<Scene>("shadow.caster")?;
        renderer.add_scene(&gpu, &descriptors, &caster)?;
        assert!(caster.get().unwrap().instances[0].mesh.get().is_none());
        let shadowed = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
        assert_eq!(center(&shadowed), [0; 3]);
        let side = (32 * 64 + 48) * 4;
        assert_eq!(&shadowed[side..side + 4], &lit[side..side + 4]);
        assert_eq!(&shadowed[..4], &lit[..4]);
        if let Some(directory) = std::env::var_os("ZENITH_SHADOW_CAPTURE") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory)?;
            std::fs::write(directory.join("lit.rgba"), &lit)?;
            std::fs::write(directory.join("shadowed.rgba"), &shadowed)?;
        }
        let completed = gpu.completed_value()?;
        assert_eq!(
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
            shadowed
        );
        assert_eq!(
            gpu.completed_value()?,
            completed + 1,
            "unchanged scene must reuse its TLAS"
        );

        settings.shadows.enabled = false;
        renderer.set_lighting(settings)?;
        assert_eq!(
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
            lit,
            "caster is outside the camera frustum"
        );
        settings.shadows.enabled = true;
        settings.shadows.max_distance = 1.0;
        renderer.set_lighting(settings)?;
        assert_eq!(frame(&mut renderer, &gpu, &descriptors, &mut cache)?, lit);
        settings.shadows.max_distance = 10000.0;
        renderer.set_lighting(settings)?;
        renderer.set_scene_visible(1, false)?;
        assert_eq!(frame(&mut renderer, &gpu, &descriptors, &mut cache)?, lit);
        renderer.set_scene_visible(1, true)?;
        assert_eq!(
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
            shadowed
        );

        let model = caster_transform()
            * Mat4::from_rotation_z(0.3)
            * Mat4::from_scale(Vec3::new(-1.0, 0.5, 1.0));
        let mesh = &mut renderer.scenes[1].meshes[0];
        mesh.model = model.to_cols_array();
        mesh.normal_matrix = model.inverse().transpose().to_cols_array();
        mesh.mirrored = true;
        assert_eq!(
            center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?),
            [0; 3]
        );
        let model = Mat4::from_translation(Vec3::new(0.0, 0.0, 4.0)) * model;
        renderer.scenes[1].meshes[0].model = model.to_cols_array();
        assert_eq!(frame(&mut renderer, &gpu, &descriptors, &mut cache)?, lit);

        let old_blas = Arc::downgrade(&renderer.scenes[1].meshes[0].geometry.blas);
        let uploads = renderer.upload_stats().meshes;
        source.insert("shadow.caster", vec![1])?;
        assets.reload(&caster)?;
        caster.wait()?;
        let revision = caster.revision().unwrap();
        let started = Instant::now();
        while renderer.scenes[1].revision != Some(revision) {
            anyhow::ensure!(
                started.elapsed() < Duration::from_secs(10),
                "shadow caster reload timed out"
            );
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
        }
        assert_eq!(frame(&mut renderer, &gpu, &descriptors, &mut cache)?, lit);
        assert_eq!(renderer.upload_stats().meshes, uploads + 1);
        assert!(old_blas.upgrade().is_none());
        source.insert("shadow.caster", vec![0])?;
        assets.reload(&caster)?;
        caster.wait()?;
        let revision = caster.revision().unwrap();
        let started = Instant::now();
        while renderer.scenes[1].revision != Some(revision) {
            anyhow::ensure!(
                started.elapsed() < Duration::from_secs(10),
                "shadow caster reload timed out"
            );
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
        }
        assert_eq!(
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
            shadowed
        );

        let sky = assets.add(CpuTexture {
            width: 1,
            height: 1,
            mip_levels: 1,
            is_cubemap: true,
            format: TextureFormat::Rgba8Unorm,
            pixels: vec![128; 24],
        });
        renderer.set_skybox(&gpu, &descriptors, &sky)?;
        settings.directional.intensity = 0.0;
        settings.sky_intensity = 1.0;
        renderer.set_lighting(settings)?;
        let ambient = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
        assert!(center(&ambient)[0] > 0);
        settings.shadows.enabled = false;
        renderer.set_lighting(settings)?;
        assert_eq!(
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
            ambient
        );
        settings.shadows.enabled = true;
        settings.sky_intensity = 0.0;
        renderer.set_lighting(settings)?;
        renderer.set_scene_visible(0, false)?;
        renderer.set_scene_visible(1, false)?;
        assert_eq!(frame(&mut renderer, &gpu, &descriptors, &mut cache)?, empty);
        for bias in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let mut invalid = settings;
            invalid.shadows.bias = bias;
            assert!(renderer.set_lighting(invalid).is_err());
        }
        for distance in [0.0, settings.shadows.bias, f32::NAN, f32::INFINITY] {
            let mut invalid = settings;
            invalid.shadows.max_distance = distance;
            assert!(renderer.set_lighting(invalid).is_err());
        }
    }
    gpu.wait_idle()?;
    drop(gpu);
    anyhow::ensure!(
        instance.validation_errors().is_empty(),
        "{:?}",
        instance.validation_errors()
    );
    Ok(())
}
