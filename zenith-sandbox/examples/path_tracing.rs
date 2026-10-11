#[path = "support/pbr_scene.rs"]
mod pbr_scene;

#[path = "support/path_tracing_scene.rs"]
mod path_tracing_scene;

use path_tracing_scene::{environment, room, white_furnace};

use anyhow::{ensure, Result};
use glam::{Mat4, Vec3};
use std::{io::Write, path::Path, sync::Arc};
use zenith::{
    asset::AssetServer,
    core::{
        camera::{Camera, CameraController},
        log,
        math::Degree,
    },
    renderer::{PathTracingSettings, RenderMode, WorldRenderer},
    rendergraph::{ImageId, RenderGraphBuilder, ResourceCache},
    rhi::{vk, Access, Descriptors, Gpu, Instance, Memory, MemoryDomain, TextureDesc},
};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

fn readback(
    builder: &mut RenderGraphBuilder<'_>,
    image: ImageId,
    memory: Arc<Memory>,
) -> Result<()> {
    let destination = builder.import_buffer(memory);
    builder.pass(
        "capture",
        vec![
            image.read(Access::COPY_READ),
            destination.write(Access::COPY_WRITE),
        ],
        move |ctx| {
            let region = vk::BufferImageCopy::default()
                .image_extent(vk::Extent3D {
                    width: WIDTH,
                    height: HEIGHT,
                    depth: 1,
                })
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                );
            unsafe {
                ctx.commands
                    .read_image(&ctx.image(image)?, &ctx.buffer(destination)?, &[region])
            }
        },
    )?;
    builder.pass(
        "host",
        vec![destination.read(Access::HOST_READ)],
        |_| Ok(()),
    )
}

fn capture(
    renderer: &mut WorldRenderer,
    gpu: &Arc<Gpu>,
    descriptors: &Arc<Descriptors>,
    cache: &mut ResourceCache,
    camera: &Camera,
    directory: &Path,
    name: &str,
) -> Result<Vec<[f32; 3]>> {
    let target = renderer.path_tracing_settings().target_spp;
    loop {
        let mut graph = RenderGraphBuilder::new(gpu, descriptors, cache)?;
        renderer.render_hdr(&mut graph, camera, WIDTH, HEIGHT)?;
        graph.record()?.submit()?.wait(60_000_000_000)?;
        if renderer.path_tracing_spp() >= target {
            break;
        }
    }
    ensure!(renderer.path_tracing_spp() == target, "incorrect final SPP");
    let hdr = gpu.allocate(u64::from(WIDTH * HEIGHT) * 16, MemoryDomain::Readback)?;
    let mut graph = RenderGraphBuilder::new(gpu, descriptors, cache)?;
    let image = renderer.render_hdr(&mut graph, camera, WIDTH, HEIGHT)?;
    readback(&mut graph, image, hdr.clone())?;
    let (commands, timings) = graph.record_profiled(true)?;
    commands.submit()?.wait(60_000_000_000)?;
    ensure!(
        !timings
            .unwrap()
            .read()?
            .iter()
            .any(|(name, _)| name == "path_tracing"),
        "completed image continued tracing"
    );
    let mut bytes = vec![0; (WIDTH * HEIGHT * 16) as usize];
    hdr.read(0, &mut bytes)?;
    let pixels: Vec<[f32; 3]> = bytes
        .chunks_exact(16)
        .map(|pixel| {
            std::array::from_fn(|c| f32::from_le_bytes(pixel[c * 4..c * 4 + 4].try_into().unwrap()))
        })
        .collect();
    ensure!(
        pixels.iter().flatten().all(|v| v.is_finite() && *v >= 0.0),
        "negative or nonfinite radiance"
    );
    let mut pfm = std::io::BufWriter::new(std::fs::File::create(
        directory.join(format!("{name}.pfm")),
    )?);
    write!(pfm, "PF\n{WIDTH} {HEIGHT}\n-1.0\n")?;
    for row in pixels.chunks_exact(WIDTH as usize).rev() {
        for pixel in row {
            for value in pixel {
                pfm.write_all(&value.to_le_bytes())?;
            }
        }
    }
    pfm.flush()?;

    let ldr = gpu.allocate(u64::from(WIDTH * HEIGHT) * 4, MemoryDomain::Readback)?;
    let mut graph = RenderGraphBuilder::new(gpu, descriptors, cache)?;
    let mut desc = TextureDesc::color(WIDTH, HEIGHT, vk::Format::R8G8B8A8_UNORM);
    desc.usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC;
    let output = graph.create_image(desc)?;
    renderer.render(&mut graph, camera, output)?;
    readback(&mut graph, output, ldr.clone())?;
    graph.record()?.submit()?.wait(60_000_000_000)?;
    let mut pixels_ldr = vec![0; (WIDTH * HEIGHT * 4) as usize];
    ldr.read(0, &mut pixels_ldr)?;
    for pixel in pixels_ldr.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let mut header = [0u8; 54];
    header[..2].copy_from_slice(b"BM");
    header[2..6].copy_from_slice(&(54 + WIDTH * HEIGHT * 4).to_le_bytes());
    header[10..14].copy_from_slice(&54u32.to_le_bytes());
    header[14..18].copy_from_slice(&40u32.to_le_bytes());
    header[18..22].copy_from_slice(&WIDTH.to_le_bytes());
    header[22..26].copy_from_slice(&(-(HEIGHT as i32)).to_le_bytes());
    header[26..28].copy_from_slice(&1u16.to_le_bytes());
    header[28..30].copy_from_slice(&32u16.to_le_bytes());
    let mut bmp = std::fs::File::create(directory.join(format!("{name}.bmp")))?;
    bmp.write_all(&header)?;
    bmp.write_all(&pixels_ldr)?;
    log::info!("Saved {name}, {target} SPP");
    Ok(pixels)
}

fn rmse(a: &[[f32; 3]], b: &[[f32; 3]]) -> f64 {
    (a.iter()
        .flatten()
        .zip(b.iter().flatten())
        .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
        .sum::<f64>()
        / (a.len() * 3) as f64)
        .sqrt()
}

fn mean(pixels: &[[f32; 3]]) -> f64 {
    pixels
        .iter()
        .flatten()
        .map(|value| f64::from(*value))
        .sum::<f64>()
        / (pixels.len() * 3) as f64
}

fn main() -> Result<()> {
    log::initialize(log::LevelFilter::Info)?;
    let directory = std::path::PathBuf::from(
        std::env::var_os("ZENITH_PATH_TRACING_DIR")
            .unwrap_or_else(|| "target/path-tracing-validation".into()),
    );
    std::fs::create_dir_all(&directory)?;
    let instance = Instance::new(&[], true)?;
    ensure!(
        instance.validation_enabled(),
        "reference validation requires Vulkan validation"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    {
        let descriptors = Descriptors::new(&gpu, 1024, 64)?;
        let assets = AssetServer::builder().with_builtin_assets().build()?;
        let scene = pbr_scene::spheres(&assets);
        let room = room(&assets);
        let white = white_furnace(&assets);
        let sky = environment(&assets, false);
        let white_sky = environment(&assets, true);
        let mut camera = Camera::new(Degree::new(60.0), WIDTH as f32 / HEIGHT as f32, 0.1);
        camera.set_position(Vec3::new(0.0, -14.0, 0.0));
        let mut controller = CameraController::default();
        controller.update_cameras(0.0, 0.0, 0.0, 0.0, std::iter::once(&mut camera));
        let mut renderer = WorldRenderer::new(&gpu, &descriptors, WIDTH, HEIGHT)?;
        renderer.set_skybox(&gpu, &descriptors, &sky)?;
        for scene in [&scene, &white, &room] {
            renderer.add_scene(&gpu, &descriptors, scene)?;
        }
        renderer.set_scene_visible(1, false)?;
        renderer.set_scene_visible(2, false)?;
        renderer.set_render_mode(RenderMode::PathTracing)?;
        let mut settings = PathTracingSettings {
            target_spp: 64,
            samples_per_frame: 16,
            max_bounces: 1,
        };
        renderer.set_path_tracing(settings)?;
        let mut lighting = renderer.lighting_settings();
        lighting.sky_intensity = 0.0;
        renderer.set_lighting(lighting)?;
        let mut cache = ResourceCache::default();
        capture(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            &camera,
            &directory,
            "spheres-directional",
        )?;
        lighting.directional.intensity = 0.0;
        lighting.sky_intensity = 1.0;
        renderer.set_lighting(lighting)?;
        settings.max_bounces = 8;
        let mut snapshots = Vec::new();
        for spp in [16, 128, 512] {
            settings.target_spp = spp;
            renderer.set_path_tracing(settings)?;
            snapshots.push(capture(
                &mut renderer,
                &gpu,
                &descriptors,
                &mut cache,
                &camera,
                &directory,
                &format!("spheres-ibl-{spp}spp"),
            )?);
        }
        let coarse = rmse(&snapshots[0], &snapshots[2]);
        let refined = rmse(&snapshots[1], &snapshots[2]);
        ensure!(
            refined < coarse * 0.6,
            "IBL did not converge: 16 SPP {coarse}, 128 SPP {refined}"
        );
        log::info!("IBL RMSE against 512 SPP: 16 SPP {coarse:.6}, 128 SPP {refined:.6}");
        lighting.directional.intensity = 1.0;
        renderer.set_lighting(lighting)?;
        settings.target_spp = 256;
        renderer.set_path_tracing(settings)?;
        capture(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            &camera,
            &directory,
            "spheres-combined",
        )?;

        renderer.set_scene_visible(0, false)?;
        renderer.set_scene_visible(1, true)?;
        renderer.set_skybox(&gpu, &descriptors, &white_sky)?;
        lighting.directional.intensity = 0.0;
        renderer.set_lighting(lighting)?;
        settings.target_spp = 512;
        renderer.set_path_tracing(settings)?;
        let furnace = capture(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            &camera,
            &directory,
            "white-furnace",
        )?;
        let mut furnace_error = 0.0f32;
        for instance in &white.get().unwrap().instances {
            let clip = camera.view_projection() * Mat4::from_cols_array(&instance.transform).w_axis;
            let x = ((clip.x / clip.w * 0.5 + 0.5) * WIDTH as f32) as usize;
            let y = ((0.5 - clip.y / clip.w * 0.5) * HEIGHT as f32) as usize;
            let mut average = Vec3::ZERO;
            for dy in y - 1..=y + 1 {
                for dx in x - 1..=x + 1 {
                    average += Vec3::from_array(furnace[dy * WIDTH as usize + dx]);
                }
            }
            furnace_error = furnace_error.max((average / 9.0 - Vec3::ONE).abs().max_element());
        }
        ensure!(furnace_error < 0.08, "white furnace error {furnace_error}");
        log::info!("White furnace maximum sphere-center error: {furnace_error:.6}");

        renderer.set_scene_visible(1, false)?;
        renderer.set_scene_visible(2, true)?;
        renderer.set_skybox(&gpu, &descriptors, &sky)?;
        camera.set_position(Vec3::new(0.0, -8.0, 1.2));
        controller.update_cameras(0.0, 0.0, 0.0, 0.0, std::iter::once(&mut camera));
        lighting.directional.direction_to_light = Vec3::new(-0.3, -0.8, 1.0);
        lighting.directional.intensity = 2.0;
        lighting.sky_intensity = 0.5;
        renderer.set_lighting(lighting)?;
        settings.target_spp = 256;
        settings.max_bounces = 1;
        renderer.set_path_tracing(settings)?;
        let direct = capture(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            &camera,
            &directory,
            "room-1bounce",
        )?;
        settings.max_bounces = 8;
        renderer.set_path_tracing(settings)?;
        let indirect = capture(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            &camera,
            &directory,
            "room-8bounces",
        )?;
        ensure!(
            mean(&indirect) > mean(&direct) * 1.01,
            "room is missing indirect light"
        );
        log::info!(
            "Room mean RGB: 1 bounce {:.6}, 8 bounces {:.6}",
            mean(&direct),
            mean(&indirect)
        );
    }
    gpu.wait_idle()?;
    drop(gpu);
    ensure!(
        instance.validation_errors().is_empty(),
        "{:?}",
        instance.validation_errors()
    );
    println!(
        "Path tracing reference validation passed; captures in {}",
        directory.display()
    );
    Ok(())
}
