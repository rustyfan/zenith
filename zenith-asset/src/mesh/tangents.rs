use super::*;
use bevy_mikktspace::{Geometry, TangentSpace};
use std::collections::BTreeMap;

struct Corners(Vec<Vertex>);

impl Geometry for Corners {
    fn num_faces(&self) -> usize {
        self.0.len() / 3
    }

    fn num_vertices_of_face(&self, _: usize) -> usize {
        3
    }

    fn position(&self, face: usize, vert: usize) -> [f32; 3] {
        self.0[face * 3 + vert].position
    }

    fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
        glam::Vec3::from_array(self.0[face * 3 + vert].normal)
            .normalize()
            .to_array()
    }

    fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
        self.0[face * 3 + vert].tex_coord
    }

    fn set_tangent(&mut self, space: Option<TangentSpace>, face: usize, vert: usize) {
        let uv = |v: usize| glam::Vec2::from_array(self.0[face * 3 + v].tex_coord);
        let area = (uv(1) - uv(0)).perp_dot(uv(2) - uv(0));
        self.0[face * 3 + vert].tangent = if area != 0.0 {
            space.map_or([0.0; 4], |s| s.tangent_encoded())
        } else {
            [0.0; 4]
        };
    }
}

impl Mesh {
    pub fn generate_tangents(&mut self) -> Result<()> {
        self.validate()?;
        let mut corners = Corners(
            self.indices
                .iter()
                .map(|&index| self.vertices[index as usize])
                .collect(),
        );
        bevy_mikktspace::generate_tangents(&mut corners)
            .map_err(|e| AssetError::caused_by(ErrorKind::Import, "generate mesh tangents", e))?;
        let mut vertices = Vec::new();
        let mut indices = Vec::with_capacity(self.indices.len());
        let mut unique = BTreeMap::new();
        for (&source, vertex) in self.indices.iter().zip(corners.0) {
            let key = (source, vertex.tangent.map(f32::to_bits));
            let index = *unique.entry(key).or_insert_with(|| {
                let index = vertices.len() as u32;
                vertices.push(vertex);
                index
            });
            indices.push(index);
        }
        let generated = Mesh::new(vertices, indices);
        generated.validate()?;
        *self = generated;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn vertex(position: [f32; 3], tex_coord: [f32; 2]) -> Vertex {
        Vertex {
            position,
            normal: [0.0, 0.0, 1.0],
            tex_coord,
            tangent: [0.0; 4],
        }
    }

    #[test]
    fn generated_frames_follow_uv_scale_and_handedness() {
        for scale in [1.0, 0.0001, 0.000001, -1.0, -0.000001] {
            let mut mesh = Mesh::new(
                vec![
                    vertex([0.0, 0.0, 0.0], [0.0, 0.0]),
                    vertex([1.0, 0.0, 0.0], [scale, 0.0]),
                    vertex([0.0, 1.0, 0.0], [0.0, scale.abs()]),
                ],
                vec![0, 1, 2],
            );
            mesh.generate_tangents().unwrap();
            for vertex in &mesh.vertices {
                let [x, y, z, sign] = vertex.tangent;
                let t = Vec3::new(x, y, z);
                assert!((t - Vec3::X * scale.signum()).length() < 1e-5);
                assert!((Vec3::Z.cross(t) * sign - Vec3::Y).length() < 1e-5);
            }
        }
    }

    #[test]
    fn mirrored_uv_seam_splits_shared_vertices() {
        let mut mesh = Mesh::new(
            vec![
                vertex([0.0, 0.0, 0.0], [0.0, 0.0]),
                vertex([1.0, 0.0, 0.0], [1.0, 0.0]),
                vertex([0.0, 1.0, 0.0], [0.0, 1.0]),
                vertex([-1.0, 0.0, 0.0], [1.0, 0.0]),
            ],
            vec![0, 1, 2, 0, 2, 3],
        );
        mesh.generate_tangents().unwrap();
        assert_eq!(mesh.vertices.len(), 6);
        assert_ne!(mesh.indices[0], mesh.indices[3]);
        assert_ne!(mesh.indices[2], mesh.indices[4]);
        for &index in &mesh.indices[..3] {
            assert_eq!(mesh.vertices[index as usize].tangent, [1.0, 0.0, 0.0, 1.0]);
        }
        for &index in &mesh.indices[3..] {
            assert_eq!(
                mesh.vertices[index as usize].tangent,
                [-1.0, 0.0, 0.0, -1.0]
            );
        }
    }

    #[test]
    fn degenerate_uvs_disable_mapping_and_invalid_input_is_rejected() {
        let mut mesh = Mesh::new(
            vec![
                vertex([0.0, 0.0, 0.0], [0.0; 2]),
                vertex([1.0, 0.0, 0.0], [0.0; 2]),
                vertex([0.0, 1.0, 0.0], [0.0; 2]),
            ],
            vec![0, 1, 2],
        );
        mesh.generate_tangents().unwrap();
        assert!(mesh.vertices.iter().all(|v| v.tangent == [0.0; 4]));
        mesh.indices[0] = u32::MAX;
        assert!(mesh.generate_tangents().is_err());
    }
}
