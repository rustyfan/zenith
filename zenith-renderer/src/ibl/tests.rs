use super::*;
use crate::gpu_assets::GpuAssets;
use anyhow::Result;
use glam::Vec3;
use zenith_asset::{
    texture::{Texture as CpuTexture, TextureFormat},
    AssetServer,
};

mod energy;

fn create_ibl(
    gpu: &Arc<Gpu>,
    descriptors: &Arc<Descriptors>,
) -> Result<ImageBasedLightingRenderer> {
    let shaders = gpu.compile_shaders([
        (
            "content/shaders/ibl_diffuse.slang",
            "main",
            ShaderStage::Compute,
        ),
        (
            "content/shaders/ibl_specular.slang",
            "main",
            ShaderStage::Compute,
        ),
        (
            "content/shaders/brdf_lut.slang",
            "main",
            ShaderStage::Compute,
        ),
        (
            "content/shaders/brdf_average.slang",
            "main",
            ShaderStage::Compute,
        ),
    ])?;
    let sampler = descriptors.sampler(vk::Filter::LINEAR, vk::SamplerAddressMode::CLAMP_TO_EDGE)?;
    ImageBasedLightingRenderer::new(
        gpu,
        descriptors,
        sampler,
        [&shaders[0], &shaders[1], &shaders[2], &shaders[3]],
    )
}

fn read_image(
    gpu: &Arc<Gpu>,
    texture: &Arc<Texture>,
    mip: u32,
    channels: usize,
) -> Result<Vec<f32>> {
    let desc = texture.desc();
    let width = (desc.extent.width >> mip).max(1);
    let height = (desc.extent.height >> mip).max(1);
    let mut result =
        vec![0.0f32; width as usize * height as usize * desc.layers as usize * channels];
    let readback = gpu.allocate((result.len() * 4) as u64, MemoryDomain::Readback)?;
    let mut commands = gpu.commands()?;
    commands.barrier(Access::ALL, Access::COPY_READ)?;
    let region = vk::BufferImageCopy::default()
        .image_extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .mip_level(mip)
                .layer_count(desc.layers),
        );
    unsafe {
        commands.read_image(texture, &readback.whole(), &[region])?;
    }
    commands.barrier(Access::COPY_WRITE, Access::HOST_READ)?;
    commands.submit()?.wait(10_000_000_000)?;
    readback.read(0, bytemuck::cast_slice_mut(&mut result))?;
    Ok(result)
}

fn read_sh(gpu: &Arc<Gpu>, buffer: &Arc<Memory>) -> Result<Vec<f32>> {
    let readback = gpu.allocate(SH_BYTES, MemoryDomain::Readback)?;
    let mut commands = gpu.commands()?;
    commands.barrier(Access::ALL, Access::COPY_READ)?;
    commands.copy(&buffer.whole(), &readback.whole())?;
    commands.barrier(Access::COPY_WRITE, Access::HOST_READ)?;
    commands.submit()?.wait(10_000_000_000)?;
    let mut values = vec![0.0f32; SH_BYTES as usize / 4];
    readback.read(0, bytemuck::cast_slice_mut(&mut values))?;
    Ok(values)
}

fn environment(size: u32, radiance: impl Fn(Vec3) -> Vec3) -> CpuTexture {
    let mut pixels = Vec::new();
    for face in 0..6 {
        for y in 0..size {
            for x in 0..size {
                let u = 2.0 * (x as f32 + 0.5) / size as f32 - 1.0;
                let v = 2.0 * (y as f32 + 0.5) / size as f32 - 1.0;
                let direction = match face {
                    0 => Vec3::new(1.0, -v, -u),
                    1 => Vec3::new(-1.0, -v, u),
                    2 => Vec3::new(u, 1.0, v),
                    3 => Vec3::new(u, -1.0, -v),
                    4 => Vec3::new(u, -v, 1.0),
                    _ => Vec3::new(-u, -v, -1.0),
                }
                .normalize();
                pixels.extend_from_slice(bytemuck::cast_slice(
                    &radiance(direction).extend(1.0).to_array(),
                ));
            }
        }
    }
    CpuTexture {
        width: size,
        height: size,
        mip_levels: 1,
        is_cubemap: true,
        format: TextureFormat::Rgba32Float,
        pixels,
    }
}

#[test]
#[ignore = "requires Vulkan validation, Slang, and supported GPU features"]
fn sh_convolution_specular_prefilter_and_lut() -> Result<()> {
    std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
    let _ = zenith_core::log::initialize(zenith_core::log::LevelFilter::Info);
    let instance = Instance::new(&[], true)?;
    anyhow::ensure!(
        instance.validation_enabled(),
        "IBL test requires Vulkan validation"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    {
        let descriptors = Descriptors::new(&gpu, 512, 32)?;
        let mut ibl = create_ibl(&gpu, &descriptors)?;
        let lut = ibl.brdf_lut.view().texture().clone();
        assert_eq!(lut.desc().extent.width, 128);
        assert_eq!(lut.desc().extent.height, 128);
        let values = read_image(&gpu, &lut, 0, 2)?;
        assert!(values
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0 && *v <= 1.05));
        assert!((values[254] - 1.0).abs() < 0.01 && values[255] < 0.001);
        assert!((values[64] + values[65] - 1.0).abs() < 0.01);
        assert!(
            values[1] > values[0] && values[0] + values[1] > 0.5,
            "{:?}",
            &values[..2]
        );
        assert!(values[values.len() - 2] > 0.25 && values[values.len() - 2] < 0.4);
        energy::check_tables(
            &values,
            &read_image(&gpu, ibl.brdf_average.view().texture(), 0, 2)?,
        );

        let assets = AssetServer::builder().build()?;
        let mut uploads = GpuAssets::new(&gpu, &descriptors);
        let color = Vec3::new(0.25, 0.5, 1.0);
        let handle = assets.add(environment(1, |_| color));
        let mut upload = uploads.begin()?;
        let binding = uploads.texture(&mut upload, &handle)?;
        uploads.submit(upload)?.wait()?;
        ibl.set_skybox(binding.clone())?;
        let buffer = ibl.buffer.clone();
        let filtered = ibl.specular.clone();
        ibl.set_skybox(binding)?;
        assert!(Arc::ptr_eq(&buffer, &ibl.buffer));
        assert!(Arc::ptr_eq(&filtered, &ibl.specular));
        let sh = read_sh(&gpu, &buffer)?;
        for channel in 0..3 {
            for i in 0..4 {
                let expected = if i == 3 { color[channel] } else { 0.0 };
                assert!((sh[channel * 4 + i] - expected).abs() < 0.002);
            }
        }
        assert!(sh[12..].iter().all(|v| v.abs() < 0.002));
        let old_sh = sh;
        for mip in 0..filtered.view().texture().desc().mip_levels {
            let pixels = read_image(&gpu, filtered.view().texture(), mip, 4)?;
            for pixel in pixels.chunks_exact(4) {
                for c in 0..3 {
                    assert!((pixel[c] - color[c]).abs() < 0.002);
                }
            }
        }

        let handle = assets.add(environment(32, |n| {
            Vec3::ONE * (1.0 + 0.5 * n.z + 0.25 * (3.0 * n.z * n.z - 1.0))
        }));
        let mut upload = uploads.begin()?;
        let binding = uploads.texture(&mut upload, &handle)?;
        uploads.submit(upload)?.wait()?;
        ibl.set_skybox(binding)?;
        assert!(!Arc::ptr_eq(&buffer, &ibl.buffer));
        let sh = read_sh(&gpu, &ibl.buffer)?;
        for channel in 0..3 {
            assert!((sh[channel * 4 + 2] - 1.0 / 3.0).abs() < 0.004, "{sh:?}");
            assert!((sh[channel * 4 + 3] - 0.9375).abs() < 0.004, "{sh:?}");
            assert!((sh[12 + channel * 4 + 2] - 0.1875).abs() < 0.004, "{sh:?}");
        }
        let pixels = read_image(&gpu, ibl.specular.view().texture(), 0, 4)?;
        let face_stride = (SPECULAR_SIZE * SPECULAR_SIZE * 4) as usize;
        let center = ((SPECULAR_SIZE / 2 * SPECULAR_SIZE + SPECULAR_SIZE / 2) * 4) as usize;
        assert!((pixels[4 * face_stride + center] - 2.0).abs() < 0.01);
        assert!((pixels[5 * face_stride + center] - 1.0).abs() < 0.01);
        let rough = read_image(&gpu, ibl.specular.view().texture(), 7, 4)?;
        assert!(rough.iter().all(|v| v.is_finite()));
        assert!(rough[16] < pixels[4 * face_stride + center] - 0.2);
        assert_eq!(read_sh(&gpu, &buffer)?, old_sh);
    }
    gpu.wait_idle()?;
    drop(gpu);
    anyhow::ensure!(
        instance.validation_errors().is_empty(),
        "IBL validation errors: {:?}",
        instance.validation_errors()
    );
    Ok(())
}
