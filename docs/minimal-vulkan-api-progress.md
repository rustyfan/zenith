# Implementation and validation evidence

Completed on 2026-09-10. Gate A passed before the render graph migration. Gate B passed after renderer parity, lifecycle stress, release verification, and performance review. The replacement is now the only RHI; the temporary feature, old Vulkan modules, reflected binders, and `zenith-rhi-derive` are removed.

## Environment and dependencies

- Rust 1.97.1, Windows MSVC.
- Ash/ash-window: vendored revision `f4c2ca3e4f6b998d5254ad101a32f024d87cdec2`, ash `0.38.0+1.4.352`.
- vk-mem 0.5.0 / VMA 3.3.0: vendored with Rust compatibility edits; C++ allocator unchanged.
- Slang 2026.17: `D:\Software\slang-2026.17-windows-x86_64`; slang-rs vendored at `60767e4d5965cc8147623ff47d02af651f7ab761` with the resource/sampler stride option setters exposed.
- Validation SDK 1.4.357.0: copy-only extraction at `target/vulkan-sdk`; synchronization validation enabled.
- SDK installer SHA-256: `81f474711e9042f4cd22b31b2f7a8870db2e428b21586fb43dd80150be97310d`.

`cargo tree --offline -i ash` confirms a single ash source throughout the workspace, including asset formats, window integration, and VMA. Dependency archives and licenses are documented in [vendor provenance](../vendor/README.md).

| Adapter | Driver | Descriptor heap | Unified layouts | Shader untyped pointers | Result |
| --- | --- | --- | --- | --- | --- |
| NVIDIA GeForce RTX 4090 | NVIDIA 610.88 | Yes | Yes | Yes | Required profile and GPU tests pass |
| AMD Radeon(TM) Graphics | AMD 26.3.1 | No | No | No | Explicitly rejected before device creation |

The NVIDIA report exposes 16 graphics/compute queues in the selected family; the RHI uses two. Image and sampler descriptors are 32 bytes on this driver, with 32-byte heap alignment. The implementation queries these values and separately aligns writable slots to non-coherent atom boundaries. Upload allocations selected `DEVICE_LOCAL | HOST_VISIBLE | HOST_COHERENT`. Capability output includes all queue families and memory types in `target/validation/capabilities.log`.

## Final commands and results

### Native descriptor heap migration — 2026-09-11

Texture and sampler set/binding mappings have been removed. Production shaders and GPU fixtures use Slang's `ResourceDescriptorHeap` and `SamplerDescriptorHeap`, compiled with `spvDescriptorHeapEXT`. Root blobs retain the existing `PUSH_ADDRESS` mapping and push-data offsets. `Gpu::compile_shader` derives both heap strides from the same calculation used by the allocator; compute/raster pipeline APIs and cache keys no longer depend on a descriptor table.

The device now requires and enables `VK_KHR_shader_untyped_pointers` and `shaderUntypedPointers`. The RTX 4090 exposes the feature; the installed AMD adapter does not. The old runtime descriptor array and sampled/storage descriptor-array indexing feature requirements are removed. Native heap access defaults to non-uniform access at the SPIR-V level; Slang source still uses `NonUniformResourceIndex` for divergent indices, as described in the [Khronos heap proposal](https://docs.vulkan.org/features/latest/features/proposals/VK_EXT_descriptor_heap.html#_spir_v_mapping).

Validation after this migration:

- `scripts/validate-vulkan.ps1 -WindowTests -Offline`: workspace tests, all debug/release builds, API/graph GPU suites, and 10,000 window frames passed with synchronization validation enabled and no validation errors.
- The new `minimal_tests/direct_heaps.rs` fixture verifies exact readback for divergent texture/sampler indices, nonzero sampler slots with different address modes, 1D/2D/3D textures, 1D/2D arrays, cubes/cube arrays, depth sampling, and storage writes. One pipeline runs against two heap capacities in the same recording. GPU descriptor copying and slot retention/reclamation are checked.
- The allocator and shaders both use `(64, 64)` byte image/sampler strides on the RTX 4090, despite the driver's 32-byte descriptor sizes.
- The standalone direct-heap shader passed SDK `spirv-val --target-env vulkan1.3 --scalar-block-layout`. Disassembly contains `SPV_EXT_descriptor_heap`, `SPV_KHR_untyped_pointers`, the resource/sampler heap built-ins, and 64-byte heap `ArrayStride` decorations. Only the root retains `DescriptorSet 0` / `Binding 0`. Artifacts: `target/direct-heaps/direct.spv` and `direct.spvasm`.
- World captures before and after this migration, frame 5 at 2,880 × 1,620: **100% exact pixels, maximum channel difference 0**. Artifacts: `target/direct-heaps/before/5.ppm`, `after/5.ppm`, and `comparison.json`.

### Full validation command

The final script completed successfully:

```powershell
./scripts/validate-vulkan.ps1 -WindowTests -Offline
```

It runs the following in the configured Slang/validation environment:

```text
cargo test --offline --workspace --all-targets
cargo run --offline -p zenith-rhi --example capabilities
cargo build --offline --workspace --all-targets
cargo build --offline --release --workspace --all-targets
cargo run --offline -p zenith-rhi --example minimal_smoke
cargo run --offline --release -p zenith-rhi --example minimal_smoke
cargo run --offline -p zenith-rendergraph --example graph_smoke
cargo run --offline --release -p zenith-rendergraph --example graph_smoke
```

For each of debug and release, it also runs `minimal_window`, sandbox clear, triangle, world, and world resize/minimize stress for 1,000 frames each: **10,000 window frames, all validation-clean**. The standalone window test additionally abandons an acquisition and verifies recreation and shutdown. All process exits are checked and window runs have 60-second limits. Detailed logs are in `target/validation`.

The explicit AMD selection also behaved as expected:

```powershell
$env:ZENITH_ADAPTER = 'AMD'
cargo run --offline -p zenith-rhi --example minimal_smoke
```

It exits with a capability error rather than attempting an unsupported fallback.

## API coverage

| Family | Evidence |
| --- | --- |
| Memory and ownership | Upload/device/readback round trips; exact 1,027-element transform; mapped CPU exclusion; checked bounds/alignment/overflow; 128 retirement cycles; stable VMA allocation count |
| Commands and synchronization | Single-use submission, polling, bounded timeout, byte copies, overlapping-copy rejection, repeated writes, split dependencies, stale pooled-recording token rejection, cross-queue timeline waits |
| Shaders and root ABI | Explicit entry/stage rejection; scalar/vector/matrix/array and nested pointer results; GPU-written roots; include-change recompilation; useful compiler failure; compute and raster specialization; pipeline reuse |
| Descriptors | Sampled 2D/cube and storage access; driver-generated descriptor bytes; GPU descriptor copy; contiguous allocation; exhaustion; delayed slot reuse; non-uniform indices |
| Images | 1D/2D/3D, arrays, cubes, cube arrays, mip/layer views and copies; odd extents; padded source/destination rows; compressed BC7 and depth readback; invalid views/footprints rejected |
| Raster | Vertex pulling; 16/32-bit indices; base vertex and first instance; direct/indirect/indexed/instanced drawing; GPU count including zero/clamped counts; per-draw fragment data |
| Attachments and state | MRT, depth rejection, stencil selection, culling, signed depth bias, scissor, dynamic alpha blending without new pipelines, color MSAA resolves |
| Graph | Checked graph-local handles; import deduplication and export; undeclared/uninitialized access rejection; byte/subresource partitioning; RAW/WAR/WAW and multiple reader stages; transient retention and reuse; headless GPU results |
| Presentation and renderer | Actual acquired image; transfer/attachment first use; per-image completion semaphores; mailbox/FIFO choice; resize/minimize/abandoned acquisition; full clear/triangle/world migration and shutdown |

The tests exposed two shader ABI issues during development: scalar constant-buffer layout needed to be explicit, and base-offset-aware vertex pulling needed Slang's Vulkan-specific vertex/instance semantics. Both have deterministic GPU regression fixtures.

The implemented scope is defined in [API contracts](vulkan-api.md). This is not an assertion that every possible format/state combination has been exhaustively tested. Non-coherent physical memory, forced dedicated allocations, actual device loss, GPU hangs, unsupported-adapter rendering, and non-Windows surfaces were not physically exercised. VMA owns the first two allocation mechanisms; device-loss recovery and deliberate GPU hangs are outside the implementation. Depth/stencil resolves, multiview, narrow arithmetic extensions, mesh/task, geometry/tessellation, and ray tracing are not advertised.

## Visual parity

The original clear, triangle, and world examples were captured before migration. The final promoted implementation was captured again after the last shader correction. Both use frame 5, 2,880 × 1,620 pixels, the same assets/camera, and triangle time zero. Captures use the official `VK_LAYER_LUNARG_screenshot` layer with `VK_SCREENSHOT_FRAMES=5` and per-scene `VK_SCREENSHOT_DIR`.

| Scene | Maximum absolute 8-bit channel difference | Exact pixels | Result |
| --- | --- | --- | --- |
| Clear | 0 | 100% | Exact |
| Triangle | 0 | 100% | Exact |
| World | 1 | 99.97951% | Within one quantization step everywhere |

World mean absolute channel difference is `0.000070659`, with no pixel beyond the one-step tolerance. Integer buffer and image-copy tests use exact equality. Original images/logs are in `target/baseline/{clear,triangle,world}`; final images/logs and amplified differences are in `target/replacement/{clear,triangle,world}`. The numerical report is `target/replacement/comparison.json`.

The old backend was not validation-clean: it lacked sampled-image non-uniform indexing enablement and produced upload/attachment/presentation hazards, live-resource destruction, and shutdown lifetime errors. These were retained as baseline defects and do not occur in the replacement runs.

## Performance and simplification

Final fixed-resolution world results, with validation, synchronization validation, and timestamp instrumentation enabled:

| Metric | Debug | Release |
| --- | --- | --- |
| First graph build/record | 4.843 ms | 1.417 ms |
| Warm graph build/record mean | 0.285 ms | 0.208 ms |
| G-buffer GPU mean | 0.037 ms | 0.037 ms |
| SH integration GPU mean | 0.148 ms | 0.146 ms |
| Lighting GPU mean | 0.050 ms | 0.051 ms |
| Peak live pipelines | 3 | 3 |
| Steady VMA allocation count | 21–21 | 21–21 |
| Image/sampler host slot writes over 1,000 frames | 3,005 / 3 | 3,005 / 3 |

Resize/minimize runs stayed between 13 and 21 allocations as frame caches were cleared, with three pipelines throughout. Pipeline count and allocation counts do not grow with frame count. Dynamic state changes do not create pipelines. GPU pass timings exclude presentation and the barrier immediately preceding each timestamp region.

A bounded original world run took 4.648 seconds; a comparable early replacement run took 4.534 seconds. Both used the same scene, dimensions, and mailbox presentation. These wall times include startup and the original run's validation-error overhead, so they are a regression sanity check, not a reliable speedup claim. The old implementation had no equivalent per-pass timestamp/descriptor-write instrumentation; those metrics are reported only for the replacement. Cold-start variability is visible in the measurements and no statistical performance guarantee is claimed.

Rust production source across RHI, graph, renderer, engine, and the removed derive crate decreased from 10,654 to 6,298 lines (about 41%). This excludes test fixtures, documentation, and vendored third-party bindings/allocator code. The reduction comes from removing reflected binders, specialized buffer classes, nested pipeline builders, public synchronization wrappers, duplicate frame bookkeeping, and the derive crate while retaining explicit dependency and lifetime checks.
