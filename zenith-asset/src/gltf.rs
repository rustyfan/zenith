use crate::{
    AssetError, ErrorKind, ImportContext, Importer, Result,
    material::{Material, MaterialData},
    mesh::{Mesh, MeshInstanceData, Scene, SceneData, SceneNode, Vertex},
    texture::{Texture, TextureCompression, TextureSettings, TextureUsage, bake_image},
};
use glam::{Mat4, Vec3};
use gltf::buffer::Data as BufferData;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GltfSettings {
    pub mipmaps: bool,
    pub compression: TextureCompression,
    pub y_up_to_z_up: bool,
}
impl Default for GltfSettings {
    fn default() -> Self {
        Self {
            mipmaps: true,
            compression: TextureCompression::Block,
            y_up_to_z_up: true,
        }
    }
}
pub struct GltfImporter;
impl Importer for GltfImporter {
    type Settings = GltfSettings;
    type Output = Scene;
    const KEY: &'static str = "zenith.gltf";
    const VERSION: u32 = 3;
    fn extensions(&self) -> &[&str] {
        &["gltf", "glb"]
    }
    fn import(
        &self,
        bytes: &[u8],
        settings: &GltfSettings,
        ctx: &mut ImportContext<'_>,
    ) -> Result<SceneData> {
        let mut document = gltf::Gltf::from_slice(bytes)
            .map_err(|e| AssetError::caused_by(ErrorKind::Import, "parse glTF", e))?;
        let mut blob = document.blob.take();
        let mut buffers = Vec::new();
        for buffer in document.buffers() {
            let data = match buffer.source() {
                gltf::buffer::Source::Bin => blob
                    .take()
                    .ok_or_else(|| invalid("missing GLB binary chunk"))?,
                gltf::buffer::Source::Uri(uri) if uri.starts_with("data:") => {
                    BufferData::from_source_and_blob(buffer.source(), None, &mut None)
                        .map_err(|e| {
                            AssetError::caused_by(ErrorKind::Import, "decode glTF buffer URI", e)
                        })?
                        .0
                }
                gltf::buffer::Source::Uri(uri) => ctx.read_relative(&decode_uri(uri)?)?,
            };
            if data.len() < buffer.length() {
                return Err(invalid("glTF buffer is shorter than its declared length"));
            }
            buffers.push(BufferData(data));
        }
        let mut textures = BTreeMap::new();
        for material in document.materials() {
            let pbr = material.pbr_metallic_roughness();
            let uses = [
                pbr.base_color_texture()
                    .map(|t| (t.texture(), t.tex_coord(), TextureUsage::Color)),
                pbr.metallic_roughness_texture()
                    .map(|t| (t.texture(), t.tex_coord(), TextureUsage::Linear)),
                material
                    .normal_texture()
                    .map(|t| (t.texture(), t.tex_coord(), TextureUsage::Normal)),
                material
                    .emissive_texture()
                    .map(|t| (t.texture(), t.tex_coord(), TextureUsage::Color)),
            ];
            for (texture, coord, usage) in uses.into_iter().flatten() {
                if coord != 0 {
                    return Err(invalid(
                        "only TEXCOORD_0 is supported by the built-in vertex layout",
                    ));
                }
                let image = texture.source();
                let key = (image.index(), usage);
                if textures.contains_key(&key) {
                    continue;
                }
                let decoded = match image.source() {
                    gltf::image::Source::Uri { uri, .. } if !uri.starts_with("data:") => {
                        image::load_from_memory(&ctx.read_relative(&decode_uri(uri)?)?).map_err(
                            |e| AssetError::caused_by(ErrorKind::Import, "decode glTF image", e),
                        )?
                    }
                    gltf::image::Source::View { view, .. } => {
                        let buffer = buffers
                            .get(view.buffer().index())
                            .ok_or_else(|| invalid("invalid image buffer"))?;
                        let end = view
                            .offset()
                            .checked_add(view.length())
                            .ok_or_else(|| invalid("image buffer range overflow"))?;
                        let bytes = buffer
                            .0
                            .get(view.offset()..end)
                            .ok_or_else(|| invalid("image buffer range is out of bounds"))?;
                        image::load_from_memory(bytes).map_err(|e| {
                            AssetError::caused_by(ErrorKind::Import, "decode embedded image", e)
                        })?
                    }
                    source => {
                        let image = gltf::image::Data::from_source(source, None, &buffers)
                            .map_err(|e| {
                                AssetError::caused_by(
                                    ErrorKind::Import,
                                    "decode embedded image URI",
                                    e,
                                )
                            })?;
                        decoded_image(image)?
                    }
                };
                let texture = bake_image(
                    &decoded,
                    &TextureSettings {
                        usage,
                        mipmaps: settings.mipmaps,
                        compression: settings.compression,
                    },
                )?;
                let usage_label = match usage {
                    TextureUsage::Color => "color",
                    TextureUsage::Linear => "linear",
                    TextureUsage::Normal => "normal",
                };
                textures.insert(
                    key,
                    ctx.emit::<Texture>(
                        &format!("images/{}/{usage_label}", image.index()),
                        texture,
                    )?,
                );
            }
        }
        ctx.emit::<Material>("materials/default", MaterialData::default())?;
        for material in document.materials() {
            let pbr = material.pbr_metallic_roughness();
            let path = |texture: gltf::Texture<'_>, usage| {
                textures.get(&(texture.source().index(), usage)).cloned()
            };
            let data = MaterialData {
                base_color: pbr.base_color_factor(),
                metallic: pbr.metallic_factor(),
                roughness: pbr.roughness_factor(),
                emissive: material.emissive_factor(),
                base_color_tex: pbr
                    .base_color_texture()
                    .and_then(|t| path(t.texture(), TextureUsage::Color)),
                mra_tex: pbr
                    .metallic_roughness_texture()
                    .and_then(|t| path(t.texture(), TextureUsage::Linear)),
                normal_tex: material
                    .normal_texture()
                    .and_then(|t| path(t.texture(), TextureUsage::Normal)),
                emissive_tex: material
                    .emissive_texture()
                    .and_then(|t| path(t.texture(), TextureUsage::Color)),
            };
            ctx.emit::<Material>(&format!("materials/{}", material.index().unwrap()), data)?;
        }
        for mesh in document.meshes() {
            for (index, primitive) in mesh.primitives().enumerate() {
                let data = bake_mesh(&primitive, &buffers)?;
                ctx.emit::<Mesh>(&mesh_label(mesh.index(), index), data)?;
            }
        }
        let coordinate = if settings.y_up_to_z_up {
            Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2)
        } else {
            Mat4::IDENTITY
        };
        let default_scene = document.default_scene().map(|s| s.index()).unwrap_or(0);
        let mut root = None;
        for scene in document.scenes() {
            let mut data = SceneData::default();
            let mut visited = BTreeSet::new();
            for node in scene.nodes() {
                add_node(node, None, 0, coordinate, &mut data, &mut visited, ctx)?;
            }
            if scene.index() == default_scene {
                root = Some(data.clone());
            }
            ctx.emit::<Scene>(&format!("scenes/{}", scene.index()), data)?;
        }
        root.ok_or_else(|| invalid("glTF document contains no scene"))
    }
}
fn mesh_label(mesh: usize, primitive: usize) -> String {
    format!("meshes/{mesh}/primitives/{primitive}")
}
fn add_node(
    node: gltf::Node<'_>,
    parent: Option<usize>,
    depth: usize,
    parent_transform: Mat4,
    scene: &mut SceneData,
    visited: &mut BTreeSet<usize>,
    ctx: &ImportContext<'_>,
) -> Result<()> {
    if depth >= 128 || !visited.insert(node.index()) || scene.nodes.len() >= 65536 {
        return Err(invalid("glTF hierarchy is cyclic, shared, or too large"));
    }
    let local = Mat4::from_cols_array_2d(&node.transform().matrix());
    let global = parent_transform * local;
    let index = scene.nodes.len();
    scene.nodes.push(SceneNode {
        source_index: node.index(),
        parent,
        transform: local.to_cols_array(),
    });
    if let Some(mesh) = node.mesh() {
        for (primitive_index, primitive) in mesh.primitives().enumerate() {
            let material_label = primitive
                .material()
                .index()
                .map_or_else(|| "materials/default".into(), |i| format!("materials/{i}"));
            scene.instances.push(MeshInstanceData {
                node: index,
                mesh: ctx.path(&mesh_label(mesh.index(), primitive_index))?,
                material: ctx.path(&material_label)?,
                transform: global.to_cols_array(),
            });
        }
    }
    for child in node.children() {
        add_node(child, Some(index), depth + 1, global, scene, visited, ctx)?;
    }
    Ok(())
}
fn bake_mesh(primitive: &gltf::Primitive<'_>, buffers: &[BufferData]) -> Result<Mesh> {
    let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(|b| b.0.as_slice()));
    let positions: Vec<_> = reader
        .read_positions()
        .ok_or_else(|| invalid("mesh has no positions"))?
        .collect();
    let tex_coords: Vec<_> = reader
        .read_tex_coords(0)
        .map(|uv| uv.into_f32().collect())
        .unwrap_or_else(|| vec![[0.0; 2]; positions.len()]);
    let source_indices: Vec<u32> = reader
        .read_indices()
        .map(|i| i.into_u32().collect())
        .unwrap_or_else(|| (0..positions.len() as u32).collect());
    if tex_coords.len() != positions.len()
        || source_indices
            .iter()
            .any(|&i| i as usize >= positions.len())
    {
        return Err(invalid("glTF vertex/index range mismatch"));
    }
    let indices = match primitive.mode() {
        gltf::mesh::Mode::Triangles => {
            if !source_indices.len().is_multiple_of(3) {
                return Err(invalid("triangle index count is not divisible by three"));
            }
            source_indices
        }
        gltf::mesh::Mode::TriangleStrip => (2..source_indices.len())
            .flat_map(|i| {
                if i % 2 == 0 {
                    [
                        source_indices[i - 2],
                        source_indices[i - 1],
                        source_indices[i],
                    ]
                } else {
                    [
                        source_indices[i - 1],
                        source_indices[i - 2],
                        source_indices[i],
                    ]
                }
            })
            .collect(),
        gltf::mesh::Mode::TriangleFan => (2..source_indices.len())
            .flat_map(|i| [source_indices[0], source_indices[i - 1], source_indices[i]])
            .collect(),
        _ => return Err(invalid("built-in mesh assets require triangle topology")),
    };
    if indices.is_empty() {
        return Err(invalid("mesh contains no triangles"));
    }
    let normals = reader
        .read_normals()
        .map(|values| values.collect::<Vec<_>>());
    let tangents = normals
        .as_ref()
        .and_then(|_| reader.read_tangents())
        .map(|values| values.collect::<Vec<_>>());
    if tangents
        .as_ref()
        .is_some_and(|values| values.len() != positions.len())
    {
        return Err(invalid("glTF tangent count mismatch"));
    }
    if tangents
        .as_ref()
        .is_some_and(|values| values.iter().any(|t| t[3].abs() != 1.0))
    {
        return Err(invalid("glTF tangent handedness must be -1 or 1"));
    }
    let mut mesh = if let Some(normals) = normals {
        if normals.len() != positions.len() {
            return Err(invalid("glTF normal count mismatch"));
        }
        let vertices = positions
            .into_iter()
            .zip(normals)
            .zip(tex_coords)
            .enumerate()
            .map(|(index, ((position, normal), tex_coord))| Vertex {
                position,
                normal,
                tex_coord,
                tangent: tangents.as_ref().map_or([0.0; 4], |values| values[index]),
            })
            .collect();
        Mesh::new(vertices, indices)
    } else {
        let mut vertices = Vec::with_capacity(indices.len());
        for triangle in indices.chunks_exact(3) {
            let a = Vec3::from_array(positions[triangle[0] as usize]);
            let b = Vec3::from_array(positions[triangle[1] as usize]);
            let c = Vec3::from_array(positions[triangle[2] as usize]);
            let normal = (b - a)
                .cross(c - a)
                .try_normalize()
                .unwrap_or(Vec3::Z)
                .to_array();
            for &index in triangle {
                vertices.push(Vertex {
                    position: positions[index as usize],
                    normal,
                    tex_coord: tex_coords[index as usize],
                    tangent: [0.0; 4],
                });
            }
        }
        let indices = (0..vertices.len() as u32).collect();
        Mesh::new(vertices, indices)
    };
    if tangents.is_none() {
        mesh.generate_tangents()?;
    }
    mesh.validate()?;
    Ok(mesh)
}
fn decode_uri(uri: &str) -> Result<String> {
    let mut decoded = Vec::new();
    let mut bytes = uri.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let a = bytes
                .next()
                .and_then(|b| (b as char).to_digit(16))
                .ok_or_else(|| invalid("invalid URI escape"))?;
            let b = bytes
                .next()
                .and_then(|b| (b as char).to_digit(16))
                .ok_or_else(|| invalid("invalid URI escape"))?;
            decoded.push((a * 16 + b) as u8);
        } else {
            decoded.push(byte);
        }
    }
    String::from_utf8(decoded)
        .map_err(|e| AssetError::caused_by(ErrorKind::InvalidPath, "glTF URI is not UTF-8", e))
}
fn decoded_image(data: gltf::image::Data) -> Result<image::DynamicImage> {
    use gltf::image::Format;
    let channels = match data.format {
        Format::R8 | Format::R16 => 1,
        Format::R8G8 | Format::R16G16 => 2,
        Format::R8G8B8 | Format::R16G16B16 | Format::R32G32B32FLOAT => 3,
        _ => 4,
    };
    let component_bytes = match data.format {
        Format::R16 | Format::R16G16 | Format::R16G16B16 | Format::R16G16B16A16 => 2,
        Format::R32G32B32FLOAT | Format::R32G32B32A32FLOAT => 4,
        _ => 1,
    };
    let mut rgba = Vec::new();
    for pixel in data.pixels.chunks_exact(channels * component_bytes) {
        let mut values = [0.0, 0.0, 0.0, 1.0];
        for (c, bytes) in pixel.chunks_exact(component_bytes).enumerate() {
            values[c] = match component_bytes {
                1 => bytes[0] as f32 / 255.0,
                2 => u16::from_ne_bytes([bytes[0], bytes[1]]) as f32 / 65535.0,
                _ => f32::from_ne_bytes(bytes.try_into().unwrap()),
            };
        }
        if channels < 3 {
            let alpha = if channels == 2 { values[1] } else { 1.0 };
            values = [values[0], values[0], values[0], alpha];
        }
        rgba.extend(values);
    }
    let image = image::ImageBuffer::from_raw(data.width, data.height, rgba)
        .ok_or_else(|| invalid("embedded image size mismatch"))?;
    if component_bytes == 4 {
        Ok(image::DynamicImage::ImageRgba32F(image))
    } else if component_bytes == 2 {
        Ok(image::DynamicImage::ImageRgba16(
            image::DynamicImage::ImageRgba32F(image).to_rgba16(),
        ))
    } else {
        Ok(image::DynamicImage::ImageRgba8(
            image::DynamicImage::ImageRgba32F(image).to_rgba8(),
        ))
    }
}
fn invalid(message: impl Into<String>) -> AssetError {
    AssetError::new(ErrorKind::Import, message)
}
