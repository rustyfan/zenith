use super::*;
use crate::material::Material;
use crate::mesh::Mesh;
use crate::texture::{Texture, TextureFormat};

#[test]
fn existing_material_cache_keeps_serde_layout() -> Result<()> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/material-v1.mat");
    let material: Material = deserialize_asset(&path)?;
    assert_eq!(
        material.base_color_tex,
        Some(AssetUrl::from("mesh/cerberus/scene_0.tex"))
    );
    assert_eq!(
        material.mra_tex,
        Some(AssetUrl::from("mesh/cerberus/scene_1.tex"))
    );
    assert_eq!(
        material.normal_tex,
        Some(AssetUrl::from("mesh/cerberus/scene_2.tex"))
    );
    let bytes = std::fs::read(path)?;
    let previous = zstd::stream::decode_all(&bytes[ZSTD_MAGIC.len() + ZSTD_GUID.len()..])?;
    assert_eq!(
        bincode::serde::encode_to_vec(&material, BINCODE_CONFIG)?,
        previous
    );
    Ok(())
}

#[test]
#[ignore = "imports and compresses the included source assets"]
fn import_included_gltf_and_hdr() -> Result<()> {
    let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../content");
    let raw = gltf::GltfLoader::load(&content.join("mesh/cerberus/scene.gltf"))?;
    let assets = gltf::GltfBaker::bake(raw, AssetUrl::from("mesh/cerberus/scene.scene"))?;
    let mesh = assets
        .iter()
        .find_map(|a| a.as_any().downcast_ref::<Mesh>())
        .unwrap();
    assert!(!mesh.vertices.is_empty() && !mesh.indices.is_empty());
    let formats: Vec<_> = assets
        .iter()
        .filter_map(|a| a.as_any().downcast_ref::<Texture>())
        .map(|t| {
            assert_eq!(
                t.pixels.len(),
                t.format.data_size_in_bytes(t.width, t.height)
            );
            &t.format
        })
        .collect();
    assert!(formats.iter().any(|f| matches!(f, TextureFormat::Bc7Srgb)));
    assert!(formats.iter().any(|f| matches!(f, TextureFormat::Bc5Unorm)));
    hdr::HdrLoader::load(&content.join("texture/minedump_flats_4k.hdr"))?;
    Ok(())
}
