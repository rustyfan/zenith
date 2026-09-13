use super::*;
use std::time::{Duration, Instant};
use zenith::rendergraph::ResourceCache;
use zenith::rhi::{vk, Instance, TextureDesc};

fn frame(
    app: &mut WorldApp,
    gpu: &Arc<Gpu>,
    descriptors: &Arc<Descriptors>,
    cache: &mut ResourceCache,
) -> anyhow::Result<()> {
    let mut builder = RenderGraphBuilder::new(gpu, descriptors, cache)?;
    let output = builder.create_image(TextureDesc::color(64, 64, vk::Format::R8G8B8A8_UNORM))?;
    app.render(
        &mut builder,
        RenderContext {
            output,
            extent: vk::Extent2D {
                width: 64,
                height: 64,
            },
            frame_index: 0,
        },
    )?;
    builder.record()?.submit()?.wait(10_000_000_000)
}

#[test]
#[ignore = "requires Vulkan validation, Slang, and the included Cerberus assets"]
fn scene_toggle_preserves_cameras_and_uploaded_scenes() -> anyhow::Result<()> {
    std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
    let _ = log::initialize(log::LevelFilter::Info);
    let instance = Instance::new(&[], true)?;
    anyhow::ensure!(
        instance.validation_enabled(),
        "scene toggle test requires Vulkan validation"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    {
        let descriptors = Descriptors::new(&gpu, 512, 32)?;
        let mut app = WorldApp::new(&Args {
            log_level: Default::default(),
            args: Vec::new(),
        })?;
        let scene = app
            .assets
            .load_blocking::<Scene>("mesh/cerberus/scene.gltf")?;
        let mesh = scene.get().unwrap().instances[0].mesh.clone();
        let mut renderer =
            WorldRenderer::new(&gpu, &descriptors, 64, 64)?.with_asset_server(&app.assets);
        let index = renderer.queue_scene(&scene);
        app.cerberus_index = Some(index);
        app.model_index = Some(index);
        app.model_to_frame = Some(scene);
        app.world_renderer = Some(renderer);
        app.alternate_camera
            .set_position(Vec3::new(0.0, -14.0, 0.0));
        app.controller.update_cameras(
            0.0,
            0.0,
            0.0,
            0.0,
            std::iter::once(&mut app.alternate_camera),
        );
        app.toggle_scene()?;
        let sphere_index = app.sphere_index.unwrap();
        let sphere_position = app.camera.position();
        assert!(app.showing_spheres);
        let mut cache = ResourceCache::default();
        let start = Instant::now();
        loop {
            frame(&mut app, &gpu, &descriptors, &mut cache)?;
            assert_eq!(app.camera.position(), sphere_position);
            let renderer = app.world_renderer.as_ref().unwrap();
            if [index, sphere_index]
                .iter()
                .all(|&i| matches!(renderer.scene_status(i), Some(SceneStatus::Ready { .. })))
            {
                break;
            }
            anyhow::ensure!(
                start.elapsed() < Duration::from_secs(10),
                "scene toggle streaming timed out"
            );
        }
        assert!(app.model_to_frame.is_none());
        assert!(mesh.get().is_none());
        let cerberus_position = app.alternate_camera.position();
        let bytes = app.world_renderer.as_ref().unwrap().upload_stats().bytes;
        for _ in 0..3 {
            app.toggle_scene()?;
            assert!(!app.showing_spheres);
            assert_eq!(app.camera.position(), cerberus_position);
            frame(&mut app, &gpu, &descriptors, &mut cache)?;
            app.toggle_scene()?;
            assert!(app.showing_spheres);
            assert_eq!(app.camera.position(), sphere_position);
            assert_eq!(app.sphere_index, Some(sphere_index));
            frame(&mut app, &gpu, &descriptors, &mut cache)?;
            assert_eq!(
                app.world_renderer.as_ref().unwrap().upload_stats().bytes,
                bytes
            );
        }
    }
    gpu.wait_idle()?;
    drop(gpu);
    anyhow::ensure!(
        instance.validation_errors().is_empty(),
        "scene toggle validation: {:?}",
        instance.validation_errors()
    );
    Ok(())
}
