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
