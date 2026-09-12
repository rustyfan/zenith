use crate::{AssetError, CookedAsset, ErrorKind, LoadContext, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureFormat {
    R8Unorm,
    Rg8Unorm,
    Rgba8Unorm,
    Rgba8Srgb,
    R16Unorm,
    Rg16Unorm,
    Rgba16Unorm,
    Rgba16Float,
    Rgba32Float,
    Bc5Unorm,
    Bc7Unorm,
    Bc7Srgb,
    Bc6hUfloat,
    Bc6hSfloat,
}
impl TextureFormat {
    pub fn is_block_compressed(self) -> bool {
        matches!(
            self,
            Self::Bc5Unorm | Self::Bc7Unorm | Self::Bc7Srgb | Self::Bc6hUfloat | Self::Bc6hSfloat
        )
    }
    pub fn bytes_per_block(self) -> usize {
        match self {
            Self::R8Unorm => 1,
            Self::Rg8Unorm | Self::R16Unorm => 2,
            Self::Rgba8Unorm | Self::Rgba8Srgb | Self::Rg16Unorm => 4,
            Self::Rgba16Unorm | Self::Rgba16Float => 8,
            _ => 16,
        }
    }
    pub fn data_size_in_bytes(self, width: u32, height: u32) -> usize {
        let block = if self.is_block_compressed() { 4 } else { 1 };
        width.div_ceil(block) as usize * height.div_ceil(block) as usize * self.bytes_per_block()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub format: TextureFormat,
    pub pixels: Vec<u8>,
    pub is_cubemap: bool,
    pub mip_levels: u32,
}
impl Texture {
    pub fn validate(&self) -> Result<()> {
        if self.width == 0
            || self.height == 0
            || self.width > 16384
            || self.height > 16384
            || (self.is_cubemap && self.width != self.height)
            || self.mip_levels == 0
            || self.mip_levels > self.width.max(self.height).ilog2() + 1
        {
            return Err(AssetError::new(
                ErrorKind::InvalidData,
                "invalid texture dimensions or mip count",
            ));
        }
        let expected: usize = (0..self.mip_levels)
            .map(|mip| {
                self.format
                    .data_size_in_bytes((self.width >> mip).max(1), (self.height >> mip).max(1))
            })
            .sum::<usize>()
            * if self.is_cubemap { 6 } else { 1 };
        if expected != self.pixels.len() {
            return Err(AssetError::new(
                ErrorKind::InvalidData,
                "texture byte count differs from its mip/layer footprint",
            ));
        }
        Ok(())
    }
}
impl CookedAsset for Texture {
    type Data = Self;
    const TYPE_KEY: &'static str = "zenith.texture";
    const SCHEMA_VERSION: u32 = 2;
    fn from_data(data: Self, _: &mut LoadContext<'_>) -> Result<Self> {
        data.validate()?;
        Ok(data)
    }
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TextureUsage {
    #[default]
    Color,
    Linear,
    Normal,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureCompression {
    None,
    #[default]
    Block,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureSettings {
    pub usage: TextureUsage,
    pub mipmaps: bool,
    pub compression: TextureCompression,
}
impl Default for TextureSettings {
    fn default() -> Self {
        Self {
            usage: TextureUsage::Color,
            mipmaps: true,
            compression: TextureCompression::Block,
        }
    }
}

#[cfg(feature = "importers")]
pub use crate::image_import::ImageImporter;
#[cfg(feature = "importers")]
pub(crate) use crate::image_import::{bake_image, downsample, pad_surface};
