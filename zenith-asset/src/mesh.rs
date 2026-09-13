use crate::{
    AssetError, AssetPath, CookedAsset, ErrorKind, Handle, LoadContext, Result, material::Material,
};
use bytemuck::{NoUninit, Pod, Zeroable};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[cfg(feature = "importers")]
mod tangents;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, Serialize, Deserialize)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub tex_coord: [f32; 2],
    pub tangent: [f32; 4],
}
pub trait VertexLayout: NoUninit + Serialize + DeserializeOwned + Send + Sync + 'static {
    const MESH_TYPE_KEY: &'static str;
    const MESH_SCHEMA_VERSION: u32 = 2;
    fn valid(&self) -> bool {
        true
    }
}
impl VertexLayout for Vertex {
    const MESH_TYPE_KEY: &'static str = "zenith.mesh.position-normal-uv";
    const MESH_SCHEMA_VERSION: u32 = 3;
    fn valid(&self) -> bool {
        let n = glam::Vec3::from_array(self.normal);
        let t = glam::Vec3::new(self.tangent[0], self.tangent[1], self.tangent[2]);
        self.position
            .iter()
            .chain(&self.normal)
            .chain(&self.tex_coord)
            .chain(&self.tangent)
            .all(|v| v.is_finite())
            && n.length_squared() > 0.0
            && n.length_squared().is_finite()
            && (self.tangent[3] == 0.0
                || (self.tangent[3].abs() == 1.0
                    && n.cross(t).length_squared() > 0.0
                    && n.cross(t).length_squared().is_finite()))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mesh<V = Vertex> {
    pub vertices: Vec<V>,
    pub indices: Vec<u32>,
}
impl<V: NoUninit> Mesh<V> {
    pub fn new(vertices: Vec<V>, indices: Vec<u32>) -> Self {
        Self { vertices, indices }
    }
    pub fn vertices_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.vertices)
    }
    pub fn indices_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.indices)
    }
}
impl<V: VertexLayout> Mesh<V> {
    pub fn validate(&self) -> Result<()> {
        if self.vertices.is_empty()
            || self.indices.is_empty()
            || !self.indices.len().is_multiple_of(3)
            || self
                .indices
                .iter()
                .any(|&i| i as usize >= self.vertices.len())
            || self.vertices.iter().any(|v| !v.valid())
        {
            return Err(AssetError::new(
                ErrorKind::InvalidData,
                "invalid triangle mesh",
            ));
        }
        Ok(())
    }
}
impl<V: VertexLayout> CookedAsset for Mesh<V> {
    type Data = Self;
    const TYPE_KEY: &'static str = V::MESH_TYPE_KEY;
    const SCHEMA_VERSION: u32 = V::MESH_SCHEMA_VERSION;
    fn from_data(data: Self, _: &mut LoadContext<'_>) -> Result<Self> {
        data.validate()?;
        Ok(data)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneNode {
    pub source_index: usize,
    pub parent: Option<usize>,
    pub transform: [f32; 16],
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshInstanceData {
    pub node: usize,
    pub mesh: AssetPath<Mesh>,
    pub material: AssetPath<Material>,
    pub transform: [f32; 16],
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SceneData {
    pub nodes: Vec<SceneNode>,
    pub instances: Vec<MeshInstanceData>,
}
#[derive(Debug, Clone)]
pub struct MeshInstance {
    pub node: usize,
    pub mesh: Handle<Mesh>,
    pub material: Handle<Material>,
    pub transform: [f32; 16],
}
#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub nodes: Vec<SceneNode>,
    pub instances: Vec<MeshInstance>,
}
impl CookedAsset for Scene {
    type Data = SceneData;
    const TYPE_KEY: &'static str = "zenith.scene";
    const SCHEMA_VERSION: u32 = 2;
    fn from_data(data: SceneData, ctx: &mut LoadContext<'_>) -> Result<Self> {
        for (index, node) in data.nodes.iter().enumerate() {
            if node.parent.is_some_and(|p| p >= index)
                || node.transform.iter().any(|v| !v.is_finite())
            {
                return Err(AssetError::new(
                    ErrorKind::InvalidData,
                    "invalid scene hierarchy or transform",
                ));
            }
        }
        let mut instances = Vec::with_capacity(data.instances.len());
        for instance in data.instances {
            if instance.node >= data.nodes.len()
                || instance.transform.iter().any(|v| !v.is_finite())
            {
                return Err(AssetError::new(
                    ErrorKind::InvalidData,
                    "invalid mesh instance",
                ));
            }
            instances.push(MeshInstance {
                node: instance.node,
                mesh: ctx.dependency(&instance.mesh)?,
                material: ctx.dependency(&instance.material)?,
                transform: instance.transform,
            });
        }
        Ok(Self {
            nodes: data.nodes,
            instances,
        })
    }
}
