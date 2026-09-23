#[path = "support/pbr_scene.rs"]
mod pbr_scene;

#[path = "support/pbr_benchmark.rs"]
mod pbr_benchmark;

use anyhow::Result;
use glam::{Mat4, Vec3};
use std::io::Write;
use zenith::{
    asset::{mesh::Scene, texture::Texture as CpuTexture, AssetServer, FileSource},
    core::{
        camera::{Camera, CameraController, NEAR_PLANE},
        log,
        math::Degree,
    },
    renderer::{DebugMode, WorldRenderer},
    rendergraph::{RenderGraphBuilder, ResourceCache},
    rhi::{vk, Access, Descriptors, Gpu, Instance, MemoryDomain, TextureDesc},
};

fn save_bmp(path: &str, width: u32, height: u32, mut pixels: Vec<u8>) -> Result<()> {
    let size = 54 + pixels.len() as u32;
    let mut header = vec![0u8; 54];
    header[..2].copy_from_slice(b"BM");
    header[2..6].copy_from_slice(&size.to_le_bytes());
    header[10..14].copy_from_slice(&54u32.to_le_bytes());
    header[14..18].copy_from_slice(&40u32.to_le_bytes());
    header[18..22].copy_from_slice(&width.to_le_bytes());
    header[22..26].copy_from_slice(&(-(height as i32)).to_le_bytes());
    header[26..28].copy_from_slice(&1u16.to_le_bytes());
    header[28..30].copy_from_slice(&32u16.to_le_bytes());
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let mut file = std::fs::File::create(path)?;
    file.write_all(&header)?;
    file.write_all(&pixels)?;
    Ok(())
}

fn main() -> Result<()> {
    log::initialize(log::LevelFilter::Info)?;
    let cerberus = std::env::args().any(|arg| arg == "cerberus");
    let closeup = std::env::args().any(|arg| arg == "closeup");
    let benchmark = std::env::args().any(|arg| arg == "benchmark");
    let furnace = std::env::args().any(|arg| arg == "furnace");
    let directory = std::path::PathBuf::from(
        std::env::var_os("ZENITH_PREVIEW_DIR").unwrap_or_else(|| "target/pbr-preview".into()),
    );
    let model = match (cerberus, closeup) {
        (true, true) => "cerberus-closeup",
        (true, false) => "cerberus",
        (false, true) => "sphere-closeup",
        (false, false) => "spheres",
    };
    let (width, height) = (3840, 2160);
    let instance = Instance::new(&[], true)?;
    anyhow::ensure!(
        instance.validation_enabled(),
        "preview requires Vulkan validation"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    {
        let descriptors = Descriptors::new(&gpu, 4096, 64)?;
        let assets = AssetServer::builder()
            .source(FileSource::new(
                std::env::var_os("ZENITH_CONTENT").unwrap_or_else(|| "content".into()),
            ))
            .cache_dir(std::env::var_os("ZENITH_ASSET_CACHE").unwrap_or_else(|| "asset".into()))
            .with_builtin_assets()
            .build()?;
        let sky = assets.load_blocking::<CpuTexture>("texture/minedump_flats_4k.hdr")?;
        let mut scene = if cerberus {
            assets.load_blocking::<Scene>("mesh/cerberus/scene.gltf")?
        } else {
            pbr_scene::spheres(&assets)
        };
        if cerberus && !closeup {
            let mut oriented = (*scene.get().unwrap()).clone();
            let mut minimum = Vec3::splat(f32::INFINITY);
            let mut maximum = Vec3::splat(f32::NEG_INFINITY);
            for instance in &oriented.instances {
                let transform = Mat4::from_cols_array(&instance.transform);
                for vertex in &instance.mesh.get().unwrap().vertices {
                    let p = transform.transform_point3(Vec3::from_array(vertex.position));
                    minimum = minimum.min(p);
                    maximum = maximum.max(p);
                }
            }
            let center = (minimum + maximum) * 0.5;
            let rotation = Mat4::from_translation(center)
                * Mat4::from_rotation_z(1.2)
                * Mat4::from_rotation_x(0.15)
                * Mat4::from_translation(-center);
            for instance in &mut oriented.instances {
                instance.transform =
                    (rotation * Mat4::from_cols_array(&instance.transform)).to_cols_array();
            }
            scene = assets.add(oriented);
        }
        let mut camera = Camera::new(Degree::from(60.0), width as f32 / height as f32, NEAR_PLANE);
        let mut position = Vec3::new(0.0, -14.0, 0.0);
        if closeup {
            position = Vec3::new(-5.25, -1.9, -2.1);
        }
        if cerberus && !closeup {
            let scene = scene.get().unwrap();
            let mut minimum = Vec3::splat(f32::INFINITY);
            let mut maximum = Vec3::splat(f32::NEG_INFINITY);
            for instance in &scene.instances {
                let transform = Mat4::from_cols_array(&instance.transform);
                for vertex in &instance.mesh.get().unwrap().vertices {
                    let p = transform.transform_point3(Vec3::from_array(vertex.position));
                    minimum = minimum.min(p);
                    maximum = maximum.max(p);
                }
            }
            position =
                (minimum + maximum) * 0.5 - Vec3::Y * (maximum - minimum).length().max(1.0) * 1.1;
        }
        if cerberus && closeup {
            camera.set_fov(Degree::from(90.0));
            position = Vec3::new(-6.0, 45.0, 12.0);
        }
        camera.set_position(position);
        CameraController::default().update_cameras(
            0.0,
            0.0,
            0.0,
            0.0,
            std::iter::once(&mut camera),
        );
        let mut renderer = WorldRenderer::new(&gpu, &descriptors, width, height)?;
        renderer.set_skybox(&gpu, &descriptors, &sky)?;
        renderer.add_scene(&gpu, &descriptors, &scene)?;
        let mut cache = ResourceCache::default();
        std::fs::create_dir_all(&directory)?;
        let modes = if furnace {
            vec![
                (
                    "furnace-on",
                    0.0,
                    1.0,
                    false,
                    false,
                    DebugMode::WHITE_FURNACE,
                    true,
                ),
                (
                    "furnace-off",
                    0.0,
                    1.0,
                    false,
                    false,
                    DebugMode::WHITE_FURNACE,
                    false,
                ),
            ]
        } else {
            vec![
                ("combined", 1.0, 1.0, true, false, DebugMode::empty(), true),
                (
                    "combined-ao",
                    1.0,
                    1.0,
                    true,
                    true,
                    DebugMode::empty(),
                    true,
                ),
                (
                    "directional",
                    1.0,
                    0.0,
                    true,
                    false,
                    DebugMode::empty(),
                    true,
                ),
                (
                    "unshadowed",
                    1.0,
                    0.0,
                    false,
                    false,
                    DebugMode::empty(),
                    true,
                ),
                ("ibl", 0.0, 1.0, true, false, DebugMode::empty(), true),
                ("ibl-ao", 0.0, 1.0, true, true, DebugMode::empty(), true),
                (
                    "ao",
                    0.0,
                    0.0,
                    false,
                    true,
                    DebugMode::AMBIENT_OCCLUSION,
                    true,
                ),
            ]
        };
        for (label, direct, sky, shadows, ao, debug, multiple_scattering) in modes {
            let mut settings = renderer.lighting_settings();
            settings.directional.intensity = direct;
            settings.sky_intensity = sky;
            settings.shadows.enabled = shadows;
            settings.ambient_occlusion.enabled = ao;
            settings.multiple_scattering = multiple_scattering;
            renderer.set_lighting(settings)?;
            renderer.set_debug_mode(debug);
            if benchmark {
                pbr_benchmark::measure(
                    &mut renderer,
                    &gpu,
                    &descriptors,
                    &camera,
                    [width, height],
                    &directory.join(format!("{model}-{label}.csv")),
                )?;
            }
            let readback = gpu.allocate(u64::from(width * height * 4), MemoryDomain::Readback)?;
            let mut builder = RenderGraphBuilder::new(&gpu, &descriptors, &mut cache)?;
            let mut desc = TextureDesc::color(width, height, vk::Format::R8G8B8A8_UNORM);
            desc.usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC;
            let output = builder.create_image(desc)?;
            renderer.render(&mut builder, &camera, output)?;
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
            let (commands, timings) = builder.record_profiled(true)?;
            commands.submit()?.wait(10_000_000_000)?;
            for (pass, milliseconds) in timings.unwrap().read()? {
                log::info!("{label}: {pass} {milliseconds:.3} ms");
            }
            let mut pixels = vec![0; (width * height * 4) as usize];
            readback.read(0, &mut pixels)?;
            let path = directory.join(format!("{model}-{label}.bmp"));
            save_bmp(&path.to_string_lossy(), width, height, pixels)?;
            log::info!("Saved {}", path.display());
        }
    }
    gpu.wait_idle()?;
    drop(gpu);
    anyhow::ensure!(
        instance.validation_errors().is_empty(),
        "preview validation errors: {:?}",
        instance.validation_errors()
    );
    Ok(())
}
