use super::*;
use zenith_core::{camera::Camera, math::Degree};
use zenith_rendergraph::ResourceCache;

#[test]
fn batch_rejects_empty_overflowing_and_unsupported_ranges() {
    for batch in [
        PathTracingBatch {
            sample_count: 0,
            ..Default::default()
        },
        PathTracingBatch {
            first_sample: u32::MAX,
            sample_count: 2,
            ..Default::default()
        },
        PathTracingBatch {
            max_bounces: 0,
            ..Default::default()
        },
        PathTracingBatch {
            max_bounces: 9,
            ..Default::default()
        },
    ] {
        assert!(batch.validate().is_err());
    }
    assert!(PathTracingBatch::default().validate().is_ok());
    assert!(PathTracingBatch {
        first_sample: u32::MAX,
        ..Default::default()
    }
    .validate()
    .is_ok());
}

pub(super) fn plane(
    y: f32,
    center_x: f32,
    half_width: f32,
    half_height: f32,
    normal: Vec3,
) -> Mesh {
    Mesh::new(
        [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .into_iter()
            .map(|(x, z)| Vertex {
                position: [center_x + x * half_width, y, z * half_height],
                normal: normal.to_array(),
                tex_coord: [0.0; 2],
                tangent: [0.0; 4],
            })
            .collect(),
        vec![0, 1, 2, 0, 2, 3],
    )
}

pub(super) fn add_plane(
    renderer: &mut WorldRenderer,
    gpu: &Arc<Gpu>,
    descriptors: &Arc<Descriptors>,
    assets: &AssetServer,
    mesh: Mesh,
    material: Material,
) -> Result<()> {
    let scene = assets.add(scene(vec![MeshInstance {
        node: 0,
        mesh: assets.add(mesh),
        material: assets.add(material),
        transform: Mat4::IDENTITY.to_cols_array(),
    }]));
    renderer.add_scene(gpu, descriptors, &scene)
}

pub(super) fn frame(
    renderer: &mut WorldRenderer,
    gpu: &Arc<Gpu>,
    descriptors: &Arc<Descriptors>,
    cache: &mut ResourceCache,
    camera: &Camera,
    batch: PathTracingBatch,
) -> Result<Vec<[f32; 4]>> {
    let readback = gpu.allocate(8 * 8 * 16, MemoryDomain::Readback)?;
    let mut builder = RenderGraphBuilder::new(gpu, descriptors, cache)?;
    let output = renderer.render_path_tracing_hdr(&mut builder, camera, 8, 8, batch)?;
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
                    width: 8,
                    height: 8,
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
    builder.record()?.submit()?.wait(10_000_000_000)?;
    let mut pixels = vec![[0.0f32; 4]; 64];
    readback.read(0, bytemuck::cast_slice_mut(&mut pixels))?;
    assert!(pixels
        .iter()
        .all(|pixel| pixel.iter().all(|value| value.is_finite()) && pixel[3] == 1.0));
    Ok(pixels)
}

fn mean(pixels: &[[f32; 4]]) -> Vec3 {
    pixels
        .iter()
        .map(|pixel| Vec3::from_slice(&pixel[..3]))
        .sum::<Vec3>()
        / pixels.len() as f32
}

#[test]
#[ignore = "requires Vulkan validation, Slang, descriptor heaps, and ray tracing pipeline support"]
fn path_integrator_resolves_visibility_environment_energy_and_bounces() -> Result<()> {
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
            .build()?;
        let mut renderer = WorldRenderer::new(&gpu, &descriptors, 8, 8)?;
        let camera = Camera::new(Degree::new(0.001), 1.0, 0.1);
        let mut cache = ResourceCache::default();
        let batch = PathTracingBatch {
            first_sample: 0,
            sample_count: 1,
            max_bounces: 1,
        };
        let color = Vec3::new(4.0, 1.0, 0.125);
        let pixels: Vec<[f32; 4]> = [4.0, 1.0, 0.125, 1.0]
            .repeat(24)
            .chunks_exact(4)
            .map(|pixel| pixel.try_into().unwrap())
            .chain([[100.0, 0.0, 0.0, 1.0]; 6])
            .collect();
        let sky = assets.add(CpuTexture {
            width: 2,
            height: 2,
            mip_levels: 2,
            is_cubemap: true,
            format: TextureFormat::Rgba32Float,
            pixels: bytemuck::cast_slice(&pixels).to_vec(),
        });
        renderer.set_skybox(&gpu, &descriptors, &sky)?;
        let mut lighting = renderer.lighting_settings();
        lighting.directional.intensity = 0.0;
        lighting.sky_intensity = 2.0;
        renderer.set_lighting(lighting)?;
        for pixel in frame(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            &camera,
            batch,
        )? {
            for c in 0..3 {
                close(pixel[c], color[c] * 2.0);
            }
        }
        let metal = Material {
            base_color: [1.0; 4],
            metallic: 1.0,
            roughness: 1.0,
            ..Default::default()
        };
        add_plane(
            &mut renderer,
            &gpu,
            &descriptors,
            &assets,
            plane(2.0, 0.0, 4.0, 4.0, -Vec3::Y),
            metal.clone(),
        )?;
        lighting.sky_intensity = 0.0;
        lighting.directional.intensity = 2.0;
        lighting.directional.color = Vec3::new(2.0, 1.0, 0.5);
        lighting.directional.direction_to_light = -Vec3::Y;
        lighting.multiple_scattering = false;
        renderer.set_lighting(lighting)?;
        let direct = frame(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            &camera,
            batch,
        )?;
        for pixel in &direct {
            for c in 0..3 {
                close(
                    pixel[c],
                    lighting.directional.color[c] * 2.0 / (4.0 * std::f32::consts::PI),
                );
            }
        }
        lighting.directional.direction_to_light = Vec3::Y;
        renderer.set_lighting(lighting)?;
        assert_eq!(
            mean(&frame(
                &mut renderer,
                &gpu,
                &descriptors,
                &mut cache,
                &camera,
                batch
            )?),
            Vec3::ZERO
        );

        lighting.directional.direction_to_light = (Vec3::X - Vec3::Y).normalize();
        renderer.set_lighting(lighting)?;
        let visible = mean(&frame(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            &camera,
            batch,
        )?);
        let cosine = std::f32::consts::FRAC_1_SQRT_2;
        for c in 0..3 {
            close(
                visible[c],
                lighting.directional.color[c] * 2.0 * cosine
                    / (2.0 * std::f32::consts::PI * (1.0 + cosine)),
            );
        }
        add_plane(
            &mut renderer,
            &gpu,
            &descriptors,
            &assets,
            plane(1.0, 1.0, 0.3, 0.3, -Vec3::Y),
            metal,
        )?;
        assert_eq!(
            mean(&frame(
                &mut renderer,
                &gpu,
                &descriptors,
                &mut cache,
                &camera,
                batch
            )?),
            Vec3::ZERO
        );
        renderer.set_scene_visible(1, false)?;
        assert_eq!(
            mean(&frame(
                &mut renderer,
                &gpu,
                &descriptors,
                &mut cache,
                &camera,
                batch
            )?),
            visible
        );

        lighting.directional.intensity = 0.0;
        lighting.sky_intensity = 1.0;
        lighting.multiple_scattering = true;
        renderer.set_lighting(lighting)?;
        let first_batch = PathTracingBatch {
            sample_count: 1024,
            ..batch
        };
        let first = frame(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            &camera,
            first_batch,
        )?;
        let furnace = mean(&first) / color;
        assert!(
            (furnace - Vec3::ONE).abs().max_element() < 0.015,
            "{furnace:?}"
        );
        let second = frame(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            &camera,
            PathTracingBatch {
                first_sample: 1024,
                ..first_batch
            },
        )?;
        let combined = frame(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            &camera,
            PathTracingBatch {
                sample_count: 2048,
                ..batch
            },
        )?;
        for ((first, second), combined) in first.iter().zip(&second).zip(&combined) {
            for c in 0..3 {
                assert!((combined[c] - (first[c] + second[c]) * 0.5).abs() < 0.00002);
            }
        }
        assert_eq!(
            first,
            frame(
                &mut renderer,
                &gpu,
                &descriptors,
                &mut cache,
                &camera,
                first_batch
            )?
        );
        lighting.multiple_scattering = false;
        renderer.set_lighting(lighting)?;
        let furnace = mean(&frame(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            &camera,
            first_batch,
        )?) / color;
        let expected = 1.0 - std::f32::consts::LN_2;
        assert!(
            (furnace - Vec3::splat(expected)).abs().max_element() < 0.005,
            "{furnace:?}"
        );

        renderer.set_scene_visible(0, false)?;
        let diffuse = Material {
            base_color: [0.8, 0.4, 0.2, 1.0],
            metallic: 0.0,
            roughness: 1.0,
            ..Default::default()
        };
        add_plane(
            &mut renderer,
            &gpu,
            &descriptors,
            &assets,
            plane(2.0, 0.0, 3.0, 1.0, -Vec3::Y),
            diffuse,
        )?;
        add_plane(
            &mut renderer,
            &gpu,
            &descriptors,
            &assets,
            plane(-2.0, 0.0, 100.0, 100.0, Vec3::Y),
            Material {
                base_color: [1.0; 4],
                metallic: 0.0,
                roughness: 1.0,
                ..Default::default()
            },
        )?;
        lighting.sky_intensity = 0.0;
        lighting.directional.intensity = 1.0;
        lighting.directional.color = Vec3::ONE;
        lighting.directional.direction_to_light = (Vec3::Y + Vec3::Z).normalize();
        lighting.multiple_scattering = true;
        renderer.set_lighting(lighting)?;
        assert_eq!(
            mean(&frame(
                &mut renderer,
                &gpu,
                &descriptors,
                &mut cache,
                &camera,
                first_batch
            )?),
            Vec3::ZERO
        );
        let indirect = mean(&frame(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            &camera,
            PathTracingBatch {
                max_bounces: 2,
                ..first_batch
            },
        )?);
        assert!(
            indirect.min_element() > 0.02 && indirect.x > indirect.y && indirect.y > indirect.z,
            "{indirect:?}"
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
