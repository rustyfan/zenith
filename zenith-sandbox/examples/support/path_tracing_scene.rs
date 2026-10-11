#[path = "sphere_mesh.rs"]
mod sphere_mesh;

use super::pbr_scene;
use glam::{Mat4, Vec3};
use zenith::asset::{
    material::Material,
    mesh::{Mesh, MeshInstance, Scene, SceneNode, Vertex},
    texture::{Texture, TextureFormat},
    AssetServer, Handle,
};

pub fn environment(assets: &AssetServer, white: bool) -> Handle<Texture> {
    let size = 32;
    let mut pixels = Vec::new();
    for face in 0..6 {
        for y in 0..size {
            for x in 0..size {
                let u = 2.0 * (x as f32 + 0.5) / size as f32 - 1.0;
                let v = 2.0 * (y as f32 + 0.5) / size as f32 - 1.0;
                let direction = match face {
                    0 => Vec3::new(1.0, -v, -u),
                    1 => Vec3::new(-1.0, -v, u),
                    2 => Vec3::new(u, 1.0, v),
                    3 => Vec3::new(u, -1.0, -v),
                    4 => Vec3::new(u, -v, 1.0),
                    _ => Vec3::new(-u, -v, -1.0),
                }
                .normalize();
                let light = direction
                    .dot(Vec3::new(-0.5, -0.7, 0.5).normalize())
                    .max(0.0)
                    .powf(40.0);
                let color = if white {
                    Vec3::ONE
                } else {
                    Vec3::new(0.08, 0.12, 0.2)
                        + Vec3::new(0.35, 0.3, 0.23) * direction.z.max(0.0)
                        + Vec3::new(6.0, 5.0, 3.5) * light
                };
                for value in color.extend(1.0).to_array() {
                    pixels.extend(value.to_le_bytes());
                }
            }
        }
    }
    assets.add(Texture {
        width: size,
        height: size,
        mip_levels: 1,
        is_cubemap: true,
        format: TextureFormat::Rgba32Float,
        pixels,
    })
}

pub fn room(assets: &AssetServer) -> Handle<Scene> {
    let sphere = sphere_mesh::sphere_mesh(assets);
    let mut instances = Vec::new();
    for (positions, normal, color) in [
        (
            [
                [-3.0, -2.0, -1.0],
                [3.0, -2.0, -1.0],
                [3.0, 6.0, -1.0],
                [-3.0, 6.0, -1.0],
            ],
            Vec3::Z,
            [0.7, 0.7, 0.7, 1.0],
        ),
        (
            [
                [-3.0, 6.0, -1.0],
                [3.0, 6.0, -1.0],
                [3.0, 6.0, 4.0],
                [-3.0, 6.0, 4.0],
            ],
            -Vec3::Y,
            [0.7, 0.7, 0.7, 1.0],
        ),
        (
            [
                [-3.0, 0.0, -1.0],
                [-3.0, 6.0, -1.0],
                [-3.0, 6.0, 4.0],
                [-3.0, 0.0, 4.0],
            ],
            Vec3::X,
            [0.75, 0.08, 0.03, 1.0],
        ),
        (
            [
                [3.0, 0.0, -1.0],
                [3.0, 6.0, -1.0],
                [3.0, 6.0, 4.0],
                [3.0, 0.0, 4.0],
            ],
            -Vec3::X,
            [0.03, 0.12, 0.75, 1.0],
        ),
    ] {
        instances.push(MeshInstance {
            node: 0,
            mesh: assets.add(Mesh::new(
                positions
                    .into_iter()
                    .map(|position| Vertex {
                        position,
                        normal: normal.to_array(),
                        tex_coord: [0.0; 2],
                        tangent: [0.0; 4],
                    })
                    .collect(),
                vec![0, 1, 2, 0, 2, 3],
            )),
            material: assets.add(Material {
                base_color: color,
                metallic: 0.0,
                roughness: 1.0,
                ..Default::default()
            }),
            transform: Mat4::IDENTITY.to_cols_array(),
        });
    }
    for (position, color, metallic, roughness) in [
        (Vec3::new(-1.1, 1.5, -0.15), [0.7, 0.7, 0.7, 1.0], 0.0, 0.8),
        (Vec3::new(1.1, 3.0, -0.15), [0.95, 0.64, 0.3, 1.0], 1.0, 0.2),
    ] {
        instances.push(MeshInstance {
            node: 0,
            mesh: sphere.clone(),
            material: assets.add(Material {
                base_color: color,
                metallic,
                roughness,
                ..Default::default()
            }),
            transform: Mat4::from_translation(position).to_cols_array(),
        });
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

pub fn white_furnace(assets: &AssetServer) -> Handle<Scene> {
    let spheres = pbr_scene::spheres(assets);
    let mut scene = (*spheres.get().unwrap()).clone();
    scene.instances.retain(|instance| {
        let metallic = instance.material.get().unwrap().metallic;
        metallic == 0.0 || metallic == 1.0
    });
    for instance in &mut scene.instances {
        let mut material = (*instance.material.get().unwrap()).clone();
        material.base_color = [1.0; 4];
        instance.material = assets.add(material);
    }
    assets.add(scene)
}
