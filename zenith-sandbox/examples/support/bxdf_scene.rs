use glam::{Mat4, Vec3};
use zenith::asset::{
    material::{ClearCoat, Hair, Material},
    mesh::{Mesh, MeshInstance, Scene, SceneNode, Vertex},
    texture::{Texture, TextureFormat},
    AssetServer, Handle,
};

#[path = "sphere_mesh.rs"]
mod sphere_mesh;

pub fn environment(assets: &AssetServer, white: bool) -> Handle<Texture> {
    let size = 32u32;
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
                let strip = (direction
                    .dot(Vec3::new(-0.5, -0.7, 0.5).normalize())
                    .max(0.0))
                .powf(80.0);
                let color = if white {
                    Vec3::ONE
                } else {
                    Vec3::new(0.08, 0.12, 0.2)
                        + Vec3::new(0.35, 0.3, 0.23) * direction.z.max(0.0)
                        + Vec3::new(6.0, 5.0, 3.5) * strip
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
        format: TextureFormat::Rgba32Float,
        pixels,
        is_cubemap: true,
        mip_levels: 1,
    })
}

pub fn scene(assets: &AssetServer) -> Handle<Scene> {
    let sphere = sphere_mesh::sphere_mesh(assets);
    let mut instances = Vec::new();
    for row in 0..3 {
        for column in 0..4 {
            let material = Material {
                base_color: [0.48, 0.075, 0.025, 1.0],
                metallic: if row == 2 { 1.0 } else { 0.0 },
                roughness: 0.5,
                clearcoat: ClearCoat {
                    weight: column as f32 / 3.0,
                    roughness: [0.06, 0.25, 0.55][row],
                    ..Default::default()
                },
                ..Default::default()
            };
            instances.push(MeshInstance {
                node: 0,
                mesh: sphere.clone(),
                material: assets.add(material),
                transform: Mat4::from_translation(Vec3::new(
                    -5.4 + column as f32 * 1.65,
                    0.0,
                    2.1 - row as f32 * 1.9,
                ))
                .to_cols_array(),
            });
        }
    }
    let normals = assets.add(Texture {
        width: 2,
        height: 2,
        format: TextureFormat::Rgba8Unorm,
        pixels: vec![
            205, 128, 255, 255, 50, 128, 255, 255, 128, 205, 255, 255, 128, 50, 255, 255,
        ],
        is_cubemap: false,
        mip_levels: 1,
    });
    for column in 0..4 {
        let material = Material {
            base_color: [0.06, 0.25, 0.5, 1.0],
            metallic: 0.0,
            roughness: 0.5,
            normal_tex: (column == 1 || column == 3).then(|| normals.clone()),
            clearcoat_normal_tex: (column >= 2).then(|| normals.clone()),
            clearcoat: ClearCoat {
                weight: 1.0,
                roughness: 0.12,
                normal_scale: 0.6,
            },
            ..Default::default()
        };
        instances.push(MeshInstance {
            node: 0,
            mesh: sphere.clone(),
            material: assets.add(material),
            transform: Mat4::from_translation(Vec3::new(-5.4 + column as f32 * 1.65, 0.0, -3.6))
                .to_cols_array(),
        });
    }
    for row in 0..2 {
        for column in 0..3 {
            let absorption = [[0.08, 0.18, 0.45], [0.4, 0.8, 1.5], [1.6, 2.4, 3.5]][column];
            let hair = Hair {
                absorption,
                longitudinal_roughness: if row == 0 { 0.2 } else { 0.35 },
                ..Default::default()
            };
            let mesh = strands(assets, row != 0, 24);
            instances.push(MeshInstance {
                node: 0,
                mesh,
                material: assets.add(Material {
                    hair: Some(hair),
                    ..Default::default()
                }),
                transform: Mat4::from_translation(Vec3::new(
                    1.6 + column as f32 * 1.7,
                    0.0,
                    2.8 - row as f32 * 3.8,
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

fn strands(assets: &AssetServer, curved: bool, count: usize) -> Handle<Mesh> {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for strand in 0..count {
        let base = vertices.len() as u32;
        let phase = strand as f32 * 2.399963;
        let position = |u: f32| {
            Vec3::new(
                (strand as f32 / (count - 1) as f32 - 0.5) * 1.1
                    + if curved {
                        0.3 * (u * 6.0 + phase * 0.15).sin()
                    } else {
                        0.08 * (u * 4.0 + phase).sin()
                    },
                0.12 * (phase + u * 3.0).sin(),
                -u * 2.9,
            )
        };
        for segment in 0..=32 {
            let u = segment as f32 / 32.0;
            let tangent = (position(u + 0.001) - position(u - 0.001)).normalize();
            let side = tangent.cross(-Vec3::Y).normalize();
            for edge in 0..2 {
                let p = position(u) + side * (edge as f32 - 0.5) * 0.045;
                vertices.push(Vertex {
                    position: p.to_array(),
                    normal: [0.0, -1.0, 0.0],
                    tex_coord: [edge as f32, u],
                    tangent: tangent.extend(1.0).to_array(),
                });
            }
            if segment < 32 {
                let a = base + segment * 2;
                indices.extend([a, a + 1, a + 2, a + 1, a + 3, a + 2]);
            }
        }
    }
    assets.add(Mesh::new(vertices, indices))
}
