# Asset system implementation

Implemented 2026-09-12 from [the design plan](asset-system-plan.md), with extensibility and API simplicity ahead of performance tuning. The public API and examples are in [zenith-asset/README.md](../zenith-asset/README.md).

## Delivered

- Owned `AssetServer`, typed `Handle<T>` slots, immutable revision snapshots, weak residency indexing, generated values, structured errors, and explicit shutdown.
- Open registration for cooked types, codecs, importers, and named sources. Generic associated data/settings types keep erasure inside the registry boundary. A downstream integration test supplies a custom vertex layout without editing core dispatch.
- Validated logical paths and serializable typed references. `handle.path()` preserves settings variants for packaged references.
- Multi-output imports, labeled assets, tracked external source inputs, typed dependency edges, deduplicated loads, cycle errors, ancestor reload propagation, and consistent graph reads through `with_snapshot`.
- Versioned manifests and BLAKE3-addressed blobs, schema/codec/target/settings checks, bounded reads/decompression, writer locking, atomic per-source manifest publication, and development cache recovery.
- A bounded background request queue, an optional two-thread pool for cached blob preparation and HDR processing, polling/events/counters, failed-reload retention, and unchanged-source polling without repeated decode. The `parallel` feature can be enabled without built-in importers.
- Corrected glTF scenes/nodes/instances, default materials, indexed/nonindexed geometry and flat normals; separate image usage variants; 16-bit/float handling; odd dimensions and tiny block-compressed mips; incremental HDR mip compression.
- Renderer caches for shared geometry and texture revisions, pooled staging with device-specific host-write alignment, asynchronous GPU submission polling, and frame-boundary publication. Old revisions remain retained by in-flight render commands.
- CPU texture formats are independent of Vulkan. The renderer performs Vulkan conversion, uses imported transforms and inverse-transpose normals, and supports mirrored instance winding. Its existing diffuse-only lighting behavior remains unchanged.
- Optional built-in importers, a cook/inspect CLI, source-free packaged loading, compiled documentation examples, and a migrated world example that frames imported scene bounds.

New direct dependencies are BLAKE3 for stable fingerprints and Serde JSON for manifests/settings. Serde JSON was already transitive. Bincode/Serde and Zstandard remain the default payload format; no async runtime was added. The asset crate no longer depends on Ash or zenith-core. The follow-up [cached debug optimizations](asset-load-optimization-plan.md) retain v2 payload compatibility and checked downcasts, add bulk texture-byte decoding, and overlap world asset loading with renderer initialization.

## Validation

| Check | Result |
|---|---|
| Workspace tests and all targets | Passed |
| CPU asset tests | 20 unit tests plus one downstream integration test passed |
| Importer-disabled runtime tests | 16 unit tests, downstream integration test, and four documentation examples passed |
| Workspace documentation tests | Passed; four asset examples compiled/executed |
| Workspace all-feature/all-target check | Passed |
| Strict asset Clippy, all targets | Passed |
| Renderer Clippy without dependency linting | Passed with the existing `lighting.rs` argument-count lint excluded |
| GPU asset tests | Both passed with Vulkan synchronization validation |
| Production asset fixture | Cerberus plus the 4K HDR baked and loaded from a relocated cache without sources |
| Release world example | Built and rendered 10 bounded frames successfully; captured image inspected |

GPU tests verify actual geometry readback, repeated-upload deduplication, staging reuse, old-revision lifetime, repeated scene additions, and a texture reload that changes rendered pixels after frame-boundary publication. The discovered noncoherent staging-alignment failure was fixed using the RHI's device alignment.

Unrestricted workspace Clippy still reports pre-existing issues in zenith-core and the renderer's existing lighting function; those unrelated APIs were not rewritten for this migration.

Validation logs and the world capture are generated under `target/validation/asset-system`. The repository validation script now includes importer-disabled asset tests and the ignored GPU asset tests.

## Measurements

These are individual release runs on this workstation, not statistically controlled comparisons with the previous architecture.

| Workload | Measurement |
|---|---|
| Fresh Cerberus + 4K HDR bake into an empty temporary cache | 16.391 s; two imports; nine decoded outputs; 88,858,426 source bytes |
| Load the relocated package with no sources | 247.0 ms; zero imports/source reads; seven decoded outputs |
| Standalone importer-disabled CLI inspection of the same content | Scene 157.1 ms; environment 70.1 ms; zero source reads |
| World startup from development cache | Source/cache loading 265.8 ms; renderer/shader construction 1,034.6 ms; GPU upload 58.6 ms |
| World upload counters | One mesh, four textures, 101,943,492 uploaded bytes, seven staging allocations |

The default environment is a 2048-square BC6H cubemap with 12 mip levels and 33,554,592 bytes. Development cached loading still reads source inputs to establish freshness; packaged loading skips those reads. The synthetic GPU test verifies that adding the same scene again adds no upload bytes and that a texture revision reuses its existing staging allocation.

## Compatibility and deliberate limits

This is an API and cache-generation migration. Old global requestors/handles and output-extension URLs are replaced by an owned server and source-relative paths. Existing legacy files are preserved; source files are needed to rebake them into `asset/v2`. Copy the complete compatible `v2` directory to ship a package.

The public source interface remains synchronous, with blocking work off the application thread. Native async sources, runtime-loaded library plugins, editor GUID/rename catalogs, cooked-asset-consuming build processors, packfiles, zero-copy archived formats, cache garbage collection, partial mip loading, and generational arenas are deferred.

GPU preparation currently copies a full changed scene's new payloads on the render thread before asynchronous submission. It has no streaming GPU wait but does not enforce a per-frame byte budget. GPU caches belong to each WorldRenderer. Generated values are immutable and do not automatically register dependency edges; mixed generated/reloadable graphs must be requeued by their application.

The existing renderer is an opaque diffuse-IBL subset. Animation, skinning, morph targets, alpha modes, extra UV sets, authored sampler/texture transforms, emissive rendering, and full PBR lighting remain outside this change. CPU material data retains emissive fields. See the API guide for supported import settings and limits.
