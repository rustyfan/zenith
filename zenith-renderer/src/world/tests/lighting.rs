use super::*;
use glam::Vec3;

fn center(pixels: &[u8]) -> [u8; 3] {
    let offset = (32 * 64 + 32) * 4;
    pixels[offset..offset + 3].try_into().unwrap()
}

#[test]
#[ignore = "requires Vulkan validation, Slang, and supported GPU features"]
fn normal_mapping_is_invariant_to_uv_scale_and_instance_transform() -> Result<()> {
    use zenith_asset::mesh::MeshInstance;

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
        let descriptors = Descriptors::new(&gpu, 512, 32)?;
        let assets = AssetServer::builder()
            .source(MemorySource::default())
            .with_builtin_assets()
            .build()?;
        let normal = assets.add(CpuTexture {
            width: 1,
            height: 1,
            mip_levels: 1,
            is_cubemap: false,
            format: TextureFormat::Rgba8Unorm,
            pixels: vec![204, 179, 230, 255],
        });
        let material = assets.add(Material {
            base_color: [0.5, 0.5, 0.5, 1.0],
            metallic: 0.0,
            roughness: 1.0,
            emissive: [0.0; 3],
            base_color_tex: None,
            mra_tex: None,
            normal_tex: Some(normal),
            emissive_tex: None,
        });
        let mut renderer = WorldRenderer::new(&gpu, &descriptors, 64, 64)?;
        let mut cache = ResourceCache::default();
        let mut settings = renderer.lighting_settings();
        settings.sky_intensity = 0.0;
        let identity = Mat4::IDENTITY.to_cols_array();
        for (handedness, light) in [
            (1.0, Vec3::X),
            (-1.0, -Vec3::X),
            (1.0, Vec3::Z),
            (-1.0, Vec3::Z),
        ] {
            settings.directional.direction_to_light = light;
            renderer.set_lighting(settings)?;
            let mut reference: Option<Vec<u8>> = None;
            for model in [
                Mat4::IDENTITY,
                Mat4::from_rotation_z(0.6) * Mat4::from_scale(Vec3::new(2.0, 0.5, 1.5)),
                Mat4::from_rotation_x(0.4) * Mat4::from_scale(Vec3::new(-1.5, 0.8, 2.5)),
            ] {
                let inverse = model.inverse();
                let mirrored = model.determinant() < 0.0;
                let normal = model
                    .transpose()
                    .transform_vector3(-Vec3::Y)
                    .normalize()
                    .to_array();
                let tangent = inverse.transform_vector3(Vec3::X * handedness).normalize();
                for scale in [1.0, 0.01, 0.0001, 0.000001, 0.0] {
                    let vertices = [
                        ([-0.8, 2.0, -0.8], [0.0, 0.0]),
                        ([0.8, 2.0, -0.8], [scale * handedness, 0.0]),
                        ([0.0, 2.0, 0.8], [0.0, scale]),
                    ]
                    .into_iter()
                    .map(|(position, tex_coord)| Vertex {
                        position: inverse
                            .transform_point3(Vec3::from_array(position))
                            .to_array(),
                        normal,
                        tex_coord,
                        tangent: if scale == 0.0 {
                            [0.0; 4]
                        } else {
                            tangent
                                .extend(handedness * if mirrored { -1.0 } else { 1.0 })
                                .to_array()
                        },
                    })
                    .collect();
                    let mesh = Mesh::new(
                        vertices,
                        if mirrored {
                            vec![0, 2, 1]
                        } else {
                            vec![0, 1, 2]
                        },
                    );
                    let scene = assets.add(Scene {
                        nodes: vec![SceneNode {
                            source_index: 0,
                            parent: None,
                            transform: identity,
                        }],
                        instances: vec![MeshInstance {
                            node: 0,
                            mesh: assets.add(mesh),
                            material: material.clone(),
                            transform: model.to_cols_array(),
                        }],
                    });
                    let index = renderer.scenes.len();
                    renderer.add_scene(&gpu, &descriptors, &scene)?;
                    let pixels = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
                    renderer.set_scene_visible(index, false)?;
                    if scale == 0.0 {
                        assert_eq!(center(&pixels), [0; 3]);
                    } else if let Some(reference) = &reference {
                        let error = pixels
                            .iter()
                            .zip(reference)
                            .map(|(a, b)| a.abs_diff(*b))
                            .max()
                            .unwrap();
                        assert!(
                            error <= 1,
                            "normal map changed at UV scale {scale}, handedness {handedness}: {error}"
                        );
                    } else {
                        assert!(
                            center(&pixels)[0] > 50,
                            "normal map must tilt toward the light"
                        );
                        reference = Some(pixels);
                    }
                }
            }
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

#[test]
#[ignore = "requires Vulkan validation, Slang, and supported GPU features"]
fn directional_and_skylight_follow_material_parameters() -> Result<()> {
    std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
    let _ = zenith_core::log::initialize(zenith_core::log::LevelFilter::Info);
    let instance = Instance::new(&[], true)?;
    anyhow::ensure!(
        instance.validation_enabled(),
        "lighting test requires Vulkan validation"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    {
        let descriptors = Descriptors::new(&gpu, 512, 32)?;
        let source = Arc::new(MemorySource::default());
        source.insert("scene.testscene", vec![128])?;
        let assets = AssetServer::builder()
            .source(source)
            .with_builtin_assets()
            .register_importer(TestScene)
            .build()?;
        let scene = assets.load_blocking::<Scene>("scene.testscene")?;
        let mut renderer = WorldRenderer::new(&gpu, &descriptors, 64, 64)?;
        renderer.add_scene(&gpu, &descriptors, &scene)?;
        let mut cache = ResourceCache::default();
        let mut settings = renderer.lighting_settings();
        settings.directional.direction_to_light = -Vec3::Y;
        settings.directional.intensity = 0.0;
        settings.sky_intensity = 0.0;
        renderer.set_lighting(settings)?;
        assert_eq!(
            center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?),
            [0; 3]
        );

        settings.directional.intensity = 1.0;
        settings.directional.direction_to_light = Vec3::Y;
        renderer.set_lighting(settings)?;
        assert_eq!(
            center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?),
            [0; 3]
        );
        settings.directional.direction_to_light = -Vec3::Y;
        renderer.set_lighting(settings)?;
        let diffuse = center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?);
        assert!(
            diffuse[0] > 60 && diffuse[0] > diffuse[1] + 20,
            "{diffuse:?}"
        );
        renderer.scenes[0].meshes[0].metallic = 1.0;
        let metal_rough = center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?);
        assert!(metal_rough[0] < diffuse[0], "{metal_rough:?}, {diffuse:?}");
        renderer.scenes[0].meshes[0].roughness = 0.15;
        let metal_smooth = center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?);
        assert!(
            metal_smooth[0] > metal_rough[0] + 40,
            "{metal_smooth:?}, {metal_rough:?}"
        );

        settings.directional.color = Vec3::Z;
        renderer.set_lighting(settings)?;
        let blue_light = center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?);
        assert_eq!(&blue_light[..2], &[0, 0]);
        assert!(blue_light[2] > 0);
        settings.directional.direction_to_light = Vec3::X;
        renderer.scenes[0].meshes[0].roughness = 0.0;
        renderer.set_lighting(settings)?;
        frame(&mut renderer, &gpu, &descriptors, &mut cache)?;

        let sky = assets.add(CpuTexture {
            width: 1,
            height: 1,
            mip_levels: 1,
            is_cubemap: true,
            format: TextureFormat::Rgba8Unorm,
            pixels: vec![128; 24],
        });
        renderer.set_skybox(&gpu, &descriptors, &sky)?;
        settings.directional.intensity = 0.0;
        settings.sky_intensity = 1.0;
        renderer.set_lighting(settings)?;
        let metal_ibl_smooth = center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?);
        assert!(metal_ibl_smooth[0] > 80, "{metal_ibl_smooth:?}");
        assert!(metal_ibl_smooth[0] > metal_ibl_smooth[1] + 30);
        renderer.scenes[0].meshes[0].roughness = 1.0;
        let metal_ibl_rough = center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?);
        assert!(metal_ibl_rough[0] < metal_ibl_smooth[0] - 20);
        renderer.scenes[0].meshes[0].metallic = 0.0;
        let dielectric_ibl = center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?);
        assert!(dielectric_ibl[0] > metal_ibl_rough[0]);
        let unorm = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
        let srgb = frame_format(
            &mut renderer,
            &gpu,
            &descriptors,
            &mut cache,
            vk::Format::R8G8B8A8_SRGB,
        )?;
        assert!(unorm.iter().zip(&srgb).all(|(a, b)| a.abs_diff(*b) <= 1));

        let uploaded = renderer.upload_stats().bytes;
        renderer.set_scene_visible(0, false)?;
        assert_ne!(frame(&mut renderer, &gpu, &descriptors, &mut cache)?, unorm);
        renderer.set_scene_visible(0, true)?;
        assert_eq!(frame(&mut renderer, &gpu, &descriptors, &mut cache)?, unorm);
        assert_eq!(renderer.upload_stats().bytes, uploaded);
        assert!(renderer.set_scene_visible(usize::MAX, false).is_err());

        settings.sky_intensity = 0.0;
        renderer.set_lighting(settings)?;
        assert_eq!(
            center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?),
            [0; 3]
        );
        let saved = renderer.lighting_settings();
        settings.directional.direction_to_light = Vec3::ZERO;
        assert!(renderer.set_lighting(settings).is_err());
        assert_eq!(
            renderer.lighting_settings().directional.direction_to_light,
            saved.directional.direction_to_light
        );
    }
    gpu.wait_idle()?;
    drop(gpu);
    anyhow::ensure!(
        instance.validation_errors().is_empty(),
        "lighting validation errors: {:?}",
        instance.validation_errors()
    );
    Ok(())
}

#[test]
#[ignore = "requires Vulkan validation, Slang, and supported GPU features"]
fn white_furnace_debug_view_and_compensation_toggle() -> Result<()> {
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
        let descriptors = Descriptors::new(&gpu, 512, 32)?;
        let source = Arc::new(MemorySource::default());
        source.insert("scene.testscene", vec![128])?;
        let assets = AssetServer::builder()
            .source(source)
            .with_builtin_assets()
            .register_importer(TestScene)
            .build()?;
        let scene = assets.load_blocking::<Scene>("scene.testscene")?;
        let sky = assets.add(CpuTexture {
            width: 1,
            height: 1,
            mip_levels: 1,
            is_cubemap: true,
            format: TextureFormat::Rgba8Unorm,
            pixels: [64, 128, 255, 255].repeat(6),
        });
        let mut renderer = WorldRenderer::new(&gpu, &descriptors, 64, 64)?;
        renderer.add_scene(&gpu, &descriptors, &scene)?;
        renderer.set_skybox(&gpu, &descriptors, &sky)?;
        renderer.scenes[0].meshes[0].metallic = 1.0;
        renderer.scenes[0].meshes[0].roughness = 1.0;
        let mut cache = ResourceCache::default();
        let mut lighting = renderer.lighting_settings();
        assert!(lighting.multiple_scattering);
        lighting.directional.direction_to_light = -Vec3::Y;
        for (direct, sky) in [(1.0, 0.0), (0.0, 1.0)] {
            lighting.directional.intensity = direct;
            lighting.sky_intensity = sky;
            lighting.multiple_scattering = true;
            renderer.set_lighting(lighting)?;
            let compensated = center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?);
            lighting.multiple_scattering = false;
            renderer.set_lighting(lighting)?;
            let single = center(&frame(&mut renderer, &gpu, &descriptors, &mut cache)?);
            assert!(
                compensated[0] > single[0] + 5,
                "direct={direct}, sky={sky}: {compensated:?}, {single:?}"
            );
        }
        lighting.multiple_scattering = true;
        renderer.set_lighting(lighting)?;
        let original = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
        let uploaded = renderer.upload_stats().bytes;
        renderer.set_debug_mode(
            DebugMode::WHITE_FURNACE | DebugMode::AMBIENT_OCCLUSION | DebugMode::DIFFUSE_SH,
        );
        renderer.set_post_processing(PostProcessingSettings { exposure: 0.0 })?;
        for format in [vk::Format::R8G8B8A8_UNORM, vk::Format::R8G8B8A8_SRGB] {
            for metallic in [0.0, 0.5, 1.0] {
                renderer.scenes[0].meshes[0].metallic = metallic;
                for roughness in [0.0, 0.5, 1.0] {
                    renderer.scenes[0].meshes[0].roughness = roughness;
                    lighting.multiple_scattering = true;
                    lighting.directional.intensity = 100.0;
                    lighting.sky_intensity = 0.0;
                    renderer.set_lighting(lighting)?;
                    let compensated =
                        frame_format(&mut renderer, &gpu, &descriptors, &mut cache, format)?;
                    assert!(
                        compensated.iter().all(|&value| value >= 254),
                        "furnace metal={metallic}, roughness={roughness}: {:?}",
                        center(&compensated)
                    );
                    lighting.multiple_scattering = false;
                    renderer.set_lighting(lighting)?;
                    let single =
                        frame_format(&mut renderer, &gpu, &descriptors, &mut cache, format)?;
                    assert_eq!(&single[..4], &[255; 4]);
                    if metallic == 1.0 && roughness == 1.0 {
                        let pixel = center(&single);
                        assert!(
                            (140..170).contains(&pixel[0])
                                && pixel[0] == pixel[1]
                                && pixel[1] == pixel[2],
                            "single-scattering furnace: {pixel:?}"
                        );
                    } else if roughness == 0.0 {
                        assert!(center(&single).iter().all(|&value| value >= 254));
                    }
                }
            }
        }
        lighting.multiple_scattering = true;
        lighting.directional.intensity = 0.0;
        lighting.sky_intensity = 1.0;
        renderer.set_lighting(lighting)?;
        renderer.set_post_processing(PostProcessingSettings::default())?;
        renderer.set_debug_mode(DebugMode::empty());
        assert_eq!(
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
            original
        );
        assert_eq!(renderer.upload_stats().bytes, uploaded);
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
#[ignore = "requires Vulkan validation, Slang, and supported GPU features"]
fn hdr_white_furnace_is_preserved() -> Result<()> {
    std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
    let instance = Instance::new(&[], true)?;
    anyhow::ensure!(
        instance.validation_enabled(),
        "furnace test requires Vulkan validation"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    {
        let descriptors = Descriptors::new(&gpu, 512, 32)?;
        let assets = AssetServer::builder().build()?;
        let radiance = [4.0f32, 1.0, 0.125, 1.0];
        let sky = assets.add(CpuTexture {
            width: 1,
            height: 1,
            mip_levels: 1,
            is_cubemap: true,
            format: TextureFormat::Rgba32Float,
            pixels: bytemuck::cast_slice(&radiance.repeat(6)).to_vec(),
        });
        let mut renderer = WorldRenderer::new(&gpu, &descriptors, 1, 1)?;
        renderer.set_skybox(&gpu, &descriptors, &sky)?;
        let mut settings = renderer.lighting_settings();
        settings.directional.intensity = 0.0;
        settings.shadows.enabled = false;
        settings.ambient_occlusion.enabled = false;
        let view = GpuViewData::new(&Camera::default().view_data());
        let mut cache = ResourceCache::default();
        for metallic in [0.0, 1.0] {
            for roughness in [0.0, 0.25, 0.5, 0.75, 1.0] {
                for nov in [0.02f32, 0.1, 0.5, 1.0] {
                    let readback = gpu.allocate(8, MemoryDomain::Readback)?;
                    let mut builder = RenderGraphBuilder::new(&gpu, &descriptors, &mut cache)?;
                    let ibl = renderer.ibl.render(&mut builder)?;
                    let scene = SceneTextures::new(&mut builder, 1, 1)?;
                    let (base, normal, ao, depth) = (
                        scene.base_color,
                        scene.normal_mra,
                        scene.global_illumination,
                        scene.depth,
                    );
                    let n = Vec3::new((1.0 - nov * nov).sqrt(), -nov, 0.0);
                    let packed = n / n.abs().element_sum() * 0.5 + Vec3::splat(0.5);
                    builder.pass(
                        "furnace_gbuffer",
                        vec![
                            base.write(COLOR_WRITE),
                            normal.write(COLOR_WRITE),
                            ao.write(COLOR_WRITE),
                            depth.write(DEPTH_WRITE),
                        ],
                        move |ctx| {
                            ctx.commands.begin_rendering(
                                &[
                                    Attachment {
                                        view: &ctx.view(base)?,
                                        clear: Some([1.0; 4]),
                                        store: true,
                                        resolve: None,
                                    },
                                    Attachment {
                                        view: &ctx.view(normal)?,
                                        clear: Some([packed.x, packed.y, metallic, roughness]),
                                        store: true,
                                        resolve: None,
                                    },
                                    Attachment {
                                        view: &ctx.view(ao)?,
                                        clear: Some([1.0; 4]),
                                        store: true,
                                        resolve: None,
                                    },
                                ],
                                Some(DepthAttachment {
                                    view: &ctx.view(depth)?,
                                    clear: Some((0.5, 0)),
                                    store: true,
                                }),
                                vk::Extent2D {
                                    width: 1,
                                    height: 1,
                                },
                            )?;
                            ctx.commands.end_rendering()
                        },
                    )?;
                    let mut desc = TextureDesc::color(1, 1, DirectLightingRenderer::COLOR_FORMAT);
                    desc.usage =
                        vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC;
                    let output = builder.create_image(desc)?;
                    renderer.lighting.render(
                        &mut builder,
                        None,
                        scene,
                        ibl,
                        view,
                        settings,
                        0,
                        output,
                    )?;
                    let destination = builder.import_buffer(readback.clone());
                    builder.pass(
                        "furnace_readback",
                        vec![
                            output.read(Access::COPY_READ),
                            destination.write(Access::COPY_WRITE),
                        ],
                        move |ctx| {
                            let region = vk::BufferImageCopy::default()
                                .image_extent(vk::Extent3D {
                                    width: 1,
                                    height: 1,
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
                    builder.record()?.submit()?.wait(10_000_000_000)?;
                    let mut pixel = [0u16; 4];
                    readback.read(0, bytemuck::cast_slice_mut(&mut pixel))?;
                    // The expected HDR values are exact binary16 powers of two, before exposure or tone mapping.
                    for (actual, expected) in
                        pixel.into_iter().zip([0x4400u16, 0x3c00, 0x3000, 0x3c00])
                    {
                        assert!(actual.abs_diff(expected) <= 2, "HDR furnace metal={metallic}, roughness={roughness}, NoV={nov}: {pixel:?}");
                    }
                }
            }
        }
    }
    gpu.wait_idle()?;
    drop(gpu);
    anyhow::ensure!(
        instance.validation_errors().is_empty(),
        "furnace validation: {:?}",
        instance.validation_errors()
    );
    Ok(())
}
