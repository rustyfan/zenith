use super::*;
use glam::Vec3;
use zenith_asset::{mesh::MeshInstance, CpuRetention};

fn quad(x: f32, y: f32, width: f32) -> Mesh {
    Mesh::new(
        [(x, -2.0), (x + width, -2.0), (x + width, 2.0), (x, 2.0)]
            .into_iter()
            .map(|(x, z)| Vertex {
                position: [x, y, z],
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

struct Occluder;
impl Importer for Occluder {
    type Settings = ();
    type Output = Scene;
    const KEY: &'static str = "test.ao-occluder";
    const VERSION: u32 = 1;
    fn extensions(&self) -> &[&str] {
        &["occluder"]
    }
    fn import(
        &self,
        bytes: &[u8],
        _: &(),
        ctx: &mut ImportContext<'_>,
    ) -> zenith_asset::Result<SceneData> {
        let mesh = ctx.emit::<Mesh>("mesh", quad(0.15 + bytes[0] as f32 * 8.0, 3.5, 2.0))?;
        let material = ctx.emit::<Material>("material", MaterialData::default())?;
        Ok(SceneData {
            nodes: vec![node()],
            instances: vec![MeshInstanceData {
                node: 0,
                mesh,
                material,
                transform: Mat4::IDENTITY.to_cols_array(),
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
fn ambient_occlusion_settings_reject_invalid_parameters() {
    for bias in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let mut settings = LightingSettings::default();
        settings.ambient_occlusion.bias = bias;
        assert!(settings.validated().is_err());
    }
    for radius in [0.0, -1.0, 0.001, f32::NAN, f32::INFINITY] {
        let mut settings = LightingSettings::default();
        settings.ambient_occlusion.radius = radius;
        assert!(settings.validated().is_err());
    }
    for samples in [0, 33, u32::MAX] {
        let mut settings = LightingSettings::default();
        settings.ambient_occlusion.samples = samples;
        assert!(settings.validated().is_err());
    }
    for samples in [1, 8, 32] {
        let mut settings = LightingSettings::default();
        settings.ambient_occlusion.samples = samples;
        assert!(settings.validated().is_ok());
    }
}

#[test]
#[ignore = "requires Vulkan validation, Slang, and ray query support"]
fn ambient_occlusion_follows_radius_toggles_and_streamed_geometry() -> Result<()> {
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
        source.insert("test.occluder", vec![0])?;
        let assets = AssetServer::builder()
            .source(source.clone())
            .with_builtin_assets()
            .register_importer(Occluder)
            .cpu_retention::<Mesh>(CpuRetention::ReleaseAfterUpload)
            .build()?;
        let mut renderer =
            WorldRenderer::new(&gpu, &descriptors, 64, 64)?.with_asset_server(&assets);
        let mut settings = renderer.lighting_settings();
        settings.shadows.enabled = false;
        settings.ambient_occlusion.samples = 32;
        settings.directional.direction_to_light = Vec3::new(2.0, -1.0, 0.0);
        renderer.set_lighting(settings)?;
        renderer.set_debug_mode(DebugMode::AMBIENT_OCCLUSION);
        let mut cache = ResourceCache::default();
        let white = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
        assert!(white.iter().all(|&value| value == 255));
        let receiver = assets.add(Scene {
            nodes: vec![node()],
            instances: vec![MeshInstance {
                node: 0,
                mesh: assets.add(quad(-3.0, 4.0, 6.0)),
                material: assets.add(Material {
                    base_color: [0.8, 0.5, 0.2, 1.0],
                    metallic: 0.0,
                    roughness: 0.8,
                    emissive: [0.0; 3],
                    base_color_tex: None,
                    mra_tex: None,
                    normal_tex: None,
                    emissive_tex: None,
                }),
                transform: Mat4::IDENTITY.to_cols_array(),
            }],
        });
        renderer.add_scene(&gpu, &descriptors, &receiver)?;
        assert_eq!(frame(&mut renderer, &gpu, &descriptors, &mut cache)?, white);
        for extent in [[65, 63], [1, 1], [64, 64]] {
            renderer.resize(extent[0], extent[1]);
            let pixels = frame_extent(
                &mut renderer,
                &gpu,
                &descriptors,
                &mut cache,
                vk::Format::R8G8B8A8_UNORM,
                extent,
            )?;
            assert!(pixels.iter().all(|&value| value == 255));
        }
        let occluder = assets.load_blocking::<Scene>("test.occluder")?;
        renderer.add_scene(&gpu, &descriptors, &occluder)?;
        assert!(occluder.get().unwrap().instances[0].mesh.get().is_none());
        let occluded = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
        assert!(
            center(&occluded)[0] > 0 && center(&occluded)[0] < 245,
            "{:?}",
            center(&occluded)
        );
        assert!(occluded
            .chunks_exact(4)
            .all(|p| p[0] == p[1] && p[1] == p[2]));
        let completed = gpu.completed_value()?;
        assert_eq!(
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
            occluded
        );
        assert_eq!(
            gpu.completed_value()?,
            completed + 1,
            "unchanged scene must reuse its TLAS"
        );

        renderer.set_post_processing(PostProcessingSettings { exposure: 0.0 })?;
        assert_eq!(
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
            occluded
        );
        let srgb = frame_format(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            vk::Format::R8G8B8A8_SRGB,
        )?;
        assert!(srgb.iter().zip(&occluded).all(|(a, b)| a.abs_diff(*b) <= 1));
        renderer.set_post_processing(PostProcessingSettings::default())?;

        settings.ambient_occlusion.radius = 0.1;
        renderer.set_lighting(settings)?;
        assert_eq!(frame(&mut renderer, &gpu, &descriptors, &mut cache)?, white);
        settings.ambient_occlusion.radius = 1.0;
        settings.ambient_occlusion.enabled = false;
        renderer.set_lighting(settings)?;
        assert_eq!(frame(&mut renderer, &gpu, &descriptors, &mut cache)?, white);
        settings.ambient_occlusion.enabled = true;
        renderer.set_lighting(settings)?;

        let sky = assets.add(CpuTexture {
            width: 1,
            height: 1,
            mip_levels: 1,
            is_cubemap: true,
            format: TextureFormat::Rgba8Unorm,
            pixels: vec![128; 24],
        });
        renderer.set_skybox(&gpu, &descriptors, &sky)?;
        renderer.set_debug_mode(DebugMode::empty());
        for (direct, sky, metallic) in [(0.0, 1.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 1.0)] {
            settings.directional.intensity = direct;
            settings.sky_intensity = sky;
            renderer.scenes[0].meshes[0].metallic = metallic;
            for shadows in [false, true] {
                settings.shadows.enabled = shadows;
                settings.ambient_occlusion.enabled = false;
                renderer.set_lighting(settings)?;
                let before = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
                settings.ambient_occlusion.enabled = true;
                renderer.set_lighting(settings)?;
                let after = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
                if sky > 0.0 && metallic == 0.0 {
                    assert!(
                        center(&after)[0] < center(&before)[0],
                        "AO must darken diffuse IBL"
                    );
                } else {
                    assert_eq!(center(&after), center(&before));
                }
                if direct > 0.0 {
                    if shadows {
                        assert_eq!(center(&after), [0; 3]);
                    } else {
                        assert!(
                            center(&after)[0] > 0,
                            "AO must not enable directional shadows"
                        );
                    }
                }
            }
        }

        renderer.set_debug_mode(DebugMode::AMBIENT_OCCLUSION);
        settings.shadows.enabled = false;
        renderer.set_lighting(settings)?;
        renderer.set_scene_visible(1, false)?;
        assert_eq!(frame(&mut renderer, &gpu, &descriptors, &mut cache)?, white);
        renderer.set_scene_visible(1, true)?;
        renderer.scenes[1].meshes[0].model = Mat4::from_translation(Vec3::X * 8.0).to_cols_array();
        assert_eq!(
            center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?),
            [255; 3]
        );
        renderer.scenes[1].meshes[0].model = Mat4::IDENTITY.to_cols_array();
        assert_eq!(
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
            occluded
        );

        let old_blas = Arc::downgrade(&renderer.scenes[1].meshes[0].geometry.blas);
        for revision_data in [1, 0] {
            source.insert("test.occluder", vec![revision_data])?;
            assets.reload(&occluder)?;
            occluder.wait()?;
            let revision = occluder.revision().unwrap();
            let start = Instant::now();
            while renderer.scenes[1].revision != Some(revision) {
                anyhow::ensure!(
                    start.elapsed() < Duration::from_secs(10),
                    "occluder reload timed out"
                );
                frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
            }
            let pixels = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
            if revision_data == 1 {
                assert_eq!(center(&pixels), [255; 3]);
            } else {
                assert_eq!(pixels, occluded);
            }
            assert!(old_blas.upgrade().is_none());
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

fn upload_filter_image(
    builder: &mut RenderGraphBuilder<'_>,
    width: u32,
    height: u32,
    format: vk::Format,
    bytes: &[u8],
) -> Result<ImageId> {
    let mut desc = TextureDesc::color(width, height, format);
    desc.usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST;
    let image = builder.create_image(desc)?;
    let upload = builder
        .gpu()
        .allocate(bytes.len() as u64, MemoryDomain::Upload)?;
    upload.write(0, bytes)?;
    let source = builder.import_buffer(upload);
    builder.pass(
        "upload_filter_image",
        vec![
            source.read(Access::COPY_READ),
            image.write(Access::COPY_WRITE),
        ],
        move |ctx| {
            let region = vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(desc.aspect())
                        .layer_count(1),
                )
                .image_extent(desc.extent);
            unsafe {
                ctx.commands
                    .upload_image(&ctx.buffer(source)?, &ctx.image(image)?, &[region])
            }
        },
    )?;
    Ok(image)
}

#[test]
#[ignore = "requires Vulkan validation, Slang, and supported GPU features"]
fn bilateral_upsampling_preserves_depth_edges_and_handles_small_odd_extents() -> Result<()> {
    std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
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
        let descriptors = Descriptors::new(&gpu, 128, 8)?;
        let [vertex, trace, upsample] = gpu.compile_shaders([
            (
                "content/shaders/screen_quad.slang",
                "vsmain",
                ShaderStage::Vertex,
            ),
            (
                "content/shaders/ambient_occlusion.slang",
                "main",
                ShaderStage::Fragment,
            ),
            (
                "content/shaders/ao_upsample.slang",
                "main",
                ShaderStage::Fragment,
            ),
        ])?;
        let renderer = AmbientOcclusionRenderer::new(&gpu, [&vertex, &trace, &upsample])?;
        let mut cache = ResourceCache::default();
        // IEEE 754 binary16 encodings of 0.5 and 1.0.
        const HALF: u16 = 0x3800;
        const ONE: u16 = 0x3c00;
        let cases: Vec<(u32, u32, Vec<f32>, Vec<u16>, Vec<u16>)> = vec![
            (
                5,
                3,
                vec![1.0; 15],
                vec![0, ONE, 0, ONE, 0, ONE],
                vec![
                    0, HALF, ONE, HALF, 0, HALF, HALF, HALF, HALF, HALF, ONE, HALF, 0, HALF, ONE,
                ],
            ),
            (
                5,
                3,
                [1.0, 1.0, 0.5, 0.5, 0.5].repeat(3),
                [0, ONE, ONE].repeat(2),
                [0, 0, ONE, ONE, ONE].repeat(3),
            ),
            (
                5,
                3,
                [1.0, 0.0, 0.5, 0.75, 0.5].repeat(3),
                [0, ONE, 0].repeat(2),
                [0, ONE, ONE, ONE, 0].repeat(3),
            ),
            (1, 1, vec![1.0], vec![HALF], vec![HALF]),
            (
                6,
                2,
                vec![1.0; 12],
                vec![0, ONE, 0],
                [0, HALF, ONE, HALF, 0, 0].repeat(2),
            ),
        ];
        for (width, height, depths, occlusion, expected) in cases {
            let readback = gpu.allocate(u64::from(width * height * 2), MemoryDomain::Readback)?;
            let mut builder = RenderGraphBuilder::new(&gpu, &descriptors, &mut cache)?;
            let depth = upload_filter_image(
                &mut builder,
                width,
                height,
                vk::Format::D32_SFLOAT,
                bytemuck::cast_slice(&depths),
            )?;
            let ao = upload_filter_image(
                &mut builder,
                width.div_ceil(2),
                height.div_ceil(2),
                vk::Format::R16_SFLOAT,
                bytemuck::cast_slice(&occlusion),
            )?;
            let desc = TextureDesc::color(width, height, SceneTextures::GLOBAL_ILLUMINATION_FORMAT);
            let output = builder.create_image(desc)?;
            renderer.upsample(&mut builder, ao, depth, output)?;
            let destination = builder.import_buffer(readback.clone());
            builder.pass(
                "readback",
                vec![
                    output.read(Access::COPY_READ),
                    destination.write(Access::COPY_WRITE),
                ],
                move |ctx| {
                    let region = vk::BufferImageCopy::default()
                        .image_subresource(
                            vk::ImageSubresourceLayers::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .layer_count(1),
                        )
                        .image_extent(desc.extent);
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
            let mut actual = vec![0u16; (width * height) as usize];
            readback.read(0, bytemuck::cast_slice_mut(&mut actual))?;
            assert_eq!(actual, expected, "bilateral result at {width}x{height}");
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
