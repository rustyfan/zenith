# Zenith

A Vulkan renderer with address-based shader inputs, descriptor heaps, VMA allocation, and an explicit render graph. Shader stages are vertex, fragment, and compute.

## Build and run on Windows

Use a Rust MSVC toolchain and Visual Studio C++ build tools. The checked configuration uses Rust 1.97.1 and Slang 2026.17. Runtime shader loading requires the Slang SDK to validate its compiler fingerprint; compilation runs only on cache misses. Rust builds require no Slang import libraries, bindings, or libclang.

```powershell
$env:SLANG_DIR = 'D:\Software\slang-2026.17-windows-x86_64'

cargo build --workspace --all-targets
cargo run -p zenith-sandbox
cargo run -p zenith-sandbox --example triangle
cargo run -p zenith-sandbox --example world
```

Run from the repository root so asset and shader paths resolve. The world example bakes the included Cerberus scene and HDR environment as needed. WASD/QE move the camera; T switches between Cerberus and the sphere comparison, M toggles diffuse SH visualization, L toggles directional lighting, and I toggles skylight. In the sphere grid, roughness increases from 0 to 1 left to right and metallic increases from 0 to 1 top to bottom. Switching preserves each view's camera and shares the lighting settings.

World lighting uses metallic/roughness materials with GGX specular, Smith height-correlated visibility, Schlick Fresnel, and Lambert diffuse. The skylight combines the stashed seven-vector cosine-convolved SH representation (including Lambert's 1/pi) with a 128-pixel GGX-prefiltered cubemap and a 128x128 RG32F BRDF LUT. The LUT is generated once per renderer. SH and the specular mip chain are regenerated on skybox replacement and submitted on the render queue; resources are retained until GPU completion. Lighting and skybox share exposure and ACES tone mapping, with one sRGB encoding at output. Directional lighting currently has no shadow map.

Configure the world-space direction toward the light, linear RGB color, intensities, and linear exposure multiplier through `WorldRenderer::set_lighting`:

```rust
let mut lighting = renderer.lighting_settings();
lighting.directional.direction_to_light = glam::Vec3::new(0.5, -0.5, 1.0);
lighting.directional.color = glam::Vec3::ONE;
lighting.directional.intensity = 1.0;
lighting.sky_intensity = 1.0;
lighting.exposure = 1.0;
renderer.set_lighting(lighting)?;
```

Generate headless BMP previews under `target/pbr-preview` with validation enabled:

```powershell
cargo run -p zenith-sandbox --example pbr_preview
cargo run -p zenith-sandbox --example pbr_preview -- cerberus
```

Each run saves combined, directional-only, and IBL-only lighting. Sphere roughness increases from 0 to 1 across columns; metallic increases from 0 to 1 down rows.

Pre-cook the required skybox once during project setup to move its HDR conversion out of interactive startup:

```powershell
./scripts/cook-startup-assets.ps1 -Offline
```

Omit `-Offline` if dependencies have not been downloaded. The script uses the existing release asset cooker and only prepares `texture/minedump_flats_4k.hdr`; subsequent calls reuse its v2 cache. `-Source` and `-Cache` override `ZENITH_CONTENT` and `ZENITH_ASSET_CACHE`. Runtime still cooks missing or stale assets automatically.

The world example starts loading its skybox before Vulkan initialization and presents a skybox-only frame, then loads and inserts Cerberus asynchronously after GPU upload. Geometry shaders and their pipeline are prepared in the background when the model becomes ready. Mesh and texture CPU payloads are released on completion, with retention configurable per asset type; completed staging buffers are also freed. Scene/material metadata stays resident by default. See [streaming and retention](zenith-asset/README.md#streaming-and-cpu-retention).

The GPU must support Vulkan 1.3, `VK_EXT_descriptor_heap`, `VK_KHR_shader_untyped_pointers`, `VK_KHR_unified_image_layouts`, maintenance5, and the queried address/rendering/synchronization features. The RTX 4090 with driver 610.88 passes. The installed AMD integrated GPU lacks the required heap, untyped-pointer, and unified-layout extensions and is rejected with a capability report. Set `ZENITH_ADAPTER` to an adapter-name substring to select a device.

Ash is vendored at Vulkan 1.4.352 because the published 0.38.0+1.3.281 bindings lack the required extensions. All crates, including ash-window and vk-mem, resolve to that same ash source. See [dependency provenance](vendor/README.md).

Shaders access textures and samplers directly through `ResourceDescriptorHeap` and `SamplerDescriptorHeap`. Compile with `Gpu::compile_shader` so Slang's descriptor strides match the allocator. Root blobs still use push-data addresses. The compiler receives both stride options directly; see the [shader ABI](docs/vulkan-api.md#shader-abi).

Shader compilation runs `ZENITH_SLANGC` when set, otherwise `SLANG_DIR/bin/slangc`, otherwise `slangc` on `PATH`. Supply the complete Slang SDK so the compiler can find its companion libraries. Successful compilation persists SPIR-V under `asset/shaders/v1`. Cache hits validate source/import content hashes, include-directory entries, compiler/SDK fingerprint, arguments, and descriptor strides without launching Slang. Compiler file size/modification metadata provides a fast check of the saved compiler fingerprint. This is a development cache and still requires source files and the SDK.

Set `ZENITH_SHADER_CACHE` to another directory, or `0` to disable shader caching. Corrupt or incompatible artifacts are recompiled; unavailable cache storage falls back to compilation. Per-artifact OS file locks and atomic writes support simultaneous requests and processes. `Gpu::compile_shaders([...])` preserves request order and compiles a batch with up to four scoped worker threads. The world example uses it for its three startup shaders and deferred geometry pair.

Debug builds compile shaders with `-O0 -g3` for source-level debugging; release builds use `-O3 -g0`. Set `ZENITH_SHADER_DEBUG=1` to debug shaders in a release application, or `0` to optimize shaders in a debug application. `ZENITH_SHADER_DUMP_DIR=target/shader-dumps` saves SPIR-V and companion text files containing compiler invocations and diagnostics. Set `RUST_LOG=debug` to log invocations and dump paths. Source entry-point names are preserved in SPIR-V.

All Rust diagnostics use `zenith_core::log`. `zenith::launch` and the standalone examples initialize the process-global logger before creating GPU resources. Applications using the RHI directly must call `zenith_core::log::initialize` once before startup. The logger remains installed through resource destruction, including Vulkan callbacks and submission/swapchain cleanup; it has no shutdown guard. Duplicate initialization returns an error without replacing the existing logger. `RUST_LOG` overrides the default level or the engine's `--log-level`. Logs, including informational test results and shutdown statistics, are written to stderr.

The default build includes PNG, JPEG, HDR, BMP, TGA, TIFF and WebP. Enable `extra-image-formats` to restore all `image` default codecs. CPU profiling is disabled by default; `cpu-profiling` enables Puffin scopes and frame collection. GPU timing through `ZENITH_PROFILE` works independently. For example, `cargo run -p zenith-sandbox --example world --features cpu-profiling,extra-image-formats` enables both. Logging retains timestamps, automatic terminal colors, and `RUST_LOG` filters, including regex filters. `RUST_LOG_STYLE` controls color output; CLI help and error messages use plain text.

Assets use an application-owned `AssetServer`, typed `Handle<T>`, registered importers/codecs, and dependency-aware reload. The world example uses source-relative glTF/HDR paths and caches version 2 manifests/artifacts under `asset/v2`; previous caches are preserved and rebaked. `ZENITH_CONTENT` and `ZENITH_ASSET_CACHE` override its directories. See the [asset API, extension examples, and packaging guide](zenith-asset/README.md). Renderer flags use `DebugMode::DIFFUSE_SH` and `DebugMode::empty()`.

## Validation

On Windows, `Instance::new(..., true)` automatically configures validation layer discovery before loading Vulkan. It checks `VULKAN_SDK/Bin`, then this build's repository-local `target/vulkan-sdk/Bin`, for the validation manifest and DLL. This works for Cargo, RustRover debugging, and direct executable launches without a special run configuration. Explicit `VK_LAYER_PATH` or `VK_ADD_LAYER_PATH` settings take precedence. With `validation = false`, the application skips discovery and does not enable the validation layer.

The SDK must still be installed or extracted; this workspace has a copy-only SDK at `target/vulkan-sdk`. The repository fallback refers to the source location at build time. On other platforms or machines without that directory, install the validation layer normally or configure its search path explicitly. If unavailable, the application warns and continues without validation.

```powershell
./scripts/validate-vulkan.ps1 -WindowTests -Offline
```

Omit `-Offline` when downloading dependencies for the first time. The script runs workspace tests, debug/release builds, GPU readbacks, the graph suite, and bounded window tests. Logs go to `target/validation`. Headless acceptance tests require the validation layer and fail if it is unavailable.

To run only the graph smoke test in debug and release, with the local Slang SDK and validation layer configured automatically:

```powershell
./scripts/validate-vulkan.ps1 -GraphOnly -Offline
```

The script accepts `-SlangDir` and `-ValidationDir` for SDKs in other locations. For a direct Cargo run, first configure Slang as above, then run from the repository root:

```powershell
$env:VK_LAYER_PATH = (Resolve-Path 'target/vulkan-sdk/Bin').Path
$env:VK_LAYER_VALIDATE_SYNC = '1'
cargo run -p zenith-rendergraph --example graph_smoke
```

The Cargo example name is `graph_smoke`, with an underscore. It is a headless GPU test that prints `PASS`; it does not open a window. If it reports `graph acceptance requires validation`, Vulkan could not find `VK_LAYER_KHRONOS_validation`; ensure `VK_LAYER_PATH` points to the directory containing `VkLayer_khronos_validation.json`.

In RustRover, select **Graph Smoke** in the run configuration dropdown, then Run or Debug. The project configuration in [.run/Graph Smoke.run.xml](.run/Graph%20Smoke.run.xml) supplies the Slang SDK location and Vulkan validation layer to the executable. Running the PowerShell validation script does not configure RustRover's environment. If the configuration does not appear after switching back to the IDE, reopen the project. For a different SDK location, edit the environment variables under **Run > Edit Configurations > Graph Smoke**.

`ZENITH_TEST_FRAMES` enables bounded application runs, `ZENITH_TEST_TIME` fixes triangle animation time, `ZENITH_TEST_RESIZE` exercises resize/minimize, `ZENITH_VALIDATION` enables validation in release builds, and `ZENITH_PROFILE` prints GPU pass timings. These controls are optional.

After building the world example, measure process creation to first accepted presentation with:

```powershell
./scripts/profile-startup.ps1 -Label cached -Runs 7
./scripts/profile-startup.ps1 -Label cold -ColdShaders -Runs 5
```

Each batch excludes one warm-up run from its median and saves every run under `target/startup-measurements`. Choose a new label for each cold-cache batch. `-NoShaderCache` measures uncached compilation without writing shaders. The script enables debug shaders, Vulkan synchronization validation, and GPU timing; it measures one frame with no model request. `ZENITH_STARTUP_PROFILE=<file.csv>` enables the underlying first-presentation and first-submission-completion timestamps. Normal runs do not wait for this additional profiling measurement. See [startup results and limitations](docs/startup-optimization-results.md).

See the [implementation plan](docs/minimal-vulkan-api-plan.md), [API contracts and examples](docs/vulkan-api.md), and [validation evidence](docs/minimal-vulkan-api-progress.md).
