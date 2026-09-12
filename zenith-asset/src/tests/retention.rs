use super::*;

#[test]
fn policies_apply_to_loaded_and_generated_assets_and_preserve_snapshot_ownership() {
    let source = Arc::new(MemorySource::default());
    source.insert("a.number", b"7".to_vec()).unwrap();
    let server = AssetServer::builder()
        .source(source)
        .register_asset::<Number>()
        .register_importer(Numbers)
        .cpu_retention::<Number>(CpuRetention::ReleaseAfterUpload)
        .build()
        .unwrap();
    let keep = server.add(String::from("keep"));
    assert_eq!(keep.cpu_retention(), CpuRetention::Keep);
    assert!(!keep.release_cpu(&keep.snapshot().unwrap()));
    for handle in [
        server.load_blocking::<Number>("a.number").unwrap(),
        server.add(Number(7)),
    ] {
        let snapshot = handle.snapshot().unwrap();
        let weak = Arc::downgrade(&snapshot.value);
        let revision = snapshot.revision;
        assert!(handle.release_cpu(&snapshot));
        assert!(handle.get().is_none());
        assert_eq!(handle.revision(), Some(revision));
        assert!(matches!(handle.state(), LoadState::CpuReleased { revision: r } if r == revision));
        assert_eq!(handle.wait().unwrap_err().kind, ErrorKind::CpuReleased);
        assert_eq!(snapshot.0, 7);
        assert!(weak.upgrade().is_some());
        drop(snapshot);
        let start = Instant::now();
        while weak.upgrade().is_some() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::yield_now();
        }
    }
}

#[test]
fn restoration_preserves_revision_and_old_acknowledgements_cannot_release_new_allocations() {
    let cache = Temp::new();
    let source = Arc::new(MemorySource::default());
    source.insert("a.number", b"7".to_vec()).unwrap();
    let server = AssetServer::builder()
        .source(source.clone())
        .cache_dir(&cache.0)
        .register_asset::<Number>()
        .register_importer(Numbers)
        .cpu_retention::<Number>(CpuRetention::ReleaseAfterUpload)
        .build()
        .unwrap();
    let handle = server.load_blocking::<Number>("a.number").unwrap();
    let original = handle.snapshot().unwrap();
    assert!(handle.release_cpu(&original));
    let loaded = server.load_blocking::<Number>("a.number").unwrap();
    assert_eq!(loaded, handle);
    let restored = handle.snapshot().unwrap();
    assert_eq!(restored.revision, original.revision);
    assert!(!Arc::ptr_eq(&restored.value, &original.value));
    assert!(!handle.release_cpu(&original));
    assert!(handle.get().is_some());
    source.insert("a.number", b"8".to_vec()).unwrap();
    server.reload(&handle).unwrap();
    handle.wait().unwrap();
    assert!(handle.revision().unwrap() > original.revision);
    assert!(!handle.release_cpu(&restored));
    assert_eq!(handle.get().unwrap().0, 8);
}

#[test]
fn watcher_preserves_eviction_and_reload_reaches_released_ancestors() {
    let source = Arc::new(MemorySource::default());
    source
        .insert("root.node", br#"[1,["leaf.node"]]"#.to_vec())
        .unwrap();
    source.insert("leaf.node", br#"[2,[]]"#.to_vec()).unwrap();
    let server = AssetServer::builder()
        .source(source.clone())
        .register_asset::<Node>()
        .register_importer(Nodes)
        .cpu_retention::<Node>(CpuRetention::ReleaseAfterUpload)
        .build()
        .unwrap();
    let root = server.load_blocking::<Node>("root.node").unwrap();
    let leaf = root.get().unwrap().children[0].clone();
    let revision = root.revision().unwrap();
    assert!(root.release_cpu(&root.snapshot().unwrap()));
    assert!(leaf.release_cpu(&leaf.snapshot().unwrap()));
    let counts = server.stats();
    server.watch(Some(Duration::from_millis(10)));
    let start = Instant::now();
    while server.stats().source_bytes <= counts.source_bytes + 100 {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(server.stats().decodes, counts.decodes);
    assert!(root.get().is_none());
    assert!(leaf.get().is_none());
    source.insert("leaf.node", br#"[3,[]]"#.to_vec()).unwrap();
    while root.revision() == Some(revision) {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    server.watch(None);
    assert_eq!(root.get().unwrap().children[0].get().unwrap().value, 3);
    assert!(root.revision().unwrap() > revision);
}
