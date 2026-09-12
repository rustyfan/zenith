use super::*;
use std::sync::{mpsc, Mutex};
use zenith_asset::{AssetSource, CpuRetention};

struct GatedSource {
    source: Arc<MemorySource>,
    gate: Mutex<Option<mpsc::Receiver<()>>>,
    entered: mpsc::SyncSender<()>,
}
impl AssetSource for GatedSource {
    fn read(&self, path: &str) -> zenith_asset::Result<Vec<u8>> {
        if path == "scene.testscene" {
            if let Some(gate) = self.gate.lock().unwrap().take() {
                self.entered.send(()).unwrap();
                gate.recv_timeout(Duration::from_secs(30))
                    .map_err(|error| {
                        AssetError::caused_by(ErrorKind::Io, "model gate timed out", error)
                    })?;
            }
        }
        self.source.read(path)
    }
}
struct TestSky;
impl Importer for TestSky {
    type Settings = ();
    type Output = CpuTexture;
    const KEY: &'static str = "test.sky";
    const VERSION: u32 = 1;
    fn extensions(&self) -> &[&str] {
        &["sky"]
    }
    fn import(
        &self,
        bytes: &[u8],
        _: &(),
        _: &mut ImportContext<'_>,
    ) -> zenith_asset::Result<CpuTexture> {
        Ok(CpuTexture {
            width: 1,
            height: 1,
            mip_levels: 1,
            is_cubemap: true,
            format: TextureFormat::Rgba8Unorm,
            pixels: vec![bytes[0]; 24],
        })
    }
}
fn until_ready(
    renderer: &mut WorldRenderer,
    index: usize,
    gpu: &Arc<Gpu>,
    descriptors: &Arc<Descriptors>,
    cache: &mut ResourceCache,
) -> Result<()> {
    let start = Instant::now();
    loop {
        frame(renderer, gpu, descriptors, cache)?;
        match renderer.scene_status(index).unwrap() {
            SceneStatus::Ready { .. } => return Ok(()),
            SceneStatus::Failed { message } => anyhow::bail!(message),
            _ => anyhow::ensure!(
                start.elapsed() < Duration::from_secs(10),
                "streaming timed out"
            ),
        }
    }
}

#[test]
#[ignore = "requires Vulkan validation, Slang, and supported GPU features"]
fn skybox_first_streaming_releases_cpu_and_restores_for_new_consumers() -> Result<()> {
    std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
    let instance = Instance::new(&[], true)?;
    anyhow::ensure!(
        instance.validation_enabled(),
        "streaming test requires Vulkan validation"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    for release_metadata in [false, true] {
        let descriptors = Descriptors::new(&gpu, 512, 32)?;
        let source = Arc::new(MemorySource::default());
        source.insert("scene.testscene", vec![50])?;
        source.insert("environment.sky", vec![255])?;
        let (release, gate) = mpsc::channel();
        let (entered, started) = mpsc::sync_channel(1);
        let mut builder = AssetServer::builder()
            .source(GatedSource {
                source: source.clone(),
                gate: Mutex::new(Some(gate)),
                entered,
            })
            .with_builtin_assets()
            .register_importer(TestScene)
            .register_importer(TestSky)
            .cpu_retention::<Mesh>(CpuRetention::ReleaseAfterUpload)
            .cpu_retention::<CpuTexture>(CpuRetention::ReleaseAfterUpload);
        if release_metadata {
            builder = builder
                .cpu_retention::<Scene>(CpuRetention::ReleaseAfterUpload)
                .cpu_retention::<Material>(CpuRetention::ReleaseAfterUpload);
        }
        let assets = builder.build()?;
        let sky = assets.load_blocking::<CpuTexture>("environment.sky")?;
        let mut renderer =
            WorldRenderer::new(&gpu, &descriptors, 64, 64)?.with_asset_server(&assets);
        renderer.set_staging_cache_budget(0);
        renderer.set_skybox(&gpu, &descriptors, &sky)?;
        assert!(sky.get().is_none());
        let counts = renderer.upload_stats();
        renderer.set_skybox(&gpu, &descriptors, &sky)?;
        assert_eq!(renderer.upload_stats().bytes, counts.bytes);
        let mut cache = ResourceCache::default();
        let empty = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
        assert!(renderer.pipeline.is_none() && renderer.geometry_job.is_none());
        assert_eq!(assets.stats().decodes, 1);
        assert_eq!(renderer.upload_stats().meshes, 0);
        let scene = assets.load::<Scene>("scene.testscene")?;
        let index = renderer.queue_scene(&scene);
        started.recv_timeout(Duration::from_secs(10))?;
        for _ in 0..3 {
            assert_eq!(frame(&mut renderer, &gpu, &descriptors, &mut cache)?, empty);
            assert_eq!(renderer.scene_status(index), Some(SceneStatus::Waiting));
            assert_eq!(renderer.upload_stats().meshes, 0);
        }
        release.send(())?;
        scene.wait()?;
        let scene_snapshot = scene.snapshot().unwrap();
        let template = (*scene_snapshot).clone();
        let mesh = scene_snapshot.instances[0].mesh.clone();
        let material = scene_snapshot.instances[0].material.clone();
        let texture = material.get().unwrap().base_color_tex.clone().unwrap();
        let held_mesh = mesh.snapshot().unwrap();
        let weak_mesh = Arc::downgrade(&held_mesh.value);
        let weak_texture = Arc::downgrade(&texture.get().unwrap());
        let weak_scene = Arc::downgrade(&scene_snapshot.value);
        let weak_material = Arc::downgrade(&material.get().unwrap());
        drop(scene_snapshot);
        renderer.geometry_ready(true)?;
        renderer.update_assets()?;
        assert_eq!(renderer.scene_status(index), Some(SceneStatus::Uploading));
        assert!(renderer.scenes[index].meshes.is_empty());
        assert!(mesh.get().is_some());
        renderer.pending.as_mut().unwrap().ticket.wait()?;
        let loaded = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
        assert_ne!(loaded, empty);
        assert!(matches!(
            renderer.scene_status(index),
            Some(SceneStatus::Ready { .. })
        ));
        assert!(mesh.get().is_none());
        assert!(texture.get().is_none());
        assert_eq!(scene.get().is_none(), release_metadata);
        assert_eq!(material.get().is_none(), release_metadata);
        assert_eq!(weak_scene.upgrade().is_none(), release_metadata);
        assert_eq!(weak_material.upgrade().is_none(), release_metadata);
        assert!(weak_texture.upgrade().is_none());
        assert_eq!(held_mesh.vertices.len(), 3);
        drop(held_mesh);
        assert!(weak_mesh.upgrade().is_none());
        assert_eq!(
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
            loaded
        );

        let counts = renderer.upload_stats();
        let revision = scene.revision();
        let decodes = assets.stats().decodes;
        let duplicate = renderer.queue_scene(&scene);
        until_ready(&mut renderer, duplicate, &gpu, &descriptors, &mut cache)?;
        assert_eq!(scene.revision(), revision);
        assert_eq!(renderer.upload_stats().bytes, counts.bytes);
        assert_eq!(
            assets.stats().decodes - decodes,
            if release_metadata { 4 } else { 0 }
        );
        assert!(Arc::ptr_eq(
            &renderer.scenes[index].meshes[0].geometry,
            &renderer.scenes[duplicate].meshes[0].geometry
        ));

        let mut second = WorldRenderer::new(&gpu, &descriptors, 64, 64)?.with_asset_server(&assets);
        second.set_staging_cache_budget(0);
        let second_index = second.queue_scene(&scene);
        let decodes = assets.stats().decodes;
        until_ready(&mut second, second_index, &gpu, &descriptors, &mut cache)?;
        assert_eq!(assets.stats().decodes - decodes, 4);
        assert_eq!(scene.revision(), revision);
        assert_eq!(second.upload_stats().meshes, 1);
        assert_eq!(second.upload_stats().textures, 1);
        assert!(mesh.get().is_none());
        let sky_revision = sky.revision();
        second.queue_skybox(&sky);
        let decodes = assets.stats().decodes;
        let start = Instant::now();
        while second.skybox.as_ref().unwrap().revision != sky_revision {
            anyhow::ensure!(
                start.elapsed() < Duration::from_secs(10),
                "skybox restoration timed out"
            );
            frame(&mut second, &gpu, &descriptors, &mut cache)?;
        }
        assert_eq!(assets.stats().decodes - decodes, 1);
        assert_eq!(sky.revision(), sky_revision);
        assert!(sky.get().is_none());
        assert_eq!(
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
            loaded
        );

        let missing = assets.load::<Scene>("missing.testscene")?;
        assert!(missing.wait().is_err());
        let failed = renderer.queue_scene(&missing);
        let counts = renderer.upload_stats();
        let decodes = assets.stats().decodes;
        for _ in 0..3 {
            assert_eq!(
                frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
                loaded
            );
            assert!(matches!(
                renderer.scene_status(failed),
                Some(SceneStatus::Failed { .. })
            ));
        }
        assert_eq!(renderer.upload_stats().bytes, counts.bytes);
        assert_eq!(assets.stats().decodes, decodes);
        source.insert("missing.testscene", vec![50])?;
        renderer.retry_scene(failed)?;
        until_ready(&mut renderer, failed, &gpu, &descriptors, &mut cache)?;

        source.remove("scene.testscene")?;
        assets.reload(&scene)?;
        assert!(scene.wait().is_err());
        let counts = renderer.upload_stats();
        for _ in 0..3 {
            assert_eq!(
                frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
                loaded
            );
            assert!(matches!(
                renderer.scene_status(index),
                Some(SceneStatus::Failed { .. })
            ));
        }
        assert_eq!(renderer.upload_stats().bytes, counts.bytes);
        source.insert("scene.testscene", vec![220])?;
        assets.reload(&scene)?;
        scene.wait()?;
        until_ready(&mut renderer, index, &gpu, &descriptors, &mut cache)?;
        until_ready(&mut renderer, duplicate, &gpu, &descriptors, &mut cache)?;
        assert!(scene.revision() > revision);
        assert_eq!(renderer.upload_stats().meshes, counts.meshes);
        assert_eq!(renderer.upload_stats().textures, counts.textures + 1);

        assets.reload(&scene)?;
        scene.wait()?;
        let invalid_mesh = assets.add(Mesh::<Vertex>::new(Vec::new(), Vec::new()));
        let mut invalid_scene = template;
        invalid_scene.instances[0].mesh = invalid_mesh.clone();
        let invalid_scene = assets.add(invalid_scene);
        let invalid_index = renderer.queue_scene(&invalid_scene);
        let counts = renderer.upload_stats();
        for _ in 0..3 {
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
            assert!(matches!(
                renderer.scene_status(invalid_index),
                Some(SceneStatus::Failed { .. })
            ));
        }
        assert_eq!(renderer.upload_stats().bytes, counts.bytes);
        assert!(invalid_scene.get().is_some());
        assert!(invalid_mesh.get().is_some());
    }
    gpu.wait_idle()?;
    drop(gpu);
    anyhow::ensure!(
        instance.validation_errors().is_empty(),
        "streaming validation errors: {:?}",
        instance.validation_errors()
    );
    Ok(())
}
