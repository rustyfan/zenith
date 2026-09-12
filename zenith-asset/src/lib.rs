#![doc = include_str!("../README.md")]

use serde::{Serialize, de::DeserializeOwned};
use std::any::Any;

mod cache;
mod error;
#[cfg(feature = "importers")]
pub mod gltf;
mod handle;
#[cfg(feature = "importers")]
pub mod hdr;
#[cfg(feature = "importers")]
mod image_import;
mod import;
pub mod material;
pub mod mesh;
mod path;
mod server;
mod source;
pub mod texture;
mod texture_codec;

pub use error::{AssetError, ErrorKind, Result};
pub use handle::{AssetId, AssetSnapshot, CpuRetention, Handle, LoadState, Revision};
pub use import::{AssetCodec, ImportContext, Importer, SerdeCodec};
pub use path::{AssetAddress, AssetPath};
pub use server::{AssetEvent, AssetServer, AssetServerBuilder, AssetStats, LoadContext};
pub use source::{AssetSource, FileSource, MemorySource};

pub trait Asset: Any + Send + Sync {}
impl<T: Any + Send + Sync> Asset for T {}

pub trait CookedAsset: Asset + Sized {
    type Data: Serialize + DeserializeOwned + Send + Sync + 'static;
    const TYPE_KEY: &'static str;
    const SCHEMA_VERSION: u32;
    fn from_data(data: Self::Data, ctx: &mut LoadContext<'_>) -> Result<Self>;
}

pub(crate) type ErasedValue = std::sync::Arc<dyn Any + Send + Sync>;
#[cfg(test)]
mod tests;
