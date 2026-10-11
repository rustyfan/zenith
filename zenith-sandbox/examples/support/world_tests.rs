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

fn ready_frames(
    app: &mut WorldApp,
    gpu: &Arc<Gpu>,
    descriptors: &Arc<Descriptors>,
    cache: &mut ResourceCache,
    indices: &[usize],
) -> anyhow::Result<()> {
    let start = Instant::now();
    loop {
        frame(app, gpu, descriptors, cache)?;
        let renderer = app.world_renderer.as_ref().unwrap();
        if indices
            .iter()
            .all(|&i| matches!(renderer.scene_status(i), Some(SceneStatus::Ready { .. })))
        {
            return Ok(());
        }
        anyhow::ensure!(
            start.elapsed() < Duration::from_secs(10),
            "scene streaming timed out"
        );
    }
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
            graphics_mode: None,
            args: Vec::new(),
        })?;
        let scene = app
            .assets
            .load_blocking::<Scene>("mesh/cerberus/scene.gltf")?;
        let mesh = scene.get().unwrap().instances[0].mesh.clone();
        let mut renderer =
            WorldRenderer::new(&gpu, &descriptors, 64, 64)?.with_asset_server(&app.assets);
        let index = renderer.queue_scene(&scene);
        app.scene_indices[ExampleScene::Cerberus as usize] = Some(index);
        app.model_index = Some(index);
        app.model_to_frame = Some(scene);
        app.world_renderer = Some(renderer);
        for scene in [ExampleScene::Spheres, ExampleScene::WhiteFurnace] {
            app.cameras[scene as usize] = Camera::new(Degree::new(60.0), 1.0, NEAR_PLANE);
            app.cameras[scene as usize].set_position(Vec3::new(0.0, -14.0, 0.0));
        }
        app.cameras[ExampleScene::ColoredRoom as usize].set_position(Vec3::new(0.0, -8.0, 1.2));
        app.controller
            .update_cameras(0.0, 0.0, 0.0, 0.0, app.cameras.iter_mut());
        app.toggle_scene()?;
        let sphere_index = app.scene_indices[ExampleScene::Spheres as usize].unwrap();
        let sphere_position = app.cameras[ExampleScene::Spheres as usize].position();
        assert_eq!(app.scene, ExampleScene::Spheres);
        let mut cache = ResourceCache::default();
        ready_frames(
            &mut app,
            &gpu,
            &descriptors,
            &mut cache,
            &[index, sphere_index],
        )?;
        assert_eq!(
            app.cameras[ExampleScene::Spheres as usize].position(),
            sphere_position
        );
        assert!(app.model_to_frame.is_none());
        assert!(mesh.get().is_none());
        let positions: [Vec3; 4] = std::array::from_fn(|i| app.cameras[i].position());
        for scene in [
            ExampleScene::WhiteFurnace,
            ExampleScene::ColoredRoom,
            ExampleScene::Cerberus,
        ] {
            app.select_scene(scene)?;
            let index = app.scene_indices[scene as usize].unwrap();
            ready_frames(&mut app, &gpu, &descriptors, &mut cache, &[index])?;
        }
        let indices = app.scene_indices;
        let meshes = app.world_renderer.as_ref().unwrap().upload_stats().meshes;
        let supported = app
            .world_renderer
            .as_ref()
            .unwrap()
            .path_tracing_supported();
        for mode in [RenderMode::Raster, RenderMode::PathTracing] {
            if mode == RenderMode::PathTracing && !supported {
                continue;
            }
            app.world_renderer.as_mut().unwrap().set_render_mode(mode)?;
            for expected in ExampleScene::ALL {
                app.toggle_scene()?;
                let scene = app.scene;
                assert_eq!(
                    scene as usize,
                    (expected as usize + 1) % ExampleScene::ALL.len()
                );
                assert_eq!(
                    app.cameras[scene as usize].position(),
                    positions[scene as usize]
                );
                assert_eq!(app.scene_indices, indices);
                assert!(app.validation_skybox.get().is_some());
                assert!(app.white_skybox.get().is_some());
                assert_eq!(app.world_renderer.as_ref().unwrap().path_tracing_spp(), 0);
                frame(&mut app, &gpu, &descriptors, &mut cache)?;
                let renderer = app.world_renderer.as_mut().unwrap();
                assert_eq!(renderer.upload_stats().meshes, meshes);
                if mode == RenderMode::PathTracing {
                    assert!(renderer.path_tracing_spp() > 0);
                    let count = renderer.prepare_path_tracing_scene()?.instance_count();
                    match scene {
                        ExampleScene::Spheres => assert_eq!(count, 18),
                        ExampleScene::WhiteFurnace => assert_eq!(count, 12),
                        ExampleScene::ColoredRoom => assert_eq!(count, 6),
                        ExampleScene::Cerberus => assert!(count > 0),
                    }
                }
                let lighting = renderer.lighting_settings();
                if scene == ExampleScene::WhiteFurnace {
                    assert_eq!(lighting.directional.intensity, 0.0);
                    assert_eq!(lighting.sky_intensity, 1.0);
                    assert!(lighting.multiple_scattering);
                }
                if scene == ExampleScene::ColoredRoom {
                    assert_eq!(lighting.directional.intensity, 2.0);
                    assert_eq!(lighting.sky_intensity, 0.5);
                }
            }
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
