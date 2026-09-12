use serde::{Deserialize, Serialize};
use zenith_asset::{
    AssetServer, CookedAsset, ImportContext, Importer, MemorySource, Result,
    mesh::{Mesh, VertexLayout},
};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, Serialize, Deserialize)]
struct Position([f32; 3]);
impl VertexLayout for Position {
    const MESH_TYPE_KEY: &'static str = "example.mesh.position";
    fn valid(&self) -> bool {
        self.0.iter().all(|v| v.is_finite())
    }
}
struct Triangle;
impl Importer for Triangle {
    type Settings = ();
    type Output = Mesh<Position>;
    const KEY: &'static str = "example.triangle";
    const VERSION: u32 = 1;
    fn extensions(&self) -> &[&str] {
        &["triangle"]
    }
    fn import(
        &self,
        _: &[u8],
        _: &(),
        _: &mut ImportContext<'_>,
    ) -> Result<<Self::Output as CookedAsset>::Data> {
        Ok(Mesh::new(
            vec![
                Position([0.0, 0.0, 0.0]),
                Position([1.0, 0.0, 0.0]),
                Position([0.0, 1.0, 0.0]),
            ],
            vec![0, 1, 2],
        ))
    }
}
#[test]
fn downstream_vertex_layout_and_importer_need_no_core_changes() {
    let source = MemorySource::default();
    source.insert("shape.triangle", Vec::new()).unwrap();
    let server = AssetServer::builder()
        .source(source)
        .register_asset::<Mesh<Position>>()
        .register_importer(Triangle)
        .build()
        .unwrap();
    let handle = server
        .load_blocking::<Mesh<Position>>("shape.triangle")
        .unwrap();
    assert_eq!(handle.get().unwrap().vertices_bytes().len(), 36);
    assert_eq!(handle.get().unwrap().vertices[1].0, [1.0, 0.0, 0.0]);
}
