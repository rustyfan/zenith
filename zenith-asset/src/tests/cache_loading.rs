use super::*;
use crate::cache::{Cache, Manifest, Output};
use crate::texture::Texture;

fn texture_cache(cache: &Temp, payload: &[u8], codec: &str) {
    let address = AssetAddress::parse("fixture.texture").unwrap();
    let cache = Cache::new(cache.0.clone(), "desktop".into());
    let _lock = cache.lock().unwrap();
    let mut output = Output {
        type_key: Texture::TYPE_KEY.into(),
        schema: Texture::SCHEMA_VERSION,
        codec: codec.into(),
        blob: String::new(),
        length: payload.len(),
        dependencies: Vec::new(),
    };
    cache.write_blob(&mut output, payload).unwrap();
    cache
        .write_manifest(&Manifest {
            format: 2,
            source: address,
            importer: "test.fixture".into(),
            importer_version: 1,
            settings: serde_json::Value::Null,
            target: "desktop".into(),
            inputs: Vec::new(),
            outputs: [(String::new(), output)].into(),
        })
        .unwrap();
}

#[test]
fn builtin_codec_reads_v2_cache_and_failed_texture_validation_keeps_snapshot() {
    let cache = Temp::new();
    let fixture = [2, 1, 2, 8, 0, 1, 2, 3, 252, 253, 254, 255, 0, 1];
    texture_cache(&cache, &fixture, "bincode-serde-2");
    let server = AssetServer::builder()
        .with_builtin_assets()
        .cache_dir(&cache.0)
        .packaged()
        .build()
        .unwrap();
    let texture = server.load_blocking::<Texture>("fixture.texture").unwrap();
    let before = texture.snapshot().unwrap();
    assert_eq!(before.pixels, fixture[4..12]);
    let mut invalid = fixture;
    invalid[0] = 3;
    texture_cache(&cache, &invalid, "bincode-serde-2");
    server.reload(&texture).unwrap();
    assert_eq!(texture.wait().unwrap_err().kind, ErrorKind::InvalidData);
    assert_eq!(texture.snapshot().unwrap().revision, before.revision);
    assert_eq!(texture.get().unwrap().pixels, before.pixels);
    texture_cache(&cache, &fixture, "bincode-serde-2");
    server.reload(&texture).unwrap();
    texture.wait().unwrap();
    assert_eq!(server.stats().imports, 0);
}

struct JsonTexture;
impl AssetCodec<Texture> for JsonTexture {
    fn key(&self) -> &'static str {
        "test.texture-json"
    }
    fn encode(&self, data: &Texture) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(data)?)
    }
    fn decode(&self, bytes: &[u8]) -> Result<Texture> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

#[test]
fn custom_texture_codec_and_public_serde_representation_stay_available() {
    let cache = Temp::new();
    let fixture = br#"{"width":1,"height":1,"format":"R8Unorm","pixels":[42],"is_cubemap":false,"mip_levels":1}"#;
    texture_cache(&cache, fixture, JsonTexture.key());
    let server = AssetServer::builder()
        .register_codec::<Texture>(JsonTexture)
        .cache_dir(&cache.0)
        .packaged()
        .build()
        .unwrap();
    let texture = server.load_blocking::<Texture>("fixture.texture").unwrap();
    assert_eq!(texture.get().unwrap().pixels, [42]);
    assert_eq!(
        JsonTexture.encode(&texture.get().unwrap()).unwrap(),
        fixture
    );
}

#[test]
fn a_corrupt_bundle_blob_cannot_publish_partial_revisions() {
    let cache = Temp::new();
    let source = Arc::new(MemorySource::default());
    source.insert("a.bundle", vec![1]).unwrap();
    let writer = AssetServer::builder()
        .source(source)
        .cache_dir(&cache.0)
        .register_asset::<Number>()
        .register_importer(BundleNumbers)
        .build()
        .unwrap();
    writer.load_blocking::<Number>("a.bundle").unwrap();
    drop(writer);
    let server = AssetServer::builder()
        .cache_dir(&cache.0)
        .packaged()
        .register_asset::<Number>()
        .build()
        .unwrap();
    let root = server.load_blocking::<Number>("a.bundle").unwrap();
    let first = server.load_blocking::<Number>("a.bundle#first").unwrap();
    let second = server.load_blocking::<Number>("a.bundle#second").unwrap();
    let before = [
        root.snapshot().unwrap(),
        first.snapshot().unwrap(),
        second.snapshot().unwrap(),
    ];
    let mut manifest: Manifest =
        serde_json::from_slice(&fs::read(cache.manifest()).unwrap()).unwrap();
    let store = Cache::new(cache.0.clone(), "desktop".into());
    let mut root_output = manifest.outputs[""].clone();
    let replacement = <SerdeCodec as AssetCodec<Number>>::encode(&SerdeCodec, &Number(77)).unwrap();
    store.write_blob(&mut root_output, &replacement).unwrap();
    manifest.outputs.insert(String::new(), root_output);
    store.write_manifest(&manifest).unwrap();
    let blob = cache
        .0
        .join("v2/blobs")
        .join(format!("{}.bin", manifest.outputs["second"].blob));
    let valid = fs::read(&blob).unwrap();
    fs::write(&blob, b"corrupt").unwrap();
    server.reload(&root).unwrap();
    assert_eq!(root.wait().unwrap_err().kind, ErrorKind::Cache);
    for (handle, before) in [&root, &first, &second].into_iter().zip(before) {
        let current = handle.snapshot().unwrap();
        assert_eq!(current.revision, before.revision);
        assert_eq!(current.0, before.0);
    }
    fs::write(blob, valid).unwrap();
    server.reload(&root).unwrap();
    assert_eq!(root.wait().unwrap().0, 77);
}
