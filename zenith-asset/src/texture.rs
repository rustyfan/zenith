use std::any::Any;
use serde::{Deserialize, Serialize};
use crate::{Asset, AssetUrl};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TextureFormat {
    R8,
    R8G8,
    R8G8B8A8,
    R16,
    R16G16,
    R16G16B16A16,
    R32G32B32A32Float,
    Bc5Unorm,
    Bc7Unorm,
    Bc7Srgb,
    Bc6hUfloat,
    Bc6hSfloat,
}

impl TextureFormat {
    pub fn bytes_per_pixel(&self) -> u32 {
        match self {
            TextureFormat::R8 => 1,
            TextureFormat::R8G8 => 2,
            TextureFormat::R8G8B8A8 => 4,
            TextureFormat::R16 => 2,
            TextureFormat::R16G16 => 4,
            TextureFormat::R16G16B16A16 => 8,
            TextureFormat::R32G32B32A32Float => 16,
            TextureFormat::Bc5Unorm | TextureFormat::Bc7Unorm | TextureFormat::Bc7Srgb | TextureFormat::Bc6hUfloat | TextureFormat::Bc6hSfloat => 0,
        }
    }

    pub fn is_block_compressed(&self) -> bool {
        matches!(self, TextureFormat::Bc5Unorm | TextureFormat::Bc7Unorm | TextureFormat::Bc7Srgb | TextureFormat::Bc6hUfloat | TextureFormat::Bc6hSfloat)
    }

    pub fn block_dimensions(&self) -> (u32, u32) {
        match self {
            TextureFormat::Bc5Unorm | TextureFormat::Bc7Unorm | TextureFormat::Bc7Srgb | TextureFormat::Bc6hUfloat | TextureFormat::Bc6hSfloat => (4, 4),
            _ => (1, 1),
        }
    }

    pub fn bytes_per_block(&self) -> u32 {
        match self {
            TextureFormat::Bc5Unorm | TextureFormat::Bc7Unorm | TextureFormat::Bc7Srgb | TextureFormat::Bc6hUfloat | TextureFormat::Bc6hSfloat => 16,
            _ => self.bytes_per_pixel(),
        }
    }

    pub fn data_size_in_bytes(&self, width: u32, height: u32) -> usize {
        if self.is_block_compressed() {
            let (bw, bh) = self.block_dimensions();
            let blocks_x = (width + bw - 1) / bw;
            let blocks_y = (height + bh - 1) / bh;
            (blocks_x * blocks_y * self.bytes_per_block()) as usize
        } else {
            (width * height * self.bytes_per_pixel()) as usize
        }
    }

    pub fn mip_chain_size_bytes(&self, base_width: u32, base_height: u32, mip_levels: u32) -> usize {
        let mut total = 0usize;
        let mut w = base_width;
        let mut h = base_height;
        for _ in 0..mip_levels {
            total += self.data_size_in_bytes(w, h);
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        total
    }

    pub fn to_vk(&self) -> ash::vk::Format {
        match self {
            TextureFormat::R8 => ash::vk::Format::R8_UNORM,
            TextureFormat::R8G8 => ash::vk::Format::R8G8_UNORM,
            TextureFormat::R8G8B8A8 => ash::vk::Format::R8G8B8A8_SRGB,
            TextureFormat::R16 => ash::vk::Format::R16_SFLOAT,
            TextureFormat::R16G16 => ash::vk::Format::R16G16_SFLOAT,
            TextureFormat::R16G16B16A16 => ash::vk::Format::R16G16B16A16_SFLOAT,
            TextureFormat::R32G32B32A32Float => ash::vk::Format::R32G32B32A32_SFLOAT,
            TextureFormat::Bc5Unorm => ash::vk::Format::BC5_UNORM_BLOCK,
            TextureFormat::Bc7Unorm => ash::vk::Format::BC7_UNORM_BLOCK,
            TextureFormat::Bc7Srgb => ash::vk::Format::BC7_SRGB_BLOCK,
            TextureFormat::Bc6hUfloat => ash::vk::Format::BC6H_UFLOAT_BLOCK,
            TextureFormat::Bc6hSfloat => ash::vk::Format::BC6H_SFLOAT_BLOCK,
        }
    }

}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Texture {
    #[serde(skip)]
    pub url: AssetUrl,
    pub width: u32,
    pub height: u32,
    pub format: TextureFormat,
    pub pixels: Vec<u8>,
    pub is_cubemap: bool,
    pub mip_levels: u32,
}

impl Asset for Texture {
    fn as_any(&self) -> &dyn Any {
        self
    }

    #[inline(always)]
    fn url(&self) -> &AssetUrl { &self.url }

    fn extension() -> &'static str {
        "tex"
    }
}
