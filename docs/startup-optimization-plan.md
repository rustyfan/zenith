# First-presentation profiling and optimization plan

Profiled on 2026-09-12. The recommended first change is a persistent SPIR-V cache behind the existing shader compiler API, followed by concurrent compilation on cache misses. For an empty asset cache, pre-cook the required skybox: importing it during startup dominates everything else.

Implementation completed on 2026-09-12: persistent SPIR-V caching, bounded parallel compilation, the skybox pre-cook script, deferred geometry preparation, unchanged-surface resize coalescing, and early skybox loading. See [implementation results and repeatable measurements](startup-optimization-results.md). The separate empty-scene skybox pass and persistent driver pipeline cache remain deferred because the repeated cache-hit startup target was met. The asset streaming design and checked downcasts remain unchanged.

The measurements and proposed order below describe the original baseline and experiments. Temporary experimental implementations were removed before production implementation; the fixed-fixture probe cache is not the production cache.

## Measurement boundary

The main comparison measures **OS process creation to the first accepted `vkQueuePresentKHR` call returning**. It includes executable loading, engine setup, application preparation, first-frame graph recording, submission, and presentation. An additional probe waited for that frame's GPU submission after presentation; completion followed the presentation return by approximately 1.1 ms.

This measures the application's first presentation, not physical monitor scanout. Desktop compositor/scanout latency and IDE build/debugger-attachment time are outside the measurement. The old `WorldApp prepare timings` alone cannot describe first-presentation latency: it excludes Vulkan initialization and the first render/presentation.

Configuration: Windows, debug world executable, existing v2 asset cache, current Slang SDK, shader flags `-O0 -g3`, Vulkan validation and synchronization validation enabled. GPU timing was enabled consistently. Each run rendered one frame, so no model was requested. The comparison alternated baseline, shader-cache, and parallel-compiler modes, using five measured runs per mode after one warm-up per mode. All these presentations were explicitly accepted, and runs exited successfully without validation errors.

## Results

| Experiment | Median process-to-present | Range | Interpretation |
|---|---:|---:|---|
| Original startup, existing asset cache | **1,179.5 ms** | 1,159.5–1,230.1 ms | Main baseline |
| Reuse previously compiled SPIR-V | **386.4 ms** | 362.0–416.4 ms | About **67% faster**; same debug shaders and validation |
| Compile five shaders concurrently, empty shader cache each run | **636.7 ms** | 575.7–663.4 ms | About **46% faster** on shader-cache misses |

The first observed launches in two profiling batches took **2.53 and 2.59 seconds**, despite existing asset caches. Their `vkCreateInstance` calls took about **970–998 ms**, and initial geometry pipeline creation took about **183–185 ms**. These are first-observation samples, not a controlled cold-OS-cache benchmark. Warm process launches were substantially faster; do not promise the warm median for every machine or first launch.

Additional isolated asset-cache-miss runs:

| Asset-cache condition | Process-to-present | Skybox CPU readiness | Notes |
|---|---:|---:|---|
| Empty asset cache, normal shader compilation | **9.665 s** | **8.293 s** | Also encountered a 951 ms Vulkan instance creation |
| Empty asset cache, reused SPIR-V | **8.486 s** | **8.120 s** | Shader caching cannot remove the required HDR bake |

These are individual runs using fresh directories under `target/startup-profile`, leaving the existing asset cache intact. Their presentation-return timestamps were captured before the explicit presentation-acceptance marker was added. The difference between them is not an isolated shader-cache speedup: Vulkan initialization and HDR bake time also varied.

## Where the existing-cache time goes

Medians from the alternating baseline runs; nested or overlapping stages must not be added together.

| Stage | Time | Consequence |
|---|---:|---|
| Process creation to Rust entry | 32.5 ms | Small relative to shader compilation |
| Engine/GPU/swapchain/descriptors setup | 199.6 ms | Includes `vkCreateInstance`, median 124.4 ms |
| Five sequential Slang invocations | **798.8 ms** | Largest avoidable cost on every launch |
| Two graphics pipeline creations, combined | 40.3 ms | Includes the lighting pipeline created during the first frame |
| Compute pipeline creation | 13.6 ms | Required by current IBL initialization |
| Cached skybox CPU load | **54.7 ms** | Overlaps shader compilation; finishes well before preparation needs it |
| Skybox upload and completion | 20.6 ms | Much smaller than compilation |
| Preparation complete to presentation return | 38.0 ms | Includes redundant startup resize handling and the first frame |
| GPU completion after presentation return | 1.1 ms | GPU execution is not the seconds-long delay |

The temporary cache reduced all five shader lookups to approximately **0.76 ms** combined. The concurrent-compilation batch took approximately **219 ms** of wall time. It used more concurrent CPU work; summing its overlapping shader durations does not measure startup latency.

The original startup path compiled:

1. `defer_shading.slang::vsmain`
2. `defer_shading.slang::psmain`
3. `screen_quad.slang::vsmain`
4. `lighting.slang::main`
5. `ibl_diffuse.slang::main`

At the baseline, `Gpu::compile_shader` invoked `slangc` synchronously for each entry point. The pipeline map contains weak references to live pipelines within one process. Production now persists SPIR-V separately; raster and compute creation still pass a null Vulkan pipeline cache.

## Implementation order

### 1. Cache compiled shaders without changing callers

**Scope:** `zenith-rhi/src/shader.rs`, a small private shader-cache module, shader tests.

Keep `Gpu::compile_shader(...) -> Result<Shader>` and its existing debug behavior. On a valid cache hit, load SPIR-V; on a miss, run Slang and store the successful result. Keep shader compilation/cache ownership separate from CPU asset retention and renderer GPU residency.

The cache key/manifest must cover:

- Shader source and transitive imported/included files, including resolved paths and content digests.
- Entry point, stage, target/profile, capabilities, include roots, compiler arguments, debug/optimization flags, and both descriptor-heap strides.
- Compiler identity/version and a cache-format version.

The installed Slang CLI supports `-depfile`; use its dependency output to discover the source closure. Parse its Makefile escaping correctly, including Windows paths. Validate dependencies on a hit without launching Slang again. Detect compiler replacement cheaply, caching its fingerprint instead of spawning a version query for every shader.

Use bounded SPIR-V reads, integrity validation, and atomic publication. Corrupt, incomplete, or incompatible cache entries should cause recompilation. A compile failure must still report the current diagnostic. Deduplicate concurrent requests for the same key and handle multiple processes writing the same artifact.

The performance probe cached fixed, unchanged shader fixtures and deliberately omitted production invalidation. Its 386 ms result demonstrates the opportunity; it is not a production-ready cache implementation.

**Target:** approximately **0.4–0.5 seconds** for repeated debug startup on this machine with assets and shaders cached, keeping validation and shader debug information enabled. This is an engineering target, not a guaranteed bound on first-process driver initialization.

### 2. Compile independent cache misses concurrently

**Scope:** a small shader-batch helper and renderer construction in `world.rs`, `lighting.rs`, and `ibl.rs`.

Use a bounded batch of scoped threads for independent shader compilation. Return typed `Shader` results and create pipelines after their required shaders are available. Join all jobs and propagate their errors. Avoid a new async runtime or borrowing the asset coordinator thread to run Slang.

The probe ran five compiler processes concurrently; production concurrency should be bounded for the host. Compile only misses after cache validation, and preserve deterministic cache keys and error reporting. The persistent cache provides the larger recurring benefit; concurrency improves a new checkout, shader edits, and cache invalidation.

**Measured opportunity:** roughly **0.58–0.66 seconds** in the alternating cold-shader-cache experiment. A production worker limit and dependency validation need their own measurement.

### 3. Pre-cook the required skybox for fresh installs/checkouts

**Scope:** existing asset CLI, bootstrap/package workflow, example documentation. No asset format or importer redesign.

Run the existing cooker before interactive startup:

```powershell
cargo run -p zenith-asset --release --example assets -- cook content asset texture/minedump_flats_4k.hdr
```

Distribute that v2 artifact and manifest with the application, or make this an explicit initial project-setup step. Match the source, importer/settings, codec, and target profile expected by runtime. Do not bake every model as a prerequisite for the skybox-only first frame.

This moves the measured **8.1–8.3 second** HDR bake out of interactive startup. It does not eliminate the computation. If first run must work immediately without prepared content, a small bundled preview skybox followed by background replacement is a separate fallback design; preserving the full requested skybox on the first frame requires that asset to be ready.

### 4. Remove unnecessary work from the empty-scene startup path

**Scope:** renderer initialization and startup resize handling; implement after the cache gains are measured.

- Defer geometry shaders/pipeline until the first model needs them. The current empty scene still initializes the geometry pipeline.
- Consider a small skybox pass that avoids material lighting, G-buffer allocation, and diffuse-SH integration while there are no models. Preserve the existing full path for model rendering and diffuse-SH debug mode. This is a larger change than shader caching, so keep it separate.
- Coalesce redundant startup resize events. Three additional swapchain rebuilds occurred before the first frame, typically costing around **15–17 ms combined**. Skip only genuinely unchanged surface state; preserve out-of-date/suboptimal recovery and minimize/restore handling.
- Once shader compilation is cached, requesting the skybox during application creation can overlap its approximately 53 ms cached load with GPU setup. Its current overlap with renderer construction already hides most of that cost, so measure the remaining wait before changing lifecycle code.

These savings are not additive with the shader-cache/parallel results. Delaying geometry setup also moves some work to first-model insertion; prepare it off the presentation path where supported and measure that transition for hitches.

### 5. Evaluate a persistent Vulkan pipeline cache only if needed

**Scope:** GPU-owned pipeline-cache lifetime plus raster/compute creation.

The warm baseline spent approximately **54 ms total** creating graphics and compute pipelines. Re-profile after steps 1–4 before adding disk persistence. A driver pipeline cache cannot remove Slang compilation or `vkCreateInstance` time.

If justified, load/save compatible driver cache data and pass the cache into pipeline creation. Validate device/driver/cache compatibility, serialize access as required, and fall back to an empty cache when data is rejected. Keep the existing live-pipeline deduplication map. No latency reduction for this step was measured here.

## Changes that are not priorities

- More handle/downcast optimization: the measured cached skybox is ready while shaders are still compiling. Keep checked downcasts.
- Disabling validation: a separate exploratory batch measured about 1.18 s, within the broader baseline variation; it does not account for the shader bottleneck.
- Switching debug shaders to `-O3 -g0`: a nearby control measured 1.142 s versus 1.124 s with optimized shaders, with overlapping ranges. This is not a material startup fix and changes debugging behavior.
- Reducing skybox resolution by default: it changes quality and does not address the approximately 800 ms spent compiling shaders.
- Replacing the asset server/scheduler: the critical recurring work is in renderer/RHI initialization.

## Acceptance and repeatability

- Measure process-to-first-accepted-presentation and first-frame GPU completion, separately from prepare time and model-ready time.
- Report repeated warm launches and first-observed launches separately. Cover asset-cache hit/miss and shader-cache hit/miss independently; exclude Cargo build/debugger setup from runtime comparisons.
- Verify source/import/compiler/entry/stage/stride/flag changes invalidate the correct shader artifacts. Test corrupt caches, failed compilation, simultaneous requests, and interrupted writes.
- Confirm identical skybox output, zero model requests before the first frame, later model insertion, CPU release, and reload behavior under Vulkan synchronization validation.
- Re-profile after each step. Cache-hit startup below 500 ms is the initial target on the tested setup; record driver initialization outliers rather than hiding them.

Per-run results are retained in [startup-profile-results.csv](startup-profile-results.csv). Raw traces, logs, probe scripts, the instrumentation diff, and a profiled executable are under `target/startup-profile`. The CSV value `not_instrumented` means earlier runs did not record the additional presentation-acceptance marker, not that presentation failed.

The retained profiled executable can repeat the alternating comparison without modifying source:

```powershell
./target/startup-profile/benchmark.ps1 -Label repeat -Executable target/startup-profile/world-profiled.exe -CompareModes -Runs 17
```

Use a new label for each comparison so each parallel run receives a fresh shader-cache directory. The saved prototype shader cache is only for these unchanged-source experiments. Do not use it for normal development or packaging.
