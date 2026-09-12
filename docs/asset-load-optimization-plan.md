# Cached debug asset-load optimization plan and results

This work addresses the reported Debug/IDE regression with an existing cache. Diagnosis initially left production code and build profiles unchanged; the optimizations below were implemented on 2026-09-12. The profiling harness and raw benchmark results are in `target/asset-load-profile`.

**Recommendation:** use the existing codec extension point to eliminate element-by-element texture-byte decoding, optimize the native decompressor in development builds, use the existing Rayon pool for independent cache blobs, and overlap CPU loading with renderer initialization. Preserve the current server, typed handles, dependency graph, cache integrity, and publication contracts. Keep checked downcasts at the erased boundaries; handle reads are already typed.

The bulk texture codec, scoped Zstandard debug optimization, two-worker cache loading, and world startup overlap are implemented. Checked downcasts remain unchanged. The original rationale and deferred options follow the integrated results.

**Integrated results — 2026-09-12**

Each CPU benchmark uses one untimed warm-up and ten measured runs of the assets CLI against the existing Cerberus/HDR v2 cache. Times sum the two asset-load measurements and exclude Cargo compilation and process startup/teardown.

| Configuration | Combined median | Range |
|---|---:|---:|
| Original debug loader | 3,094.263 ms | 3,088.112–3,137.027 ms |
| Bulk texture codec | 435.941 ms | 432.619–444.629 ms |
| Bulk codec + optimized `zstd-sys` | 235.044 ms | 229.320–265.610 ms |
| Parallel implementation, one worker for comparison | 229.388 ms | 224.960–233.024 ms |
| Final debug loader, two workers | **198.539 ms** | **195.502–200.909 ms** |
| Final release loader | 157.549 ms | 152.645–160.291 ms |
| Final packaged debug runtime, `parallel` without importers | 145.745 ms | 142.642–147.568 ms |

The final debug median is approximately **15.6 times faster**, saving **2.896 seconds** versus the original implementation. Two workers reduce the median by about **13.5%** versus one worker in the same parallel implementation. The production pool is restored to two workers; there is no benchmark-only worker-count setting in the public API.

Every development-cache run reports zero imports, seven decodes, two cache hits, 88,858,426 source bytes, and 101,842,891 artifact bytes. Packaged runs report the same cache/decode/artifact counts and zero source bytes. Existing v2 assets load without rebaking. Raw results are `baseline.json`, `bulk-codec.json`, `bulk-codec-zstd.json`, `parallel-one-worker.json`, `parallel-two-workers.json`, `release-final.json`, and `packaged-debug-parallel.json` under the harness directory; `benchmark.ps1` reproduces the measurements.

These measurements use debug binaries without an attached IDE debugger and a warm filesystem cache on this workstation. They establish the CPU-loading improvement and meet the sub-500 ms target. They do not measure debugger instrumentation overhead or cold-storage startup.

The world example requests both assets before constructing the renderer and waits before inspecting/uploading them. In individual debug Vulkan smoke runs, prepare time was 1,271.140 ms with sequential startup after the CPU optimizations, and 1,209.319 ms with overlap. Renderer construction varied from 1,009.264 to 1,128.540 ms, so these single-run totals are illustrative rather than a stable speedup estimate. The remaining asset wait after renderer construction was **0.011 ms**. Both runs rendered ten frames, retained the same mesh/texture/upload counters, and reported no validation errors. No separate first-present timestamp was collected.

`assets_ready_by` is elapsed time when the main thread observes readiness after renderer construction, not the actual worker completion timestamp. `asset_wait_after_renderer` isolates the remaining wait. Use the CLI measurements above for CPU loading cost; the renderer and asset timings overlap and must not be added together.

Implementation locations:

- `zenith-asset/src/texture_codec.rs`: private borrowed/owned byte representation, preserving public `Texture` Serde and v2 wire bytes.
- `zenith-asset/src/import.rs`: shared bounded Bincode helpers; custom codec interfaces stay unchanged.
- `zenith-asset/src/server.rs`: parallel payload collection and receiver placement; dependency staging/publication stay serialized.
- `zenith-asset/Cargo.toml`: optional `parallel` feature using the existing Rayon dependency, also enabled by `importers`.
- Root `Cargo.toml`: development optimization for `zstd-sys` only.
- `zenith-sandbox/examples/world.rs`: renderer/loading overlap and explicit elapsed/wait timings.

Validation passed: workspace all-target tests; asset tests and doctests with default features, no default features, and `parallel` without importers; strict asset Clippy with all targets/features; workspace all-feature checks; both ignored Vulkan asset tests; and the ignored production bake/relocated-package test. That production test baked in 16.221 s and loaded its relocated package in 119.542 ms in release. Added tests cover fixed v2 payloads, all texture formats/mips/layers, length-prefix boundaries, malformed buffers, custom JSON texture codecs, invalid-footprint reload retention, concurrent completion/error/panic joining, and corrupt-bundle rollback. The validation script now includes the parallel runtime configuration.

The smaller follow-up options in the plan remain deferred because the target is met. No unchecked casts, new dependencies, graph scheduler, or cache-format migration were introduced.

**Measured baseline**

Three sequential runs of the existing assets CLI, loading Cerberus and the HDR environment from the development cache:

| Build | Scene | Environment | Combined |
|---|---:|---:|---:|
| Debug | 2.119–2.127 s | 0.968–0.980 s | 3.087–3.107 s |
| Release | 183–189 ms | 79–81 ms | 262–269 ms |

Every run reported zero imports, seven decodes, two cache hits, 88,858,426 source bytes, and 101,842,891 decoded artifact bytes. The workload is cached loading, not rebaking. Cargo compilation time is excluded.

Previous debug world logs show approximately 2.15–2.28 seconds for `request_load`. That supports a regression of roughly 0.8–1.0 seconds in these runs. Those historical logs are not a controlled comparison of identical binaries and payloads, so they do not establish the exact cause or size of the user's 1–2 second observation.

A separate debug harness measured the current cache stages using the existing manifests/blobs:

| Stage | Observed time |
|---|---:|
| Source reads and BLAKE3 fingerprints | 28–30 ms |
| Artifact reads and BLAKE3 integrity checks | 23–24 ms |
| Zstandard read/decompression | 306–325 ms |
| Texture deserialization through the current bounded Serde path | 2.75–2.86 s |
| Revision hashing over decoded payloads | 14–15 ms |
| Texture footprint validation | Below 0.1 ms |

These stage measurements are separate experiments, not an exact additive profile of the world example. Mesh conversion, scheduling, allocations/destruction, and GPU work are not fully represented.

The principal cost is `Texture::pixels: Vec<u8>` through generic Serde sequence decoding. The installed Bincode Serde adapter handles `deserialize_seq` element by element, whereas its byte-buffer path invokes Bincode's bulk `Vec<u8>` decoder. Serde documents the separate byte serialization interface and why ordinary vectors do not automatically use it. [Serde documentation](https://serde.rs/impl-serialize.html#other-special-cases)

The new size-limited decoder increases the cost of that existing per-byte loop. The isolated unbounded comparison still took about 2.34–2.40 seconds, versus 2.75–2.86 seconds with limits. Removing the limit would recover only part of the cost and weaken the design; use bulk decoding while retaining the limit.

**1. Add an internal bulk-byte texture codec — highest priority**

Use `AssetCodec<Texture>` and register it through `with_builtin_assets()`. Keep the public `Texture`, `CookedAsset::Data`, and generic `SerdeCodec` interfaces unchanged. Applications that explicitly register another codec keep control of their representation.

The internal wire representation should preserve the current field order and Bincode configuration, but serialize pixels through `serialize_bytes` and deserialize through `deserialize_byte_buf`. Move the resulting vector into `Texture`; use a borrowed wire representation when encoding to avoid a second pixel copy. A small private Serde adapter can implement this using existing dependencies.

A harness prototype decoded the actual cached textures in **10.2–10.7 ms**, retained the 512 MiB Bincode limit, produced identical pixel vectors, and re-encoded the entire texture payloads byte-for-byte identically to the existing v2 payloads. This is a measured decoder-stage result, not yet an integrated startup result.

Compatibility gates:

- Decode current v2 texture fixtures and compare all fields and pixels.
- Verify old-to-new and new-to-old default codec compatibility, including length-prefix boundaries and all texture formats.
- Retain the existing codec key/schema only after these compatibility tests pass.
- Reject truncated payloads, oversized lengths, trailing data, and invalid texture footprints.
- Leave custom texture codecs and source invalidation behavior unaffected.

Primary changes would be in `zenith-asset/src/texture.rs` or a private texture-codec module, plus built-in codec registration in `server.rs`. No manager type switch or new public API is needed.

**2. Optimize only Zstandard's native dependency for debug builds**

Proposed workspace-root Cargo configuration:

```toml
[profile.dev.package.zstd-sys]
opt-level = 3
```

The same isolated harness with this package override reduced the existing file-based decompression stage to **101–106 ms**, from 306–325 ms. The optimization was tested only in the scratch harness's manifest.

Engine, renderer, server, importers, and application Rust code retain ordinary debug compilation. The tradeoff is increased compilation time for the native library and less straightforward debugging inside Zstandard itself. Release behavior and cache bytes do not change. Cargo supports package-scoped profile overrides; broad dependency overrides can behave unexpectedly for generic code instantiated by another crate. [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides)

Apply this narrowly. Optimizing Bincode as a dependency is not a substitute for fixing the generic per-byte path.

**3. Parallelize independent cache work with the existing pool**

Keep one coordinator owning each `Work` transaction. Reuse the existing two-thread Rayon pool for parallel blob reads, integrity verification, and decompression inside `Work::read_cached`. Each task takes immutable manifest/cache references and returns an owned payload; the coordinator collects the results before resolving dependencies. No new scheduler, async runtime, shared mutable graph, or dependency is required.

The current pool is used by HDR processing, but cached blob loading is serial. Also, `pool.install(run)` currently wraps the entire receiver loop, so an idle `recv_timeout` occupies a Rayon worker. Move the receive loop onto the existing `zenith-assets` thread and call `pool.install` only around a received load or refresh operation. Nested parallel iterators and HDR work then use the same bounded pool. [Rayon ThreadPool::install](https://docs.rs/rayon/latest/rayon/struct.ThreadPool.html#method.install)

Within `read_cached`, first validate the complete output metadata and aggregate 512 MiB payload limit, then replace the blob loop with a parallel map/collect. The core operation is:

```rust
let bytes: BTreeMap<String, Arc<Vec<u8>>> = manifest.outputs
    .par_iter()
    .map(|(label, output)| {
        let payload = cache.read_blob(output)?;
        Ok((label.clone(), Arc::new(payload)))
    })
    .collect::<Result<_>>()?;
```

Retain the current atomic byte counters and add source/label context to errors in the actual implementation. A task must finish all existing header, digest, compression, and decoded-length checks before returning its payload. Collecting into the label-keyed `BTreeMap` preserves deterministic traversal. If multiple blobs fail, which error is returned need not be deterministic. All parallel work must be joined before fallback import, transaction failure, or publication.

Start with the current two workers. A single-output HDR cache entry has no inter-blob parallelism; its improvements come from the bulk codec and decompressor optimization. Cerberus has multiple independent outputs and can benefit. Source fingerprint reads can later use the same bounded pool if their remaining cost warrants it; keep them sequential in the first patch to limit simultaneous source buffers.

Keep `default-features = false` usable without Rayon. Extract the existing optional dependency into a small `parallel = ["dep:rayon"]` feature, have `importers` enable `parallel`, and gate pool construction and parallel iteration on `parallel`. The sequential branch should reuse the same blob-reading closure. A packaged runtime can then opt into `features = ["parallel"]` without pulling in image/glTF/ISPC importers. This reorganizes an existing dependency; it introduces no new package.

The boundaries are deliberate:

| Operation | Execution |
|---|---|
| Request deduplication, dependency traversal, cycle detection | Coordinator |
| Independent cached blob reads, hashes, decompression | Two-thread Rayon pool |
| `AssetCodec::decode` and `CookedAsset::from_data` | Coordinator, using the bulk texture codec |
| Revision propagation, cache commit, snapshot publication | Coordinator |
| Renderer initialization | Application thread, overlapping CPU loading |

The erased codec currently combines byte decoding with `from_data`, which takes a mutable `LoadContext` and recursively stages dependencies. Applying `par_iter` to that entire operation would require changing graph ownership and failure handling. A separate parallel decode-data stage is possible, but introduces another erased-data boundary and staging storage; defer it unless decoding remains significant after the bulk-byte fix.

Similarly, running whole `process` jobs concurrently would still contend on the current cache-wide file lock and would require coordinating overlapping dependency graphs and reloads. Parallelizing the independent work inside one transaction preserves those contracts with a much smaller change.

For later cold-import optimization, glTF image/usage pairs can be prepared in bounded batches, baked with `par_iter`, and emitted through `ImportContext` sequentially in stable label order. Avoid retaining every decoded image at once. HDR already parallelizes faces, so reuse its pool. Neither change is necessary to resolve the existing-cache regression.

**4. Overlap loading with renderer initialization**

The current world example waits for the scene and environment before constructing `WorldRenderer`. Start both requests through existing `load` calls, construct the renderer while the asset worker is running, then wait for readiness before framing the camera and uploading.

Conceptually:

```text
Current: load scene → load environment → initialize renderer → upload
Planned: [load scene → load environment] alongside [initialize renderer] → upload
```

This reuses the existing background worker and readiness rules. The two CPU asset jobs still run through the current coordinator; this is overlap with renderer initialization, not a claim of parallel graph decoding.

It reduces first-frame latency without changing asset identity, ownership, dependency resolution, or GPU publication. Re-measure under CPU contention because shader compilation and asset work may compete for resources. Keep separate timings for asset completion, renderer construction, upload, and first frame; moving waits must not merely hide costs in another timer.

**5. Keep checked downcasts at erased boundaries**

Knowing `TypeId::of::<T>()` identifies the requested type. An unchecked cast additionally requires proof that the actual erased allocation has exactly the target type. For a handle that target is `Slot<T>`, not `T`. Comparing the actual dynamic type against the expected type supplies that proof, but performs the check that `downcast` already provides. Rust's `Arc::downcast_unchecked` is currently nightly-only and using the wrong contained type is undefined behavior. [Rust Arc documentation](https://doc.rust-lang.org/std/sync/struct.Arc.html#method.downcast_unchecked)

In this implementation:

| Location | Current type guarantee | Recommendation |
|---|---|---|
| `Handle<T>::get` / `snapshot` | Handle owns `Arc<Slot<T>>`; snapshot owns `Arc<T>` | Already no downcast; retain typed storage |
| `AssetServer::load_path<T>` | Registry checks `TypeId`, then uses the registered slot factory | Keep the single checked conversion from the erased slot |
| `LoadContext::dependency<T>` | Recursive resolution receives `T::TYPE_KEY`, a downstream-defined string | Keep the check; the string alone does not prove Rust type identity |
| `Slot<T>::publish` | Receives an erased value from codec/runtime conversion | Keep the checked value conversion |

A concrete mismatch is a downstream type reusing another type's `TYPE_KEY` and requesting it through a dependency. The registry may contain only the original type, while `resolve` still finds that key. The checked `Handle::from_erased` returns `TypeMismatch`; replacing it blindly with an unchecked cast would turn that ordinary extension error into undefined behavior. Add a regression test for this case if handle construction changes.

An internal unchecked helper could be sound if every caller proves the exact allocation type through a closed, enforced invariant. A debug assertion alone would not establish that invariant in release builds. The existing boundary checks are not implicated by the measured seconds of texture decoding, so this plan does not introduce unsafe casts or require nightly. Where a slot is constructed as `Arc<Slot<T>>` locally, retain that typed value directly instead of erasing and recovering it.

**6. Address smaller costs only after those changes**

The current source fingerprints, blob integrity checks, and revision hashes total only about 65–70 ms in the harness. Keep them intact.

Bounded preallocation or exact-size decompression can reduce temporary growth/copies, but should be measured after optimizing Zstandard. Validate the declared size first, bound allocation, and check for both truncated and excess decoded output. Do not introduce unsafe uninitialized buffers for this optimization.

Loading only reachable cached outputs and avoiding a second read of a blob are reasonable future refinements. Cerberus uses nearly all of its large texture outputs, so lazy output loading is unlikely to recover seconds here. A larger worker pool, arenas, lock-free handles, packfiles, new serialization frameworks, weaker freshness checks, or disabling validation are not justified by these measurements.

**Acceptance and order**

1. Land the codec optimization and compatibility tests; benchmark it independently.
2. Add the scoped native-library optimization; benchmark again.
3. Parallelize cached blob preparation in the existing bounded pool; compare the same workload with one and two workers after the first two changes. Report this incremental gain separately.
4. Overlap startup work and measure actual first-frame latency.
5. Stop if the target is reached; profile the new remaining bottleneck before broadening scope.

Initial target: **under 500 ms for combined cached debug CPU loading** on the same content and workstation, with zero imports and all current validation enabled. The stage experiments make this a reasonable target, not a promised end-to-end result.

Use one untimed warm-up and at least ten measured runs for final acceptance; report the median and range. Keep debug settings, Slang mode, Vulkan validation, cache contents, watcher state, and debugger attachment consistent. Measure under the IDE as well as directly: these diagnostic runs used debug binaries without an attached debugger. Treat cold filesystem-cache results separately.

Run existing CPU, importer-disabled, packaged-loading, corruption/recovery, dependency/reload, and GPU revision tests. Verify that packaged v2 caches load without rebaking and that failed reloads still preserve the last good value.

For the threading patch, exercise both serial and `parallel` runtime features, including packaged builds without importers. Verify duplicate concurrent requests still decode once, diamonds share dependency slots, cycles fail, and a corrupt blob in a multi-output bundle cannot publish partial revisions. Use a bounded synchronization test to establish that independent blob work overlaps; avoid timing-threshold assertions. Verify errors/panics settle the request and a subsequent load succeeds. Keep production-content timing as a benchmark, not a unit-test assertion.
