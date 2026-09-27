use anyhow::{ensure, Result};
use glam::Vec3;
use std::{io::Write, sync::Arc};
use zenith::{
    asset::{AssetServer, MemorySource},
    core::{
        camera::{Camera, CameraController, NEAR_PLANE},
        log,
        math::Degree,
    },
    renderer::{DebugMode, WorldRenderer},
    rendergraph::{RenderGraphBuilder, ResourceCache},
    rhi::*,
};

pub fn run() -> Result<()> {
    log::initialize(log::LevelFilter::Info)?;
    let instance = Instance::new(&[], true)?;
    ensure!(
        instance.validation_enabled(),
        "BxDF validation requires Vulkan validation layers"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    let descriptors = Descriptors::new(&gpu, 2048, 32)?;
    let assets = AssetServer::builder()
        .source(MemorySource::default())
        .with_builtin_assets()
        .build()?;
    let einar = std::env::args().any(|arg| arg == "einar");
    let scene = if einar {
        super::einar::scene(
            &assets,
            std::path::Path::new("content/mesh/einar/einar.bin"),
        )?
    } else {
        super::scene::scene(&assets)
    };
    let mut renderer = WorldRenderer::new(&gpu, &descriptors, 960, 640)?;
    renderer.add_scene(&gpu, &descriptors, &scene)?;
    renderer.set_skybox(
        &gpu,
        &descriptors,
        &super::scene::environment(&assets, false),
    )?;
    let mut camera = Camera::new(Degree::new(60.0), 1.5, NEAR_PLANE);
    camera.set_position(if einar {
        Vec3::new(0.0, -5.5, 0.0)
    } else {
        Vec3::new(-0.3, -17.5, -0.5)
    });
    CameraController::default().update_cameras(0.0, 0.0, 0.0, 0.0, std::iter::once(&mut camera));
    let mut lighting = renderer.lighting_settings();
    lighting.ambient_occlusion.enabled = false;
    lighting.directional.direction_to_light = Vec3::new(-0.5, -0.8, 0.35).normalize();
    lighting.directional.intensity = 3.0;
    lighting.sky_intensity = 0.4;
    renderer.set_lighting(lighting)?;
    let directory = std::path::Path::new(if einar {
        "target/einar-lab"
    } else {
        "target/bxdf-lab"
    });
    std::fs::create_dir_all(directory)?;
    let mut cache = ResourceCache::default();
    let front = capture(
        &mut renderer,
        &gpu,
        &descriptors,
        &mut cache,
        &camera,
        directory,
        "front",
    )?;
    let repeated = capture(
        &mut renderer,
        &gpu,
        &descriptors,
        &mut cache,
        &camera,
        directory,
        "repeat",
    )?;
    ensure!(
        front == repeated,
        "fixed scene rendering is not deterministic"
    );
    lighting.directional.direction_to_light.y *= -1.0;
    renderer.set_lighting(lighting)?;
    let back = capture(
        &mut renderer,
        &gpu,
        &descriptors,
        &mut cache,
        &camera,
        directory,
        "back",
    )?;
    ensure!(
        difference(&front, &back) > 1.0,
        "light rotation did not change the scene"
    );
    renderer.set_debug_mode(DebugMode::SHADING_MODEL);
    let models = capture(
        &mut renderer,
        &gpu,
        &descriptors,
        &mut cache,
        &camera,
        directory,
        "shading-models",
    )?;
    for channel in if einar { vec![0, 2] } else { vec![0, 1, 2] } {
        ensure!(
            models.chunks_exact(4).any(|pixel| pixel[channel] > 160
                && (0..3).filter(|&c| c != channel).all(|c| pixel[c] < 32)),
            "Missing shading model debug channel {channel}"
        );
    }
    renderer.set_debug_mode(DebugMode::HAIR_TANGENT);
    capture(
        &mut renderer,
        &gpu,
        &descriptors,
        &mut cache,
        &camera,
        directory,
        "tangents",
    )?;
    renderer.set_debug_mode(DebugMode::empty());
    lighting.directional.direction_to_light.y *= -1.0;
    renderer.set_lighting(lighting)?;
    let mut reversed = scene.get().unwrap().as_ref().clone();
    reversed.instances.reverse();
    renderer.set_scene_visible(0, false)?;
    renderer.add_scene(&gpu, &descriptors, &assets.add(reversed))?;
    let reverse = capture(
        &mut renderer,
        &gpu,
        &descriptors,
        &mut cache,
        &camera,
        directory,
        "reverse-order",
    )?;
    ensure!(
        difference(&front, &reverse) < 0.2,
        "hair compositing depends on draw order"
    );
    if einar {
        capture_hdr(&mut renderer, &gpu, &descriptors, &mut cache, &camera)?;
        validate_thin_strand(&gpu, &descriptors, &assets)?;
        println!("Einar front/back lighting, tangent, draw-order and finite HDR checks passed");
        return Ok(());
    }
    renderer.set_debug_mode(DebugMode::WHITE_FURNACE);
    let mut hair = renderer.hair_settings();
    hair.environment_samples = 128;
    renderer.set_hair_settings(hair)?;
    capture(
        &mut renderer,
        &gpu,
        &descriptors,
        &mut cache,
        &camera,
        directory,
        "furnace",
    )?;
    renderer.set_debug_mode(DebugMode::empty());
    let mut fast = renderer.hair_settings();
    fast.environment_samples = 16;
    renderer.set_hair_settings(fast)?;
    let fast_hdr = capture_hdr(&mut renderer, &gpu, &descriptors, &mut cache, &camera)?;
    fast.environment_samples = 128;
    renderer.set_hair_settings(fast)?;
    let hdr = capture_hdr(&mut renderer, &gpu, &descriptors, &mut cache, &camera)?;
    let mut hair = renderer.hair_settings();
    hair.environment_samples = 256;
    hair.reference_quadrature = true;
    renderer.set_hair_settings(hair)?;
    let reference = capture_hdr(&mut renderer, &gpu, &descriptors, &mut cache, &camera)?;
    let error = |image: &[f32]| {
        let mut sum = 0.0;
        let mut energy = 0.0;
        for (index, (a, b)) in image
            .chunks_exact(3)
            .zip(reference.chunks_exact(3))
            .enumerate()
        {
            if index % 240 >= 135 && (30..135).contains(&(index / 240)) {
                for channel in 0..3 {
                    sum += (a[channel] as f64 - b[channel] as f64).powi(2);
                    energy += (b[channel] as f64).powi(2);
                }
            }
        }
        (sum / energy.max(1e-10)).sqrt()
    };
    let relative_rmse = error(&hdr);
    let fast_rmse = error(&fast_hdr);
    println!("Hair-region HDR RMSE: fast {fast_rmse:.5}; 128/256 samples {relative_rmse:.5}");
    ensure!(
        relative_rmse < 0.1,
        "hair environment integration did not converge"
    );
    ensure!(
        fast_rmse < 0.2,
        "fast hair environment approximation exceeds the lab error budget"
    );
    gpu.wait_idle()?;
    drop(renderer);
    drop(cache);
    drop(descriptors);
    drop(gpu);
    ensure!(
        instance.validation_errors().is_empty(),
        "Vulkan validation errors: {:?}",
        instance.validation_errors()
    );
    println!(
        "BxDF scene validation passed; captures in {}",
        directory.display()
    );
    Ok(())
}

fn capture_hdr(
    renderer: &mut WorldRenderer,
    gpu: &Arc<Gpu>,
    descriptors: &Arc<Descriptors>,
    cache: &mut ResourceCache,
    camera: &Camera,
) -> Result<Vec<f32>> {
    let (width, height) = (240, 160);
    let readback = gpu.allocate(width as u64 * height as u64 * 8, MemoryDomain::Readback)?;
    let mut graph = RenderGraphBuilder::new(gpu, descriptors, cache)?;
    let image = renderer.render_hdr(&mut graph, camera, width, height)?;
    let destination = graph.import_buffer(readback.clone());
    graph.pass(
        "hdr_capture",
        vec![
            image.read(Access::COPY_READ),
            destination.write(Access::COPY_WRITE),
        ],
        move |ctx| unsafe {
            ctx.commands.read_image(
                &ctx.image(image)?,
                &ctx.buffer(destination)?,
                &[vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })],
            )
        },
    )?;
    graph.pass(
        "hdr_host",
        vec![destination.read(Access::HOST_READ)],
        |_| Ok(()),
    )?;
    graph.record()?.submit()?.wait(60_000_000_000)?;
    let mut bytes = vec![0; (width * height * 8) as usize];
    readback.read(0, &mut bytes)?;
    let mut output = Vec::new();
    for pixel in bytes.chunks_exact(8) {
        for component in pixel[..6].chunks_exact(2) {
            let bits = u16::from_le_bytes(component.try_into()?);
            let exponent = (bits >> 10) & 31;
            ensure!(
                bits & 0x8000 == 0 && exponent != 31,
                "negative or nonfinite HDR output"
            );
            let fraction = (bits & 1023) as f32;
            output.push(if exponent == 0 {
                fraction * 2.0f32.powi(-24)
            } else {
                (1.0 + fraction / 1024.0) * 2.0f32.powi(exponent as i32 - 15)
            });
        }
    }
    Ok(output)
}

fn difference(a: &[u8], b: &[u8]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (*a as f64 - *b as f64).abs())
        .sum::<f64>()
        / a.len() as f64
}

fn capture(
    renderer: &mut WorldRenderer,
    gpu: &Arc<Gpu>,
    descriptors: &Arc<Descriptors>,
    cache: &mut ResourceCache,
    camera: &Camera,
    directory: &std::path::Path,
    name: &str,
) -> Result<Vec<u8>> {
    let (width, height) = (960, 640);
    let readback = gpu.allocate(width as u64 * height as u64 * 4, MemoryDomain::Readback)?;
    let mut graph = RenderGraphBuilder::new(gpu, descriptors, cache)?;
    let mut desc = TextureDesc::color(width, height, vk::Format::R8G8B8A8_UNORM);
    desc.usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC;
    let image = graph.create_image(desc)?;
    renderer.render(&mut graph, camera, image)?;
    let destination = graph.import_buffer(readback.clone());
    graph.pass(
        "capture",
        vec![
            image.read(Access::COPY_READ),
            destination.write(Access::COPY_WRITE),
        ],
        move |ctx| unsafe {
            ctx.commands.read_image(
                &ctx.image(image)?,
                &ctx.buffer(destination)?,
                &[vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })],
            )
        },
    )?;
    graph.pass(
        "capture_host",
        vec![destination.read(Access::HOST_READ)],
        |_| Ok(()),
    )?;
    let (commands, timings) = graph.record_profiled(true)?;
    commands.submit()?.wait(60_000_000_000)?;
    for (pass, ms) in timings.unwrap().read()? {
        if pass.contains("hair") || pass == "lighting" || pass == "gbuffer" {
            println!("{name}/{pass}: {ms:.3} ms");
        }
    }
    let mut pixels = vec![0; (width * height * 4) as usize];
    readback.read(0, &mut pixels)?;
    let mut header = [0u8; 54];
    header[..2].copy_from_slice(b"BM");
    header[2..6].copy_from_slice(&(54 + pixels.len() as u32).to_le_bytes());
    header[10..14].copy_from_slice(&54u32.to_le_bytes());
    header[14..18].copy_from_slice(&40u32.to_le_bytes());
    header[18..22].copy_from_slice(&width.to_le_bytes());
    header[22..26].copy_from_slice(&(-(height as i32)).to_le_bytes());
    header[26..28].copy_from_slice(&1u16.to_le_bytes());
    header[28..30].copy_from_slice(&32u16.to_le_bytes());
    let mut bmp = pixels.clone();
    for pixel in bmp.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let mut file = std::fs::File::create(directory.join(format!("{name}.bmp")))?;
    file.write_all(&header)?;
    file.write_all(&bmp)?;
    Ok(pixels)
}

fn validate_thin_strand(
    gpu: &Arc<Gpu>,
    descriptors: &Arc<Descriptors>,
    assets: &AssetServer,
) -> Result<()> {
    use glam::Mat4;
    use zenith::asset::{
        material::{Hair, Material},
        mesh::{Mesh, MeshInstance, Scene, SceneNode, Vertex},
    };
    let mut renderer = WorldRenderer::new(gpu, descriptors, 240, 160)?;
    renderer.set_skybox(gpu, descriptors, &super::scene::environment(assets, false))?;
    renderer.set_debug_mode(DebugMode::HAIR_TANGENT);
    let mut hair = renderer.hair_settings();
    hair.self_shadows = false;
    renderer.set_hair_settings(hair)?;
    let mut camera = Camera::new(Degree::new(60.0), 1.5, NEAR_PLANE);
    camera.set_position(Vec3::new(0.0, -5.5, 0.0));
    CameraController::default().update_cameras(0.0, 0.0, 0.0, 0.0, std::iter::once(&mut camera));
    let mut cache = ResourceCache::default();
    let baseline = capture_hdr(&mut renderer, gpu, descriptors, &mut cache, &camera)?;
    let material = assets.add(Material {
        shading_model: zenith::asset::material::ShadingModel::Hair,
        hair: Some(Hair::default()),
        ..Default::default()
    });
    let mut energies = Vec::new();
    for step in 0..8 {
        let shift = step as f32 / 8.0 * (2.0 * 5.5 * 30.0f32.to_radians().tan() / 240.0);
        let vertices = [1.0, -1.0]
            .into_iter()
            .flat_map(|z| {
                [0.0, 1.0].into_iter().map(move |edge| Vertex {
                    position: [shift, (edge * 2.0 - 1.0) * 0.001, z],
                    normal: [1.0, 0.0, 0.0],
                    tex_coord: [edge, (1.0 - z) * 0.5],
                    tangent: [0.0, 0.0, -1.0, 1.0],
                })
            })
            .collect();
        let model = assets.add(Scene {
            nodes: vec![SceneNode {
                source_index: 0,
                parent: None,
                transform: Mat4::IDENTITY.to_cols_array(),
            }],
            instances: vec![MeshInstance {
                node: 0,
                mesh: assets.add(Mesh::new(vertices, vec![0, 1, 2, 1, 3, 2])),
                material: material.clone(),
                transform: Mat4::IDENTITY.to_cols_array(),
            }],
        });
        renderer.add_scene(gpu, descriptors, &model)?;
        let pixels = capture_hdr(&mut renderer, gpu, descriptors, &mut cache, &camera)?;
        let energy: f32 = pixels
            .iter()
            .zip(&baseline)
            .map(|(a, b)| (a - b).abs())
            .sum();
        ensure!(
            energy > 0.01,
            "Subpixel edge-on strand disappeared at phase {step}"
        );
        energies.push(energy);
        renderer.set_scene_visible(step, false)?;
    }
    let minimum = energies.iter().copied().fold(f32::INFINITY, f32::min);
    let maximum = energies.iter().copied().fold(0.0, f32::max);
    ensure!(
        maximum / minimum < 1.15,
        "Subpixel strand coverage varies with pixel phase: {energies:?}"
    );
    println!("Subpixel edge-on strand coverage: {minimum:.4}..{maximum:.4}");
    Ok(())
}
