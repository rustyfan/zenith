# Vulkan API contracts

The public entry points are in `zenith-rhi/src/lib.rs`. The implementation uses Vulkan directly behind concrete owners; there is no backend trait, descriptor-set binder, runtime shader-binding reflection, public command pool, or public fence. The render graph remains in `zenith-rendergraph`.

## Device and memory

Create an `Instance`, inspect `adapters()`, then create a `Gpu`. Windowed applications normally create `Swapchain` and use its `gpu()`. The required device profile is reported before allocation or shader execution. Mesh/task, tessellation, geometry, and ray-tracing pipelines are excluded. The public `ShaderStage` has exactly vertex, fragment, and compute variants.

The device extension list is limited to the implemented features:

| Extension | Enabled for | Purpose |
| --- | --- | --- |
| `VK_EXT_descriptor_heap` | Every device | Descriptor encoding, resource/sampler heaps, and push data |
| `VK_KHR_shader_untyped_pointers` | Every device | Native SPIR-V descriptor heap access |
| `VK_KHR_maintenance5` | Every device | The heap dependency and 64-bit pipeline flags under the Vulkan 1.3 application profile |
| `VK_KHR_unified_image_layouts` | Every device | The renderer's unified `GENERAL` image layout contract |
| `VK_EXT_extended_dynamic_state3` | Devices supporting all three used blend features | Dynamic blend enable, equation, and color write mask |
| `VK_KHR_swapchain` | Windowed devices | Image acquisition and presentation |

Instance extensions are the platform surface extensions requested by `ash-window` for windowed applications, plus `VK_EXT_debug_utils` when validation is enabled. Buffer device addresses, timeline semaphores, scalar layout, dynamic rendering, and Synchronization2 use core Vulkan features rather than additional extension names. The heap dependencies are documented in the [Vulkan specification](https://docs.vulkan.org/refpages/latest/refpages/source/VK_EXT_descriptor_heap.html).

`Gpu::allocate(size, domain)` returns `Arc<Memory>`. VMA 3.3 handles memory types, pooling, buffer/image requirements, granularity, and dedicated allocation decisions. `Upload` and `Readback` are mapped; `Device` favors device-local memory. `host_memory_properties()` reports the chosen type. `MemorySlice` carries ownership, offset, bounds, and a `GpuAddress`; there are no vertex/uniform/storage buffer classes.

`Memory::write` and `read` check mapping and bounds, serialize host access, and call VMA flush/invalidate. Recording retains the touched ranges. CPU access overlapping a retained non-coherent atom is rejected until the owning submission is retired. Call `Submission::wait` or `poll` before readback; waiting on the device alone does not release a still-owned recording. `Arguments::push` appends POD data without wrapping or overwriting existing roots.

## Shader ABI

Compile an explicit file, entry point, and stage with `Gpu::compile_shader`. A valid persistent SPIR-V cache entry bypasses Slang; a miss invokes `slangc` with column-major matrices, scalar layout, SPIR-V 1.6, and `spvDescriptorHeapEXT+nonuniformqualifier`. The latter explicitly declares Slang's capability group for `NonUniformResourceIndex`, avoiding implicit profile upgrades (E41012) when accessing descriptor heaps. Source entry-point names are preserved and stage-mismatch diagnostics are errors. Debug builds use `-O0 -g3`, release builds use `-O3 -g0`; `ZENITH_SHADER_DEBUG=0` or `1` overrides this. `ZENITH_SHADER_DUMP_DIR` saves SPIR-V and compiler command/diagnostic text on hits and misses. See [compiler/cache setup](../README.md#build-and-run-on-windows). The device must expose `VK_KHR_shader_untyped_pointers` and `shaderUntypedPointers` in addition to descriptor heaps. Changed imported shader code invalidates the shader artifact; changed SPIR-V produces a new pipeline identity. Specialization accepts explicit `(constant_id, 32_bit_value)` pairs for compute, vertex, and fragment stages; use `f32::to_bits()` for floating-point constants.

`Gpu::compile_shaders` accepts a fixed array of `(path, entry, stage)` requests and returns a same-sized array in request order. It joins all workers before returning a compilation error and uses at most four scoped threads. `compile_shader` remains synchronous and its signature is unchanged. Shader disk caching is separate from asset CPU retention and the in-process live-pipeline map; driver pipeline data is not persisted.

Shader resources use:

| Shader declaration | Meaning |
| --- | --- |
| Set 0, binding 0 | Root uniform data, addressed by the current draw/dispatch |
| `ResourceDescriptorHeap[index]` | Typed sampled or storage image from the image heap |
| `SamplerDescriptorHeap[index]` | Sampler from the sampler heap |

A 16-byte internal push-data packet holds the vertex/compute root address and fragment root address. Vulkan descriptor-heap mappings interpret these addresses directly. No application pipeline layout or push-constant object is needed. Root addresses must be eight-byte aligned; the argument arena provides stronger device-appropriate alignment.

The root alone uses a `PUSH_ADDRESS` mapping: vertex/compute at push-data offset 0 and fragment at offset 8. Texture and sampler accesses have no descriptor set/binding mappings. Indices remain `u32` slot indices, not byte offsets:

```slang
TextureCube<float4> skybox = ResourceDescriptorHeap[NonUniformResourceIndex(root.skybox)];
Texture2D<float> depth = ResourceDescriptorHeap[NonUniformResourceIndex(root.depth)];
SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(root.sampler)];
```

`Gpu::descriptor_strides()` returns the image and sampler slot strides. The allocator and compiler use the same calculation: descriptor size rounded up to the greater of descriptor alignment and non-coherent atom size. Slang receives both stride overrides explicitly; its default descriptor-size stride can differ from the allocator's padded stride. On the validated RTX 4090, descriptors are 32 bytes and both slot strides are 64 bytes. Pipeline creation rejects shaders compiled for different strides. `Gpu::compute`, `compute_specialized`, `raster`, and `raster_specialized` no longer take a descriptor table; pipeline identity is independent of heap capacity. Bind the desired heaps through `Commands::bind_descriptors` before shader access.

Use `#[repr(C)]` and `bytemuck::Pod` for Rust argument structs, explicit padding where needed, and matching Slang structs. A scalar-layout `float4` following a pointer begins immediately after that pointer; do not assume the default constant-buffer packing rules. The GPU fixtures cover vectors, column-major matrices, arrays, nested pointers, and roots written by compute.

Use `SV_VulkanVertexID` and `SV_VulkanInstanceID` for vertex pulling and instance selection when draw base offsets must be included. Slang's `SV_VertexID` and `SV_InstanceID` subtract those offsets. `SV_DrawIndex` selects per-draw data for multi-draw; pass that selection to fragment shaders through a flat varying. This is Slang's documented [SPIR-V semantic mapping](https://github.com/shader-slang/slang/blob/master/docs/user-guide/a2-01-spirv-target-specific.md).

## Pointer and descriptor safety

Draws and dispatches are `unsafe` because shader pointer graphs cannot be checked from Rust. The caller must ensure every GPU address points to live memory of the correct layout, every index and shader access is within bounds, and all producer/consumer dependencies are recorded. Retain all pointer-reachable memory with the `reachable` slices or `Commands::retain`, including indirect pointer targets that are not direct command operands.

`Descriptors::image` and `sampler` return owning bindings with compact indices. The driver writes opaque descriptor bytes using its reported sizes and alignment. Never synthesize descriptor bytes in a shader. Bind the table and retain every image/sampler binding that the shader can reach. Holding only the table keeps its memory alive but does not retain the images or occupied slots. Contiguous image arrays, explicit exhaustion, delayed slot reuse, and GPU copies of valid descriptor bytes are supported.

Graph declarations provide these retentions for graph resources. Import persistent material bindings with `import_sampled`, declare every material image and pointer-reachable buffer in the pass, and obtain root data through `PassContext::arguments`. Unknown shader accesses require a conservative declaration of every resource they can reach. An undeclared pointer target is a caller error, even if the shader compiler cannot diagnose it.

## Recording, synchronization, and presentation

`Gpu::commands` begins one recording. `submit` consumes it and returns a timeline-backed `Submission`; abandoned recordings release their resources. Pools are recycled after completion. `commands_on` selects either of the available queues in the chosen graphics/compute family. `submit_after` waits for a previous submission, including another queue. Both queues share one family, so this profile needs no queue-family ownership transfers. Dedicated transfer/compute families are not exposed.

`Access` contains Synchronization2 stages and accesses. Add dependencies for RAW, WAR, and WAW hazards, including equal successive write accesses. Indirect and descriptor-copy consumers need their corresponding indirect/heap read accesses. `signal_dependency` and `wait_dependency` form a scoped split dependency within the same recording; tokens from another or recycled recording are rejected.

Images use `GENERAL` for ordinary operations. A newly allocated image still needs initialization; presentation still needs its special boundary transition. Low-level `initialize`, `transition`, image upload/readback, and shader commands have caller-controlled usage/order requirements. Texture views and copy footprints are checked; uploads support mip/layer regions, padded rows, compressed blocks, and depth aspects. Color MSAA resolves are supported through rendering attachments. Depth/stencil resolves, multiview, sparse images, and arbitrary placement are outside this API.

`Swapchain::acquire` returns the actual `Frame` image or no frame while minimized. Use that image between acquisition and `Frame::present`. Present consumes the recording and uses an acquire semaphore plus a render-complete semaphore owned by each swapchain image. Mailbox is preferred, with FIFO fallback. Abandoned acquisitions, resize, and old swapchains have explicit lifetime handling. Do not use a retained old frame image as a later acquired image.

Explicit waits have caller-selected timeouts. Dropping a pending submission waits up to ten seconds. If completion fails without device loss, its recording and resources are deliberately retained instead of being destroyed while potentially in use. This failure policy does not implement device recovery or intentionally hang/reset a GPU.

## Render graph

`RenderGraphBuilder` creates/imports resources and records passes in declared order. `BufferId` and `ImageId` belong to one graph; foreign handles and pass lookups without declarations fail. Imports of the same resource are deduplicated. `import_image` assumes initialized `GENERAL` content; `import_frame` discards the acquired image's old contents. New transient data cannot be read before a declared write.

Declare reads, writes, or read-write accesses and their pipeline stages. `.bytes(range)` and `.subresources(range)` narrow dependencies. The planner partitions overlapping ranges, retains the last writer and all reader scopes, and batches global barriers. This avoids reflection guesses and covers visibility to multiple reader stages. Writes to disjoint initialized ranges do not create a false resource dependency. Image initialization is conservative across the complete image.

`PassContext` resolves declared buffer ranges, views, image indices, and samplers. The graph retains arguments and declared resources through the returned recording. `record()` returns `Commands` for headless submission or presentation; `record_profiled(true)` also returns timestamp results that can be read after completion.

`ResourceCache` retains reusable transient allocations. It leases an object only when the cache is its sole owner. Submitted work, exported resources, and pending recordings therefore prevent reuse. The engine owns three frame slots, waits before recycling each slot, and imports the real acquired image before graph execution. Cross-submission imports start conservatively at all-command memory access; asynchronous external producers must also be joined through submission waits.

See [the triangle renderer](../zenith-renderer/src/triangle.rs) for a complete draw, [the graph acceptance example](../zenith-rendergraph/examples/graph_smoke.rs) for ranged dependencies and readback, and [the standalone suite](../zenith-rhi/examples/minimal_smoke.rs) for low-level API usage.
