# Dependency optimization

## Task 1: rendering

Replace the Slang Rust FFI dependency with the SDK's `slangc` executable. Use
binary stdout, with no intermediate files or additional Rust crates. Preserve
SPIR-V 1.6, descriptor heap capabilities and device strides, column-major
matrices, scalar layout, explicit shader stages and fresh compilation of imports.
Treat Slang's stage-mismatch diagnostic as an error.

Debug builds use `-O0 -g3`; release builds use `-O3 -g0`. Keep entry-point names
in SPIR-V. Allow a runtime debug override, compiler executable override, and
optional SPIR-V/command dumps. Report the command and source diagnostics when
compilation fails. Remove bindgen/libclang from the application build path.

Validate compiler errors, named vertex/fragment/compute entries, debug/release
output, include reload, descriptor heap GPU readbacks and render-graph execution
before starting Task 2.

## Task 2: other dependencies

Audit each direct dependency and its enabled features. Remove unused crates and
redundant serialization derives. Narrow broad feature sets, make CPU profiling
opt-in, and preserve existing asset cache compatibility and supported import
formats. Retain dependencies when removing them would add complexity or lose
useful functionality. Validate asset imports, caches and application examples.

## Compiler reference

[Slang CLI options](https://docs.shader-slang.org/en/stable/external/slang/docs/command-line-slangc-reference.html)
document stage selection, binary stdout, debug levels, optimization, scalar
layout and descriptor heap stride overrides. The local Slang 2026.17 CLI help
also confirms these options.

## Direct dependency decisions

Every workspace manifest was inspected alongside source use sites and the Cargo
feature graph. No replacement crate was added to the resolved graph: `bitflags`
was already used by Vulkan/window dependencies. Internal workspace edges remain
because their types or implementations are used by the consuming crate.

| Crate | Decision and reason |
| --- | --- |
| shader-slang / shader-slang-sys | Removed along with their vendor directory; invoke the external SDK's `slangc`. Removes bindgen and clang-sys from Rust builds. |
| enumflags2 | Replaced with existing `bitflags`; renderer needs a simple `u32` mask, not enum conversion/procedural derives. |
| derive_builder | Removed; struct literals, `Mesh::new`, `Material::default`, and `AssetLoadRequest::new(...).with_source(...)` cover the actual construction sites. Required fields are checked by Rust. |
| paste | Removed; five direct crate re-exports preserve facade paths without identifier concatenation. |
| puffin_http | Removed; no server was ever initialized or used. |
| profiling | Kept as an inert facade by default; `cpu-profiling` opts into Puffin scopes and frame collection. |
| image | Kept with PNG/JPEG/HDR/BMP/TGA/TIFF/WebP enabled. Heavy AVIF/EXR and other default codecs are available through `extra-image-formats`. |
| bincode | Kept with only `std` and `serde`; removed unused `Encode`/`Decode` derives and their macro dependency. The actual cache already used Serde. |
| serde | Kept for asset and glTF serialization; derives are used. |
| zstd | Kept for compressed v2 asset containers; disabled legacy decoders, dictionary training and unused array helpers. |
| derive_more | Kept only for angle arithmetic/conversion/deref derives; disabled `full`. Removed the asset crate's direct dependency by writing its one `From<PathBuf>` implementation. |
| env_logger | Kept default features, including timestamps, automatic terminal colors and regex filtering. Preserves process-global logging, module filters and teardown diagnostics. |
| clap | Kept with derive/std/help/usage/error-context; disabled color and suggestions. Avoids writing a custom parser for forwarding positional arguments and handling errors. |
| hashbrown | Kept fast input/asset maps; enabled only default-hasher and inline-more. Removes allocator-api2 and unused raw-entry/equivalent features. |
| foldhash | Kept for the public hasher and hashbrown; replacing it would alter hashing performance without eliminating the transitive dependency. |
| smallvec | Kept for inline key binding storage; avoids allocations for one-key bindings. |
| parking_lot | Kept for the shared asset registry; compact locks and existing non-poisoning behavior. |
| rayon | Kept for parallel HDR conversion, mip generation and BC6H compression. Serial replacement would remove useful parallelism. |
| ispc-texcomp | Kept for BC5/BC6H/BC7 encoding. A replacement must preserve quality, formats and throughput; no speculative codec rewrite. Uses the crate's shipped kernels, not build-time ISPC compilation. |
| half | Kept for f32-to-f16 conversion consumed by BC6H. |
| gltf | Kept import/utils for data URIs, embedded images and mesh readers; disabled unused names. |
| memmap2 | Removed from Zenith's direct dependencies along with the unused core helper and asset mapping experiment. Still transitive through `winit` on other platforms. |
| anyhow | Kept for contextual errors throughout the workspace. |
| log | Kept for the shared logger and Vulkan callbacks. |
| glam | Kept for SIMD math in cameras, geometry, shaders and examples. |
| bytemuck | Kept for checked POD derives and GPU byte layouts; replacing these with unchecked casts would weaken the ABI contract. |
| bitflags | Kept/reused for renderer debug modes; declarative macro, no derive dependency. |
| ash | Kept the vendored Vulkan 1.4.352 bindings required by descriptor heaps. std/debug/loaded are used. |
| ash-window | Kept platform surface integration against the same ash version. |
| vk-mem | Kept the Vulkan allocator; a custom allocator would add substantial implementation and validation work. |
| raw-window-handle | Kept for portable window/display surface handles. |
| winit | Kept for events, window lifecycle and input. Platform defaults are preserved. Sandbox references moved to dev-dependencies because only examples need them. |

## Results and compatibility

For the default workspace graph on `x86_64-pc-windows-msvc`, resolved packages
decreased **218 → 206 after rendering → 136 after the remaining work** (82 fewer,
37.6%). These counts include the seven workspace packages and build/dev
dependencies. Optional feature graphs are larger. Counts are dependency counts,
not measured clean-build time or runtime performance improvements.

With both optional features enabled, the graph contains 187 packages (31 fewer
than the original). This retains all image default codecs and CPU profiling.

Runtime shader compilation still needs the complete Slang SDK and incurs process
startup per compile. Source imports are freshly read each time, with no stale
shader cache. Debug builds deliberately favor debuggability over shader speed.
Only Windows/NVIDIA GPU behavior was exercised on this machine.

Public API migrations: asset `*Builder` types are replaced with constructors and
struct literals; `DebugMode::DiffuseSH`/`BitFlags<DebugMode>` become
`DebugMode::DIFFUSE_SH`/`DebugMode`; bincode-native Encode/Decode implementations
and the `paste` re-export are removed. Engine module facade paths are preserved.
The later asset-system migration replaced the old cache format with v2 containers.
Current regression tests preserve v2 texture payload compatibility; the unused
v1 material fixture has been removed.

`cargo metadata --offline --filter-platform x86_64-pc-windows-msvc --format-version 1`
and source usage searches provided the measurements. The complete resolved
package audit is in [dependency-packages.md](dependency-packages.md).

## Validation evidence

Completed on 2026-09-12 using Windows MSVC, Slang 2026.17 and an NVIDIA RTX 4090.
`./scripts/validate-vulkan.ps1 -WindowTests -Offline` passed all stages:

- Workspace tests and doctests, including persistent logger teardown checks and
  asset cache regression coverage.
- Explicit Slang SDK tests for named entry points, debug symbols, optimized
  output, missing entries and stage-mismatch diagnostics.
- Included glTF/PNG import and BC5/BC7 compression, plus HDR decoding.
- All-target checks with `cpu-profiling` and `extra-image-formats` enabled.
- Debug/release workspace builds, RHI GPU readback suites and render-graph
  acceptance. Includes shader import reload and diagnostics in paths with spaces.
- Debug/release bounded window API, clear, triangle, world, and world resize/
  minimize runs. World runs completed 1,000 frames in each configuration.

The current GPU/readback and world logs contain no Vulkan validation errors.
Logs are under `target/validation`. SPIR-V and companion command dumps were
also exercised under `target/shader-dumps`. `git diff --check` passed.

Doctest validation exposed existing asset example snippets that did not compile
and an ASCII coordinate diagram interpreted as Rust. The snippets now use the
current constructors and the diagram is marked as text.

After restoring env_logger defaults, the logger integration tests passed.
Direct subprocess checks verified timestamps and ANSI color with
`RUST_LOG_STYLE=always`, and timestamps without ANSI color with `never`.
