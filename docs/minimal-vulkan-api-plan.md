# Minimal Vulkan API implementation plan

Status: implementation completed; standalone and renderer acceptance gates passed. The user authorized implementation after these revisions: current ash bindings, vertex/fragment/compute only, and VMA for internal memory allocation.

Implementation revision: vendor ash at Git revision `f4c2ca3e4f6b998d5254ad101a32f024d87cdec2` (Vulkan 1.4.352) across the dependency graph, including ash-window and vk-mem. Use `vk-mem` 0.5 / VMA 3.3 for buffer and image allocation, memory-type selection, pooling, dedicated-allocation requirements, and mapped-memory flushing. Retain only an argument arena above VMA. Shader stages are limited to vertex, fragment, and compute. Mesh/task shaders and ray-tracing pipelines are out of scope. The runtime probe confirms descriptor heaps and unified layouts on the RTX 4090; use these as required features of the replacement. The AMD adapter is unsupported by this profile.

Follow `AGENTS.md` and all instructions in `skills/`: prefer simple, direct, robust code; add code comments only when necessary to explain obscure implementation details.

**Objective**

Replace Zenith's current RHI with a small Vulkan implementation, preserve the render graph, migrate the renderer, and delete the superseded implementation only after the replacement passes its tests. Breaking internal interfaces and removing redundant code are authorized.

The inspiration is Sebastian Aaltonen's [No Graphics API](https://www.sebastianaaltonen.com/blog/no-graphics-api): address-based shader inputs, indexed textures, less pipeline state, transient commands, and simpler synchronization. This plan is a concrete Rust/Vulkan design for Zenith, with explicit adaptations where Vulkan does not provide the proposed semantics.

**Verified starting point — 2026-09-09**

- `cargo check -p zenith-sandbox --all-targets --locked` passes with the configured Slang and libclang environment. This checks the sandbox and its examples; it does not establish runtime correctness.
- The earlier incomplete BRDF LUT changes are absent from the current checkout. They are not a prerequisite repair for this migration.
- Shader sources currently live in `content/shaders/`.
- Windows reports an NVIDIA GeForce RTX 4090 and AMD Radeon integrated graphics. At that point, Vulkan extensions, features, limits, memory types, and validation-layer availability still required a runtime probe.
- Before migration, the `ash` package was `0.38.0+1.3.281`. Its generated bindings did not include unified image layouts or descriptor heaps.

**Architecture and migration boundary**

Keep `zenith-rhi` as the final crate name. The replacement was developed in `zenith-rhi/src/minimal/`, enabled by a temporary `minimal-api` Cargo feature. During that phase its device, memory, commands, and resources were independent of the old RHI. Low-level integration tests still exercise the final RHI directly, without the engine or render graph.

Both gates passed in sequence. The render graph and its consumers now use the replacement, the new modules are at the root of `zenith-rhi/src`, and the temporary feature and legacy implementation have been deleted. No permanent backend trait, compatibility facade, or runtime backend selector was added.

Final responsibilities:

| Layer | Responsibility |
| --- | --- |
| `zenith-rhi` | Vulkan device, allocation, images, descriptor storage, shader compilation, pipelines, recording, synchronization, presentation |
| `zenith-rendergraph` | Pass order, explicit resource dependencies, transient lifetime planning, barrier planning, execution |
| `zenith-renderer` | Root argument structs, passes, material and mesh GPU data, lighting and IBL |
| `zenith` | Window events and one frame loop consuming the graph |
| `zenith-asset` | CPU loading/baking; GPU upload callers change only where necessary |

Final RHI modules: `device`, `memory`, `texture`, `descriptors`, `shader`, `raster`, `pipeline_cache`, `command`, `swapchain`, and `timestamps`. Merge small helpers into their owner instead of creating another abstraction layer. Vulkan format and flag types may remain where reusing them avoids redundant enums; native resource handles and unsafe Vulkan calls stay inside the RHI.

**Concrete API decisions**

| Area | Proposed contract |
| --- | --- |
| Ownership | A concrete `Gpu` owner and small resource owners/handles. Recorded work retains explicitly declared resources until submission completes. Reusing a descriptor slot, allocation, or pipeline before completion is forbidden. |
| Buffer memory | `Memory`, checked `MemorySlice`, and a copyable, non-dereferenceable `GpuAddress`. Use one allocation model for shader data, geometry, indices, and indirect arguments; remove uniform/vertex/storage buffer classes. |
| Memory domains | `Upload`, `Device`, `Readback`. Prefer host-visible device-local memory for uploads when appropriate; use host-visible staging otherwise. Report the selected memory type instead of assuming ReBAR. |
| CPU access | Scoped mapped writes and readback access; no safe CPU dereference of a GPU address. Flush/invalidate non-coherent ranges. GPU/CPU ownership and completion must be established before access. |
| Textures | Concrete `Texture` and subresource views; VMA handles allocation requirements and binding internally. No public manual image placement or general-purpose allocator. |
| Descriptors | One logical image heap and one sampler heap, with compact indices in shader structs. Driver descriptor bytes are opaque. Slang uses native `ResourceDescriptorHeap` and `SamplerDescriptorHeap` accesses with device-specific slot strides. |
| Shaders | Slang to SPIR-V with an explicit entry point and stage, fixed root convention, and checked Rust/Slang data layout. Reflection may validate ABI during build/tests; it does not drive per-draw bindings or graph barriers. |
| Root arguments | A fixed push-data packet holds vertex/compute and fragment addresses. Heap shader mappings resolve root uniform data through these addresses. Public calls receive argument slices; applications do not construct pipeline layouts. |
| Pipelines | Concrete compute and raster descriptions. One cache keyed by shader content, entry points, specialization, and genuinely baked state. Debug names and resource addresses are not normal cache keys. |
| Dynamic state | Depth/stencil, viewport, scissor, and supported raster state are command data. Use plain state structs; do not create heap-allocated state objects that merely wrap Vulkan setters. Dynamic blend is capability-dependent. |
| Commands | Single-use recording; consuming submit returns a `Submission` completion value. Command pools are internal and recycled only after completion. |
| Synchronization | An explicit stage/access dependency lowered to Synchronization2 memory barriers. Descriptor and indirect-argument hazards get the appropriate consumer accesses. Image initialization and presentation remain explicit internally. |
| Presentation | `acquire`, record, `submit`, `present`; acquired images are imported as the actual frame image before graph execution. Resize and minimized windows have defined behavior. |

For command operands such as copies, index data, and indirect records, `MemorySlice` retains allocation identity, bounds, and offset. Vulkan still needs a backing `VkBuffer` for these operations. This avoids an address-to-buffer lookup table while preserving simple address-based shader data. Raw shader pointer graphs cannot be made safe merely by wrapping a `u64`; resources reachable through them must also be retained and declared to the graph, with an explicit unsafe escape hatch for unchecked usage.

**Vulkan boundaries that affect the plan**

1. Enable buffer device address at device creation and use the necessary buffer usage and allocation flags. An existing address getter alone is insufficient. Slang already supports physical-storage-buffer pointers; verify the installed compiler and CPU/GPU layouts with executable tests. [Khronos BDA guide](https://docs.vulkan.org/guide/latest/buffer_device_address.html), [Slang SPIR-V guide](https://shader-slang.org/slang/user-guide/spirv-target-specific.html).
2. Use `VK_EXT_descriptor_heap`, whose runtime support is confirmed on the RTX 4090. Query image/sampler descriptor sizes, heap alignment, and reserved ranges. Driver-generated bytes are opaque; do not assume 32-byte descriptors. GPU copies of valid descriptors are supported; shader-generated arbitrary image descriptors are excluded. [Khronos descriptor-heap guide](https://docs.vulkan.org/guide/latest/descriptor_heap.html).
3. Enable the heap extension's maintenance5 dependency and its feature, along with buffer device address. Enable `VK_KHR_shader_untyped_pointers` and `shaderUntypedPointers` for native SPIR-V heap access. Keep only the root-address mapping; image and sampler accesses use native heap built-ins with allocator-matched strides. There are no descriptor-set layouts or pipeline layouts. [Heap extension dependencies](https://docs.vulkan.org/refpages/latest/refpages/source/VK_EXT_descriptor_heap.html).
4. Use `GENERAL` for the supported ordinary image operations to keep public layout state out of the API. With `unifiedImageLayouts`, this has an efficiency guarantee, but initialization, presentation, and ownership still need special handling. Devices without it are rejected; the extension does not remove initialization, presentation, or memory dependencies. Image-specific barriers can remain an internal optimization. [Unified-image-layout proposal](https://docs.vulkan.org/features/latest/features/proposals/VK_KHR_unified_image_layouts.html).
5. Delegate memory-type compatibility, alignment, granularity, pooling, and dedicated allocations to VMA. A shader buffer address is not an image-memory binding token. [VMA documentation](https://gpuopen-librariesandsdks.github.io/VulkanMemoryAllocator/html/).
6. Queue completion uses timeline semaphores; window-system acquire/present uses private binary semaphores. A render-complete timeline value alone does not prove that presentation has consumed its semaphore. [Presentation requirements](https://docs.vulkan.org/refpages/latest/refpages/source/vkQueuePresentKHR.html).
7. Split dependencies use scoped Vulkan events on one queue, with valid paired synchronization scopes. Cross-queue waits use submission semaphores and ownership handling. Arbitrary memory-pointer waits with comparison/atomic-OR semantics from the article are outside this Vulkan contract; do not emulate them with GPU spin loops. [Vulkan event waits](https://docs.vulkan.org/refpages/latest/refpages/source/vkCmdWaitEvents2.html).
8. Multi-draw uses Vulkan indirect records and draw count, with shader draw IDs selecting root data. Fragment shaders receive the per-draw selection through a flat varying where necessary. This is a lowering with extra shader indirection, not native replacement of push constants by the command processor. [Indirect-count drawing](https://docs.vulkan.org/refpages/latest/refpages/source/vkCmdDrawIndexedIndirectCount.html).
9. Dynamic blend needs individual extended-dynamic-state-3 feature bits. If absent, use an explicitly baked blend description; never silently build a new pipeline in a state setter. [Extended dynamic state 3](https://docs.vulkan.org/features/latest/features/proposals/VK_EXT_extended_dynamic_state3.html).

Required profile: Vulkan 1.3, maintenance5, descriptor heaps, shader untyped pointers, unified image layouts, buffer device address, timeline semaphores, Synchronization2, dynamic rendering, scalar layout, shader int64, and indirect drawing features used by the ABI. Native heap accesses support divergent resource indices without the old descriptor-array feature requirements. Query individual feature bits. Dynamic blending is optional; narrow scalar arithmetic is deferred. Unsupported devices fail with a capability report.

The replacement covers vertex/fragment raster and compute, including indirect operations. Mesh/task shaders, ray tracing pipelines, raw-pointer event operations, portable shader-generated descriptors, work graphs, video, sparse residency, and a new shader language are outside this migration.

**Ordered feature tasks**

Every task is independently reviewable. A checkbox is marked only after implementation and its acceptance check are complete. Tests requiring GPU execution run once the recording/submission foundation exists. Sequential phases are the default; listed dependencies indicate where work can be separated without changing implementation order.

**Phase 0 — Establish the baseline and test boundary**

| Done | ID | Small task | Acceptance check |
| --- | --- | --- | --- |
| [x] | 01 | Capture baseline debug/release builds and sandbox/triangle/world behavior. | Save commands, deterministic scenes, screenshots/readbacks, validation output, and known failures separately. |
| [x] | 02 | Add a device capability probe and explicit adapter selection. | Report RTX 4090 and AMD capabilities, queue families, memory types, descriptor limits, formats, and validation support; explain rejection of unsupported adapters. |
| [x] | 03 | Freeze the Vulkan profile, root ABI, descriptor-table convention, and dependency versions. | Minimal Slang pointer and table-indexing fixtures compile; feature requirements are recorded. |
| [x] | 04 | Add the temporary module, headless test harness, validation collector, and GPU timeouts. | A test can select an adapter and fail on validation errors; unavailable capabilities are reported as skipped, never passed. |

**Phase 1 — Memory and addresses; depends on 02–04**

| Done | ID | Small task | Acceptance check |
| --- | --- | --- | --- |
| [x] | 05 | Implement checked sizes, alignment, address offsets, and allocation slices. | Zero/overflow/misaligned/out-of-range cases are rejected without integer wraparound. |
| [x] | 06 | Implement upload, device, and readback allocation plus BDA setup. | Mapped domains and address flags match actual memory properties; failure paths release created objects. |
| [x] | 07 | Add persistent mapping, explicit writes, flush, and invalidate. | Non-coherent atom alignment is correct; GPU copy/readback round trips verify visibility once phase 2 is available. |
| [x] | 08 | Use VMA pooling and add a frame argument arena. | Mixed sizes align correctly; exhaustion errors; the arena never wraps into live allocations. |
| [x] | 09 | Add resource retention and completion-based retirement. | Dropped owners, abandoned recordings, and delayed submissions cannot reuse live ranges; repeated cycles return to a stable allocation count. |

**Phase 2 — Recording, submission, and synchronization; depends on phase 1**

| Done | ID | Small task | Acceptance check |
| --- | --- | --- | --- |
| [x] | 10 | Implement single-use command recording and pool recycling. | Double submission is prevented; pools reset only after the associated completion value. |
| [x] | 11 | Implement timeline submit, polling, waiting, and bounded readback waits. | Ordered submissions and timeout paths work; queue submission failure is propagated. |
| [x] | 12 | Implement checked buffer copy and fill operations. | Exact byte readback for offsets and boundary sizes; invalid overlap and alignment are rejected. |
| [x] | 13 | Implement stage/access barriers, including write-to-write and indirect/descriptor consumers. | Chained copy/compute/readback tests produce the expected values; same-access writes still synchronize. |
| [x] | 14 | Implement same-queue split dependencies and explicit cross-queue submission waits. | Producer/consumer tests pass; queue-family ownership is handled; unsupported extra queues are reported. |

**Phase 3 — Shader ABI and compute; depends on phase 2**

| Done | ID | Small task | Acceptance check |
| --- | --- | --- | --- |
| [x] | 15 | Separate Slang compilation from runtime descriptor reflection. | Compile a requested entry point with useful diagnostics; shader include changes invalidate compiled results. |
| [x] | 16 | Implement the shared root packet and typed Rust/Slang argument fixtures. | Scalar/vector/matrix/array layouts and nested addresses round-trip correctly; optional narrow types are tested separately. |
| [x] | 17 | Implement compute pipeline creation, bind, and dispatch using a root address. | A nontrivial multi-workgroup transform matches CPU reference output, including partial last groups. |
| [x] | 18 | Implement explicit specialization data and one pipeline cache. | Repeated identical requests reuse pipelines; changed shader/specialization creates the intended variant; invalid specialization reports an error. |
| [x] | 19 | Add compute-to-compute, pointer-chain, and GPU-written-root-data tests. | Results remain correct across several submissions and frames without per-buffer descriptor binding. |

**Phase 4 — Textures and descriptor storage; depends on phases 1–3**

| Done | ID | Small task | Acceptance check |
| --- | --- | --- | --- |
| [x] | 20 | Implement VMA-backed image creation, destruction, and initialization. | Required formats/usages and dedicated allocations work; invalid descriptors fail before recording. |
| [x] | 21 | Implement mip/layer views for 1D, 2D, arrays, 3D, cubes, and cube arrays. | View bounds/type/format checks pass; small fixtures verify each advertised kind. |
| [x] | 22 | Implement image uploads, readbacks, copies, and supported resolves. | Odd dimensions, row pitch, mip chains, cube faces, compressed blocks, and depth/aspect cases obey Vulkan rules. |
| [x] | 23 | Implement descriptor heaps and sampled/storage/sampler encoding. | Device-reported sizes, alignment, and reserved ranges are used; sampling and storage writes return expected values. |
| [x] | 24 | Implement contiguous table allocation and delayed slot reuse. | Table exhaustion is explicit; no in-flight overwrite; cube/array/storage indexing and non-uniform indices work. |
| [x] | 25 | Implement batched texture/data upload and synchronized copies of valid descriptor bytes. | One submission uploads multiple resources; a later dispatch reads GPU-copied descriptors correctly. |

**Phase 5 — Raster rendering; depends on phase 4**

| Done | ID | Small task | Acceptance check |
| --- | --- | --- | --- |
| [x] | 26 | Implement dynamic rendering attachments and load/store/clear operations. | Color, depth, stencil, MRT, subresource views, and selected MSAA resolves pass deterministic readback tests. |
| [x] | 27 | Implement vertex/fragment pipelines with a fixed layout and minimal baked state. | A procedural triangle renders correctly; repeated passes reuse the same pipeline. |
| [x] | 28 | Implement pointer-based vertex pulling and indexed/instanced draws. | 16/32-bit indices, base vertex, instance offsets, and distinct vertex layouts match reference geometry. |
| [x] | 29 | Implement plain depth/stencil/raster state and dynamic viewport/scissor. | Occlusion, stencil operations, culling, bias, and clipped rectangles match expected pixels. |
| [x] | 30 | Implement embedded blend and feature-gated dynamic blend. | Alpha/additive/write-mask cases match CPU formulas; changing dynamic state does not grow the pipeline cache. |

**Phase 6 — Indirect operations and ABI validation; depends on phase 5**

| Done | ID | Small task | Acceptance check |
| --- | --- | --- | --- |
| [x] | 31 | Implement indirect dispatch. | Compute-generated dimensions drive a dependent dispatch with correct synchronization. |
| [x] | 32 | Implement indexed indirect drawing. | GPU-generated records render the expected instance/index ranges; byte bounds and usage are checked. |
| [x] | 33 | Implement multi-draw with GPU count and per-draw root selection. | Zero/count-limit cases, shared roots, different strides, and fragment material selection produce correct results. |
| [x] | 34 | Test indirect root-data addressing and vertex-to-fragment draw selection. | Distinct per-draw arguments and shared roots produce correct pixels with no per-draw binding. |
| [x] | 35 | Verify feature failures and shader-stage restrictions. | Only vertex/fragment/compute are accepted; missing required features are reported before device creation. |

**Phase 7 — Presentation and standalone API acceptance; depends on phases 1–6**

| Done | ID | Small task | Acceptance check |
| --- | --- | --- | --- |
| [x] | 36 | Implement surface/swapchain creation and actual acquired-image handles. | Correct format, extent, usage, and present queue selection; images remain externally owned. |
| [x] | 37 | Implement acquire/submit/present synchronization. | Clear-by-transfer and color-attachment-first frames both use appropriate acquire wait stages; private binary semaphore reuse is safe. |
| [x] | 38 | Implement resize, zero extent, out-of-date/suboptimal recovery, and shutdown. | Repeated minimize/restore/resize works; old swapchains remain alive until GPU and presentation use is finished. |
| [x] | 39 | Add debug labels, diagnostics, and a capture/readback verification path. | A captured frame identifies passes/resources; test failures show adapter, operation, and validation messages. |
| [x] | 40 | Run the complete standalone API acceptance suite. | All required APIs and supported optional APIs pass; unsupported features are documented; no unimplemented advertised methods remain. |

Gate A: task 40 must pass before redirecting the production render graph to the new backend. Keep the existing engine path intact until this gate. Literal article features excluded above are not counted as implemented.

**Phase 8 — Preserve and simplify the render graph; starts after Gate A**

| Done | ID | Small task | Acceptance check |
| --- | --- | --- | --- |
| [x] | 41 | Replace buffer/texture graph storage with new concrete resources and checked graph-local handles. | Create/import/export and descriptor lookup work without the existing generic `transmute` access paths; foreign graph handles are rejected. |
| [x] | 42 | Replace binding/state enums with explicit resource access declarations and stages. | Reads, writes, read-write, indirect inputs, and subresource/byte ranges are represented without shader-name lookup. |
| [x] | 43 | Compile dependency declarations into batched stage/access barriers. | RAW/WAR/WAW, multiple readers, same-state writes, disjoint ranges, and imported final access are covered by planner and GPU tests. |
| [x] | 44 | Replace `GraphBindable`, `PipelineResourceBinder`, and reflected node binding with root-data creation. | Execution context resolves declared accesses to addresses or texture indices; pointer-reachable resources/table ranges remain retained. |
| [x] | 45 | Unify graph recording and submission through the new command API. | Headless execution and swapchain execution work; first-use acquire waits follow the actual transfer/compute/graphics consumer. |
| [x] | 46 | Move transient caching and completion handling into graph-owned lifetime management. | Resources and descriptor slots are recycled after the last consuming submission; repeated frames and imported resources remain correct. |

Keep declared pass order initially. Preserve resource dependency information even when Vulkan barriers are global. A pass with unknown bindless accesses must conservatively declare the reachable resource group/table range and retain it; it cannot simply omit dependencies. Remove reflection-based guesses and duplicate state machines, not the graph's knowledge of resource usage. More aggressive scheduling and memory aliasing are separate optimizations after correctness is established.

**Phase 9 — Renderer and engine migration; depends on phase 8**

| Done | ID | Small task | Acceptance check |
| --- | --- | --- | --- |
| [x] | 47 | Migrate the sandbox clear pass and triangle example. | Blue clear, animated triangle, and resizing match the baseline through the preserved graph. |
| [x] | 48 | Migrate mesh storage, materials, camera data, and texture upload callers. | Shared geometry and material indices survive multiple frames; asset file formats remain unchanged unless an explicit ABI need requires it. |
| [x] | 49 | Migrate the world G-buffer pass and its shaders to vertex pulling/root data. | Mesh transforms, normals, UVs, depth, and material textures match the baseline. |
| [x] | 50 | Migrate lighting, skybox, tone mapping, and current SH diffuse IBL. | Deterministic camera/environment captures match within declared tolerances; unfinished BRDF LUT work is not invented as a baseline requirement. |
| [x] | 51 | Replace engine fence/pool/deferred-release collections with the new frame submission flow. | Frames in flight remain bounded; close, resize, and error paths release resources correctly. |
| [x] | 52 | Remove legacy imports from all production consumers and shader helpers. | Engine, renderer, sandbox, and examples compile using only the replacement API. |

**Phase 10 — Integration evidence; depends on phase 9**

| Done | ID | Small task | Acceptance check |
| --- | --- | --- | --- |
| [x] | 53 | Run full debug/release build and CPU/GPU test matrices. | All crates/examples compile; required and supported optional GPU tests pass on the RTX 4090. |
| [x] | 54 | Compare baseline and replacement images/readbacks. | Fixed camera, assets, time, exposure, and dimensions; exact integer checks and justified floating-point/image tolerances. |
| [x] | 55 | Stress lifetime, table exhaustion, allocation pressure, resize, and shutdown. | At least 1,000 frames plus repeated resize/recreation cycles; no validation errors, leaks, stale slots, or arena overwrite. |
| [x] | 56 | Measure CPU recording, pipeline creation/count, descriptor writes, allocation count, and GPU pass times. | Warm/cold results use identical workloads; regressions are explained and resolved before removal of the old backend. |
| [x] | 57 | Verify capability reporting on the AMD adapter and record the support matrix. | Every API is labeled passed, unsupported, or untested; unavailable hardware features are not reported as validated. |

Gate B: visual/functional parity, supported API coverage, lifecycle stress, and performance review must pass before deleting the old implementation. Device-loss cleanup can be tested with injected API failures; do not deliberately hang the physical GPU to test recovery.

**Phase 11 — Final replacement and deletion; starts after Gate B**

| Done | ID | Small task | Acceptance check |
| --- | --- | --- | --- |
| [x] | 58 | Promote the replacement modules to the root of `zenith-rhi` and delete old modules. | No duplicate implementation, migration feature, compatibility binder, or legacy public export remains. |
| [x] | 59 | Delete unused derive/builder dependencies and obsolete shader helpers. | Remove `zenith-rhi-derive` only after its last use is gone; dependency search finds no dangling references. |
| [x] | 60 | Update examples/build instructions/API examples and run final verification. | Clean debug/release builds, tests, sandbox/triangle/world runs, and the API support matrix agree with the final code. |

**Expected removals and retained responsibilities**

| Current implementation | Final disposition |
| --- | --- |
| `zenith-rhi/src/descriptor.rs` binders, writers, pools, per-shader layouts | Delete; replace with descriptor heaps and fixed shader convention. |
| `zenith-rhi/src/bindless.rs` typed buffer heap and packed resource-type handles | Replace with image/sampler tables; buffers use addresses. |
| `zenith-rhi/src/buffer.rs` specialized constructors/range binding machinery | Replace with allocation/slice/address operations. |
| `pipeline.rs` vertex layouts and nested builder objects | Delete those structures; keep compact raster/compute descriptions and explicit dynamic state. |
| `pipeline_registry.rs` reflection-driven layout construction | Replace with one cache; retain shader/pipeline reuse. |
| `shader.rs` / `slang_compiler.rs` runtime binding reflection | Remove from rendering; keep compiler diagnostics and optional ABI validation. |
| `barrier.rs` and graph per-resource layout-state mirroring | Replace with one dependency planner and small internal image-transition handling. |
| `defer_release.rs`, public fences, public command pools | Consolidate into submission-based lifetime management; private Vulkan objects still exist where required. |
| `zenith-rendergraph/src/resource.rs` binding traits and unsafe casts | Replace with concrete checked resource access; retain graph ownership and import/export. |
| `zenith-rendergraph/src/graph.rs` reflected binding and split execute/present machinery | Replace execution internals; retain pass declarations, dependency planning, and transients. |
| `zenith/src/engine.rs` parallel frame resource collections | Replace with a small frame loop driven by acquired images and submission completion. |

Use measured reduction in public types, duplicate lifetime/state mechanisms, per-draw binding work, and pipeline variants to judge simplification. Do not set an arbitrary source-line target or delete bounds checks and completion tracking to make the API look smaller.

**Completion evidence**

The final task record must contain the exact commands run, device/driver/capability report, results for each API family, reference and replacement captures, performance measurements, and unresolved limitations. Tests are organized by observable GPU behavior rather than one test for every thin Vulkan wrapper. Validation layers are necessary but do not replace readback checks for pointer contents, race-free reuse, or shader ABI correctness.

At completion the render graph still exists, all production rendering uses the replacement, and the old backend is gone.

Execution details, test limitations, hardware support, comparison tolerances, and timing results are recorded in [implementation evidence](minimal-vulkan-api-progress.md). The final [API contracts](vulkan-api.md) define the implemented scope; non-coherent hardware and real device-loss behavior were not physically exercised.
