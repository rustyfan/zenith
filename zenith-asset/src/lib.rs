use std::any::{Any, TypeId};
use std::fs::File;
use std::io::{Cursor, Write};
use std::marker::PhantomData;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use anyhow::{anyhow, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use zenith_core::collections::hashmap::HashMap;
use zenith_core::file::load_with_memory_mapping;
use zenith_core::{log, workspace_root};
use crate::manager::EngineDirectory;

pub mod mesh;
pub mod manager;
pub mod gltf;
pub mod hdr;
pub mod texture;
pub mod material;

#[cfg(test)]
mod tests;

const ZSTD_MAGIC: &[u8; 5] = b"ZSTD1";
const ZSTD_GUID: &str = "7f9c2e2f-9b9b-4c51-9b65-2f7a6c3e0b2d";
const ZSTD_LEVEL: i32 = 3;

static ASSET_REGISTRY: OnceLock<AssetRegistry> = OnceLock::new();

pub fn initialize() -> Result<()> {
    ASSET_REGISTRY.set(AssetRegistry::new()).map_err(|_| anyhow!("Failed to initialize asset registry!"))
}

type AssetId = (AssetUrl, TypeId);
type AssetMap = HashMap<AssetId, Arc<dyn Asset>>;

#[derive(Default)]
pub struct AssetRegistry {
    assets_map: RwLock<AssetMap>,
}

unsafe impl Send for AssetRegistry {}
unsafe impl Sync for AssetRegistry {}

impl AssetRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an asset.
    pub fn register<A: Asset>(&self, asset: A) {
        let key = (asset.url().clone(), TypeId::of::<A>());
        self.assets_map.write().insert(key, Arc::new(asset));
    }

    /// Unregister an asset, return true if this asset was exists.
    pub fn unregister<A: Asset>(&self, url: impl Into<AssetUrl>) -> bool {
        let key = (url.into(), TypeId::of::<A>());
        self.assets_map.write().remove(&key).is_some()
    }

    /// Get an asset by url. Return None is this asset had NOT been loaded.
    fn get<A: Asset>(&self, url: &AssetUrl) -> Option<AssetRef<'_, A>> {
        if url.asset_type() != extension_asset_type(A::extension()) {
            log::warn!("Mismatch asset type. Try to get {:?} with an asset type of {:?}.", url, extension_asset_type(A::extension()));
            return None;
        }

        let assets = self.assets_map.read();
        let key = (url.clone(), TypeId::of::<A>());

        assets.get(&key)
            .map(Arc::clone)
            .map(AssetRef::new)
    }
}

/// Engine asset type.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetType {
    Mesh,
    Texture,
    Material,
    Scene,
}

// TODO: write the extension once
fn asset_type_extension(ty: AssetType) -> &'static str {
    match ty {
        AssetType::Mesh => "mesh",
        AssetType::Texture => "tex",
        AssetType::Material => "mat",
        AssetType::Scene => "scene",
    }
}

fn extension_asset_type(extension: &str) -> AssetType {
    match extension {
        "mesh" => AssetType::Mesh,
        "tex" => AssetType::Texture,
        "mat" => AssetType::Material,
        "scene" => AssetType::Scene,
        _ => unreachable!()
    }
}

fn extension_raw_asset_type(extension: &str) -> RawAssetType {
    match extension {
        "gltf" => RawAssetType::Gltf,
        "hdr" => RawAssetType::Hdr,
        _ => unreachable!()
    }
}

impl AssetType {
    pub fn extension(&self) -> &str {
        asset_type_extension(*self)
    }
}

/// Url to unique identify an asset.
/// This is a relative path start with words, points to a file located inside content/ folder.
///
/// # Example
///
/// ```
/// use zenith_asset::AssetUrl;
/// use std::path::PathBuf;
/// let asset_url: AssetUrl = PathBuf::from("mesh/cerberus/scene.mesh").into();
/// ```
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetUrl {
    path: PathBuf,
}

impl From<&str> for AssetUrl {
    fn from(value: &str) -> Self { Self { path: value.into() } }
}

impl From<PathBuf> for AssetUrl {
    fn from(path: PathBuf) -> Self { Self { path } }
}

impl Default for AssetUrl {
    fn default() -> Self {
        Self::invalid()
    }
}

impl AssetUrl {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        AssetUrl { path: path.into() }
    }

    /// Return an invalid url represents nothing.
    pub fn invalid() -> Self {
        Self {
            path: Default::default(),
        }
    }

    /// Return the asset type this AssetUrl points to.
    pub fn asset_type(&self) -> AssetType {
        let extension = self
            .path
            .extension()
            .and_then(|os_str| os_str.to_str())
            .unwrap_or("unknown");
        extension_asset_type(&extension.to_ascii_lowercase())
    }

    pub fn absolute_path(&self) -> PathBuf {
        workspace_root()
            .join(EngineDirectory::Asset.folder_name())
            .join(&self.path)
    }
}

impl AsRef<Path> for AssetUrl {
    fn as_ref(&self) -> &Path {
        self.path.as_path()
    }
}

/// Asset handle represents a loaded and registered asset.
pub struct AssetHandle<A> {
    url: AssetUrl,
    _marker: PhantomData<A>,
}

impl<A: Asset> Default for AssetHandle<A> {
    fn default() -> Self {
        Self::null()
    }
}

impl<A: Asset> AssetHandle<A> {
    /// Return a null asset handle which points to nothing.
    pub fn null() -> Self {
        Self {
            url: AssetUrl::invalid(),
            _marker: PhantomData,
        }
    }

    /// Create a new asset handle using AssetUrl.
    pub fn new(url: AssetUrl) -> Self {
        Self {
            url,
            _marker: PhantomData,
        }
    }

    #[inline]
    pub fn url(&self) -> &AssetUrl {
        &self.url
    }

    /// Get the underlying asset data if this asset is successfully loaded and registered.
    pub fn get(&self) -> Option<AssetRef<'_, A>> {
        ASSET_REGISTRY.get().unwrap().get(&self.url)
    }
}

pub struct AssetRef<'a, A> {
    asset: Arc<dyn Asset>,
    _marker: PhantomData<&'a A>,
}

impl<'a, A: Asset> AssetRef<'a, A> {
    fn new(asset: Arc<dyn Asset>) -> Self {
        Self {
            asset,
            _marker: PhantomData,
        }
    }
}

impl<'a, A: Asset> Deref for AssetRef<'a, A> {
    type Target = A;

    fn deref(&self) -> &Self::Target {
        unsafe {
            // Safety: asset type is checked by TypeId in AssetRegistry when calling get()
            self.asset.as_ref().as_any().downcast_ref::<A>().unwrap_unchecked()
        }
    }
}

/// Asset is any type of data which can be serialized and deserialized.
/// Asset should be read-only which is thread-safe.
///
/// Raw data is stored at content/ folder.
/// The baked asset which had been turned into engine representation is stored at cache/ folder.
pub trait Asset: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn url(&self) -> &AssetUrl;
    fn extension() -> &'static str where Self: Sized;

}

// TODO: this is NOT extensible, make it able to add any raw asset type
#[derive(Debug, Clone, Copy)]
pub enum RawAssetType {
    Gltf,
    Hdr,
    Png,
}

/// Type represents a raw resource.
pub trait RawAsset: Sized {
    fn raw_asset_type(&self) -> RawAssetType;
}

/// Raw resource loader interface.
/// AssetLoader is responds for transforming absolute_path into RawAsset
pub trait AssetLoader {
    type Raw: RawAsset;

    fn load(absolute_path: &Path) -> Result<Self::Raw>;
}

/// Raw resource baker interface.
/// AssetLoader is responds for transforming RawAsset into Asset
pub trait AssetBaker {
    type Raw: RawAsset;

    fn bake(raw: Self::Raw, url: AssetUrl) -> Result<Vec<Box<dyn Asset>>>;
}

/// Data needed to send an asset load request.
#[derive(Clone, Debug)]
pub struct AssetLoadRequest {
    /// Relative path in content folder (None when loading a dependency from cache only).
    raw_asset_path: Option<PathBuf>,
    /// Relative path in asset folder
    url: AssetUrl,
}

impl AssetLoadRequest {
    pub fn new(url: impl Into<AssetUrl>) -> Self {
        Self { url: url.into(), raw_asset_path: None }
    }

    pub fn with_source(mut self, path: impl Into<PathBuf>) -> Self {
        self.raw_asset_path = Some(path.into());
        self
    }

    fn absolute_raw_asset_path(&self) -> PathBuf {
        workspace_root()
            .join(EngineDirectory::Content.folder_name())
            .join(self.raw_asset_path.as_ref().expect("raw_asset_path required for bake"))
    }

    fn absolute_asset_path(&self) -> PathBuf {
        workspace_root()
            .join(EngineDirectory::Asset.folder_name())
            .join(&self.url.path)
    }

    #[inline]
    pub fn raw_asset_path(&self) -> Option<&PathBuf> {
        self.raw_asset_path.as_ref()
    }

    #[inline]
    pub fn asset_type(&self) -> AssetType {
        self.url.asset_type()
    }

    #[inline]
    pub fn raw_asset_type(&self) -> RawAssetType {
        let path = self.raw_asset_path.as_ref().expect("raw_asset_path required for bake");
        if let Some(extension) = path.extension() {
            extension_raw_asset_type(extension.to_ascii_lowercase().to_str().unwrap())
        } else {
            panic!("Unsupported raw asset type!")
        }
    }
}

const BINCODE_CONFIG: bincode::config::Configuration = bincode::config::standard();

fn serialize_asset<A: Asset + Serialize>(asset: &A) -> Result<()> {
    let absolute_path = asset.url().absolute_path();
    if let Some(parent) = absolute_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let encoded_data = bincode::serde::encode_to_vec(asset, BINCODE_CONFIG)?;
    let compressed = zstd::stream::encode_all(Cursor::new(&encoded_data), ZSTD_LEVEL)?;

    let mut file = File::create(absolute_path)?;
    file.write_all(ZSTD_MAGIC)?;
    file.write_all(ZSTD_GUID.as_bytes())?;
    file.write_all(&compressed)?;
    file.flush()?;

    Ok(())
}

fn deserialize_asset<A: Asset + DeserializeOwned>(absolute_path: &Path) -> Result<A> {
    // TODO: load differently by asset type
    let absolute_path = absolute_path.canonicalize()?;
    let bytes = match absolute_path.extension().and_then(|ext| ext.to_str()) {
        Some("scene") | Some("mat") => std::fs::read(&absolute_path)?,
        _ => load_with_memory_mapping(&absolute_path)?.to_vec(),
    };

    let header_len = ZSTD_MAGIC.len() + ZSTD_GUID.len();
    if bytes.len() < header_len
        || &bytes[..ZSTD_MAGIC.len()] != ZSTD_MAGIC
        || &bytes[ZSTD_MAGIC.len()..header_len] != ZSTD_GUID.as_bytes()
    {
        anyhow::bail!("Unsupported asset format: {:?}", absolute_path);
    }

    let compressed = &bytes[header_len..];
    let decompressed = zstd::stream::decode_all(Cursor::new(compressed))?;
    let (asset, _) = bincode::serde::decode_from_slice::<A, _>(&decompressed, BINCODE_CONFIG)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize asset {:?}: {}", absolute_path, e))?;
    Ok(asset)
}
