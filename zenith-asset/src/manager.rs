use std::any::Any;
use std::path::PathBuf;
use anyhow::Result;
use zenith_core::log::info;
use zenith_core::{workspace_root};
use crate::gltf::{GltfLoader, GltfBaker};
use crate::hdr::{HdrLoader, RawHdrProcessor};
use crate::{AssetBaker, AssetLoadRequest, AssetType, AssetLoader, ASSET_REGISTRY, Asset, deserialize_asset, RawAssetType, serialize_asset};
use crate::material::Material;
use crate::mesh::{Mesh, Scene};
use crate::texture::Texture;

/// Managing the loading, registering of assets.
pub struct AssetRequestor {
    asset_dir: PathBuf,
    // content_dir: PathBuf,
}

/// Bump this when asset structs (Scene, Mesh, Material, Texture, AssetUrl) or bincode layout change
/// so existing baked caches are invalidated and re-baked.
const CACHE_VERSION: &str = "18797930-6c96-4c99-8b8d-dd74b85299a3";
const CACHE_VERSION_FILE: &str = ".cache_version";

#[derive(Debug, Clone, Copy)]
pub enum EngineDirectory {
    Content,
    Asset,
}

impl EngineDirectory {
    pub fn folder_name(&self) -> &str {
        match self {
            EngineDirectory::Content => "content",
            EngineDirectory::Asset => "asset",
        }
    }
}

impl AssetRequestor {
    pub fn new() -> Self {
        let root = workspace_root();
        Self {
            asset_dir: root.to_owned().join("asset/"),
            // content_dir: root.join("content/"),
        }
    }

    /// Send a load request to the asset manager.
    /// Loading will complete synchronously before returning.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use zenith_asset::manager::AssetRequestor;
    /// let manager = AssetRequestor::new();
    /// let request = zenith_asset::AssetLoadRequest::new("mesh/cerberus/scene.scene")
    ///     .with_source("mesh/cerberus/scene.gltf");
    /// manager.request_load(request).expect("Failed to load asset");
    /// ```
    #[profiling::function]
    pub fn request_load(&self, request: AssetLoadRequest) -> Result<()> {
        if self.should_bake_asset(&request) {
            self.request_load_raw(request)?;
        } else {
            self.request_load_asset(request)?;
        }

        Ok(())
    }

    #[profiling::function]
    fn should_bake_asset(&self, request: &AssetLoadRequest) -> bool {
        let Some(_) = request.raw_asset_path() else {
            return false; // load from cache only (e.g. dependency)
        };
        let raw_path = request.absolute_raw_asset_path();

        if self.is_engine_cache_version_dirty() {
            return true;
        }

        let cached_file_path = request.absolute_asset_path();

        // if no cache had been found, rebake
        if !cached_file_path.exists() {
            return true;
        }

        let asset_metadata = match std::fs::metadata(cached_file_path) {
            Ok(metadata) => metadata,
            Err(_) => return false,
        };

        let source_metadata = match std::fs::metadata(raw_path) {
            Ok(metadata) => metadata,
            Err(_) => return false,
        };

        let asset_last_modified_time = match asset_metadata.modified() {
            Ok(time) => time,
            Err(_) => return false,
        };

        let raw_last_modified_time = match source_metadata.modified() {
            Ok(time) => time,
            Err(_) => return false,
        };

        // if the raw asset had been modified, rebake
        raw_last_modified_time > asset_last_modified_time
    }

    fn is_engine_cache_version_dirty(&self) -> bool {
        let version_path = self.asset_dir.join(CACHE_VERSION_FILE);
        let contents = match std::fs::read_to_string(version_path) {
            Ok(contents) => contents,
            Err(_) => return false,
        };
        contents.trim() != CACHE_VERSION
    }

    fn write_cache_version(&self) -> Result<()> {
        std::fs::create_dir_all(&self.asset_dir)?;
        let version_path = self.asset_dir.join(CACHE_VERSION_FILE);
        std::fs::write(version_path, CACHE_VERSION)?;
        Ok(())
    }

    #[profiling::function]
    fn request_load_raw(&self, request: AssetLoadRequest) -> Result<()> {
        // TODO: make it close for modification
        let assets = match request.raw_asset_type() {
            RawAssetType::Gltf => {
                let raw = GltfLoader::load(&request.absolute_raw_asset_path())?;
                GltfBaker::bake(raw, request.url.clone())?
            }
            RawAssetType::Hdr => {
                let raw = HdrLoader::load(&request.absolute_raw_asset_path())?;
                RawHdrProcessor::bake(raw, request.url.clone())?
            }
            _ => anyhow::bail!("Unsupported asset format: {:?}", request.url),
        };

        for asset in assets {
            let ty = asset.url().asset_type();
            let asset = asset as Box<dyn Any>;

            // TODO: make it close for modification
            match ty {
                AssetType::Mesh => {
                    let asset = *asset.downcast::<Mesh>().unwrap();
                    serialize_asset(&asset)?;
                    Self::register_asset(asset);
                },
                AssetType::Texture => {
                    let asset = *asset.downcast::<Texture>().unwrap();
                    serialize_asset(&asset)?;
                    Self::register_asset(asset);
                },
                AssetType::Material => {
                    let asset = *asset.downcast::<Material>().unwrap();
                    serialize_asset(&asset)?;
                    Self::register_asset(asset);
                },
                AssetType::Scene => {
                    let asset = *asset.downcast::<Scene>().unwrap();
                    serialize_asset(&asset)?;
                    Self::register_asset(asset);
                },
            }

        }

        self.write_cache_version()?;
        info!("Asset {:?} baked successfully.", request.raw_asset_path().unwrap_or(&request.url.path));
        Ok(())
    }

    #[profiling::function]
    fn request_load_asset(&self, request: AssetLoadRequest) -> Result<()> {
        let asset_type = request.url.asset_type();
        let asset_path = request.absolute_asset_path();
        info!("Loading baked asset: {:?}", asset_path);

        // TODO: load dependencies
        // TODO: notice a 1-to-1 mapping between AssetType and static asset type, further abstract the deserialize logic
        if asset_type == AssetType::Scene {
            let mut asset: Scene = deserialize_asset(&asset_path)?;
            asset.url = request.url.clone();

            for mesh_url in &asset.meshes {
                self.request_load_asset(AssetLoadRequest::new(mesh_url.clone()))?;
            }

            Self::register_asset(asset);

            return Ok(());
        }

        match asset_type {
            AssetType::Mesh => {
                let mut asset: Mesh = deserialize_asset(&asset_path)?;
                asset.url = request.url.clone();
                if let Some(mat) = &asset.material {
                    self.request_load_asset(AssetLoadRequest::new(mat.clone()))?;
                }
                Self::register_asset(asset);
            }
            AssetType::Texture => {
                let mut asset: Texture = deserialize_asset(&asset_path)?;
                asset.url = request.url.clone();
                Self::register_asset(asset);
            }
            AssetType::Material => {
                let mut asset: Material = deserialize_asset(&asset_path)?;
                asset.url = request.url.clone();

                let tex_urls = [
                    asset.base_color_tex.clone(),
                    asset.mra_tex.clone(),
                    asset.normal_tex.clone(),
                    asset.emissive_tex.clone(),
                ];
                for url in tex_urls.into_iter().flatten() {
                    self.request_load_asset(AssetLoadRequest::new(url))?;
                }

                Self::register_asset(asset);
            }
            _ => unreachable!()
        }

        Ok(())
    }

    fn register_asset<A: Asset>(asset: A) {
        ASSET_REGISTRY
            .get()
            .unwrap()
            .register(asset);
    }

}
