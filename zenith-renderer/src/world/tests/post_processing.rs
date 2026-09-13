use super::*;

#[test]
#[ignore = "requires Vulkan validation, Slang, and supported GPU features"]
fn hdr_lighting_is_exposed_tonemapped_and_encoded_once() -> Result<()> {
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
        let assets = AssetServer::builder().with_builtin_assets().build()?;
        let sky = assets.add(CpuTexture {
            width: 1,
            height: 1,
            mip_levels: 1,
            is_cubemap: true,
            format: TextureFormat::Rgba32Float,
            pixels: bytemuck::cast_slice(&[4.0f32, 1.0, 0.125, 1.0].repeat(6)).to_vec(),
        });
        let mut renderer = WorldRenderer::new(&gpu, &descriptors, 64, 64)?;
        renderer.set_skybox(&gpu, &descriptors, &sky)?;
        let mut cache = ResourceCache::default();
        let mut settings = renderer.post_processing_settings();
        assert_eq!(settings.exposure, 1.0);
        for format in [vk::Format::R8G8B8A8_UNORM, vk::Format::R8G8B8A8_SRGB] {
            for (exposure, expected) in [
                (0.0, [0, 0, 0, 255]),
                (0.125, [206, 115, 20, 255]),
                (1.0, [252, 232, 115, 255]),
                (4.0, [255, 252, 206, 255]),
                (f32::MAX, [255, 255, 255, 255]),
            ] {
                settings.exposure = exposure;
                renderer.set_post_processing(settings)?;
                let pixels = frame_format(&mut renderer, &gpu, &descriptors, &mut cache, format)?;
                assert!(
                    pixels
                        .chunks_exact(4)
                        .all(|pixel| pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1)),
                    "exposure {exposure}, format {format:?}: {:?}, expected {expected:?}",
                    &pixels[..4]
                );
            }
        }

        settings.exposure = 0.125;
        renderer.set_post_processing(settings)?;
        let reference = frame(&mut renderer, &gpu, &descriptors, &mut cache)?;
        let mut lighting = renderer.lighting_settings();
        lighting.sky_intensity = 8.0;
        renderer.set_lighting(lighting)?;
        settings.exposure /= 8.0;
        renderer.set_post_processing(settings)?;
        assert_eq!(
            frame(&mut renderer, &gpu, &descriptors, &mut cache)?,
            reference
        );
        let saved = renderer.post_processing_settings();
        for exposure in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            settings.exposure = exposure;
            assert!(renderer.set_post_processing(settings).is_err());
            assert_eq!(renderer.post_processing_settings().exposure, saved.exposure);
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
