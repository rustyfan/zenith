use super::*;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

static TEMPS: AtomicU64 = AtomicU64::new(1);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "zenith-assets-test-{}-{}",
            std::process::id(),
            TEMPS.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn manifest(&self) -> PathBuf {
        fs::read_dir(self.0.join("v2/manifests"))
            .unwrap()
            .map(|p| p.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "json"))
            .unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        if let (Ok(path), Ok(parent)) = (self.0.canonicalize(), std::env::temp_dir().canonicalize())
            && path.parent() == Some(parent.as_path())
            && path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("zenith-assets-test-")
        {
            let _ = fs::remove_dir_all(path);
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
struct Number(u32);
impl CookedAsset for Number {
    type Data = Self;
    const TYPE_KEY: &'static str = "test.number";
    const SCHEMA_VERSION: u32 = 1;
    fn from_data(data: Self, _: &mut LoadContext<'_>) -> Result<Self> {
        if data.0 == 999 {
            return Err(AssetError::new(ErrorKind::InvalidData, "rejected value"));
        }
        Ok(data)
    }
}
#[derive(Serialize, Deserialize)]
struct Scale {
    factor: u32,
}
impl Default for Scale {
    fn default() -> Self {
        Self { factor: 1 }
    }
}
struct Numbers;
impl Importer for Numbers {
    type Settings = Scale;
    type Output = Number;
    const KEY: &'static str = "test.numbers";
    const VERSION: u32 = 1;
    fn extensions(&self) -> &[&str] {
        &["number"]
    }
    fn import(
        &self,
        bytes: &[u8],
        settings: &Scale,
        ctx: &mut ImportContext<'_>,
    ) -> Result<Number> {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| AssetError::caused_by(ErrorKind::Import, "number text", e))?;
        if text == "panic" {
            panic!("test extension panic");
        }
        let value = if let Some(path) = text.strip_prefix('@') {
            std::str::from_utf8(&ctx.read_relative(path)?)
                .unwrap()
                .parse::<u32>()
                .unwrap()
        } else {
            text.parse::<u32>()
                .map_err(|e| AssetError::caused_by(ErrorKind::Import, "parse number", e))?
        };
        Ok(Number(value * settings.factor))
    }
}
fn numbers(source: Arc<MemorySource>, cache: Option<&Temp>) -> AssetServer {
    let mut builder = AssetServer::builder()
        .source(source)
        .register_asset::<Number>()
        .register_importer(Numbers);
    if let Some(cache) = cache {
        builder = builder.cache_dir(&cache.0);
    }
    builder.build().unwrap()
}
#[derive(Debug, Serialize, Deserialize)]
struct NodeData {
    value: u32,
    children: Vec<AssetPath<Node>>,
}
#[derive(Debug)]
struct Node {
    value: u32,
    children: Vec<Handle<Node>>,
}
impl CookedAsset for Node {
    type Data = NodeData;
    const TYPE_KEY: &'static str = "test.node";
    const SCHEMA_VERSION: u32 = 1;
    fn from_data(data: NodeData, ctx: &mut LoadContext<'_>) -> Result<Self> {
        Ok(Node {
            value: data.value,
            children: data
                .children
                .iter()
                .map(|p| ctx.dependency(p))
                .collect::<Result<_>>()?,
        })
    }
}
struct Nodes;
impl Importer for Nodes {
    type Settings = ();
    type Output = Node;
    const KEY: &'static str = "test.nodes";
    const VERSION: u32 = 1;
    fn extensions(&self) -> &[&str] {
        &["node"]
    }
    fn import(&self, bytes: &[u8], _: &(), _: &mut ImportContext<'_>) -> Result<NodeData> {
        let (value, children): (u32, Vec<String>) = serde_json::from_slice(bytes)?;
        Ok(NodeData {
            value,
            children: children
                .iter()
                .map(|p| AssetPath::new(p))
                .collect::<Result<_>>()?,
        })
    }
}
fn nodes(source: Arc<MemorySource>) -> AssetServer {
    AssetServer::builder()
        .source(source)
        .register_asset::<Node>()
        .register_importer(Nodes)
        .build()
        .unwrap()
}
#[test]
fn paths_are_typed_validated_and_round_trip() {
    for invalid in [
        "",
        "../a.number",
        "/a.number",
        "C:/a.number",
        r"\\host\share\a",
        "x#../bad",
        "a.#label",
        "x#",
    ] {
        assert!(AssetPath::<Number>::new(invalid).is_err(), "{invalid}");
    }
    let path = AssetPath::<Number>::new(r"models\.\box.gltf#meshes/0").unwrap();
    assert_eq!(path.address().path(), "models/box.gltf");
    let encoded = bincode::serde::encode_to_vec(&path, bincode::config::standard()).unwrap();
    let (decoded, _): (AssetPath<Number>, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(path, decoded);
    assert_eq!(
        path.address().relative("../data.bin").unwrap().path(),
        "data.bin"
    );
    assert!(path.address().relative("../../data.bin").is_err());
}
#[test]
fn downstream_type_and_generated_assets_need_no_global_initialization() {
    let source = Arc::new(MemorySource::default());
    source.insert("a.number", b"7".to_vec()).unwrap();
    let server = numbers(source, None);
    let handle = server.load_blocking::<Number>("a.number").unwrap();
    assert_eq!(handle.get().unwrap().0, 7);
    let generated = server.add(String::from("generated"));
    assert_eq!(&**generated.get().as_ref().unwrap(), "generated");
    assert!(generated.address().is_none());
    let snapshot = handle.get().unwrap();
    drop(server);
    assert_eq!(snapshot.0, 7);
    assert_eq!(handle.get().unwrap().0, 7);
}
#[test]
fn concurrent_requests_share_slots_and_decode_once() {
    let source = Arc::new(MemorySource::default());
    source.insert("a.number", b"8".to_vec()).unwrap();
    let server = numbers(source, None);
    let handles = std::thread::scope(|scope| {
        (0..16)
            .map(|_| scope.spawn(|| server.load::<Number>("a.number").unwrap()))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect::<Vec<_>>()
    });
    for handle in &handles {
        assert_eq!(handle.wait().unwrap().0, 8);
        assert_eq!(handle.id(), handles[0].id());
    }
    assert_eq!(server.stats().imports, 1);
    assert_eq!(server.stats().decodes, 1);
    let stats = server.stats();
    server.load_blocking::<Number>("a.number").unwrap();
    assert_eq!(server.stats().source_bytes, stats.source_bytes);
}
#[test]
fn servers_are_isolated_and_handles_control_residency() {
    let a = Arc::new(MemorySource::default());
    let b = Arc::new(MemorySource::default());
    a.insert("a.number", b"1".to_vec()).unwrap();
    b.insert("a.number", b"2".to_vec()).unwrap();
    let first = numbers(a, None);
    let second = numbers(b, None);
    let handle = first.load_blocking::<Number>("a.number").unwrap();
    assert_ne!(
        handle.id(),
        second.load_blocking::<Number>("a.number").unwrap().id()
    );
    assert!(second.reload(&handle).is_err());
    let weak = Arc::downgrade(&handle.get().unwrap());
    drop(handle);
    let start = Instant::now();
    while weak.upgrade().is_some() && start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(weak.upgrade().is_none());
}
#[test]
fn diamond_dependencies_are_deduplicated_and_cycles_fail() {
    let source = Arc::new(MemorySource::default());
    for (path, value) in [
        ("a.node", r#"[1,["b.node","c.node"]]"#),
        ("b.node", r#"[2,["d.node"]]"#),
        ("c.node", r#"[3,["d.node"]]"#),
        ("d.node", "[4,[]]"),
    ] {
        source.insert(path, value.as_bytes().to_vec()).unwrap();
    }
    let server = nodes(source.clone());
    let a = server.load_blocking::<Node>("a.node").unwrap();
    let a = a.get().unwrap();
    assert_eq!(a.value, 1);
    assert_eq!(
        a.children[0].get().unwrap().children[0].id(),
        a.children[1].get().unwrap().children[0].id()
    );
    assert_eq!(server.stats().imports, 4);
    assert_eq!(server.stats().decodes, 4);
    source
        .insert("cycle.node", br#"[0,["cycle.node"]]"#.to_vec())
        .unwrap();
    let error = server.load_blocking::<Node>("cycle.node").unwrap_err();
    assert_eq!(error.kind, ErrorKind::DependencyCycle);
    assert!(error.to_string().contains("cycle.node"));
}
#[test]
fn source_dependency_changes_rebuild_and_failed_reload_keeps_snapshot() {
    let cache = Temp::new();
    let source = Arc::new(MemorySource::default());
    source.insert("a.number", b"@data.txt".to_vec()).unwrap();
    source.insert("data.txt", b"3".to_vec()).unwrap();
    let server = numbers(source.clone(), Some(&cache));
    let handle = server.load_blocking::<Number>("a.number").unwrap();
    let previous = handle.snapshot().unwrap();
    source.insert("data.txt", b"4".to_vec()).unwrap();
    server.reload(&handle).unwrap();
    assert_eq!(handle.wait().unwrap().0, 4);
    assert_eq!(previous.0, 3);
    assert!(handle.snapshot().unwrap().revision > previous.revision);
    let committed = fs::read(cache.manifest()).unwrap();
    source.insert("data.txt", b"999".to_vec()).unwrap();
    server.reload(&handle).unwrap();
    assert!(handle.wait().is_err());
    assert_eq!(handle.get().unwrap().0, 4);
    assert_eq!(fs::read(cache.manifest()).unwrap(), committed);
}
#[test]
fn settings_variants_do_not_overwrite_and_packaged_loads_need_no_source() {
    let cache = Temp::new();
    let source = Arc::new(MemorySource::default());
    source.insert("a.number", b"6".to_vec()).unwrap();
    let server = numbers(source, Some(&cache));
    let normal = server.load_blocking::<Number>("a.number").unwrap();
    let scaled = server
        .load_with::<Numbers>("a.number", &Scale { factor: 5 })
        .unwrap();
    assert_eq!(scaled.wait().unwrap().0, 30);
    assert_eq!(normal.get().unwrap().0, 6);
    assert_ne!(normal.id(), scaled.id());
    let variant = AssetPath::<Number>::from_address(scaled.address().unwrap().clone());
    drop(server);
    let packaged = AssetServer::builder()
        .register_asset::<Number>()
        .cache_dir(&cache.0)
        .packaged()
        .build()
        .unwrap();
    assert_eq!(
        packaged
            .load_blocking::<Number>("a.number")
            .unwrap()
            .get()
            .unwrap()
            .0,
        6
    );
    assert_eq!(packaged.load_path(&variant).unwrap().wait().unwrap().0, 30);
}
#[test]
fn corrupt_and_missing_artifacts_rebuild_without_touching_legacy_cache() {
    let cache = Temp::new();
    fs::write(cache.0.join("legacy.tex"), b"legacy").unwrap();
    let source = Arc::new(MemorySource::default());
    source.insert("a.number", b"12".to_vec()).unwrap();
    let server = numbers(source.clone(), Some(&cache));
    server.load_blocking::<Number>("a.number").unwrap();
    drop(server);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(cache.manifest()).unwrap()).unwrap();
    let digest = manifest["outputs"][""]["blob"].as_str().unwrap();
    let blob = cache.0.join("v2/blobs").join(format!("{digest}.bin"));
    fs::write(&blob, b"truncated").unwrap();
    let server = numbers(source.clone(), Some(&cache));
    assert_eq!(
        server
            .load_blocking::<Number>("a.number")
            .unwrap()
            .get()
            .unwrap()
            .0,
        12
    );
    assert_eq!(server.stats().imports, 1);
    drop(server);
    fs::remove_file(blob).unwrap();
    let server = numbers(source, Some(&cache));
    assert_eq!(
        server
            .load_blocking::<Number>("a.number")
            .unwrap()
            .get()
            .unwrap()
            .0,
        12
    );
    assert_eq!(fs::read(cache.0.join("legacy.tex")).unwrap(), b"legacy");
}
#[test]
fn wrong_types_unknown_formats_and_panics_return_errors() {
    let source = Arc::new(MemorySource::default());
    source.insert("a.number", b"42".to_vec()).unwrap();
    source.insert("panic.number", b"panic".to_vec()).unwrap();
    let server = AssetServer::builder()
        .source(source)
        .register_asset::<Number>()
        .register_asset::<Node>()
        .register_importer(Numbers)
        .build()
        .unwrap();
    assert_eq!(
        server.load_blocking::<Node>("a.number").unwrap_err().kind,
        ErrorKind::TypeMismatch
    );
    assert_eq!(
        server
            .load_blocking::<Number>("a.unknown")
            .unwrap_err()
            .kind,
        ErrorKind::UnsupportedImporter
    );
    assert_eq!(
        server
            .load_blocking::<Number>("panic.number")
            .unwrap_err()
            .kind,
        ErrorKind::Import
    );
    assert_eq!(
        server
            .load_blocking::<Number>("a.number")
            .unwrap()
            .get()
            .unwrap()
            .0,
        42
    );
    assert!(
        AssetServer::builder()
            .register_asset::<Number>()
            .register_asset::<Number>()
            .build()
            .is_err()
    );
}
#[test]
fn two_writers_share_a_cache_without_duplicate_bakes() {
    let cache = Temp::new();
    let source = Arc::new(MemorySource::default());
    source.insert("a.number", b"18".to_vec()).unwrap();
    let a = numbers(source.clone(), Some(&cache));
    let b = numbers(source, Some(&cache));
    let first = a.load::<Number>("a.number").unwrap();
    let second = b.load::<Number>("a.number").unwrap();
    assert_eq!(first.wait().unwrap().0, 18);
    assert_eq!(second.wait().unwrap().0, 18);
    assert_eq!(a.stats().imports + b.stats().imports, 1);
}
#[test]
fn watching_reloads_live_assets_and_reports_revisions() {
    let source = Arc::new(MemorySource::default());
    source.insert("a.number", b"1".to_vec()).unwrap();
    let server = numbers(source.clone(), None);
    let handle = server.load_blocking::<Number>("a.number").unwrap();
    server.watch(Some(Duration::from_millis(50)));
    source.insert("a.number", b"2".to_vec()).unwrap();
    let start = Instant::now();
    while handle.get().unwrap().0 != 2 && start.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
    }
    server.watch(None);
    assert_eq!(handle.get().unwrap().0, 2);
    assert!(server.drain_events().iter().any(|e| matches!(
        e,
        AssetEvent::Ready {
            revision: Revision(2),
            ..
        }
    )));
}

#[test]
fn leaf_reload_updates_ancestors_in_one_publication() {
    let source = Arc::new(MemorySource::default());
    source
        .insert("a.node", br#"[1,["b.node"]]"#.to_vec())
        .unwrap();
    source.insert("b.node", b"[2,[]]".to_vec()).unwrap();
    let server = nodes(source.clone());
    let root = server.load_blocking::<Node>("a.node").unwrap();
    let leaf = root.get().unwrap().children[0].clone();
    let before = root.snapshot().unwrap().revision;
    source.insert("b.node", b"[3,[]]".to_vec()).unwrap();
    server.reload(&leaf).unwrap();
    leaf.wait().unwrap();
    root.with_snapshot(|snapshot| {
        let snapshot = snapshot.unwrap();
        assert!(snapshot.revision > before);
        assert_eq!(snapshot.children[0].get().unwrap().value, 3);
    });
}

#[test]
fn target_profiles_coexist_and_invalid_manifests_recover() {
    let cache = Temp::new();
    let source = Arc::new(MemorySource::default());
    source.insert("a.number", b"21".to_vec()).unwrap();
    for target in ["desktop", "other"] {
        let server = AssetServer::builder()
            .source(source.clone())
            .cache_dir(&cache.0)
            .target(target)
            .register_asset::<Number>()
            .register_importer(Numbers)
            .build()
            .unwrap();
        server.load_blocking::<Number>("a.number").unwrap();
    }
    for target in ["desktop", "other"] {
        let server = AssetServer::builder()
            .cache_dir(&cache.0)
            .target(target)
            .packaged()
            .register_asset::<Number>()
            .build()
            .unwrap();
        assert_eq!(
            server
                .load_blocking::<Number>("a.number")
                .unwrap()
                .get()
                .unwrap()
                .0,
            21
        );
    }
    for path in fs::read_dir(cache.0.join("v2/manifests"))
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
    {
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["outputs"][""]["length"] = serde_json::json!(u64::MAX);
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }
    let packaged = AssetServer::builder()
        .cache_dir(&cache.0)
        .packaged()
        .register_asset::<Number>()
        .build()
        .unwrap();
    assert!(packaged.load_blocking::<Number>("a.number").is_err());
    let dev = numbers(source, Some(&cache));
    assert_eq!(
        dev.load_blocking::<Number>("a.number")
            .unwrap()
            .get()
            .unwrap()
            .0,
        21
    );
    assert_eq!(dev.stats().imports, 1);
}

struct JsonNumber;
impl AssetCodec<Number> for JsonNumber {
    fn key(&self) -> &'static str {
        "test.json-v1"
    }
    fn encode(&self, data: &Number) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(data)?)
    }
    fn decode(&self, bytes: &[u8]) -> Result<Number> {
        Ok(serde_json::from_slice(bytes)?)
    }
}
#[test]
fn custom_codec_round_trips_through_packaged_runtime() {
    let cache = Temp::new();
    let source = Arc::new(MemorySource::default());
    source.insert("a.number", b"17".to_vec()).unwrap();
    let server = AssetServer::builder()
        .source(source)
        .cache_dir(&cache.0)
        .register_codec::<Number>(JsonNumber)
        .register_importer(Numbers)
        .build()
        .unwrap();
    server.load_blocking::<Number>("a.number").unwrap();
    drop(server);
    let runtime = AssetServer::builder()
        .cache_dir(&cache.0)
        .packaged()
        .register_codec::<Number>(JsonNumber)
        .build()
        .unwrap();
    assert_eq!(
        runtime
            .load_blocking::<Number>("a.number")
            .unwrap()
            .get()
            .unwrap()
            .0,
        17
    );
}

#[cfg(feature = "importers")]
mod builtins;

mod cache_loading;
mod retention;

struct BundleNumbers;
impl Importer for BundleNumbers {
    type Settings = ();
    type Output = Number;
    const KEY: &'static str = "test.bundle";
    const VERSION: u32 = 1;
    fn extensions(&self) -> &[&str] {
        &["bundle"]
    }
    fn import(&self, bytes: &[u8], _: &(), ctx: &mut ImportContext<'_>) -> Result<Number> {
        ctx.emit::<Number>("first", Number(bytes[0] as u32))?;
        match bytes[0] {
            0 => {}
            1 => {
                ctx.emit::<Number>("second", Number(11))?;
            }
            2 => {
                ctx.emit::<Number>("second", Number(999))?;
            }
            _ => {
                ctx.emit::<Number>("first", Number(12))?;
            }
        }
        Ok(Number(42))
    }
}
#[test]
fn bundle_failure_rolls_back_and_removed_labels_report_errors() {
    let cache = Temp::new();
    let source = Arc::new(MemorySource::default());
    source.insert("a.bundle", vec![1]).unwrap();
    let server = AssetServer::builder()
        .source(source.clone())
        .cache_dir(&cache.0)
        .register_asset::<Number>()
        .register_importer(BundleNumbers)
        .build()
        .unwrap();
    let root = server.load_blocking::<Number>("a.bundle").unwrap();
    let first = server.load_blocking::<Number>("a.bundle#first").unwrap();
    let second = server.load_blocking::<Number>("a.bundle#second").unwrap();
    let committed = fs::read(cache.manifest()).unwrap();
    for byte in [2, 3] {
        source.insert("a.bundle", vec![byte]).unwrap();
        server.reload(&root).unwrap();
        assert!(root.wait().is_err());
        assert_eq!(first.get().unwrap().0, 1);
        assert_eq!(second.get().unwrap().0, 11);
        assert_eq!(fs::read(cache.manifest()).unwrap(), committed);
    }
    source.insert("a.bundle", vec![0]).unwrap();
    server.reload(&root).unwrap();
    root.wait().unwrap();
    assert_eq!(first.get().unwrap().0, 0);
    assert_eq!(second.get().unwrap().0, 11);
    assert_eq!(second.last_error().unwrap().kind, ErrorKind::MissingAsset);
}

#[test]
fn unchanged_watched_sources_do_not_decode_again() {
    let source = Arc::new(MemorySource::default());
    source.insert("a.number", b"1".to_vec()).unwrap();
    let server = numbers(source, None);
    let handle = server.load_blocking::<Number>("a.number").unwrap();
    server.watch(Some(Duration::from_millis(50)));
    let start = Instant::now();
    while server.stats().source_bytes < 3 && start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    server.watch(None);
    assert!(server.stats().source_bytes >= 3);
    assert_eq!(server.stats().decodes, 1);
    assert_eq!(handle.get().unwrap().0, 1);
}
