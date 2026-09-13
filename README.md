# Zenith

A Vulkan renderer with address-based shader inputs, descriptor heaps, VMA allocation, and an explicit render graph. Shader stages are vertex, fragment, and compute.

## Prerequisites

The verified development configuration is Windows x64, Rust 1.97.1 with the MSVC toolchain, Slang 2026.17, and an RTX 4090 with driver 610.88. Commands below use PowerShell from the repository root.

| Requirement | Installation |
| --- | --- |
| Git | Install [Git for Windows](https://git-scm.com/download/win) and make `git` available on `PATH`. |
| C/C++ compiler and linker | Install Visual Studio or its Build Tools with **Desktop development with C++**, including the MSVC x64/x86 tools and a Windows SDK. Native dependencies compile C/C++ during the Rust build. See [Rust's MSVC prerequisites](https://rust-lang.github.io/rustup/installation/windows-msvc.html). |
| Rust and Cargo | Install the x64 Windows MSVC toolchain using [rustup](https://rust-lang.org/tools/install/). The commands below select the tested Rust version for this checkout. |
| Slang SDK | Download the Windows x86-64 binary archive from [Slang 2026.17](https://github.com/shader-slang/slang/releases/tag/v2026.17) and extract the complete SDK, including `bin/slangc.exe` and its companion libraries. |
| Vulkan driver | Install a GPU driver that exposes the features listed below. The GPU driver supplies the Vulkan runtime. |
| Vulkan validation layers | For validation and GPU acceptance tests, install the [LunarG Vulkan SDK](https://vulkan.lunarg.com/sdk/home). Keep its validation manifest and DLL together in the SDK's `Bin` directory. |

The renderer requires Vulkan 1.3, `VK_EXT_descriptor_heap`, `VK_KHR_shader_untyped_pointers`, `VK_KHR_unified_image_layouts`, `VK_KHR_maintenance5`, and the queried buffer device address, timeline semaphore, dynamic rendering, synchronization, and shader features. Vulkan 1.3 support alone is insufficient. Unsupported adapters are rejected with a capability report; the tested AMD integrated GPU lacks the required extensions.

Rust compilation does not require the Slang SDK, Slang import libraries, bindings, or libclang. Shader-based examples require the complete Slang SDK at runtime, including on shader cache hits. Reopen PowerShell after installing tools so it inherits their updated environment.

## Clone and configure

```powershell
git clone https://github.com/rustyfan/zenith.git
cd zenith
rustup toolchain install 1.97.1-x86_64-pc-windows-msvc
rustup override set 1.97.1-x86_64-pc-windows-msvc
rustc --version
cargo --version
```

For an existing checkout, start in its root directory and configure its toolchain there. Vendored dependencies are already in `vendor/`; no submodule initialization is needed.

Set `SLANG_DIR` to your extracted SDK directory, replacing the example path:

```powershell
$env:SLANG_DIR = 'C:\SDKs\slang-2026.17-windows-x86_64'
& "$env:SLANG_DIR\bin\slangc.exe" -version
```

For validation, set the layer path after installing the Vulkan SDK:

```powershell
$env:VK_LAYER_PATH = Join-Path $env:VULKAN_SDK 'Bin'
Test-Path "$env:VK_LAYER_PATH\VkLayer_khronos_validation.json"
Test-Path "$env:VK_LAYER_PATH\VkLayer_khronos_validation.dll"
$env:VK_LAYER_VALIDATE_SYNC = '1'
```

Both checks should return `True`. If `VULKAN_SDK` is unset, assign `VK_LAYER_PATH` directly to your extracted SDK's `Bin` directory instead. These environment settings apply to the current PowerShell process and programs launched from it. Repeat them in a new terminal, or configure them in your user environment or IDE run configuration.

## Assets

The `world` and `pbr_preview` examples and the production asset import test need `content/mesh/cerberus/scene.gltf`, its `scene.bin` and three PNG textures, and `content/texture/minedump_flats_4k.hdr`. These assets are currently tracked in the repository. Preserve the Cerberus license and attribution alongside the model. Sphere geometry is generated in code; its IBL preview still needs the HDR environment.

The planned external asset dependency is **`zenith-assets`**, hosted at `ghcr.io/rustyfan/zenith-assets`, containing **`zenith-assets.zip`**. The migration will add a root `Setup.bat` to fetch the pinned package and restore the existing `content/` paths. Package publication and `Setup.bat` are not implemented in this revision; the current build instructions use the checked-in assets. Shader source remains in Git, and cooked assets and shader caches are generated locally.

## Build and run

Build all workspace crates and examples:

```powershell
cargo build --workspace --all-targets
```

The first build downloads Cargo dependencies and compiles native libraries. Add `--offline` only after dependencies have been downloaded. Build output goes to `target/debug`; add `--release` for optimized binaries in `target/release`.

Inspect the installed Vulkan adapters before launching a demo:

```powershell
cargo run -p zenith-rhi --example capabilities
```

This prints adapter capabilities; successful enumeration does not imply every adapter can run the renderer. To select a particular GPU, set an adapter-name substring, for example `$env:ZENITH_ADAPTER = 'RTX 4090'`. Remove the override with `Remove-Item Env:ZENITH_ADAPTER -ErrorAction SilentlyContinue`.

Run one demo at a time and close its window to return to PowerShell:

| Demo | Command |
| --- | --- |
| Clear-color window | `cargo run -p zenith-sandbox` |
| Animated triangle | `cargo run -p zenith-sandbox --example triangle` |
| Cerberus PBR scene and sphere comparison | `cargo run -p zenith-sandbox --example world` |
| Optimized PBR demo | `cargo run -p zenith-sandbox --example world --release` |

Run from the repository root so relative asset and shader paths resolve, including when launching `target/debug/examples/world.exe` directly or through an IDE. The first world launch cooks the HDR and model assets and compiles shaders, so startup takes longer than subsequent runs. The skybox can appear before Cerberus finishes loading.

### World controls

| Input | Action |
| --- | --- |
| Hold left mouse button and move mouse | Look around; release the button to release the cursor. |
| W / S | Move forward / backward. |
| A / D | Move left / right. |
| Q / E | Move down / up along the camera's local up axis. |
| T | Switch between Cerberus and the roughness/metallic sphere grid. |
| M | Toggle diffuse SH visualization. |
| L | Toggle directional lighting. |
| I | Toggle skylight. |

In the sphere grid, roughness increases from 0 to 1 left to right and metallic increases from 0 to 1 top to bottom. Switching preserves each view's camera and shares the lighting settings.

### Headless previews

Generate BMP previews under `target/pbr-preview` with validation enabled:

```powershell
cargo run -p zenith-sandbox --example pbr_preview
cargo run -p zenith-sandbox --example pbr_preview -- cerberus
cargo run -p zenith-sandbox --example pbr_preview -- cerberus closeup
```

Each run saves combined, directional-only, and IBL-only lighting without opening a window. These previews require a compatible GPU, Slang, the validation layer, and the corresponding scene assets.

### Optional asset cooking

Pre-cook the skybox to move HDR conversion out of interactive startup:

```powershell
./scripts/cook-startup-assets.ps1
```

The script only prepares `texture/minedump_flats_4k.hdr`; subsequent calls reuse a valid cache. `-Source` and `-Cache` override `ZENITH_CONTENT` and `ZENITH_ASSET_CACHE`. Add `-Offline` once dependencies are available. To cook both the model and skybox in advance:

```powershell
cargo run -p zenith-asset --release --example assets -- cook content asset mesh/cerberus/scene.gltf texture/minedump_flats_4k.hdr
```

Runtime still cooks missing or stale assets automatically. Source assets live in `content/`; generated data lives in `asset/` by default. `ZENITH_CONTENT` and `ZENITH_ASSET_CACHE` can point the examples to other directories. `ZENITH_CONTENT` changes model/texture lookup; shader paths still resolve from the repository root.

## Rendering and development

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

The world example starts loading its skybox before Vulkan initialization and presents a skybox-only frame, then loads and inserts Cerberus asynchronously after GPU upload. Geometry shaders and their pipeline are prepared in the background when the model becomes ready. Mesh and texture CPU payloads are released on completion, with retention configurable per asset type; completed staging buffers are also freed. Scene/material metadata stays resident by default. See [streaming and retention](zenith-asset/README.md#streaming-and-cpu-retention).

Ash is vendored at Vulkan 1.4.352 because the published 0.38.0+1.3.281 bindings lack the required extensions. All crates, including ash-window and vk-mem, resolve to that same ash source. See [dependency provenance](vendor/README.md).

Shaders access textures and samplers directly through `ResourceDescriptorHeap` and `SamplerDescriptorHeap`. Compile with `Gpu::compile_shader` so Slang's descriptor strides match the allocator. Root blobs still use push-data addresses. The compiler receives both stride options directly; see the [shader ABI](docs/vulkan-api.md#shader-abi).

Shader compilation runs `ZENITH_SLANGC` when set, otherwise `SLANG_DIR/bin/slangc`, otherwise `slangc` on `PATH`. Supply the complete Slang SDK so the compiler can find its companion libraries. Successful compilation persists SPIR-V under `asset/shaders/v1`. Cache hits validate source/import content hashes, include-directory entries, compiler/SDK fingerprint, arguments, and descriptor strides without launching Slang. Compiler file size/modification metadata provides a fast check of the saved compiler fingerprint. This is a development cache and still requires source files and the SDK.

Set `ZENITH_SHADER_CACHE` to another directory, or `0` to disable shader caching. Corrupt or incompatible artifacts are recompiled; unavailable cache storage falls back to compilation. Per-artifact OS file locks and atomic writes support simultaneous requests and processes. `Gpu::compile_shaders([...])` preserves request order and compiles a batch with up to four scoped worker threads. The world example uses it for its three startup shaders and deferred geometry pair.

Debug builds compile shaders with `-O0 -g3` for source-level debugging; release builds use `-O3 -g0`. Set `ZENITH_SHADER_DEBUG=1` to debug shaders in a release application, or `0` to optimize shaders in a debug application. `ZENITH_SHADER_DUMP_DIR=target/shader-dumps` saves SPIR-V and companion text files containing compiler invocations and diagnostics. Set `RUST_LOG=debug` to log invocations and dump paths. Source entry-point names are preserved in SPIR-V.

All Rust diagnostics use `zenith_core::log`. `zenith::launch` and the standalone examples initialize the process-global logger before creating GPU resources. Applications using the RHI directly must call `zenith_core::log::initialize` once before startup. The logger remains installed through resource destruction, including Vulkan callbacks and submission/swapchain cleanup; it has no shutdown guard. Duplicate initialization returns an error without replacing the existing logger. `RUST_LOG` overrides the default level or the engine's `--log-level`. Logs, including informational test results and shutdown statistics, are written to stderr.

The default build includes PNG, JPEG, HDR, BMP, TGA, TIFF and WebP. Enable `extra-image-formats` to restore all `image` default codecs. CPU profiling is disabled by default; `cpu-profiling` enables Puffin scopes and frame collection. GPU timing through `ZENITH_PROFILE` works independently. For example, `cargo run -p zenith-sandbox --example world --features cpu-profiling,extra-image-formats` enables both. Logging retains timestamps, automatic terminal colors, and `RUST_LOG` filters, including regex filters. `RUST_LOG_STYLE` controls color output; CLI help and error messages use plain text.

Assets use an application-owned `AssetServer`, typed `Handle<T>`, registered importers/codecs, and dependency-aware reload. The world example uses source-relative glTF/HDR paths and caches version 2 manifests/artifacts under `asset/v2`; previous caches are preserved and rebaked. `ZENITH_CONTENT` and `ZENITH_ASSET_CACHE` override its directories. See the [asset API, extension examples, and packaging guide](zenith-asset/README.md). Renderer flags use `DebugMode::DIFFUSE_SH` and `DebugMode::empty()`.

## Tests and validation

Run the default workspace tests and documentation tests:

```powershell
cargo test --workspace --all-targets
cargo test --workspace --doc
```

GPU/Slang acceptance tests and the production asset import test are marked ignored and require explicit execution. Configure the SDKs above before running the complete validation script below. Interactive demos enable validation in debug builds; set `$env:ZENITH_VALIDATION = '1'` to enable it in release builds.

On Windows, `Instance::new(..., true)` automatically configures validation layer discovery before loading Vulkan. It checks `VULKAN_SDK/Bin`, then this build's repository-local `target/vulkan-sdk/Bin`, for the validation manifest and DLL. This works for Cargo, RustRover debugging, and direct executable launches without a special run configuration. Explicit `VK_LAYER_PATH` or `VK_ADD_LAYER_PATH` settings take precedence. With `validation = false`, the application skips discovery and does not enable the validation layer.

The SDK must still be installed or extracted. `target/vulkan-sdk` is an optional local SDK location and is not included in a clone. The repository fallback refers to the source location at build time. On other platforms or machines without that directory, install the validation layer normally or configure its search path explicitly. If unavailable, interactive applications warn and continue without validation; GPU acceptance tests require it.

```powershell
./scripts/validate-vulkan.ps1 -SlangDir $env:SLANG_DIR -ValidationDir $env:VK_LAYER_PATH -WindowTests
```

Add `-Offline` once dependencies are downloaded. The script runs workspace tests, debug/release builds, GPU readbacks, the graph suite, and bounded window tests. Logs go to `target/validation`. Omit `-WindowTests` to skip opening test windows. The SDK arguments avoid the script's machine-specific fallback paths.

To run only the graph smoke test in debug and release:

```powershell
./scripts/validate-vulkan.ps1 -SlangDir $env:SLANG_DIR -ValidationDir $env:VK_LAYER_PATH -GraphOnly
```

For a direct Cargo run, first configure Slang and validation as above, then run from the repository root:

```powershell
cargo run -p zenith-rendergraph --example graph_smoke
```

The Cargo example name is `graph_smoke`, with an underscore. It is a headless GPU test that prints `PASS`; it does not open a window. If it reports `graph acceptance requires validation`, Vulkan could not find `VK_LAYER_KHRONOS_validation`; ensure `VK_LAYER_PATH` points to the directory containing `VkLayer_khronos_validation.json`.

To run the Cerberus/sphere toggle regression test explicitly:

```powershell
cargo test -p zenith-sandbox --example world -- --ignored --test-threads=1
```

It requires the production assets, Slang, Vulkan validation, and a compatible GPU. The main validation script does not explicitly run this ignored example test.

In RustRover, select **Graph Smoke** in the run configuration dropdown. Before Run or Debug, edit **Run > Edit Configurations > Graph Smoke** to set `SLANG_DIR` and `VK_LAYER_PATH` to your SDK directories and the working directory to the repository root. The checked-in [.run/Graph Smoke.run.xml](.run/Graph%20Smoke.run.xml) contains paths from the development machine. Running the PowerShell validation script does not configure RustRover's environment. If the configuration does not appear, reopen the project.

`ZENITH_TEST_FRAMES` enables bounded application runs, `ZENITH_TEST_TIME` fixes triangle animation time, `ZENITH_TEST_RESIZE` exercises resize/minimize, `ZENITH_VALIDATION` enables validation in release builds, and `ZENITH_PROFILE` prints GPU pass timings. These controls are optional.

### Startup profiling

After building the debug world example and configuring the SDK environment above, measure process creation to first accepted presentation with:

```powershell
./scripts/profile-startup.ps1 -Label cached -Runs 7
./scripts/profile-startup.ps1 -Label cold -ColdShaders -Runs 5
```

Each batch excludes one warm-up run from its median and saves every run under `target/startup-measurements`. Choose a new label for each cold-cache batch. `-NoShaderCache` measures uncached compilation without writing shaders. The script enables debug shaders, Vulkan synchronization validation, and GPU timing; it measures one frame with no model request. `ZENITH_STARTUP_PROFILE=<file.csv>` enables the underlying first-presentation and first-submission-completion timestamps. Normal runs do not wait for this additional profiling measurement. See [startup results and limitations](docs/startup-optimization-results.md).

See the [implementation plan](docs/minimal-vulkan-api-plan.md), [API contracts and examples](docs/vulkan-api.md), and [validation evidence](docs/minimal-vulkan-api-progress.md).

## Troubleshooting

| Symptom | Resolution |
| --- | --- |
| `cargo` or `rustc` is not found | Reopen PowerShell after installing rustup and check that `%USERPROFILE%\.cargo\bin` is on `PATH`. |
| Missing `link.exe`, `cl.exe`, or Windows libraries | Install the C++ workload and Windows SDK, confirm the MSVC Rust toolchain, then retry from Visual Studio's Developer PowerShell configured for x64. |
| Vulkan loader fails to load or no adapters are enumerated | Install the GPU vendor's Vulkan-capable driver; the validation SDK alone does not provide a GPU driver. |
| Adapter rejected for missing features/extensions | Read the capability report, install a driver exposing the required extensions, and select a compatible GPU using `ZENITH_ADAPTER`. |
| `slangc` is missing, cannot load a DLL, or rejects heap-related shader arguments | Use the complete Slang 2026.17 SDK and check `SLANG_DIR`. An existing `ZENITH_SLANGC` override takes precedence; clear it if it points to an old compiler. |
| Shader file or source asset not found | Launch from the repository root, verify `content/shaders` and the model/HDR files, and check `ZENITH_CONTENT` overrides. |
| `graph acceptance requires validation` or validation layer unavailable | Set `VK_LAYER_PATH` to the SDK's `Bin` containing both `VkLayer_khronos_validation.json` and its DLL. Check for stale `VK_LAYER_PATH` or `VK_ADD_LAYER_PATH` overrides. |
| PowerShell refuses to run a repository script | After reviewing the script, invoke it with a process-local policy, for example `powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\cook-startup-assets.ps1`. |
| First world launch is slow or initially shows only the skybox | Allow asset cooking and streaming to finish, or run the optional cooking command before launching. |
| More diagnostics are needed | Set `$env:RUST_LOG = 'debug'` and `$env:RUST_BACKTRACE = '1'`, then rerun the failing command. Logs are written to stderr. |
