use crate::{
    texture::Texture, AssetError, AssetPath, CookedAsset, ErrorKind, Handle, LoadContext, Result,
};
use serde::{Deserialize, Serialize};

#[repr(u32)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShadingModel {
    #[default]
    Opaque = 0,
    ClearCoat = 1,
    Hair = 2,
}
impl ShadingModel {
    pub fn validate(self, coat: ClearCoat, hair: Option<Hair>) -> Result<()> {
        validate_layers(coat, hair)?;
        if (self == Self::Hair) != hair.is_some() || (self == Self::Opaque && coat.weight != 0.0) {
            return Err(AssetError::new(
                ErrorKind::InvalidData,
                "shading model does not match material parameters",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ClearCoat {
    pub weight: f32,
    pub roughness: f32,
    pub normal_scale: f32,
}
impl Default for ClearCoat {
    fn default() -> Self {
        Self {
            weight: 0.0,
            roughness: 0.1,
            normal_scale: 1.0,
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Hair {
    pub absorption: [f32; 3],
    pub longitudinal_roughness: f32,
    pub azimuthal_roughness: f32,
    pub cuticle_tilt: f32,
    pub ior: f32,
    pub opacity: f32,
}
impl Default for Hair {
    fn default() -> Self {
        Self {
            absorption: [0.4, 0.8, 1.5],
            longitudinal_roughness: 0.2,
            azimuthal_roughness: 0.3,
            cuticle_tilt: 0.035,
            ior: 1.55,
            opacity: 0.85,
        }
    }
}
pub fn validate_layers(coat: ClearCoat, hair: Option<Hair>) -> Result<()> {
    let unit = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
    let valid = unit(coat.weight)
        && unit(coat.roughness)
        && coat.normal_scale.is_finite()
        && coat.normal_scale >= 0.0
        && hair.is_none_or(|h| {
            h.absorption.iter().all(|v| v.is_finite() && *v >= 0.0)
                && unit(h.longitudinal_roughness)
                && h.longitudinal_roughness >= 0.03
                && unit(h.azimuthal_roughness)
                && h.azimuthal_roughness >= 0.03
                && h.cuticle_tilt.is_finite()
                && h.cuticle_tilt.abs() <= 0.2
                && h.ior.is_finite()
                && (1.01..=2.5).contains(&h.ior)
                && unit(h.opacity)
                && coat.weight == 0.0
        });
    if !valid {
        return Err(AssetError::new(
            ErrorKind::InvalidData,
            "invalid coat or hair parameters",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialData {
    pub shading_model: ShadingModel,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub base_color_tex: Option<AssetPath<Texture>>,
    pub mra_tex: Option<AssetPath<Texture>>,
    pub normal_tex: Option<AssetPath<Texture>>,
    pub emissive_tex: Option<AssetPath<Texture>>,
    pub clearcoat: ClearCoat,
    pub clearcoat_tex: Option<AssetPath<Texture>>,
    pub clearcoat_roughness_tex: Option<AssetPath<Texture>>,
    pub clearcoat_normal_tex: Option<AssetPath<Texture>>,
    pub hair: Option<Hair>,
}
impl Default for MaterialData {
    fn default() -> Self {
        Self {
            shading_model: ShadingModel::Opaque,
            base_color: [1.0; 4],
            metallic: 1.0,
            roughness: 1.0,
            emissive: [0.0; 3],
            base_color_tex: None,
            mra_tex: None,
            normal_tex: None,
            emissive_tex: None,
            clearcoat: ClearCoat::default(),
            clearcoat_tex: None,
            clearcoat_roughness_tex: None,
            clearcoat_normal_tex: None,
            hair: None,
        }
    }
}
#[derive(Debug, Clone)]
pub struct Material {
    pub shading_model: ShadingModel,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub base_color_tex: Option<Handle<Texture>>,
    pub mra_tex: Option<Handle<Texture>>,
    pub normal_tex: Option<Handle<Texture>>,
    pub emissive_tex: Option<Handle<Texture>>,
    pub clearcoat: ClearCoat,
    pub clearcoat_tex: Option<Handle<Texture>>,
    pub clearcoat_roughness_tex: Option<Handle<Texture>>,
    pub clearcoat_normal_tex: Option<Handle<Texture>>,
    pub hair: Option<Hair>,
}
impl CookedAsset for Material {
    type Data = MaterialData;
    const TYPE_KEY: &'static str = "zenith.material";
    const SCHEMA_VERSION: u32 = 4;
    fn from_data(data: Self::Data, ctx: &mut LoadContext<'_>) -> Result<Self> {
        data.shading_model.validate(data.clearcoat, data.hair)?;
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
            shading_model: data.shading_model,
            clearcoat: data.clearcoat,
            hair: data.hair,
            clearcoat_tex: data
                .clearcoat_tex
                .as_ref()
                .map(|p| ctx.dependency(p))
                .transpose()?,
            clearcoat_roughness_tex: data
                .clearcoat_roughness_tex
                .as_ref()
                .map(|p| ctx.dependency(p))
                .transpose()?,
            clearcoat_normal_tex: data
                .clearcoat_normal_tex
                .as_ref()
                .map(|p| ctx.dependency(p))
                .transpose()?,
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

impl Default for Material {
    fn default() -> Self {
        Self {
            shading_model: ShadingModel::Opaque,
            base_color: [1.0; 4],
            metallic: 1.0,
            roughness: 1.0,
            emissive: [0.0; 3],
            base_color_tex: None,
            mra_tex: None,
            normal_tex: None,
            emissive_tex: None,
            clearcoat: ClearCoat::default(),
            clearcoat_tex: None,
            clearcoat_roughness_tex: None,
            clearcoat_normal_tex: None,
            hair: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shading_models_validate_and_round_trip() {
        for (model, id) in [
            (ShadingModel::Opaque, 0),
            (ShadingModel::ClearCoat, 1),
            (ShadingModel::Hair, 2),
        ] {
            assert_eq!(model as u32, id);
            let data = MaterialData {
                shading_model: model,
                hair: (model == ShadingModel::Hair).then(Hair::default),
                ..Default::default()
            };
            assert!(model.validate(data.clearcoat, data.hair).is_ok());
            let bytes = bincode::serde::encode_to_vec(&data, bincode::config::standard()).unwrap();
            let (decoded, _): (MaterialData, usize) =
                bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
            assert_eq!(decoded.shading_model, model);
        }
        assert!(ShadingModel::Hair
            .validate(ClearCoat::default(), None)
            .is_err());
        assert!(ShadingModel::Opaque
            .validate(ClearCoat::default(), Some(Hair::default()))
            .is_err());
        assert!(ShadingModel::Opaque
            .validate(
                ClearCoat {
                    weight: 1.0,
                    ..Default::default()
                },
                None
            )
            .is_err());
    }

    #[test]
    fn layers_reject_invalid_parameters_and_round_trip() {
        assert!(validate_layers(ClearCoat::default(), Some(Hair::default())).is_ok());
        for value in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            assert!(validate_layers(
                ClearCoat {
                    weight: value,
                    ..Default::default()
                },
                None
            )
            .is_err());
        }
        assert!(validate_layers(
            ClearCoat {
                weight: 1.0,
                ..Default::default()
            },
            Some(Hair::default())
        )
        .is_err());
        assert!(validate_layers(
            ClearCoat::default(),
            Some(Hair {
                absorption: [-1.0; 3],
                ..Default::default()
            })
        )
        .is_err());
        assert!(validate_layers(
            ClearCoat::default(),
            Some(Hair {
                longitudinal_roughness: 0.0,
                ..Default::default()
            })
        )
        .is_err());
        let material = MaterialData {
            shading_model: ShadingModel::Hair,
            hair: Some(Hair::default()),
            ..Default::default()
        };
        let bytes = bincode::serde::encode_to_vec(&material, bincode::config::standard()).unwrap();
        let (decoded, _): (MaterialData, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert_eq!(
            decoded.hair.unwrap().absorption,
            material.hair.unwrap().absorption
        );
        assert_eq!(decoded.clearcoat.weight, 0.0);
    }
}
