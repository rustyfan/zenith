use std::any::Any;
use serde::{Deserialize, Serialize};
use crate::{Asset, AssetUrl};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Material {
    #[serde(skip)]
    pub url: AssetUrl,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],

    // Texture assets referenced by URL (baked separately as `.tex`).
    pub base_color_tex: Option<AssetUrl>,
    pub mra_tex: Option<AssetUrl>,
    pub normal_tex: Option<AssetUrl>,
    pub emissive_tex: Option<AssetUrl>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            url: AssetUrl::default(),
            base_color: [1.0, 0.0, 1.0, 1.0],
            metallic: 1.0,
            roughness: 0.5,
            emissive: [0.0; 3],
            base_color_tex: None,
            mra_tex: None,
            normal_tex: None,
            emissive_tex: None,
        }
    }
}

impl Asset for Material {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn url(&self) -> &AssetUrl { &self.url }

    fn extension() -> &'static str {
        "mat"
    }
}
