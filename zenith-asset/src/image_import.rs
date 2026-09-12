use crate::{
    AssetError, ErrorKind, ImportContext, Importer, Result,
    texture::{Texture, TextureCompression, TextureFormat, TextureSettings, TextureUsage},
};
pub struct ImageImporter;
impl Importer for ImageImporter {
    type Settings = TextureSettings;
    type Output = Texture;
    const KEY: &'static str = "zenith.image";
    const VERSION: u32 = 2;
    fn extensions(&self) -> &[&str] {
        #[cfg(feature = "extra-image-formats")]
        {
            &[
                "png", "jpg", "jpeg", "bmp", "tga", "tif", "tiff", "webp", "avif", "dds", "exr",
                "ff", "gif", "ico", "pbm", "pgm", "ppm", "pnm", "qoi",
            ]
        }
        #[cfg(not(feature = "extra-image-formats"))]
        {
            &["png", "jpg", "jpeg", "bmp", "tga", "tif", "tiff", "webp"]
        }
    }
    fn import(
        &self,
        bytes: &[u8],
        settings: &Self::Settings,
        ctx: &mut ImportContext<'_>,
    ) -> Result<Texture> {
        let extension = std::path::Path::new(ctx.address().path())
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        let format = image::ImageFormat::from_extension(extension).ok_or_else(|| {
            AssetError::new(
                ErrorKind::UnsupportedImporter,
                "unsupported image extension",
            )
        })?;
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        reader.limits(limits);
        let image = reader
            .decode()
            .map_err(|e| AssetError::caused_by(ErrorKind::Import, "decode image", e))?;
        bake_image(&image, settings)
    }
}
pub(crate) fn bake_image(
    image: &image::DynamicImage,
    settings: &TextureSettings,
) -> Result<Texture> {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(AssetError::new(
            ErrorKind::Import,
            "image dimensions must be in 1..=8192",
        ));
    }
    let sixteen = matches!(
        image.color(),
        image::ColorType::L16
            | image::ColorType::La16
            | image::ColorType::Rgb16
            | image::ColorType::Rgba16
    );
    let floating = matches!(
        image.color(),
        image::ColorType::Rgb32F | image::ColorType::Rgba32F
    );
    let srgb = settings.usage == TextureUsage::Color && !floating;
    let format = match settings.compression {
        TextureCompression::Block => match settings.usage {
            TextureUsage::Normal => TextureFormat::Bc5Unorm,
            TextureUsage::Color => TextureFormat::Bc7Srgb,
            TextureUsage::Linear => TextureFormat::Bc7Unorm,
        },
        TextureCompression::None if floating => TextureFormat::Rgba32Float,
        TextureCompression::None if sixteen => TextureFormat::Rgba16Unorm,
        TextureCompression::None if srgb => TextureFormat::Rgba8Srgb,
        TextureCompression::None => TextureFormat::Rgba8Unorm,
    };
    let mut pixels: Vec<[f32; 4]> = image.to_rgba32f().pixels().map(|p| p.0).collect();
    for pixel in &mut pixels {
        if pixel.iter().any(|v| !v.is_finite()) {
            return Err(AssetError::new(
                ErrorKind::Import,
                "non-finite image sample",
            ));
        }
        if floating
            && settings.compression == TextureCompression::Block
            && pixel.iter().any(|v| !(0.0..=1.0).contains(v))
        {
            return Err(AssetError::new(
                ErrorKind::Import,
                "floating image exceeds normalized BC formats; select uncompressed output",
            ));
        }
        if srgb {
            for value in &mut pixel[..3] {
                *value = srgb_to_linear(*value);
            }
        }
    }
    let mip_levels = if settings.mipmaps {
        width.max(height).ilog2() + 1
    } else {
        1
    };
    let mut bytes = Vec::new();
    let (mut w, mut h) = (width, height);
    for mip in 0..mip_levels {
        if settings.usage == TextureUsage::Normal {
            for pixel in &mut pixels {
                let normal = glam::Vec3::new(
                    pixel[0] * 2.0 - 1.0,
                    pixel[1] * 2.0 - 1.0,
                    pixel[2] * 2.0 - 1.0,
                )
                .try_normalize()
                .unwrap_or(glam::Vec3::Z);
                pixel[..3].copy_from_slice(&((normal + glam::Vec3::ONE) * 0.5).to_array());
            }
        }
        let mut encoded = Vec::new();
        for pixel in &pixels {
            for (channel, &value) in pixel.iter().enumerate() {
                let value = if channel < 3
                    && matches!(format, TextureFormat::Rgba8Srgb | TextureFormat::Bc7Srgb)
                {
                    linear_to_srgb(value)
                } else {
                    value
                };
                match format {
                    TextureFormat::Rgba32Float => encoded.extend_from_slice(&value.to_le_bytes()),
                    TextureFormat::Rgba16Unorm => encoded.extend_from_slice(
                        &((value.clamp(0.0, 1.0) * 65535.0).round() as u16).to_le_bytes(),
                    ),
                    _ => encoded.push((value.clamp(0.0, 1.0) * 255.0).round() as u8),
                }
            }
        }
        if format.is_block_compressed() {
            let (padded, pw, ph) = pad_surface(&encoded, w, h, 4);
            let surface = ispc_texcomp::RgbaSurface {
                data: &padded,
                width: pw,
                height: ph,
                stride: pw * 4,
            };
            let mut compressed = vec![0; format.data_size_in_bytes(w, h)];
            if format == TextureFormat::Bc5Unorm {
                ispc_texcomp::bc5::compress_blocks_into(&surface, &mut compressed);
            } else {
                ispc_texcomp::bc7::compress_blocks_into(
                    &ispc_texcomp::bc7::alpha_fast_settings(),
                    &surface,
                    &mut compressed,
                );
            }
            bytes.extend(compressed);
        } else {
            bytes.extend(encoded);
        }
        if mip + 1 < mip_levels {
            pixels = downsample(&pixels, w, h);
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
    }
    let texture = Texture {
        width,
        height,
        format,
        pixels: bytes,
        is_cubemap: false,
        mip_levels,
    };
    texture.validate()?;
    Ok(texture)
}
pub(crate) fn pad_surface(
    bytes: &[u8],
    width: u32,
    height: u32,
    stride: usize,
) -> (std::borrow::Cow<'_, [u8]>, u32, u32) {
    let (pw, ph) = (width.div_ceil(4) * 4, height.div_ceil(4) * 4);
    if width == pw && height == ph {
        return (std::borrow::Cow::Borrowed(bytes), pw, ph);
    }
    let mut padded = Vec::with_capacity(pw as usize * ph as usize * stride);
    for y in 0..ph {
        for x in 0..pw {
            let offset =
                (y.min(height - 1) as usize * width as usize + x.min(width - 1) as usize) * stride;
            padded.extend_from_slice(&bytes[offset..offset + stride]);
        }
    }
    (std::borrow::Cow::Owned(padded), pw, ph)
}
pub(crate) fn downsample<const N: usize>(
    pixels: &[[f32; N]],
    width: u32,
    height: u32,
) -> Vec<[f32; N]> {
    let (nw, nh) = ((width / 2).max(1), (height / 2).max(1));
    let mut output = Vec::with_capacity((nw * nh) as usize);
    for y in 0..nh {
        for x in 0..nw {
            let (x0, x1) = (x * width / nw, (x + 1) * width / nw);
            let (y0, y1) = (y * height / nh, (y + 1) * height / nh);
            let mut sum = [0.0; N];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let pixel = pixels[(sy * width + sx) as usize];
                    for c in 0..N {
                        sum[c] += pixel[c];
                    }
                }
            }
            for value in &mut sum {
                *value /= ((x1 - x0) * (y1 - y0)) as f32;
            }
            output.push(sum);
        }
    }
    output
}
fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}
fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}
