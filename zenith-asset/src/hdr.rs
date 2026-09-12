use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use glam::Vec3;
use ispc_texcomp::{RgbaSurface};
use rayon::prelude::*;
use zenith_core::log;
use crate::{Asset, AssetBaker, RawAsset, AssetLoader, AssetUrl, RawAssetType};
use crate::texture::{Texture, TextureFormat};

#[derive(Debug, Clone)]
pub struct HdrLoader;

impl HdrLoader {
    pub fn new() -> Self {
        Self
    }
}

/// Raw HDR data container (equirectangular RGB32F)
pub struct RawHdr {
    width: u32,
    height: u32,
    pixels: Vec<f32>, // RGB32F data (3 floats per pixel)
}

impl RawAsset for RawHdr {
    #[inline(always)]
    fn raw_asset_type(&self) -> RawAssetType {
        RawAssetType::Hdr
    }
}

impl AssetLoader for HdrLoader {
    type Raw = RawHdr;

    #[profiling::function]
    fn load(path: &Path) -> Result<Self::Raw> {
        let img = image::open(path)?;
        let width = img.width();
        let height = img.height();

        // Convert to RGB32F
        let rgb_img = img.to_rgb32f();
        let pixels_flat: Vec<f32> = rgb_img.into_raw();

        log::info!("Loaded HDR image: {}x{} ({} pixels)", width, height, pixels_flat.len() / 3);

        Ok(RawHdr {
            width,
            height,
            pixels: pixels_flat,
        })
    }
}

pub struct RawHdrProcessor;

impl RawHdrProcessor {
    pub fn new() -> Self {
        Self
    }

    /// Convert equirectangular HDR to 6 cubemap faces (parallelized with rayon)
    #[profiling::function]
    fn equirect_to_cubemap(
        equirect_pixels: &[f32],
        equirect_width: u32,
        equirect_height: u32,
        face_size: u32,
    ) -> [Vec<f32>; 6] {
        let pixels_per_face = (face_size * face_size * 3) as usize;
        let mut faces: [Vec<f32>; 6] = std::array::from_fn(|_| vec![0.0f32; pixels_per_face]);

        // Process all 6 faces in parallel
        faces.par_iter_mut().enumerate().for_each(|(face_idx, face)| {
            // Process rows within each face in parallel
            face.par_chunks_mut((face_size * 3) as usize)
                .enumerate()
                .for_each(|(y, row)| {
                    for x in 0..face_size {
                        let u = (x as f32 + 0.5) / face_size as f32 * 2.0 - 1.0;
                        let v = (y as f32 + 0.5) / face_size as f32 * 2.0 - 1.0;

                        let dir = Self::cubemap_direction(face_idx, u, v);
                        let (theta, phi) = Self::direction_to_spherical(&dir);
                        let equirect_u = phi / (2.0 * std::f32::consts::PI);
                        let equirect_v = theta / std::f32::consts::PI;

                        let color = Self::sample_equirect(
                            equirect_pixels,
                            equirect_width,
                            equirect_height,
                            equirect_u,
                            equirect_v,
                        );

                        let pixel_idx = (x * 3) as usize;
                        row[pixel_idx] = color[0];
                        row[pixel_idx + 1] = color[1];
                        row[pixel_idx + 2] = color[2];
                    }
                });
        });

        faces
    }

    /// Standard Vulkan cubemap face directions (Vulkan spec Section 16.5.4).
    /// Right-hand Z-up coordinate system: X=right, Y=forward, Z=up.
    fn cubemap_direction(face: usize, u: f32, v: f32) -> Vec3 {
        let dir = match face {
            0 => Vec3::new( 1.0,  -v,  -u),  // +X (right)
            1 => Vec3::new(-1.0,  -v,   u),  // -X (left)
            2 => Vec3::new(   u,  1.0,   v), // +Y (forward)
            3 => Vec3::new(   u, -1.0,  -v), // -Y (backward)
            4 => Vec3::new(   u,  -v,  1.0), // +Z (up)
            5 => Vec3::new(  -u,  -v, -1.0), // -Z (down)
            _ => unreachable!(),
        };

        dir.normalize()
    }

    /// Convert 3D direction to spherical coordinates for equirectangular sampling.
    /// Z-up: polar angle from +Z, azimuthal in XY plane.
    /// atan2(-x, -y) places +Y (forward) at phi=PI => equirect_u=0.5 (image center).
    fn direction_to_spherical(dir: &Vec3) -> (f32, f32) {
        let theta = dir.z.clamp(-1.0, 1.0).acos(); // [0, PI]
        let phi = (-dir.x).atan2(-dir.y); // [-PI, PI]
        let phi = if phi < 0.0 { phi + 2.0 * std::f32::consts::PI } else { phi };
        (theta, phi)
    }

    fn sample_equirect(
        pixels: &[f32],
        width: u32,
        height: u32,
        u: f32,
        v: f32,
    ) -> [f32; 3] {
        // Bilinear interpolation
        let x = u * (width - 1) as f32;
        let y = v * (height - 1) as f32;

        let x0 = x.floor() as u32;
        let y0 = y.floor() as u32;
        let x1 = (x0 + 1).min(width - 1);
        let y1 = (y0 + 1).min(height - 1);

        let fx = x - x0 as f32;
        let fy = y - y0 as f32;

        let get_pixel = |px: u32, py: u32| {
            let idx = ((py * width + px) * 3) as usize;
            [pixels[idx], pixels[idx + 1], pixels[idx + 2]]
        };

        let c00 = get_pixel(x0, y0);
        let c10 = get_pixel(x1, y0);
        let c01 = get_pixel(x0, y1);
        let c11 = get_pixel(x1, y1);

        // Bilinear blend
        let c0 = [
            c00[0] * (1.0 - fx) + c10[0] * fx,
            c00[1] * (1.0 - fx) + c10[1] * fx,
            c00[2] * (1.0 - fx) + c10[2] * fx,
        ];
        let c1 = [
            c01[0] * (1.0 - fx) + c11[0] * fx,
            c01[1] * (1.0 - fx) + c11[1] * fx,
            c01[2] * (1.0 - fx) + c11[2] * fx,
        ];

        [
            c0[0] * (1.0 - fy) + c1[0] * fy,
            c0[1] * (1.0 - fy) + c1[1] * fy,
            c0[2] * (1.0 - fy) + c1[2] * fy,
        ]
    }

    fn compress_bc6h(pixels: &[f32], width: u32, height: u32) -> Vec<u8> {
        // Convert RGB f32 to RGBA f16 (ispc_texcomp bc6h expects f16 input)
        let pixel_count = (width * height) as usize;
        let mut rgba_f16 = Vec::with_capacity(pixel_count * 4);
        for i in (0..pixels.len()).step_by(3) {
            rgba_f16.push(half::f16::from_f32(pixels[i]));
            rgba_f16.push(half::f16::from_f32(pixels[i + 1]));
            rgba_f16.push(half::f16::from_f32(pixels[i + 2]));
            rgba_f16.push(half::f16::from_f32(1.0));
        }

        let output_size = ispc_texcomp::bc6h::calc_output_size(width, height);
        let mut output = vec![0u8; output_size];
        let rgba_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                rgba_f16.as_ptr() as *const u8,
                rgba_f16.len() * size_of::<half::f16>(),
            )
        };
        let surface = RgbaSurface {
            data: rgba_bytes,
            width,
            height,
            stride: width * 4 * 2, // 4 channels * 2 bytes per f16
        };
        let settings = ispc_texcomp::bc6h::fast_settings();
        ispc_texcomp::bc6h::compress_blocks_into(&settings, &surface, &mut output);
        output
    }

    /// Downsample an RGB32F face by 2x using a box filter.
    fn downsample_2x(pixels: &[f32], width: u32, height: u32) -> Vec<f32> {
        let new_w = (width / 2).max(1);
        let new_h = (height / 2).max(1);
        let mut out = vec![0.0f32; (new_w * new_h * 3) as usize];

        for y in 0..new_h {
            for x in 0..new_w {
                let sx = (x * 2) as usize;
                let sy = (y * 2) as usize;
                let w = width as usize;

                let mut r = 0.0f32;
                let mut g = 0.0f32;
                let mut b = 0.0f32;
                let mut count = 0u32;

                for dy in 0..2u32 {
                    let py = sy + dy as usize;
                    if py >= height as usize { continue; }
                    for dx in 0..2u32 {
                        let px = sx + dx as usize;
                        if px >= width as usize { continue; }
                        let idx = (py * w + px) * 3;
                        r += pixels[idx];
                        g += pixels[idx + 1];
                        b += pixels[idx + 2];
                        count += 1;
                    }
                }

                let inv = 1.0 / count as f32;
                let dst = ((y * new_w + x) * 3) as usize;
                out[dst] = r * inv;
                out[dst + 1] = g * inv;
                out[dst + 2] = b * inv;
            }
        }

        out
    }

    fn split_asset_path(asset_url: &str, fallback: &str) -> (PathBuf, String) {
        let main = Path::new(asset_url);
        let parent = main.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        let stem = main
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(fallback)
            .to_owned();
        (parent, stem)
    }
}

impl AssetBaker for RawHdrProcessor {
    type Raw = RawHdr;

    #[profiling::function]
    fn bake(raw: Self::Raw, url: AssetUrl) -> Result<Vec<Box<dyn Asset>>> {
        let RawHdr {
            width,
            height,
            pixels,
            ..
        } = raw;

        let face_size = width.min(height).next_power_of_two();
        let mip_levels = (face_size as f32).log2() as u32 + 1;

        log::info!(
            "Converting {}x{} equirectangular HDR to {}x{} cubemap ({} mip levels)",
            width, height, face_size, face_size, mip_levels,
        );

        // Generate mip 0 faces (parallelized)
        let base_faces = Self::equirect_to_cubemap(&pixels, width, height, face_size);

        // Build mip chain for each face: faces_mips[face_idx][mip_level]
        let mut faces_mips: Vec<Vec<Vec<f32>>> = base_faces
            .into_iter()
            .map(|face| {
                let mut mips = Vec::with_capacity(mip_levels as usize);
                mips.push(face);
                mips
            })
            .collect();

        // Generate subsequent mip levels
        let mut mip_w = face_size;
        let mut mip_h = face_size;
        for _mip in 1..mip_levels {
            let prev_w = mip_w;
            let prev_h = mip_h;
            mip_w = (mip_w / 2).max(1);
            mip_h = (mip_h / 2).max(1);

            // Downsample all 6 faces for this mip level in parallel
            let new_mip_faces: Vec<Vec<f32>> = (0..6)
                .into_par_iter()
                .map(|face_idx| {
                    let prev = &faces_mips[face_idx].last().unwrap();
                    Self::downsample_2x(prev, prev_w, prev_h)
                })
                .collect();

            for (face_idx, mip_data) in new_mip_faces.into_iter().enumerate() {
                faces_mips[face_idx].push(mip_data);
            }
        }

        // Compress all face*mip combinations with BC6H (parallelized)
        // Layout: mip-major [mip0_all_faces, mip1_all_faces, ...]
        let mut total_pixels: Vec<u8> = Vec::new();
        let mut mip_w = face_size;
        let mut mip_h = face_size;

        for mip in 0..mip_levels {
            // Compress all 6 faces for this mip level in parallel
            let compressed: Vec<Vec<u8>> = (0..6usize)
                .into_par_iter()
                .map(|face_idx| {
                    Self::compress_bc6h(&faces_mips[face_idx][mip as usize], mip_w, mip_h)
                })
                .collect();

            for face_data in compressed {
                total_pixels.extend(face_data);
            }

            mip_w = (mip_w / 2).max(1);
            mip_h = (mip_h / 2).max(1);
        }

        log::info!("Total compressed cubemap size: {} bytes ({} mip levels)", total_pixels.len(), mip_levels);

        let asset_url_str = url.path.to_str().ok_or(anyhow!("Invalid asset url"))?;
        let (parent, stem) = Self::split_asset_path(asset_url_str, "cubemap");

        let mut cubemap_path = parent.join(format!("{}", stem));
        cubemap_path.set_extension(Texture::extension());
        let cubemap_url: AssetUrl = cubemap_path.into();

        let texture = Texture {
            url: cubemap_url.clone(),
            width: face_size,
            height: face_size,
            format: TextureFormat::Bc6hUfloat,
            pixels: total_pixels,
            is_cubemap: true,
            mip_levels,
        };

        log::info!("HDR cubemap baked: {:?} ({} mips)", cubemap_url, mip_levels);
        Ok(vec![Box::new(texture) as Box<dyn Asset>])
    }
}
