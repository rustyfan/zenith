# Startup optimization results

Implemented and measured on 2026-09-12. Repeated debug startup with cached assets and shaders reaches first accepted presentation in **335.5 ms**, compared with the earlier **1,179.5 ms** baseline: approximately **72% less startup time** on this machine.

## Implemented changes

- Persistent SPIR-V cache behind the unchanged `Gpu::compile_shader` API, under `asset/shaders/v1`. Source/import content digests, include-directory entries, compiler/SDK fingerprint, compiler arguments, environment, and heap strides validate reuse. Corrupt entries trigger recompilation. A successful artifact and its manifest publish together by atomic rename, protected by per-key OS file locks across threads and processes.
- `Gpu::compile_shaders` returns an ordered, typed array and uses up to four scoped workers. All workers finish before an error is returned. Shader debug flags and diagnostics are preserved.
- Empty-scene startup requires only screen-quad, lighting, and IBL shaders. Geometry shaders and their raster pipeline initialize in a background job after a model becomes ready. Rendering polls completion; the existing blocking `add_scene` helper waits. Pipeline statistics use a nonblocking query so they cannot stall presentation behind this job.
- Skybox loading starts during application creation and overlaps Vulkan setup. Models are still requested after the skybox-only first frame. Existing CPU retention policies, upload acknowledgement, revision handling, and checked downcasts are preserved.
- Unchanged surface configurations skip swapchain recreation. Out-of-date/suboptimal recovery, extent changes, and minimize/restore still rebuild as needed.
- `scripts/cook-startup-assets.ps1` runs the existing release asset cooker for the required skybox only. `scripts/profile-startup.ps1` captures repeatable first-presentation measurements. Runtime timing is opt-in through `ZENITH_STARTUP_PROFILE`; it adds a first-submission wait only when enabled.

The RHI now directly depends on BLAKE3, Serde, and serde_json, which were already workspace dependencies. No asset format, async runtime, or public handle design was introduced.

## Measurements

Windows, RTX 4090, debug world executable, existing v2 asset cache, Slang 2026.17, `-O0 -g3`, Vulkan validation and synchronization validation enabled. GPU timing remained enabled. Every startup measurement rendered one frame, loaded one cached skybox, uploaded zero meshes, and requested no model.

| Configuration | Measured runs | Median process-to-present | Range |
|---|---:|---:|---:|
| Earlier baseline, shaders compiled sequentially | 5 | 1,179.5 ms | 1,159.5–1,230.1 ms |
| Implemented cache, 3 hits / 0 compiler processes | 7 | **335.5 ms** | 329.2–342.3 ms |
| Empty shader cache for every run, 3 compiler processes | 5 | **571.1 ms** | 561.5–597.9 ms |

The implementation batches each excluded one initial run from its median; all runs are retained in [startup-optimized-results.csv](startup-optimized-results.csv). The cache-hit batch's initial run took **1,619.3 ms despite zero shader compilations**. The empty-shader-cache batch's initial run took 701.8 ms. Earlier implementation batches measured cache-hit medians of 339.9–343.6 ms and cache-miss medians of 594.7–605.3 ms. These results establish the repeated-launch improvement, not a bound on a first launch after idle, reboot, or driver activity. The earlier baseline and its stage breakdown remain in [the original report](startup-optimization-plan.md).

The boundary is OS process creation to an accepted `vkQueuePresentKHR` return. First-frame GPU submission completion followed approximately 1.18 ms later. This does not measure physical monitor scanout, Cargo build time, or IDE debugger attachment. The earlier baseline and final implementation were measured in separate batches, so the percentage is an approximate comparison.

Fresh skybox preparation with the release cooker took **1.474 s**, excluding its build time; the second invocation took **49.4 ms** with zero imports and one cache hit. A debug world launch using that newly prepared directory also reported zero imports and one cache hit. This moves cooking out of interactive startup. Without prepared assets, runtime still performs the HDR bake; the earlier debug measurements were approximately 8.1–8.3 seconds for that operation.

## Validation

- Workspace tests and all-target/all-feature compilation passed.
- Shader tests cover source content changes with preserved timestamps, imported source edits, directory changes, entry/stage/target/profile/flags/strides, compiler and runtime-library replacement, unchanged unrelated shader contents, corrupt/truncated artifacts, partial writes, inaccessible cache storage, compilation failures and recovery, and simultaneous threads/processes.
- Both real-Slang tests passed, including debug/release output and stage diagnostics. RHI Clippy passed with the repository's existing `manual_is_multiple_of`, `missing_safety_doc`, `collapsible_if`, `too_many_arguments`, and `useless_vec` warnings allowed; an unrestricted `-D warnings` run still encounters those existing warnings.
- All four Vulkan renderer tests passed, including skybox readback, whole-model insertion, CPU release, retained snapshots, reload, and restoration for new consumers.
- Debug and release builds passed. Debug 1,000-frame runs passed with cached and initially missing shaders, and with repeated resize/minimize/restore. A release 1,000-frame run passed. Models reached `Ready`, with no Vulkan validation errors observed.
- After making pipeline statistics nonblocking, the debug run with initially missing shaders recorded a **0.304 ms mean / 28.720 ms peak** CPU graph build/record time after the first frame. The release run recorded **0.219 ms mean / 21.502 ms peak**. These measurements include the model transition and still expose a brief CPU hitch; uploads have not been split into a per-frame byte budget.

Raw logs and cache fixtures are under `target/startup-validation` and `target/startup-measurements`.

## Reproduce

```powershell
$env:SLANG_DIR = 'D:\Software\slang-2026.17-windows-x86_64'
./scripts/cook-startup-assets.ps1 -Offline
cargo build -p zenith-sandbox --example world --offline
./scripts/profile-startup.ps1 -Label cached-repeat -Runs 7
./scripts/profile-startup.ps1 -Label cold-repeat -ColdShaders -Runs 5
```

Choose a new label for each cold-cache batch. `ZENITH_SHADER_CACHE` overrides the runtime cache directory; `0` disables it. `-ShaderCache` selects a directory for the profiling script. Source files and the Slang SDK remain required even on hits, because this cache validates the development compiler and input dependencies. Asset packaging remains available through the existing asset cooker; standalone shader packaging is outside this change.

The dedicated empty-scene skybox pass and persistent Vulkan driver pipeline cache remain deferred: the initial repeated-startup target of 500 ms was met. They would add renderer or driver-cache lifecycle complexity for a smaller remaining opportunity. The full rendering path and skybox quality are preserved.
