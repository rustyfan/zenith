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
            start.elapsed() < Duration::from_secs(60),
            "scene streaming timed out"
        );
    }
}

#[test]
#[ignore = "requires Vulkan validation, Slang, and the included render assets"]
fn living_room_default_streams_and_renders() -> anyhow::Result<()> {
    use std::io::Write;
    use zenith::rhi::{Access, MemoryDomain};

    std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
    let _ = log::initialize(log::LevelFilter::Info);
    let instance = Instance::new(&[], true)?;
    anyhow::ensure!(
        instance.validation_enabled(),
        "Vulkan validation is required"
    );
    let gpu = Gpu::new(instance.clone(), None)?;
    {
        let descriptors = Descriptors::new(&gpu, 2048, 32)?;
        let mut app = WorldApp::new(&Args {
            log_level: Default::default(),
            graphics_mode: None,
            args: Vec::new(),
        })?;
        assert_eq!(app.scene, ExampleScene::LivingRoom2);
        app.cameras[app.scene as usize] = living_room_camera(16.0 / 9.0);
        app.skybox.wait()?;
        let mut renderer =
            WorldRenderer::new(&gpu, &descriptors, 960, 540)?.with_asset_server(&app.assets);
        renderer.set_skybox(&gpu, &descriptors, &app.skybox)?;
        app.world_renderer = Some(renderer);
        let mut cache = ResourceCache::default();
        frame(&mut app, &gpu, &descriptors, &mut cache)?;
        app.tick(0.0);
        let index = app.scene_indices[ExampleScene::LivingRoom2 as usize].unwrap();
        ready_frames(&mut app, &gpu, &descriptors, &mut cache, &[index])?;
        assert!(app.scene_indices[ExampleScene::Cerberus as usize].is_none());
        let readback = gpu.allocate(960 * 540 * 4, MemoryDomain::Readback)?;
        let mut builder = RenderGraphBuilder::new(&gpu, &descriptors, &mut cache)?;
        let mut desc = TextureDesc::color(960, 540, vk::Format::R8G8B8A8_UNORM);
        desc.usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC;
        let output = builder.create_image(desc)?;
        app.render(
            &mut builder,
            RenderContext {
                output,
                extent: vk::Extent2D {
                    width: 960,
                    height: 540,
                },
                frame_index: 1,
            },
        )?;
        let destination = builder.import_buffer(readback.clone());
        builder.pass(
            "readback",
            vec![
                output.read(Access::COPY_READ),
                destination.write(Access::COPY_WRITE),
            ],
            move |ctx| {
                let region = vk::BufferImageCopy::default()
                    .image_extent(vk::Extent3D {
                        width: 960,
                        height: 540,
                        depth: 1,
                    })
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    );
                unsafe {
                    ctx.commands.read_image(
                        &ctx.image(output)?,
                        &ctx.buffer(destination)?,
                        &[region],
                    )
                }
            },
        )?;
        builder.pass(
            "host",
            vec![destination.read(Access::HOST_READ)],
            |_| Ok(()),
        )?;
        builder.record()?.submit()?.wait(10_000_000_000)?;
        let mut pixels = vec![0; 960 * 540 * 4];
        readback.read(0, &mut pixels)?;
        assert!(pixels
            .chunks_exact(4)
            .any(|p| p[0] > 32 && p[1] > 32 && p[2] > 32));
        let mut header = [0u8; 54];
        header[..2].copy_from_slice(b"BM");
        header[2..6].copy_from_slice(&(54 + pixels.len() as u32).to_le_bytes());
        header[10..14].copy_from_slice(&54u32.to_le_bytes());
        header[14..18].copy_from_slice(&40u32.to_le_bytes());
        header[18..22].copy_from_slice(&960i32.to_le_bytes());
        header[22..26].copy_from_slice(&(-540i32).to_le_bytes());
        header[26..28].copy_from_slice(&1u16.to_le_bytes());
        header[28..30].copy_from_slice(&32u16.to_le_bytes());
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        std::fs::create_dir_all("target/living-room-integration")?;
        let mut file = std::fs::File::create("target/living-room-integration/living-room-2.bmp")?;
        file.write_all(&header)?;
        file.write_all(&pixels)?;
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

#[test]
#[ignore = "requires Vulkan validation, Slang, and the included render assets"]
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
        let descriptors = Descriptors::new(&gpu, 2048, 32)?;
        let mut app = WorldApp::new(&Args {
            log_level: Default::default(),
            graphics_mode: None,
            args: Vec::new(),
        })?;
        let scene = app
            .assets
            .load_blocking::<Scene>("mesh/cerberus/scene.gltf")?;
        assert_eq!(app.scene, ExampleScene::LivingRoom2);
        app.scene = ExampleScene::Cerberus;
        app.cameras[ExampleScene::LivingRoom2 as usize] = living_room_camera(1.0);
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
        let positions: [Vec3; 5] = std::array::from_fn(|i| app.cameras[i].position());
        for scene in [
            ExampleScene::WhiteFurnace,
            ExampleScene::ColoredRoom,
            ExampleScene::LivingRoom2,
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
                        ExampleScene::Cerberus | ExampleScene::LivingRoom2 => assert!(count > 0),
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
