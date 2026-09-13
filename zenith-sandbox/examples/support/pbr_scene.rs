use glam::{Mat4, Vec3};
use zenith::asset::{
    material::Material,
    mesh::{Mesh, MeshInstance, Scene, SceneNode, Vertex},
    AssetServer,
};

pub fn spheres(assets: &AssetServer) -> zenith::asset::Handle<Scene> {
    let (rows, columns) = (32, 64);
    let mut vertices = Vec::new();
    for row in 0..=rows {
        let theta = std::f32::consts::PI * row as f32 / rows as f32;
        for column in 0..=columns {
            let phi = std::f32::consts::TAU * column as f32 / columns as f32;
            let n = Vec3::new(
                theta.sin() * phi.cos(),
                theta.sin() * phi.sin(),
                theta.cos(),
            );
            vertices.push(Vertex {
                position: (n * 0.85).to_array(),
                normal: n.to_array(),
                tex_coord: [column as f32 / columns as f32, row as f32 / rows as f32],
                tangent: [-phi.sin(), phi.cos(), 0.0, -1.0],
            });
        }
    }
    let mut indices = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let a = row * (columns + 1) + column;
            let b = a + columns + 1;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    let mesh = assets.add(Mesh::new(vertices, indices));
    let mut instances = Vec::new();
    for row in 0..3 {
        for column in 0..6 {
            let material = assets.add(Material {
                base_color: [0.8, 0.35, 0.08, 1.0],
                metallic: row as f32 * 0.5,
                roughness: column as f32 / 5.0,
                emissive: [0.0; 3],
                base_color_tex: None,
                mra_tex: None,
                normal_tex: None,
                emissive_tex: None,
            });
            instances.push(MeshInstance {
                node: 0,
                mesh: mesh.clone(),
                material,
                transform: Mat4::from_translation(Vec3::new(
                    (column as f32 - 2.5) * 2.1,
                    0.0,
                    (1.0 - row as f32) * 2.1,
                ))
                .to_cols_array(),
            });
        }
    }
    assets.add(Scene {
        nodes: vec![SceneNode {
            source_index: 0,
            parent: None,
            transform: Mat4::IDENTITY.to_cols_array(),
        }],
        instances,
    })
}
