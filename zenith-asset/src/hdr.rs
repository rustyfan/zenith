use crate::{
    AssetError, ErrorKind, ImportContext, Importer, Result,
    texture::{Texture, TextureCompression, TextureFormat, downsample, pad_surface},
};
use glam::Vec3;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HdrSettings {
    pub face_size: Option<u32>,
    pub mipmaps: bool,
    pub compression: TextureCompression,
}
impl Default for HdrSettings {
    fn default() -> Self {
        Self {
            face_size: None,
            mipmaps: true,
            compression: TextureCompression::Block,
        }
    }
}
pub struct HdrImporter;
impl Importer for HdrImporter {
    type Settings = HdrSettings;
    type Output = Texture;
    const KEY: &'static str = "zenith.hdr-cubemap";
    const VERSION: u32 = 2;
    fn extensions(&self) -> &[&str] {
        &["hdr"]
    }
    fn import(
        &self,
        bytes: &[u8],
        settings: &HdrSettings,
        _: &mut ImportContext<'_>,
    ) -> Result<Texture> {
        let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Hdr)
            .map_err(|e| AssetError::caused_by(ErrorKind::Import, "decode HDR", e))?
            .to_rgb32f();
        let (width, height) = image.dimensions();
        let face_size = settings
            .face_size
            .unwrap_or_else(|| width.min(height).clamp(1, 2048).next_power_of_two());
        if width == 0 || height == 0 || !face_size.is_power_of_two() || face_size > 2048 {
            return Err(AssetError::new(
                ErrorKind::Import,
                "HDR face size must be a power of two in 1..=2048",
            ));
        }
        let pixels: Vec<[f32; 3]> = image.pixels().map(|p| p.0).collect();
        drop(image);
        if pixels.iter().flatten().any(|v| {
            !v.is_finite()
                || (settings.compression == TextureCompression::Block && *v < 0.0)
                || v.abs() > 65504.0
        }) {
            return Err(AssetError::new(
                ErrorKind::Import,
                "HDR sample is outside the selected HDR format range",
            ));
        }
        let mut faces: Vec<Vec<[f32; 3]>> = (0..6)
            .into_par_iter()
            .map(|face| {
                let mut output = Vec::with_capacity((face_size * face_size) as usize);
                for y in 0..face_size {
                    for x in 0..face_size {
                        let u = (x as f32 + 0.5) / face_size as f32 * 2.0 - 1.0;
                        let v = (y as f32 + 0.5) / face_size as f32 * 2.0 - 1.0;
                        let direction = match face {
                            0 => Vec3::new(1.0, -v, -u),
                            1 => Vec3::new(-1.0, -v, u),
                            2 => Vec3::new(u, 1.0, v),
                            3 => Vec3::new(u, -1.0, -v),
                            4 => Vec3::new(u, -v, 1.0),
                            _ => Vec3::new(-u, -v, -1.0),
                        }
                        .normalize();
                        let theta = direction.z.clamp(-1.0, 1.0).acos();
                        let phi = (-direction.x)
                            .atan2(-direction.y)
                            .rem_euclid(std::f32::consts::TAU);
                        output.push(sample(
                            &pixels,
                            width,
                            height,
                            phi / std::f32::consts::TAU,
                            theta / std::f32::consts::PI,
                        ));
                    }
                }
                output
            })
            .collect();
        drop(pixels);
        let mip_levels = if settings.mipmaps {
            face_size.ilog2() + 1
        } else {
            1
        };
        let format = if settings.compression == TextureCompression::None {
            TextureFormat::Rgba16Float
        } else {
            TextureFormat::Bc6hUfloat
        };
        let mut output = Vec::new();
        let mut size = face_size;
        for mip in 0..mip_levels {
            let compressed: Vec<_> = faces
                .par_iter()
                .map(|face| compress(face, size, format))
                .collect();
            for bytes in compressed {
                output.extend(bytes);
            }
            if mip + 1 < mip_levels {
                faces = faces
                    .into_par_iter()
                    .map(|face| downsample(&face, size, size))
                    .collect();
                size = (size / 2).max(1);
            }
        }
        let texture = Texture {
            width: face_size,
            height: face_size,
            format,
            pixels: output,
            is_cubemap: true,
            mip_levels,
        };
        texture.validate()?;
        Ok(texture)
    }
}
fn compress(pixels: &[[f32; 3]], size: u32, format: TextureFormat) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(pixels.len() * 8);
    for pixel in pixels {
        for &value in pixel {
            rgba.extend_from_slice(&half::f16::from_f32(value).to_bits().to_le_bytes());
        }
        rgba.extend_from_slice(&half::f16::ONE.to_bits().to_le_bytes());
    }
    if format == TextureFormat::Rgba16Float {
        return rgba;
    }
    let (padded, width, height) = pad_surface(&rgba, size, size, 8);
    let surface = ispc_texcomp::RgbaSurface {
        data: &padded,
        width,
        height,
        stride: width * 8,
    };
    let mut output = vec![0; format.data_size_in_bytes(size, size)];
    ispc_texcomp::bc6h::compress_blocks_into(
        &ispc_texcomp::bc6h::fast_settings(),
        &surface,
        &mut output,
    );
    output
}
fn sample(pixels: &[[f32; 3]], width: u32, height: u32, u: f32, v: f32) -> [f32; 3] {
    let x = u * width as f32 - 0.5;
    let y = (v * height as f32 - 0.5).clamp(0.0, height as f32 - 1.0);
    let x0 = (x.floor() as i64).rem_euclid(width as i64) as u32;
    let x1 = (x0 + 1) % width;
    let y0 = y.floor() as u32;
    let y1 = (y0 + 1).min(height - 1);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let at = |x, y| pixels[(y * width + x) as usize];
    let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    std::array::from_fn(|i| {
        (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy
    })
}
