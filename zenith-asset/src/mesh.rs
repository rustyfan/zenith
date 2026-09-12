use std::any::Any;
use bytemuck::{NoUninit, Pod, Zeroable};
use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};
use super::{Asset, AssetUrl};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, Serialize, Deserialize)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub tex_coord: [f32; 2],
}

impl Vertex {
    pub fn new(position: Vec3, normal: Vec3, tex_coord: Vec2) -> Self {
        Self {
            position: position.to_array(),
            normal: normal.to_array(),
            tex_coord: tex_coord.to_array(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mesh<V = Vertex> {
    #[serde(skip)]
    pub url: AssetUrl,
    pub vertices: Vec<V>,
    pub indices: Vec<u32>,
    pub material: Option<AssetUrl>,
}

impl<V: NoUninit> Mesh<V> {
    pub fn new(url: AssetUrl, vertices: Vec<V>, indices: Vec<u32>, material: Option<AssetUrl>) -> Self {
        Self {
            url,
            vertices,
            indices,
            material,
        }
    }
    
    pub fn vertices_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.vertices)
    }

    pub fn indices_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.indices)
    }
}

impl<V: 'static + Send + Sync + NoUninit> Asset for Mesh<V> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    #[inline(always)]
    fn url(&self) -> &AssetUrl { &self.url }

    fn extension() -> &'static str {
        "mesh"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    #[serde(skip)]
    pub url: AssetUrl,
    // pub raw_asset_path: PathBuf,
    pub meshes: Vec<AssetUrl>,
    // pub materials: Vec<AssetUrl>,
}

impl Asset for Scene {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn url(&self) -> &AssetUrl { &self.url }

    fn extension() -> &'static str {
        "scene"
    }
}

impl Scene {
    pub fn new(url: AssetUrl) -> Self {
        Self {
            url,
            // raw_asset_path: raw_asset_path.as_ref().into(),
            meshes: vec![],
            // materials: vec![],
        }
    }

    pub fn add_mesh(&mut self, mesh_url: AssetUrl) {
        self.meshes.push(mesh_url);
        // self.materials.push(mat_url);
    }

    /// Iterate mesh/material pairs. This is fallible because the two lists may be mismatched.
    pub fn iter(&self) -> impl Iterator<Item = &AssetUrl> {
        // if self.meshes.len() != self.materials.len() {
        //     anyhow::bail!(
        //         "MeshCollection meshes/materials length mismatch ({} vs {})",
        //         self.meshes.len(),
        //         self.materials.len()
        //     );
        // }

        self.meshes.iter()
    }

    // // "mesh/cerberus/scene.gltf" -> "mesh/cerberus/scene.mscl"
    // pub fn asset_url(&self) -> AssetUrl {
    //     let mut baked_asset_path = self.raw_asset_path.clone();
    //     baked_asset_path.set_extension(Self::extension());
    //     baked_asset_path.into()
    // }
}