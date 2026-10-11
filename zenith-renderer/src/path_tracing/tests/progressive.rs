use super::integration::{add_plane, frame, plane};
use super::*;
use crate::{PathTracingSettings, RenderMode};
use std::time::{Duration, Instant};
use zenith_asset::{
    ImportContext, Importer, LoadState,
    mesh::{MeshInstanceData, SceneData},
};
use zenith_core::{camera::Camera, math::Degree};
use zenith_rendergraph::ResourceCache;

struct Receiver;
impl Importer for Receiver {
    type Settings = ();
    type Output = Scene;
    const KEY: &'static str = "test.path-tracing-receiver";
    const VERSION: u32 = 1;
    fn extensions(&self) -> &[&str] {
        &["ptscene"]
    }
    fn import(
        &self,
        bytes: &[u8],
        _: &(),
        ctx: &mut ImportContext<'_>,
    ) -> zenith_asset::Result<SceneData> {
        Ok(SceneData {
            nodes: scene(Vec::new()).nodes,
            instances: vec![MeshInstanceData {
                node: 0,
                mesh: ctx.emit::<Mesh>("mesh", plane(2.0, 0.0, 4.0, 4.0, -Vec3::Y))?,
                material: ctx.emit::<Material>(
                    "material",
                    zenith_asset::material::MaterialData {
                        base_color: [bytes[0] as f32 / 255.0, 0.4, 0.2, 1.0],
                        metallic: 0.0,
                        roughness: 0.7,
                        ..Default::default()
                    },
                )?,
                transform: Mat4::IDENTITY.to_cols_array(),
            }],
        })
    }
}

#[test]
fn progressive_settings_reject_invalid_values() {
    for settings in [
        PathTracingSettings {
            target_spp: 0,
            ..Default::default()
        },
        PathTracingSettings {
            samples_per_frame: 0,
            ..Default::default()
        },
        PathTracingSettings {
            max_bounces: 0,
            ..Default::default()
        },
        PathTracingSettings {
            max_bounces: 9,
            ..Default::default()
        },
    ] {
        assert!(settings.validate().is_err());
    }
    assert!(PathTracingSettings::default().validate().is_ok());
}

fn progressive_frame(
    renderer: &mut WorldRenderer,
    gpu: &Arc<Gpu>,
    descriptors: &Arc<Descriptors>,
    cache: &mut ResourceCache,
    camera: &Camera,
    [width, height]: [u32; 2],
) -> Result<(Vec<[f32; 4]>, Arc<Texture>, Vec<String>)> {
    let readback = gpu.allocate(u64::from(width * height) * 16, MemoryDomain::Readback)?;
    let mut builder = RenderGraphBuilder::new(gpu, descriptors, cache)?;
    let output = renderer.render_hdr(&mut builder, camera, width, height)?;
    let texture = builder.export_image(output)?;
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
                    width,
                    height,
                    depth: 1,
                })
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                );
            unsafe {
                ctx.commands
                    .read_image(&ctx.image(output)?, &ctx.buffer(destination)?, &[region])
            }
        },
    )?;
    builder.pass(
        "host",
        vec![destination.read(Access::HOST_READ)],
        |_| Ok(()),
    )?;
    let (commands, timings) = builder.record_profiled(true)?;
    commands.submit()?.wait(10_000_000_000)?;
    let mut pixels = vec![[0.0f32; 4]; (width * height) as usize];
    readback.read(0, bytemuck::cast_slice_mut(&mut pixels))?;
    assert!(
        pixels
            .iter()
            .all(|p| p.iter().all(|v| v.is_finite()) && p[3] == 1.0)
    );
    Ok((
        pixels,
        texture,
        timings
            .unwrap()
            .read()?
            .into_iter()
            .map(|(name, _)| name)
            .collect(),
    ))
}

fn compare(actual: &[[f32; 4]], expected: &[[f32; 4]]) {
    for (actual, expected) in actual.iter().zip(expected) {
        for c in 0..4 {
            close(actual[c], expected[c]);
        }
    }
}

#[test]
#[ignore = "requires Vulkan validation, Slang, descriptor heaps, and ray tracing pipeline support"]
fn progressive_frames_cap_resume_reset_and_survive_discarded_graphs() -> Result<()> {
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
        let source = Arc::new(MemorySource::default());
        source.insert("receiver.ptscene", vec![180])?;
        let assets = AssetServer::builder()
            .source(source.clone())
            .with_builtin_assets()
            .register_importer(Receiver)
            .build()?;
        let mut renderer = WorldRenderer::new(&gpu, &descriptors, 8, 8)?.with_asset_server(&assets);
        assert_eq!(renderer.render_mode(), RenderMode::Raster);
        assert_eq!(
            renderer.path_tracing_settings(),
            PathTracingSettings::default()
        );
        let sky = |color: [f32; 4]| {
            assets.add(CpuTexture {
                width: 1,
                height: 1,
                mip_levels: 1,
                is_cubemap: true,
                format: TextureFormat::Rgba32Float,
                pixels: bytemuck::cast_slice(&color.repeat(6)).to_vec(),
            })
        };
        renderer.set_skybox(&gpu, &descriptors, &sky([2.0, 1.0, 0.5, 1.0]))?;
        add_plane(
            &mut renderer,
            &gpu,
            &descriptors,
            &assets,
            plane(2.0, 0.0, 4.0, 4.0, -Vec3::Y),
            Material {
                base_color: [0.8, 0.4, 0.2, 1.0],
                metallic: 1.0,
                roughness: 0.7,
                ..Default::default()
            },
        )?;
        let mut camera = Camera::new(Degree::new(0.001), 1.0, 0.1);
        let mut reference_cache = ResourceCache::default();
        let batch = PathTracingBatch {
            sample_count: 5,
            max_bounces: 1,
            ..Default::default()
        };
        let reference = frame(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut reference_cache,
            &camera,
            batch,
        )?;
        renderer.set_render_mode(RenderMode::PathTracing)?;
        let mut settings = PathTracingSettings {
            target_spp: 5,
            samples_per_frame: 2,
            max_bounces: 1,
        };
        renderer.set_path_tracing(settings)?;
        let mut cache = ResourceCache::default();
        for record in [false, true] {
            let mut graph = RenderGraphBuilder::new(&gpu, &descriptors, &mut cache)?;
            renderer.render_hdr(&mut graph, &camera, 8, 8)?;
            assert_eq!(renderer.path_tracing_spp(), 0);
            let mut other_cache = ResourceCache::default();
            let mut other = RenderGraphBuilder::new(&gpu, &descriptors, &mut other_cache)?;
            assert!(renderer.render_hdr(&mut other, &camera, 8, 8).is_err());
            if record {
                drop(graph.record()?);
            } else {
                drop(graph);
            }
            assert_eq!(renderer.path_tracing_spp(), 0);
        }
        let mut graph = RenderGraphBuilder::new(&gpu, &descriptors, &mut cache)?;
        renderer.render_hdr(&mut graph, &camera, 8, 8)?;
        graph.pass("abort", Vec::new(), |_| {
            anyhow::bail!("discard this recording")
        })?;
        assert!(graph.record().is_err());
        assert_eq!(renderer.path_tracing_spp(), 0);

        let mut render = |renderer: &mut WorldRenderer, camera: &Camera, extent| {
            progressive_frame(renderer, &gpu, &descriptors, &mut cache, camera, extent)
        };
        let (_, texture, _) = render(&mut renderer, &camera, [8, 8])?;
        assert_eq!(renderer.path_tracing_spp(), 2);
        let (_, second, _) = render(&mut renderer, &camera, [8, 8])?;
        assert!(Arc::ptr_eq(&texture, &second));
        assert_eq!(renderer.path_tracing_spp(), 4);
        let (pixels, _, _) = render(&mut renderer, &camera, [8, 8])?;
        assert_eq!(renderer.path_tracing_spp(), 5);
        compare(&pixels, &reference);
        let (frozen, _, passes) = render(&mut renderer, &camera, [8, 8])?;
        assert_eq!(pixels, frozen);
        assert!(!passes.iter().any(|name| name == "path_tracing"));

        let directory = std::path::Path::new("target/shader-reload-progressive")
            .join(std::process::id().to_string());
        std::fs::create_dir_all(&directory)?;
        let trigger_path = directory.join("trigger.slang");
        let trigger_source = "[shader(\"compute\")][numthreads(1,1,1)] void main() {}";
        std::fs::write(&trigger_path, trigger_source)?;
        let _trigger = gpu.compile_shader(&trigger_path, "main", ShaderStage::Compute)?;
        let generation = gpu.shader_reload_generation();
        std::fs::write(&trigger_path, format!("{trigger_source}\n"))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while gpu.shader_reload_generation() == generation {
            drop(gpu.shader_reload_scope());
            anyhow::ensure!(Instant::now() < deadline, "shader reload timed out");
            std::thread::sleep(Duration::from_millis(25));
        }
        render(&mut renderer, &camera, [8, 8])?;
        assert_eq!(renderer.path_tracing_spp(), 2);
        render(&mut renderer, &camera, [8, 8])?;
        let (reloaded, _, _) = render(&mut renderer, &camera, [8, 8])?;
        assert_eq!(renderer.path_tracing_spp(), 5);
        compare(&reloaded, &reference);

        renderer.reset_path_tracing();
        let mut queued_cache = ResourceCache::default();
        let mut queued = Vec::new();
        for _ in 0..2 {
            let mut graph = RenderGraphBuilder::new(&gpu, &descriptors, &mut queued_cache)?;
            renderer.render_hdr(&mut graph, &camera, 8, 8)?;
            queued.push(graph.record()?.submit()?);
        }
        let (queued_pixels, texture, _) = render(&mut renderer, &camera, [8, 8])?;
        compare(&queued_pixels, &reference);
        assert_eq!(renderer.path_tracing_spp(), 5);
        for submission in &mut queued {
            assert!(submission.poll()?);
        }

        settings.target_spp = 8;
        settings.samples_per_frame = 3;
        renderer.set_path_tracing(settings)?;
        let expected = frame(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut reference_cache,
            &camera,
            PathTracingBatch {
                sample_count: 8,
                ..batch
            },
        )?;
        compare(&render(&mut renderer, &camera, [8, 8])?.0, &expected);
        assert_eq!(renderer.path_tracing_spp(), 8);
        settings.target_spp = 4;
        settings.samples_per_frame = 1;
        renderer.set_path_tracing(settings)?;
        assert!(
            !render(&mut renderer, &camera, [8, 8])?
                .2
                .iter()
                .any(|name| name == "path_tracing")
        );
        assert_eq!(renderer.path_tracing_spp(), 8);
        settings.target_spp = 100;
        renderer.set_path_tracing(settings)?;
        let mut post = renderer.post_processing_settings();
        post.exposure = 0.25;
        renderer.set_post_processing(post)?;
        render(&mut renderer, &camera, [8, 8])?;
        assert_eq!(renderer.path_tracing_spp(), 9);

        camera.set_fov(Degree::new(0.002));
        let (_, changed_camera, _) = render(&mut renderer, &camera, [8, 8])?;
        assert!(Arc::ptr_eq(&texture, &changed_camera));
        assert_eq!(renderer.path_tracing_spp(), 1);
        render(&mut renderer, &camera, [8, 8])?;
        camera.set_position(Vec3::new(0.1, 0.0, 0.0));
        render(&mut renderer, &camera, [8, 8])?;
        assert_eq!(renderer.path_tracing_spp(), 1);
        render(&mut renderer, &camera, [8, 8])?;
        let (_, resized, _) = render(&mut renderer, &camera, [7, 5])?;
        assert!(!Arc::ptr_eq(&texture, &resized));
        assert_eq!(renderer.path_tracing_spp(), 1);
        let mut lighting = renderer.lighting_settings();
        for change in 0..6 {
            render(&mut renderer, &camera, [7, 5])?;
            match change {
                0 => lighting.directional.intensity = 2.0,
                1 => lighting.directional.color = Vec3::new(0.3, 0.5, 0.8),
                2 => lighting.directional.direction_to_light = Vec3::new(0.2, -1.0, 0.1),
                3 => lighting.sky_intensity = 0.5,
                4 => lighting.multiple_scattering = false,
                _ => lighting.shadows.bias = 0.002,
            }
            renderer.set_lighting(lighting)?;
            let (_, image, _) = render(&mut renderer, &camera, [7, 5])?;
            assert!(Arc::ptr_eq(&resized, &image));
            assert_eq!(renderer.path_tracing_spp(), 1, "change {change}");
        }
        lighting.shadows.enabled = false;
        lighting.ambient_occlusion.enabled = false;
        renderer.set_lighting(lighting)?;
        renderer.set_debug_mode(crate::DebugMode::WHITE_FURNACE);
        render(&mut renderer, &camera, [7, 5])?;
        assert_eq!(renderer.path_tracing_spp(), 2);
        settings.max_bounces = 2;
        renderer.set_path_tracing(settings)?;
        assert_eq!(renderer.path_tracing_spp(), 0);
        render(&mut renderer, &camera, [7, 5])?;
        assert_eq!(renderer.path_tracing_spp(), 1);
        render(&mut renderer, &camera, [7, 5])?;
        renderer.set_skybox(&gpu, &descriptors, &sky([0.25, 1.0, 2.0, 1.0]))?;
        render(&mut renderer, &camera, [7, 5])?;
        assert_eq!(renderer.path_tracing_spp(), 1);
        render(&mut renderer, &camera, [7, 5])?;
        renderer.set_scene_visible(0, false)?;
        render(&mut renderer, &camera, [7, 5])?;
        assert_eq!(renderer.path_tracing_spp(), 1);
        render(&mut renderer, &camera, [7, 5])?;
        renderer.set_scene_visible(0, true)?;
        render(&mut renderer, &camera, [7, 5])?;
        assert_eq!(renderer.path_tracing_spp(), 1);
        renderer.set_scene_visible(0, false)?;
        let streamed = assets.load_blocking::<Scene>("receiver.ptscene")?;
        renderer.add_scene(&gpu, &descriptors, &streamed)?;
        render(&mut renderer, &camera, [7, 5])?;
        render(&mut renderer, &camera, [7, 5])?;
        assert_eq!(renderer.path_tracing_spp(), 2);
        source.insert("receiver.ptscene", vec![60])?;
        assets.reload(&streamed)?;
        streamed.wait()?;
        let start = Instant::now();
        loop {
            render(&mut renderer, &camera, [7, 5])?;
            if matches!(renderer.scene_status(1), Some(crate::SceneStatus::Ready { revision }) if Some(revision) == streamed.revision())
            {
                assert_eq!(renderer.path_tracing_spp(), 1);
                break;
            }
            ensure!(
                start.elapsed() < Duration::from_secs(10),
                "scene reload timed out: {:?}",
                streamed.state()
            );
        }
        assert!(!matches!(streamed.state(), LoadState::Loading));
        renderer.set_render_mode(RenderMode::Raster)?;
        assert_eq!(renderer.path_tracing_spp(), 0);
        renderer.set_render_mode(RenderMode::PathTracing)?;
        render(&mut renderer, &camera, [7, 5])?;
        assert_eq!(renderer.path_tracing_spp(), 1);
        renderer.reset_path_tracing();
        assert_eq!(renderer.path_tracing_spp(), 0);
        render(&mut renderer, &camera, [7, 5])?;
        assert_eq!(renderer.path_tracing_spp(), 1);
        drop(render);

        renderer.reset_path_tracing();
        let mut submissions = Vec::new();
        let mut weak = Weak::new();
        for _ in 0..3 {
            let mut graph = RenderGraphBuilder::new(&gpu, &descriptors, &mut cache)?;
            let output = renderer.render_hdr(&mut graph, &camera, 7, 5)?;
            weak = Arc::downgrade(&graph.export_image(output)?);
            submissions.push(graph.record()?.submit()?);
        }
        assert_eq!(renderer.path_tracing_spp(), 3);
        renderer.reset_path_tracing();
        assert!(weak.upgrade().is_some());
        for submission in &mut submissions {
            submission.wait(10_000_000_000)?;
        }
        assert!(weak.upgrade().is_none());
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
