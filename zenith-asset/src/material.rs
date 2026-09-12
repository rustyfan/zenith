use crate::{
    AssetError, AssetPath, CookedAsset, ErrorKind, Handle, LoadContext, Result, texture::Texture,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialData {
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub base_color_tex: Option<AssetPath<Texture>>,
    pub mra_tex: Option<AssetPath<Texture>>,
    pub normal_tex: Option<AssetPath<Texture>>,
    pub emissive_tex: Option<AssetPath<Texture>>,
}
impl Default for MaterialData {
    fn default() -> Self {
        Self {
            base_color: [1.0; 4],
            metallic: 1.0,
            roughness: 1.0,
            emissive: [0.0; 3],
            base_color_tex: None,
            mra_tex: None,
            normal_tex: None,
            emissive_tex: None,
        }
    }
}
#[derive(Debug, Clone)]
pub struct Material {
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub base_color_tex: Option<Handle<Texture>>,
    pub mra_tex: Option<Handle<Texture>>,
    pub normal_tex: Option<Handle<Texture>>,
    pub emissive_tex: Option<Handle<Texture>>,
}
impl CookedAsset for Material {
    type Data = MaterialData;
    const TYPE_KEY: &'static str = "zenith.material";
    const SCHEMA_VERSION: u32 = 2;
    fn from_data(data: Self::Data, ctx: &mut LoadContext<'_>) -> Result<Self> {
        if !data.metallic.is_finite()
            || !data.roughness.is_finite()
            || data
                .base_color
                .iter()
                .chain(&data.emissive)
                .any(|v| !v.is_finite())
        {
            return Err(AssetError::new(
                ErrorKind::InvalidData,
                "non-finite material parameters",
            ));
        }
        Ok(Self {
            base_color: data.base_color,
            metallic: data.metallic,
            roughness: data.roughness,
            emissive: data.emissive,
            base_color_tex: data
                .base_color_tex
                .as_ref()
                .map(|p| ctx.dependency(p))
                .transpose()?,
            mra_tex: data
                .mra_tex
                .as_ref()
                .map(|p| ctx.dependency(p))
                .transpose()?,
            normal_tex: data
                .normal_tex
                .as_ref()
                .map(|p| ctx.dependency(p))
                .transpose()?,
            emissive_tex: data
                .emissive_tex
                .as_ref()
                .map(|p| ctx.dependency(p))
                .transpose()?,
        })
    }
}
