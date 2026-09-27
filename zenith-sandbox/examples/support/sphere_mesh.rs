use glam::Vec3;
use zenith::asset::{
    mesh::{Mesh, Vertex},
    AssetServer,
};

pub fn sphere_mesh(assets: &AssetServer) -> zenith::asset::Handle<Mesh> {
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
    assets.add(Mesh::new(vertices, indices))
}
