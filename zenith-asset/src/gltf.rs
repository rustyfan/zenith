use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use gltf::{buffer::Data as BufferData, image::Data as ImageData, Document, Primitive};
use ispc_texcomp::{RgbaSurface, bc5, bc7};
use zenith_core::file::load_with_memory_mapping;
use zenith_core::log;
use zenith_core::log::info;
use crate::mesh::{Mesh, Scene, Vertex};
use crate::{Asset, AssetBaker, RawAsset, AssetLoader, AssetUrl, RawAssetType};
use crate::material::Material;
use crate::texture::{Texture, TextureFormat};

#[derive(Debug, Clone)]
pub struct GltfLoader;

impl GltfLoader {
    pub fn new() -> Self {
        Self
    }
}

pub struct RawGltf {
    gltf: gltf::Gltf,
    buffers: Vec<BufferData>,
    images: Vec<ImageData>,
}

#[derive(Clone, Copy, Default)]
struct TextureUsageMask(u8);

impl TextureUsageMask {
    const BASE_COLOR: u8 = 1 << 0;
    const MRA: u8 = 1 << 1;
    const NORMAL: u8 = 1 << 2;
    const EMISSIVE: u8 = 1 << 3;

    fn add_base_color(&mut self) {
        self.0 |= Self::BASE_COLOR;
    }

    fn add_mra(&mut self) {
        self.0 |= Self::MRA;
    }

    fn add_normal(&mut self) {
        self.0 |= Self::NORMAL;
    }

    fn add_emissive(&mut self) {
        self.0 |= Self::EMISSIVE;
    }

    fn has(&self, flag: u8) -> bool {
        (self.0 & flag) != 0
    }
}

impl RawAsset for RawGltf {
    #[inline(always)]
    fn raw_asset_type(&self) -> RawAssetType {
        RawAssetType::Gltf
    }
}

impl AssetLoader for GltfLoader {
    type Raw = RawGltf;

    #[profiling::function]
    fn load(absolute_path: &Path) -> Result<Self::Raw> {
        let mmap = load_with_memory_mapping(absolute_path)?;

        let gltf = gltf::Gltf::from_slice(&mmap)
            .map_err(|e| anyhow!("Failed to parse GLTF: {}", e))?;

        let mut raw = RawGltf {
            gltf,
            buffers: vec![],
            images: vec![],
        };
        
        Self::load_gltf(absolute_path, &mut raw)?;

        Ok(raw)
    }
}

pub struct GltfBaker;

impl GltfBaker {
    pub fn new() -> Self {
        Self
    }
}

impl GltfBaker {
    #[profiling::function]
    fn process_node(
        node: &gltf::Node,
        buffers: &[BufferData],
        materials: &[Material],
        parent: &Path,
        stem: &String,
        base_idx: usize,
    ) -> Result<Vec<Mesh>> {
        let mut assets = vec![];

        if let Some(mesh) = node.mesh() {
            for (idx, primitive) in mesh.primitives().enumerate() {
                let mesh = Self::bake_mesh(&primitive, buffers, materials, parent, stem, base_idx + idx)?;
                assets.push(mesh);
            }
        }

        for child in node.children() {
            let meshes = Self::process_node(&child, buffers, materials, parent, stem, assets.len())?;
            assets.extend(meshes.into_iter());
        }

        Ok(assets)
    }

    #[profiling::function]
    fn bake_mesh(
        primitive: &Primitive,
        buffers: &[BufferData],
        materials: &[Material],
        parent: &Path,
        stem: &String,
        idx: usize,
    ) -> Result<Mesh> {
        let reader = primitive.reader(|buffer| Some(&*buffers[buffer.index()]));

        let positions = reader
            .read_positions()
            .ok_or(anyhow!("Missing positions"))?
            .collect::<Vec<_>>();

        let normals = if let Some(normals) = reader.read_normals() {
            normals.collect::<Vec<_>>()
        } else {
            // Generate flat normals if missing
            Self::generate_flat_normals(&positions)?
        };

        let tex_coords = if let Some(tex_coords) = reader.read_tex_coords(0) {
            tex_coords.into_f32().collect::<Vec<_>>()
        } else {
            // Generate default UV coordinates
            vec![[0.0, 0.0]; positions.len()]
        };

        let indices = reader
            .read_indices()
            .ok_or(anyhow!("Missing indices"))?
            .into_u32()
            .collect::<Vec<_>>();

        if positions.len() != normals.len() || positions.len() != tex_coords.len() {
            return Err(anyhow!("Vertex attribute count mismatch"));
        }

        let vertices: Vec<Vertex> = positions
            .into_iter()
            .zip(normals.into_iter())
            .zip(tex_coords.into_iter())
            .map(|((pos, norm), uv)| {
                Vertex::new(
                    glam::Vec3::from_array(pos),
                    glam::Vec3::from_array(norm),
                    glam::Vec2::from_array(uv),
                )
            })
            .collect();

        let material_idx = primitive.material().index();

        let mesh = Mesh::new(
            Self::resource_url::<Mesh>(parent, stem, idx),
            vertices,
            indices,
            materials.get(material_idx.unwrap_or(0)).map(|mat| mat.url.clone()),
        );

        Ok(mesh)
    }

    #[profiling::function]
    fn generate_flat_normals(positions: &Vec<[f32; 3]>) -> Result<Vec<[f32; 3]>> {
        if positions.len() % 3 != 0 {
            return Err(anyhow!("Position count must be divisible by 3 for flat normals"));
        }

        let mut normals = vec![[0.0, 0.0, 0.0]; positions.len()];

        for i in (0..positions.len()).step_by(3) {
            let v0 = glam::Vec3::from_array(positions[i]);
            let v1 = glam::Vec3::from_array(positions[i + 1]);
            let v2 = glam::Vec3::from_array(positions[i + 2]);

            let normal = (v1 - v0).cross(v2 - v0).normalize();

            normals[i] = normal.to_array();
            normals[i + 1] = normal.to_array();
            normals[i + 2] = normal.to_array();
        }

        Ok(normals)
    }

    #[profiling::function]
    fn bake_materials(gltf: &Document, parent: &Path, stem: &String, texture_assets: &[Texture]) -> Result<Vec<Material>> {
        let mut materials = Vec::new();

        for(idx, material) in gltf.materials().enumerate() {
            let pbr = material.pbr_metallic_roughness();

            let mut baked = Material {
                base_color: pbr.base_color_factor(),
                metallic: pbr.metallic_factor(),
                roughness: pbr.roughness_factor(),
                emissive: material.emissive_factor(),
                url: Self::resource_url::<Material>(&parent, &stem, idx),
                ..Material::default()
            };

            if let Some(texture) = pbr.base_color_texture() {
                let image_index = texture.texture().source().index();
                if let Some(tex) = texture_assets.get(image_index) {
                    baked.base_color_tex = Some(tex.url().clone());
                }
            }

            if let Some(texture) = pbr.metallic_roughness_texture() {
                let image_index = texture.texture().source().index();
                if let Some(tex) = texture_assets.get(image_index) {
                    baked.mra_tex = Some(tex.url().clone());
                }
            }

            if let Some(texture) = material.normal_texture() {
                let image_index = texture.texture().source().index();
                if let Some(tex) = texture_assets.get(image_index) {
                    baked.normal_tex = Some(tex.url().clone());
                }
            }

            // if let Some(texture) = material.occlusion_texture() {
            //     let image_index = texture.texture().source().index();
            //     if let Some(image_data) = images.get(image_index) {
            //         pbr_material.textures.occlusion = Some(TextureData {
            //             pixels: image_data.pixels.clone(),
            //             width: image_data.width,
            //             height: image_data.height,
            //             format: image_data.format,
            //         });
            //     }
            // }

            if let Some(texture) = material.emissive_texture() {
                let image_index = texture.texture().source().index();
                if let Some(tex) = texture_assets.get(image_index) {
                    baked.emissive_tex = Some(tex.url().clone());
                }
            }

            materials.push(baked);
        }

        Ok(materials)
    }

    fn collect_texture_usages(gltf: &Document, image_count: usize) -> Vec<TextureUsageMask> {
        let mut usages = vec![TextureUsageMask::default(); image_count];

        for material in gltf.materials() {
            let pbr = material.pbr_metallic_roughness();
            if let Some(texture) = pbr.base_color_texture() {
                let image_index = texture.texture().source().index();
                if let Some(slot) = usages.get_mut(image_index) {
                    slot.add_base_color();
                }
            }
            if let Some(texture) = pbr.metallic_roughness_texture() {
                let image_index = texture.texture().source().index();
                if let Some(slot) = usages.get_mut(image_index) {
                    slot.add_mra();
                }
            }
            if let Some(texture) = material.normal_texture() {
                let image_index = texture.texture().source().index();
                if let Some(slot) = usages.get_mut(image_index) {
                    slot.add_normal();
                }
            }
            if let Some(texture) = material.emissive_texture() {
                let image_index = texture.texture().source().index();
                if let Some(slot) = usages.get_mut(image_index) {
                    slot.add_emissive();
                }
            }
        }

        usages
    }

    fn split_asset_path(asset_url: &AssetUrl, fallback: &str) -> (PathBuf, String) {
        let main = Path::new(asset_url.as_ref());
        let parent = main.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        let stem = main
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(fallback)
            .to_owned();
        (parent, stem)
    }

    fn rgb8_to_rgba(pixels: &[u8]) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(pixels.len() * 4 / 3);
        for chunk in pixels.chunks(3) {
            rgba.extend_from_slice(chunk);
            rgba.push(255);
        }
        rgba
    }

    #[profiling::function]
    fn create_texture_from_gltf_image(image_data: &ImageData, usage: TextureUsageMask, parent: &Path, stem: &String, idx: usize) -> Result<Texture> {
        if let Some((pixels, format)) = Self::compress_texture_if_possible(image_data, usage) {
            return Ok(Texture {
                url: Self::resource_url::<Texture>(&parent, &stem, idx),
                width: image_data.width,
                height: image_data.height,
                format,
                pixels,
                is_cubemap: false,
                mip_levels: 1,
            });
        }

        // Fallback: store uncompressed pixels.
        let (pixels, texture_format) = Self::convert_gltf_pixels(image_data);
        log::warn!("Uncompressed glTF image: format[{:?}]", texture_format);

        Ok(Texture {
            url: Self::resource_url::<Texture>(&parent, &stem, idx),
            width: image_data.width,
            height: image_data.height,
            format: texture_format,
            pixels,
            is_cubemap: false,
            mip_levels: 1,
        })
    }

    fn compress_texture_if_possible(image_data: &ImageData, usage: TextureUsageMask) -> Option<(Vec<u8>, TextureFormat)> {
        if !Self::is_8bit_format(image_data.format) {
            return None;
        }

        if usage.has(TextureUsageMask::NORMAL) {
            let rg_pixels = Self::rg8_from_gltf(image_data)?;
            let rgba_pixels = Self::rg8_to_rgba(&rg_pixels);
            let compressed = Self::compress_bc5(&rgba_pixels, image_data.width, image_data.height);
            return Some((compressed, TextureFormat::Bc5Unorm));
        }

        if usage.has(TextureUsageMask::BASE_COLOR) || usage.has(TextureUsageMask::EMISSIVE) {
            let rgba_pixels = Self::rgba8_from_gltf(image_data)?;
            let compressed = Self::compress_bc7(&rgba_pixels, image_data.width, image_data.height, true);
            return Some((compressed, TextureFormat::Bc7Srgb));
        }

        if usage.has(TextureUsageMask::MRA) {
            let rgba_pixels = Self::rgba8_from_gltf(image_data)?;
            let compressed = Self::compress_bc7(&rgba_pixels, image_data.width, image_data.height, false);
            return Some((compressed, TextureFormat::Bc7Unorm));
        }

        None
    }

    fn is_8bit_format(format: gltf::image::Format) -> bool {
        matches!(
            format,
            gltf::image::Format::R8
                | gltf::image::Format::R8G8
                | gltf::image::Format::R8G8B8
                | gltf::image::Format::R8G8B8A8
        )
    }

    fn rgba8_from_gltf(data: &ImageData) -> Option<Vec<u8>> {
        match data.format {
            gltf::image::Format::R8G8B8A8 => Some(data.pixels.clone()),
            gltf::image::Format::R8G8B8 => {
                Some(Self::rgb8_to_rgba(&data.pixels))
            }
            gltf::image::Format::R8G8 => {
                let mut rgba = Vec::with_capacity(data.pixels.len() * 2);
                for chunk in data.pixels.chunks(2) {
                    rgba.push(chunk[0]);
                    rgba.push(chunk[1]);
                    rgba.push(0);
                    rgba.push(255);
                }
                Some(rgba)
            }
            gltf::image::Format::R8 => {
                let mut rgba = Vec::with_capacity(data.pixels.len() * 4);
                for value in data.pixels.iter().copied() {
                    rgba.push(value);
                    rgba.push(0);
                    rgba.push(0);
                    rgba.push(255);
                }
                Some(rgba)
            }
            _ => None,
        }
    }

    fn rg8_from_gltf(data: &ImageData) -> Option<Vec<u8>> {
        match data.format {
            gltf::image::Format::R8G8 => Some(data.pixels.clone()),
            gltf::image::Format::R8G8B8 => {
                let mut rg = Vec::with_capacity(data.pixels.len() * 2 / 3);
                for chunk in data.pixels.chunks(3) {
                    rg.push(chunk[0]);
                    rg.push(chunk[1]);
                }
                Some(rg)
            }
            gltf::image::Format::R8G8B8A8 => {
                let mut rg = Vec::with_capacity(data.pixels.len() / 2);
                for chunk in data.pixels.chunks(4) {
                    rg.push(chunk[0]);
                    rg.push(chunk[1]);
                }
                Some(rg)
            }
            gltf::image::Format::R8 => {
                let mut rg = Vec::with_capacity(data.pixels.len() * 2);
                for value in data.pixels.iter().copied() {
                    rg.push(value);
                    rg.push(0);
                }
                Some(rg)
            }
            _ => None,
        }
    }

    fn rg8_to_rgba(pixels: &[u8]) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(pixels.len() * 2);
        for chunk in pixels.chunks(2) {
            rgba.push(chunk[0]);
            rgba.push(chunk[1]);
            rgba.push(0);
            rgba.push(255);
        }
        rgba
    }

    fn compress_bc7(pixels: &[u8], width: u32, height: u32, use_alpha: bool) -> Vec<u8> {
        let output_size = bc7::calc_output_size(width, height);
        let mut output = vec![0u8; output_size];
        let surface = RgbaSurface {
            data: pixels,
            width,
            height,
            stride: width * 4,
        };
        let settings = if use_alpha {
            bc7::alpha_fast_settings()
        } else {
            bc7::opaque_fast_settings()
        };
        bc7::compress_blocks_into(&settings, &surface, &mut output);
        output
    }

    fn compress_bc5(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
        let output_size = bc5::calc_output_size(width, height);
        let mut output = vec![0u8; output_size];
        let surface = RgbaSurface {
            data: pixels,
            width,
            height,
            stride: width * 4,
        };
        bc5::compress_blocks_into(&surface, &mut output);
        output
    }

    #[profiling::function]
    fn convert_gltf_pixels(data: &ImageData) -> (Vec<u8>, TextureFormat) {
        match data.format {
            gltf::image::Format::R8G8B8 => {
                // Convert RGB to RGBA
                (Self::rgb8_to_rgba(&data.pixels), TextureFormat::R8G8B8A8)
            }
            gltf::image::Format::R16G16B16 => {
                // Convert RGB16 to RGBA16
                let mut rgba_data = Vec::with_capacity(data.pixels.len() * 8 / 6);
                for chunk in data.pixels.chunks(6) { // 6 bytes = 3 * 16-bit values
                    rgba_data.extend_from_slice(chunk);
                    rgba_data.extend_from_slice(&[255, 255]); // Add alpha = 1.0 (16-bit)
                }
                (rgba_data, TextureFormat::R16G16B16A16)
            }
            gltf::image::Format::R32G32B32FLOAT => {
                // Convert RGB32F to RGBA32F
                let mut rgba_data = Vec::with_capacity(data.pixels.len() * 16 / 12);
                for chunk in data.pixels.chunks(12) { // 12 bytes = 3 * 32-bit floats
                    rgba_data.extend_from_slice(chunk);
                    rgba_data.extend_from_slice(&1.0f32.to_le_bytes()); // Add alpha = 1.0 (32-bit float)
                }
                (rgba_data, TextureFormat::R32G32B32A32Float)
            }
            gltf::image::Format::R8 => {
                (data.pixels.clone(), TextureFormat::R8)
            }
            gltf::image::Format::R8G8 => {
                (data.pixels.clone(), TextureFormat::R8G8)
            }
            gltf::image::Format::R8G8B8A8 => {
                (data.pixels.clone(), TextureFormat::R8G8B8A8)
            }
            gltf::image::Format::R16 => {
                (data.pixels.clone(), TextureFormat::R16)
            }
            gltf::image::Format::R16G16 => {
                (data.pixels.clone(), TextureFormat::R16G16)
            }
            gltf::image::Format::R16G16B16A16 => {
                (data.pixels.clone(), TextureFormat::R16G16B16A16)
            }
            gltf::image::Format::R32G32B32A32FLOAT => {
                (data.pixels.clone(), TextureFormat::R32G32B32A32Float)
            }
        }
    }

    fn resource_url<A: Asset>(parent: &Path, stem: &String, idx: usize) -> AssetUrl {
        let mut url = parent.join(format!("{stem}_{idx}"));
        url.set_extension(A::extension());
        AssetUrl::new(url)
    }
}

impl AssetBaker for GltfBaker {
    type Raw = RawGltf;

    #[profiling::function]
    fn bake(raw: Self::Raw, url: AssetUrl) -> Result<Vec<Box<dyn Asset>>> {
        let RawGltf {
            gltf,
            buffers,
            images,
            ..
        } = raw;

        let (parent, stem) = Self::split_asset_path(&url, "mesh");

        let texture_usage_by_image = Self::collect_texture_usages(&gltf, images.len());
        let textures = images.iter()
            .enumerate()
            .map(|(idx, img)| {
                let usage = texture_usage_by_image.get(idx).copied().ok_or(anyhow!("Unknown texture usage!"))?;
                Ok(Self::create_texture_from_gltf_image(img, usage, &parent, &stem, idx)?)
            })
            .collect::<Result<Vec<_>>>()?;

        let materials = Self::bake_materials(&gltf, &parent, &stem, &textures)?;

        let mut meshes = vec![];
        for scene in gltf.scenes() {
            for node in scene.nodes() {
                meshes.extend(Self::process_node(&node, &buffers, &materials, &parent, &stem, 0)?.into_iter());
            }
        }

        let mut scene = Scene::new(url.clone());
        for mesh in &meshes {
            scene.add_mesh(mesh.url.clone());
        }

        info!("[{:?}] is loaded and serialized. ({} meshes, {} materials, {} textures)", &url, meshes.len(), materials.len(), textures.len());

        let assets = textures.into_iter().map(|asset| Box::new(asset) as Box<dyn Asset>)
            .chain(materials.into_iter().map(|asset| Box::new(asset) as Box<dyn Asset>))
            .chain(meshes.into_iter().map(|asset| Box::new(asset) as Box<dyn Asset>))
            .chain(std::iter::once(scene).map(|asset| Box::new(asset) as Box<dyn Asset>))
            .collect();

        Ok(assets)
    }
}

impl GltfLoader {
    #[profiling::function]
    fn load_gltf<P: AsRef<Path>>(path: P, raw: &mut RawGltf) -> Result<()> {
        let base_dir = path.as_ref().parent().ok_or(anyhow!("Invalid gltf load path."))?;

        let buffer_count = raw.gltf.buffers().len();
        let image_count = raw.gltf.images().len();

        raw.buffers.clear();
        raw.buffers.reserve(buffer_count);

        for buffer in raw.gltf.buffers() {
            match buffer.source() {
                gltf::buffer::Source::Uri(uri) => {
                    if uri.starts_with("data:") {
                        info!("inspecting gltf buffer uri: {:?}", uri);

                        let mut blob = None;
                        let data = BufferData::from_source_and_blob(buffer.source(), None, &mut blob)
                            .map_err(|e| anyhow!("Failed to decode data URI: {}", e))?;
                        
                        raw.buffers.push(data);
                    } else {
                        info!("inspecting gltf buffer uri: {:?}", uri);

                        let buffer_path = base_dir.join(uri);
                        let mmap = load_with_memory_mapping(&buffer_path)?;

                        raw.buffers.push(BufferData(mmap[..].to_vec()));
                    }
                }
                gltf::buffer::Source::Bin => {
                    return Err(anyhow!("Unexpected binary chunk in .gltf file"));
                }
            }
        }

        raw.images.clear();
        raw.images.reserve(image_count);

        for image in raw.gltf.images() {
            match image.source() {
                gltf::image::Source::Uri { uri, .. } => {
                    if uri.starts_with("data:") {
                        info!("inspecting gltf image uri: {:?}", uri);

                        let data = ImageData::from_source(image.source(), None, &raw.buffers)
                            .map_err(|e| anyhow!("Failed to decode image data uri: {}", e))?;
                        
                        raw.images.push(data);
                    } else {
                        info!("inspecting gltf image uri: {:?}", uri);

                        let image_path = base_dir.join(uri);
                        let uri = uri.to_owned();
                        let mmap = load_with_memory_mapping(&image_path)?;

                        raw.images.push(Self::decode_image(&mmap, &uri).expect("Failed to decode gltf image"));
                    }
                }
                gltf::image::Source::View { .. } => {
                    let data = ImageData::from_source(image.source(), None, &raw.buffers)
                        .map_err(|e| anyhow!("Failed to decode embedded image: {}", e))?;
                    
                    raw.images.push(data);
                }
            }
        }

        Ok(())
    }

    #[profiling::function]
    fn decode_image(data: &[u8], filename: &str) -> Result<ImageData> {
        // Fast path: try to guess format from magic bytes first (no file extension parsing)
        let format = image::guess_format(data).unwrap_or_else(|_| {
            // Fallback: use file extension
            let extension = Path::new(filename)
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
                .to_lowercase();
                
            match extension.as_str() {
                "png" => image::ImageFormat::Png,
                "jpg" | "jpeg" => image::ImageFormat::Jpeg,
                "bmp" => image::ImageFormat::Bmp,
                "tga" => image::ImageFormat::Tga,
                "tiff" | "tif" => image::ImageFormat::Tiff,
                "webp" => image::ImageFormat::WebP,
                _ => image::ImageFormat::Png, // Default fallback
            }
        });
        
        // Use optimized image loading with format hint
        let img = image::load_from_memory_with_format(data, format)
            .map_err(|e| anyhow!("Failed to decode image {}: {}", filename, e))?;
            
        // Convert to RGBA8 efficiently
        let rgba_img = match img {
            image::DynamicImage::ImageRgba8(rgba) => rgba,
            other => other.into_rgba8(),
        };
        
        let (width, height) = rgba_img.dimensions();
        
        Ok(ImageData {
            pixels: rgba_img.into_raw(),
            format: gltf::image::Format::R8G8B8A8,
            width,
            height,
        })
    }
}
