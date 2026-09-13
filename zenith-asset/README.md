# Zenith assets

The application owns an `AssetServer`. Loading returns a typed handle; dependencies and cache publication are managed by the server.

```no_run
use zenith_asset::{AssetServer, FileSource, mesh::Scene, texture::Texture};

let assets = AssetServer::builder()
    .source(FileSource::new("content"))
    .cache_dir("asset")
    .with_builtin_assets()
    .build()?;

let scene = assets.load_blocking::<Scene>("mesh/cerberus/scene.gltf")?;
let sky = assets.load_blocking::<Texture>("texture/minedump_flats_4k.hdr")?;
assert!(scene.get().is_some());
# Ok::<(), zenith_asset::AssetError>(())
```

`WorldRenderer::add_scene(gpu, descriptors, &scene)` and `set_skybox(gpu, descriptors, &sky)` are blocking startup conveniences. `queue_scene(&scene)` and `queue_skybox(&sky)` accept loading handles. Rendering polls upload submissions and publishes prepared resources at frame boundaries. Preparation still copies a whole scene's new payloads on the render thread; it has no GPU wait, but is not yet an upload-bytes-per-frame budget.

## Streaming and CPU retention

The world example loads only the skybox during preparation, draws its first frame, then requests and queues Cerberus. Models become visible after their upload completes. `scene_status(index)` returns `Waiting`, `Uploading`, `Ready { revision }`, or `Failed { message }`. A failed optional model preserves the skybox and previously committed models; the same failed revision is not prepared again every frame. `retry_scene(index)` requests a reload through the bound server. A successful content revision also resumes preparation.

Configure CPU retention per Rust type when building the server:

```no_run
use zenith_asset::{AssetServer, CpuRetention, FileSource, mesh::{Mesh, Scene}, texture::Texture};

let assets = AssetServer::builder()
    .source(FileSource::new("content"))
    .cache_dir("asset")
    .with_builtin_assets()
    .cpu_retention::<Mesh>(CpuRetention::ReleaseAfterUpload)
    .cpu_retention::<Texture>(CpuRetention::ReleaseAfterUpload)
    .build()?;
let scene = assets.load::<Scene>("mesh/cerberus/scene.gltf")?;
# Ok::<(), zenith_asset::AssetError>(())
```

`Keep` is the default for all types. The same configuration supports `Scene`, `Material`, custom types, and generated assets. The renderer acknowledges only consumed snapshots after committing a successful GPU upload. It copies scene transforms and material parameters into GPU scene entries, so scene/material CPU payloads may also be released. Unused dependencies are not acknowledged.

Construct the renderer with `WorldRenderer::new(gpu, descriptors, width, height)?.with_asset_server(&assets)` and enqueue with `let index = renderer.queue_scene(&scene)`. The server binding lets a GPU cache miss restore missing CPU data asynchronously through the existing root reload. Restoration preserves the revision when content is unchanged, including when only scene/material metadata must be restored. Keep using the same server for those handles. Existing blocking helpers remain available.

After release, `get()` and `snapshot()` return `None`, `state()` reports `CpuReleased { revision }`, and `revision()` remains available for GPU cache lookup. `wait()` returns `ErrorKind::CpuReleased` if no load is active. Explicit `load` or `reload` restores CPU data; a retained scene whose children were released needs a root `reload`. Unchanged watcher polls do not restore CPU values.

Custom uploaders call `handle.release_cpu(&consumed_snapshot)` only after successful consumption. This respects the type policy and checks both revision and allocation identity, preventing an old completion from evicting a replacement. Release removes the slot's owning reference; external snapshots and `Arc`s keep their allocations alive until dropped. Never call `release_cpu`, request loads, or wait for loading inside `with_snapshot`: that closure holds the publication read lock.

Generated assets have no source to restore. Their existing GPU representation survives CPU release, but recreating it on a cache miss requires application regeneration; use `Keep` when CPU recovery is needed.

`renderer.set_staging_cache_budget(bytes)` controls completed upload-buffer reuse separately. The default is 128 MiB; the world example uses zero so completed staging allocations are freed. In-flight uploads retain their buffers until completion. This implements whole-model streaming; large upload preparation can still cause a frame hitch.

## Extending the system

A runtime value satisfies `Asset` automatically when it is `Any + Send + Sync`. Generated values need no codec or registration.

```
use zenith_asset::AssetServer;

let assets = AssetServer::builder().build()?;
let handle = assets.add(String::from("procedural"));
assert_eq!(&*handle.get().unwrap(), "procedural");
# Ok::<(), zenith_asset::AssetError>(())
```

Persistent types implement `CookedAsset`; importers produce their associated serializable `Data`. This example uses only the public API and works with default features disabled.

```
use serde::{Deserialize, Serialize};
use zenith_asset::{
    AssetServer, CookedAsset, ImportContext, Importer, LoadContext,
    MemorySource, Result,
};

#[derive(Serialize, Deserialize)]
struct Settings {
    exposure: f32,
}

impl CookedAsset for Settings {
    type Data = Self;
    const TYPE_KEY: &'static str = "my_app.settings";
    const SCHEMA_VERSION: u32 = 1;

    fn from_data(data: Self, _: &mut LoadContext<'_>) -> Result<Self> {
        Ok(data)
    }
}

struct JsonSettings;
impl Importer for JsonSettings {
    type Settings = ();
    type Output = Settings;
    const KEY: &'static str = "my_app.settings-json";
    const VERSION: u32 = 1;

    fn extensions(&self) -> &[&str] { &["settings"] }

    fn import(&self, bytes: &[u8], _: &(), _: &mut ImportContext<'_>) -> Result<Settings> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

let source = MemorySource::default();
source.insert("render.settings", br#"{"exposure":1.5}"#.to_vec())?;
let assets = AssetServer::builder()
    .source(source)
    .register_asset::<Settings>()
    .register_importer(JsonSettings)
    .build()?;
let settings = assets.load_blocking::<Settings>("render.settings")?;
assert_eq!(settings.get().unwrap().exposure, 1.5);
# Ok::<(), zenith_asset::AssetError>(())
```

Use a unique stable `TYPE_KEY`; increment `SCHEMA_VERSION` when the cooked layout or runtime conversion changes. Register an `AssetCodec<T>` with `register_codec::<T>(codec)` to replace the default Bincode/Serde encoding. Its `key()` identifies the codec version. Custom codecs must bound allocations and reject trailing/malformed input. Rust `TypeId` is used only for checked runtime dispatch.

`with_builtin_assets()` registers an internal bulk-byte texture codec that preserves the existing v2 Bincode payload, codec key, and 512 MiB decoding limit. It avoids per-pixel-byte Serde iteration. The public `Texture` Serde representation and explicitly registered custom codecs are unchanged. `Handle<T>::get()` and `snapshot()` access typed slots; downcasts remain checked at erased construction and publication boundaries.

`Mesh<V>` supports a custom `VertexLayout` with a distinct `MESH_TYPE_KEY`, Serde, and Bytemuck `NoUninit`. The built-in renderer and glTF importer use `Vertex`; custom layouts require a matching renderer.

Extension collisions and duplicate registrations are setup errors. Register one importer per extension. For explicit options, use `load_with::<Importer>(path, &settings)`; the importer key and canonical settings produce a distinct variant address.

## Dependencies and multi-output imports

Persist `AssetPath<T>` in cooked data, then call `LoadContext::dependency(&path)` during `from_data` to obtain `Handle<T>`. This records readiness, retention, invalidation, and manifest edges. Construct references there; do not wait for, dereference, or load through the same server inside an importer/codec/conversion. Dependencies are staged and become readable when publication finishes.

Importers use `ImportContext::emit::<T>("label", data)` for additional outputs, `path::<T>("label")` for forward references, and `read_relative(path)` for external source inputs. All emitted outputs are validated before their source manifest is committed. Required dependency cycles return a path chain. Referencing another cooked asset does not make its value available to the importer as a build input; processors that consume cooked values are a future API extension.

Addresses use `source://path#label`, with the default namespace omitted. Separators are normalized; root traversal and absolute paths are rejected. Source namespaces are registered through `mount`. Paths preserve case. Use consistent case on case-insensitive filesystems. File sources resolve symlinks and enforce root containment. Paths/labels identify source structure, so renames and reordered glTF indices change identity. No rename/GUID catalog is provided.

## Ownership and reload

- `load<T>` is nonblocking. A live ready handle is reused without disk reads.
- `wait()` waits for the active attempt; failures return an error.
- `get()` returns the last good resident `Arc<T>`; `snapshot()` captures its revision and value together. `revision()` remains available after CPU release.
- `state()`, `last_error()`, `drain_events()`, and `stats()` expose progress and diagnostics.
- `reload(&handle)` checks that source and its required dependencies. Changed leaves also rebuild live ancestors.
- `refresh()` schedules a scan of live sources. `watch(Some(interval))` enables polling; `watch(None)` disables it.

A failed reload retains the previous revision and any resident value. Removed labels retain their last resident value with a missing-output error. Handles follow successful revisions; acquired snapshots retain the old value. A snapshot's dependency handles still follow their slots. Use `handle.with_snapshot(|snapshot| ...)` when reading several related handles from the same server consistently; its closure holds a publication read lock. Keep that closure short, do not nest it, and do not release CPU data or block on loading or reload completion inside it.

Only dependencies registered through `LoadContext` participate in automatic ancestor invalidation. For generated scenes that reference reloadable handles, requeue the scene after those dependencies change.

The slot index is weak. Handles, dependency references, and active work pin values. Dropping the last server disconnects the bounded queue and joins the worker. Existing handles/snapshots remain readable. The queue holds 64 jobs and returns `QueueFull` instead of growing unboundedly. One coordinator resolves whole dependency graphs without waiting for queued child jobs. With `parallel` enabled, independent cached blob reads, integrity checks, and decompression use a two-thread Rayon pool per server, also shared by HDR processing. The receiver waits outside that pool. Dependency conversion and graph publication stay serialized, and all blob work joins before success or failure is reported. Polling hashes source contents and can be expensive for large sources; enable it only for development workflows that need it.

For startup overlap, request assets with `load`, perform independent renderer initialization, then call `wait` before CPU inspection or blocking GPU upload. The world example follows this pattern. Its `assets_ready_by` timing is the elapsed time when readiness is observed after renderer initialization; `asset_wait_after_renderer` measures the remaining wait. Neither is an isolated asset-loader benchmark.

Renderer caches are owned by each `WorldRenderer` and bound to its GPU and descriptor heap. Asset ID plus revision identifies geometry/textures independently of CPU residency; instances and repeated scene additions share them. The cache keeps weak GPU references and pools completed staging buffers up to its configured budget. Render graph submissions retain old resources until their last GPU use completes.

## Cache and packaging

Manifests and blobs live directly under `cache_dir/manifests` and `cache_dir/blobs`; writers coordinate through `cache_dir/writer.lock`. The container format remains version 2, independently of the directory layout. Development loads rebuild missing or stale assets from source; packaged loads require compatible artifacts. Copy both `manifests` and `blobs` into the package's cache directory. Existing nested caches must be moved to this layout or rebuilt; the runtime does not search a `v2` subdirectory.

Manifest identity includes source address, settings variant, and target profile. Manifests record importer/version/settings, source-input BLAKE3 digests, labels, type/schema/codec, sizes, dependencies, and blob integrity digests. Blobs use a bounded versioned container and Zstandard payload. Writers serialize through a filesystem lock, write/sync blobs, and atomically replace the source manifest last. The commit unit is one source bundle; a multi-source graph does not have a single filesystem-wide atomic commit. Unreferenced old blobs are retained; garbage collection is deferred.

Development loads rebuild missing, corrupt, stale, or incompatible containers from available sources. Packaged loads validate manifests/blobs without reading sources or checking importer versions; matching type schemas, codecs, and target remain required. Conversion errors are reported with dependency context. A payload that passes container validation but fails a custom conversion requires correcting its producer or rebaking.

```no_run
use zenith_asset::{AssetServer, mesh::Scene};

let assets = AssetServer::builder()
    .cache_dir("shipped-assets")
    .target("desktop")
    .with_builtin_assets()
    .packaged()
    .build()?;
let scene = assets.load_blocking::<Scene>("mesh/cerberus/scene.gltf")?;
# Ok::<(), zenith_asset::AssetError>(())
```

Use `zenith-asset = { ..., default-features = false }` for a runtime without built-in importers or Rayon. Add `features = ["parallel"]` for parallel cached loading without the importers. Built-in data types, sources, codecs, and custom importer registration remain available. `with_builtin_assets` installs the built-in codecs and, when enabled, their importers. The `importers` default feature enables glTF/image/HDR/ISPC/Half and `parallel`; `parallel` enables the existing optional Rayon dependency. `extra-image-formats` adds Image's optional codecs.

The workspace optimizes only `zstd-sys` at level 3 in development builds. Engine Rust code retains normal debug compilation and checks. See [the measured optimization results](../docs/asset-load-optimization-plan.md).

The included CLI supports built-in scene and texture outputs:

```powershell
cargo run -p zenith-asset --release --example assets -- cook content asset mesh/cerberus/scene.gltf texture/minedump_flats_4k.hdr
cargo run -p zenith-asset --release --no-default-features --example assets -- inspect asset mesh/cerberus/scene.gltf texture/minedump_flats_4k.hdr
```

Set `ZENITH_ASSET_TARGET` for a non-default target in both commands. Persist `handle.path()` as an `AssetPath<T>` when shipping a settings variant; the simple CLI cooks default settings only.

## Built-in semantics

glTF/GLB imports emit `meshes/N/primitives/M`, `materials/N`, `materials/default`, `images/N/color|normal|linear`, and `scenes/N`. The root is the default scene, falling back to scene zero. Node hierarchy, world transforms, multiple roots, shared mesh instances, default materials, nonindexed triangles, triangle strips/fans, and generated flat normals are supported. The default converts Y-up to Zenith's Z-up; settings can disable it. Geometry remains in mesh-local coordinates. Scene node transforms retain glTF local coordinates; instance transforms include coordinate conversion.

Color maps filter in linear light and use sRGB GPU formats when appropriate. Linear-data maps avoid gamma conversion; normals are renormalized through mip generation and use BC5. The same image can emit distinct usage variants. Uncompressed 16-bit inputs use RGBA16 UNORM; floating inputs use RGBA32 float. Odd sizes and tiny BC tails are padded with edge pixels. Texture validation checks dimensions, mip counts, and packed byte footprints.

HDR import samples a cubemap with seam-wrapping bilinear filtering, processes one mip level at a time, and defaults to unsigned BC6H. `HdrSettings::compression = TextureCompression::None` selects RGBA16 float. Face sizes are powers of two up to 2048; smaller explicit sizes reduce bake cost. Settings do not yet expose platform-specific format selection or compression-quality presets.

The renderer forwards material factors and handles world transforms, inverse-transpose normals, tangent-space normal maps, and mirrored-instance winding. Its existing lighting pass still forces nonmetallic diffuse IBL; specular lighting and emissive output remain outside this migration. Emissive fields are retained in CPU materials. Skinning, animation, morph targets, alpha modes, additional UV sets, per-texture samplers/transforms, normal-scale/occlusion controls, and advanced glTF material extensions are not implemented. Unsupported nonzero UV sets and non-triangle topology produce errors; the other listed features are outside the renderer's current subset.

## Verification

```powershell
cargo test -p zenith-asset
cargo test -p zenith-asset --no-default-features
cargo test -p zenith-asset --no-default-features --features parallel
cargo test -p zenith-asset --release --lib -- --ignored --nocapture
cargo test -p zenith-renderer --lib -- --ignored --nocapture
```

The ignored asset test imports the included Cerberus/HDR content and loads a relocated package without sources. The ignored renderer tests require Vulkan validation and the supported GPU features; world-rendering tests also need Slang. See the repository validation script for SDK environment setup.
