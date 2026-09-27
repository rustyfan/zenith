use glam::{Mat4, Vec3};
use zenith::asset::{
    material::Material,
    mesh::{MeshInstance, Scene, SceneNode},
    AssetServer,
};

#[path = "sphere_mesh.rs"]
mod sphere_mesh;
use sphere_mesh::sphere_mesh;

pub fn spheres(assets: &AssetServer) -> zenith::asset::Handle<Scene> {
    let mesh = sphere_mesh(assets);
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
                ..Default::default()
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
