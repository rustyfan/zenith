# Zenith

A Vulkan renderer with address-based shader inputs, descriptor heaps, VMA allocation, and an explicit render graph. Shader stages are vertex, fragment, and compute.

## Build and run on Windows

Use a Rust MSVC toolchain and Visual Studio C++ build tools. The checked configuration uses Rust 1.97.1 and Slang 2026.17. Slang is needed only when compiling shaders at runtime. Rust builds require no Slang import libraries, bindings, or libclang.

```powershell
$env:SLANG_DIR = 'D:\Software\slang-2026.17-windows-x86_64'

cargo build --workspace --all-targets
cargo run -p zenith-sandbox
cargo run -p zenith-sandbox --example triangle
cargo run -p zenith-sandbox --example world
```

Run from the repository root so asset and shader paths resolve. The world example bakes the included Cerberus scene and HDR environment as needed. WASD/QE move the camera; M toggles diffuse SH visualization.

The GPU must support Vulkan 1.3, `VK_EXT_descriptor_heap`, `VK_KHR_shader_untyped_pointers`, `VK_KHR_unified_image_layouts`, maintenance5, and the queried address/rendering/synchronization features. The RTX 4090 with driver 610.88 passes. The installed AMD integrated GPU lacks the required heap, untyped-pointer, and unified-layout extensions and is rejected with a capability report. Set `ZENITH_ADAPTER` to an adapter-name substring to select a device.

Ash is vendored at Vulkan 1.4.352 because the published 0.38.0+1.3.281 bindings lack the required extensions. All crates, including ash-window and vk-mem, resolve to that same ash source. See [dependency provenance](vendor/README.md).

Shaders access textures and samplers directly through `ResourceDescriptorHeap` and `SamplerDescriptorHeap`. Compile with `Gpu::compile_shader` so Slang's descriptor strides match the allocator. Root blobs still use push-data addresses. The compiler receives both stride options directly; see the [shader ABI](docs/vulkan-api.md#shader-abi).

Shader compilation runs `ZENITH_SLANGC` when set, otherwise `SLANG_DIR/bin/slangc`, otherwise `slangc` on `PATH`. Supply the complete Slang SDK so the compiler can find its companion libraries. Each invocation reads current source/imports and captures SPIR-V through stdout. This removes the Rust FFI/build dependency, but retains the external compiler requirement and adds process startup per compilation.

Debug builds compile shaders with `-O0 -g3` for source-level debugging; release builds use `-O3 -g0`. Set `ZENITH_SHADER_DEBUG=1` to debug shaders in a release application, or `0` to optimize shaders in a debug application. `ZENITH_SHADER_DUMP_DIR=target/shader-dumps` saves SPIR-V and companion text files containing compiler invocations and diagnostics. Set `RUST_LOG=debug` to log invocations and dump paths. Source entry-point names are preserved in SPIR-V.

All Rust diagnostics use `zenith_core::log`. `zenith::launch` and the standalone examples initialize the process-global logger before creating GPU resources. Applications using the RHI directly must call `zenith_core::log::initialize` once before startup. The logger remains installed through resource destruction, including Vulkan callbacks and submission/swapchain cleanup; it has no shutdown guard. Duplicate initialization returns an error without replacing the existing logger. `RUST_LOG` overrides the default level or the engine's `--log-level`. Logs, including informational test results and shutdown statistics, are written to stderr.

The default build includes PNG, JPEG, HDR, BMP, TGA, TIFF and WebP. Enable `extra-image-formats` to restore all `image` default codecs. CPU profiling is disabled by default; `cpu-profiling` enables Puffin scopes and frame collection. GPU timing through `ZENITH_PROFILE` works independently. For example, `cargo run -p zenith-sandbox --example world --features cpu-profiling,extra-image-formats` enables both. Logging retains timestamps, automatic terminal colors, and `RUST_LOG` filters, including regex filters. `RUST_LOG_STYLE` controls color output; CLI help and error messages use plain text.

Asset construction uses `Mesh::new`, `Material { ..Default::default() }`, `Texture { ... }`, and `AssetLoadRequest::new(url).with_source(path)`. Generated builder types have been removed. Existing Serde/bincode/Zstandard asset caches remain compatible. Renderer flags use `DebugMode::DIFFUSE_SH` and `DebugMode::empty()`. See the [dependency audit and validation](docs/dependency-optimization.md).

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

See the [implementation plan](docs/minimal-vulkan-api-plan.md), [API contracts and examples](docs/vulkan-api.md), and [validation evidence](docs/minimal-vulkan-api-progress.md).
