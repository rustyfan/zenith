use anyhow::{ensure, Context, Result};
use glam::Mat4;
use std::{
    io::{Cursor, Read},
    path::Path,
};
use zenith::asset::{
    material::{Hair, Material},
    mesh::{Mesh, MeshInstance, Scene, SceneNode, Vertex},
    AssetServer, Handle,
};

pub fn scene(assets: &AssetServer, path: &Path) -> Result<Handle<Scene>> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Read {}. Run scripts/setup-einar.ps1 first", path.display()))?;
    let mut input = Cursor::new(bytes.as_slice());
    let mut magic = [0; 8];
    input.read_exact(&mut magic)?;
    ensure!(&magic == b"ZEINAR01", "Unsupported Einar export");
    let count = uint(&mut input)?;
    ensure!((1..=128).contains(&count), "Invalid mesh count");
    let mut instances = Vec::new();
    for _ in 0..count {
        let kind = uint(&mut input)?;
        let nv = uint(&mut input)? as usize;
        let ni = uint(&mut input)? as usize;
        ensure!(
            kind <= 2 && nv > 0 && ni > 0 && ni % 3 == 0,
            "Invalid Einar mesh"
        );
        ensure!(
            (nv as u64 * 48 + ni as u64 * 4) <= bytes.len() as u64 - input.position(),
            "Truncated Einar mesh"
        );
        let mut vertices = Vec::with_capacity(nv);
        for _ in 0..nv {
            let mut values = [0.0; 12];
            for value in &mut values {
                *value = f32::from_bits(uint(&mut input)?);
            }
            vertices.push(Vertex {
                position: values[0..3].try_into()?,
                normal: values[3..6].try_into()?,
                tex_coord: values[6..8].try_into()?,
                tangent: values[8..12].try_into()?,
            });
        }
        let indices = (0..ni)
            .map(|_| uint(&mut input))
            .collect::<Result<Vec<_>>>()?;
        let mesh = Mesh::new(vertices, indices);
        mesh.validate()?;
        let material = if kind == 2 {
            Material {
                hair: Some(Hair {
                    absorption: [0.12, 0.20, 0.32],
                    longitudinal_roughness: 0.3,
                    ..Default::default()
                }),
                ..Default::default()
            }
        } else {
            Material {
                base_color: if kind == 0 {
                    [0.45, 0.28, 0.20, 1.0]
                } else {
                    [0.12, 0.12, 0.12, 1.0]
                },
                metallic: 0.0,
                roughness: 0.65,
                ..Default::default()
            }
        };
        instances.push(MeshInstance {
            node: 0,
            mesh: assets.add(mesh),
            material: assets.add(material),
            transform: Mat4::IDENTITY.to_cols_array(),
        });
    }
    ensure!(
        input.position() == bytes.len() as u64,
        "Unexpected Einar data"
    );
    Ok(assets.add(Scene {
        nodes: vec![SceneNode {
            source_index: 0,
            parent: None,
            transform: Mat4::IDENTITY.to_cols_array(),
        }],
        instances,
    }))
}

fn uint(input: &mut impl Read) -> Result<u32> {
    let mut value = [0; 4];
    input.read_exact(&mut value)?;
    Ok(u32::from_le_bytes(value))
}
