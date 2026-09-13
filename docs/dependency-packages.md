# Resolved package audit

Default workspace features, Windows MSVC. Includes build/dev dependencies. Optional codec/profiling packages are absent from this graph but available through the documented features. Versions reflect the local Cargo resolution; Cargo.lock is ignored by this repository.

| Package | Before | After | Decision / current dependents |
| --- | --- | --- | --- |
| `adler2 2.0.1` | yes | yes | Retained via `miniz_oxide` |
| `aho-corasick 1.1.4` | yes | yes | Retained via `regex`, `regex-automata` |
| `aligned 0.4.3` | yes | no | Removed from default graph |
| `aligned-vec 0.6.4` | yes | no | Removed from default graph |
| `allocator-api2 0.2.21` | yes | no | Removed from default graph |
| `anstream 0.6.21` | yes | yes | Retained via `env_logger` |
| `anstyle 1.0.13` | yes | yes | Retained via `anstream`, `anstyle-wincon`, `clap_builder`, `env_logger` |
| `anstyle-parse 0.2.7` | yes | yes | Retained via `anstream` |
| `anstyle-query 1.1.5` | yes | yes | Retained via `anstream` |
| `anstyle-wincon 3.0.11` | yes | yes | Retained via `anstream` |
| `anyhow 1.0.101` | yes | yes | Retained via `zenith`, `zenith-asset`, `zenith-core`, `zenith-renderer`, `zenith-rendergraph`, `zenith-rhi`, `zenith-sandbox` |
| `arg_enum_proc_macro 0.3.4` | yes | no | Removed from default graph |
| `arrayvec 0.7.6` | yes | no | Removed from default graph |
| `as-slice 0.2.1` | yes | no | Removed from default graph |
| `ash 0.38.0+1.4.352` | yes | yes | Retained via `ash-window`, `vk-mem`, `zenith-asset`, `zenith-rhi` |
| `ash-window 0.13.0` | yes | yes | Retained via `zenith-rhi` |
| `autocfg 1.5.0` | yes | yes | Retained via `num-traits` |
| `av-scenechange 0.14.1` | yes | no | Removed from default graph |
| `av1-grain 0.2.5` | yes | no | Removed from default graph |
| `avif-serialize 0.8.8` | yes | no | Removed from default graph |
| `base64 0.13.1` | yes | yes | Retained via `gltf` |
| `bincode 1.3.3` | yes | no | Removed from default graph |
| `bincode 2.0.1` | yes | yes | Retained via `zenith-asset` |
| `bincode_derive 2.0.1` | yes | no | Removed from default graph |
| `bindgen 0.72.1` | yes | no | Removed from default graph |
| `bit_field 0.10.3` | yes | no | Removed from default graph |
| `bitflags 2.10.0` | yes | yes | Retained via `png`, `vk-mem`, `winit`, `zenith-renderer` |
| `bitstream-io 4.9.0` | yes | no | Removed from default graph |
| `built 0.8.0` | yes | no | Removed from default graph |
| `bumpalo 3.19.1` | yes | no | Removed from default graph |
| `bytemuck 1.25.0` | yes | yes | Retained via `image`, `zenith-asset`, `zenith-renderer`, `zenith-rendergraph`, `zenith-rhi` |
| `bytemuck_derive 1.10.2` | yes | yes | Retained via `bytemuck` |
| `byteorder 1.5.0` | yes | yes | Retained via `gltf` |
| `byteorder-lite 0.1.0` | yes | yes | Retained via `image`, `image-webp` |
| `cc 1.2.55` | yes | yes | Retained via `ispc-texcomp`, `vk-mem`, `zstd-sys` |
| `cexpr 0.6.0` | yes | no | Removed from default graph |
| `cfg-if 1.0.4` | yes | yes | Retained via `crc32fast`, `getrandom`, `half`, `parking_lot_core` |
| `cfg_aliases 0.2.1` | yes | yes | Retained via `winit` |
| `clang-sys 1.8.1` | yes | no | Removed from default graph |
| `clap 4.5.58` | yes | yes | Retained via `zenith-core` |
| `clap_builder 4.5.58` | yes | yes | Retained via `clap` |
| `clap_derive 4.5.55` | yes | yes | Retained via `clap` |
| `clap_lex 1.0.0` | yes | yes | Retained via `clap_builder` |
| `color_quant 1.1.0` | yes | no | Removed from default graph |
| `colorchoice 1.0.4` | yes | yes | Retained via `anstream` |
| `convert_case 0.10.0` | yes | no | Removed from default graph |
| `core2 0.4.0` | yes | no | Removed from default graph |
| `crc32fast 1.5.0` | yes | yes | Retained via `flate2`, `png` |
| `crossbeam-channel 0.5.15` | yes | no | Removed from default graph |
| `crossbeam-deque 0.8.6` | yes | yes | Retained via `rayon-core` |
| `crossbeam-epoch 0.9.18` | yes | yes | Retained via `crossbeam-deque` |
| `crossbeam-utils 0.8.21` | yes | yes | Retained via `crossbeam-deque`, `crossbeam-epoch`, `rayon-core` |
| `cursor-icon 1.2.0` | yes | yes | Retained via `winit` |
| `darling 0.20.11` | yes | no | Removed from default graph |
| `darling_core 0.20.11` | yes | no | Removed from default graph |
| `darling_macro 0.20.11` | yes | no | Removed from default graph |
| `derive_builder 0.20.2` | yes | no | Removed from default graph |
| `derive_builder_core 0.20.2` | yes | no | Removed from default graph |
| `derive_builder_macro 0.20.2` | yes | no | Removed from default graph |
| `derive_more 2.1.1` | yes | yes | Retained via `zenith-core` |
| `derive_more-impl 2.1.1` | yes | yes | Retained via `derive_more` |
| `dpi 0.1.2` | yes | yes | Retained via `winit` |
| `either 1.15.0` | yes | yes | Retained via `rayon` |
| `enumflags2 0.7.12` | yes | no | Removed from default graph |
| `enumflags2_derive 0.7.12` | yes | no | Removed from default graph |
| `env_filter 1.0.0` | yes | yes | Retained via `env_logger` |
| `env_logger 0.11.9` | yes | yes | Retained via `zenith-core` |
| `equator 0.4.2` | yes | no | Removed from default graph |
| `equator-macro 0.4.2` | yes | no | Removed from default graph |
| `equivalent 1.0.2` | yes | no | Removed from default graph |
| `exr 1.74.0` | yes | no | Removed from default graph |
| `fax 0.2.6` | yes | yes | Retained via `tiff` |
| `fax_derive 0.2.0` | yes | yes | Retained via `fax` |
| `fdeflate 0.3.7` | yes | yes | Retained via `png` |
| `find-msvc-tools 0.1.9` | yes | yes | Retained via `cc` |
| `flate2 1.1.9` | yes | yes | Retained via `png`, `tiff` |
| `fnv 1.0.7` | yes | no | Removed from default graph |
| `foldhash 0.1.5` | yes | yes | Retained via `hashbrown`, `zenith-core` |
| `getrandom 0.3.4` | yes | yes | Retained via `jobserver` |
| `gif 0.14.1` | yes | no | Removed from default graph |
| `glam 0.32.0` | yes | yes | Retained via `zenith-asset`, `zenith-core`, `zenith-renderer`, `zenith-sandbox` |
| `glob 0.3.3` | yes | yes | Retained via `ispc-texcomp` |
| `gltf 1.4.1` | yes | yes | Retained via `zenith-asset` |
| `gltf-derive 1.4.1` | yes | yes | Retained via `gltf-json` |
| `gltf-json 1.4.1` | yes | yes | Retained via `gltf` |
| `half 2.7.1` | yes | yes | Retained via `tiff`, `zenith-asset` |
| `hashbrown 0.15.5` | yes | yes | Retained via `zenith-core` |
| `heck 0.5.0` | yes | yes | Retained via `clap_derive` |
| `ident_case 1.0.1` | yes | no | Removed from default graph |
| `image 0.25.9` | yes | yes | Retained via `gltf`, `zenith-asset` |
| `image-webp 0.2.4` | yes | yes | Retained via `image` |
| `imgref 1.12.0` | yes | no | Removed from default graph |
| `inflections 1.1.1` | yes | yes | Retained via `gltf-derive` |
| `is_terminal_polyfill 1.70.2` | yes | yes | Retained via `anstream` |
| `ispc-texcomp 0.1.20` | yes | yes | Retained via `zenith-asset` |
| `ispc_rt 1.1.0` | yes | yes | Retained via `ispc-texcomp` |
| `itertools 0.10.5` | yes | no | Removed from default graph |
| `itertools 0.13.0` | yes | no | Removed from default graph |
| `itertools 0.14.0` | yes | no | Removed from default graph |
| `itoa 1.0.17` | yes | yes | Retained via `serde_json` |
| `jiff 0.2.20` | yes | yes | Retained via `env_logger` |
| `jobserver 0.1.34` | yes | yes | Retained via `cc` |
| `lazy_static 1.5.0` | yes | yes | Retained via `gltf` |
| `lebe 0.5.3` | yes | no | Removed from default graph |
| `libc 0.2.181` | yes | yes | Retained via `ispc_rt` |
| `libloading 0.8.9` | yes | yes | Retained via `ash` |
| `lock_api 0.4.14` | yes | yes | Retained via `parking_lot` |
| `log 0.4.29` | yes | yes | Retained via `env_filter`, `env_logger`, `jiff`, `zenith-core` |
| `loop9 0.1.5` | yes | no | Removed from default graph |
| `lz4_flex 0.11.5` | yes | no | Removed from default graph |
| `maybe-rayon 0.1.1` | yes | no | Removed from default graph |
| `memchr 2.8.0` | yes | yes | Retained via `aho-corasick`, `regex`, `regex-automata`, `serde_json` |
| `memmap2 0.9.9` | yes | no | Removed from Zenith's direct dependencies and the Windows build graph; still transitive through `winit` on other platforms |
| `minimal-lexical 0.2.1` | yes | no | Removed from default graph |
| `miniz_oxide 0.8.9` | yes | yes | Retained via `flate2`, `png` |
| `more-asserts 0.3.1` | yes | yes | Retained via `ispc-texcomp` |
| `moxcms 0.7.11` | yes | yes | Retained via `image` |
| `new_debug_unreachable 1.0.6` | yes | no | Removed from default graph |
| `nom 7.1.3` | yes | no | Removed from default graph |
| `nom 8.0.0` | yes | no | Removed from default graph |
| `noop_proc_macro 0.3.0` | yes | no | Removed from default graph |
| `num-bigint 0.4.6` | yes | no | Removed from default graph |
| `num-derive 0.4.2` | yes | no | Removed from default graph |
| `num-integer 0.1.46` | yes | no | Removed from default graph |
| `num-rational 0.4.2` | yes | no | Removed from default graph |
| `num-traits 0.2.19` | yes | yes | Retained via `image`, `moxcms`, `pxfm` |
| `num_cpus 1.17.0` | yes | yes | Retained via `ispc_rt` |
| `once_cell 1.21.3` | yes | no | Removed from default graph |
| `once_cell_polyfill 1.70.2` | yes | yes | Retained via `anstyle-wincon` |
| `parking_lot 0.12.5` | yes | yes | Retained via `zenith-asset` |
| `parking_lot_core 0.9.12` | yes | yes | Retained via `parking_lot` |
| `paste 1.0.15` | yes | no | Removed from default graph |
| `pastey 0.1.1` | yes | no | Removed from default graph |
| `pin-project-lite 0.2.16` | yes | yes | Retained via `tracing` |
| `pkg-config 0.3.32` | yes | yes | Retained via `zstd-sys` |
| `png 0.18.0` | yes | yes | Retained via `image` |
| `prettyplease 0.2.37` | yes | no | Removed from default graph |
| `proc-macro2 1.0.106` | yes | yes | Retained via `bytemuck_derive`, `clap_derive`, `derive_more-impl`, `fax_derive`, `gltf-derive`, `quote`, `serde_derive`, `syn`, `zerocopy-derive` |
| `profiling 1.0.17` | yes | yes | Retained via `zenith`, `zenith-asset`, `zenith-core` |
| `profiling-procmacros 1.0.17` | yes | yes | Retained via `profiling` |
| `puffin 0.19.1` | yes | no | Removed from default graph |
| `puffin_http 0.16.1` | yes | no | Removed from default graph |
| `pxfm 0.1.27` | yes | yes | Retained via `moxcms` |
| `qoi 0.4.1` | yes | no | Removed from default graph |
| `quick-error 2.0.1` | yes | yes | Retained via `image-webp`, `tiff` |
| `quote 1.0.44` | yes | yes | Retained via `bytemuck_derive`, `clap_derive`, `derive_more-impl`, `fax_derive`, `gltf-derive`, `profiling-procmacros`, `serde_derive`, `syn`, `zerocopy-derive` |
| `rav1e 0.8.1` | yes | no | Removed from default graph |
| `ravif 0.12.0` | yes | no | Removed from default graph |
| `raw-window-handle 0.6.2` | yes | yes | Retained via `ash-window`, `winit`, `zenith-rhi` |
| `rayon 1.11.0` | yes | yes | Retained via `zenith-asset` |
| `rayon-core 1.13.0` | yes | yes | Retained via `rayon` |
| `regex 1.12.3` | yes | yes | Retained via `env_filter` |
| `regex-automata 0.4.14` | yes | yes | Retained via `regex` |
| `regex-syntax 0.8.9` | yes | yes | Retained via `regex`, `regex-automata` |
| `rgb 0.8.52` | yes | no | Removed from default graph |
| `rustc-hash 2.1.1` | yes | no | Removed from default graph |
| `rustc_version 0.4.1` | yes | yes | Retained via `derive_more-impl` |
| `rustversion 1.0.22` | yes | no | Removed from default graph |
| `scopeguard 1.2.0` | yes | yes | Retained via `lock_api` |
| `semver 1.0.27` | yes | yes | Retained via `rustc_version` |
| `serde 1.0.228` | yes | yes | Retained via `bincode`, `gltf-json`, `smol_str`, `zenith-asset` |
| `serde_core 1.0.228` | yes | yes | Retained via `jiff`, `serde`, `serde_json` |
| `serde_derive 1.0.228` | yes | yes | Retained via `gltf-json`, `serde` |
| `serde_json 1.0.149` | yes | yes | Retained via `gltf`, `gltf-json` |
| `shader-slang 0.1.0` | yes | no | Removed from default graph |
| `shader-slang-sys 0.1.0` | yes | no | Removed from default graph |
| `shlex 1.3.0` | yes | yes | Retained via `cc` |
| `simd-adler32 0.3.8` | yes | yes | Retained via `fdeflate`, `miniz_oxide` |
| `simd_helpers 0.1.0` | yes | no | Removed from default graph |
| `smallvec 1.15.1` | yes | yes | Retained via `parking_lot_core`, `zenith-core` |
| `smol_str 0.2.2` | yes | yes | Retained via `winit` |
| `stable_deref_trait 1.2.1` | yes | no | Removed from default graph |
| `strsim 0.11.1` | yes | no | Removed from default graph |
| `syn 2.0.115` | yes | yes | Retained via `bytemuck_derive`, `clap_derive`, `derive_more-impl`, `fax_derive`, `gltf-derive`, `profiling-procmacros`, `serde_derive`, `zerocopy-derive` |
| `thiserror 2.0.18` | yes | no | Removed from default graph |
| `thiserror-impl 2.0.18` | yes | no | Removed from default graph |
| `tiff 0.10.3` | yes | yes | Retained via `image` |
| `tracing 0.1.44` | yes | yes | Retained via `winit` |
| `tracing-core 0.1.36` | yes | yes | Retained via `tracing` |
| `unicode-ident 1.0.23` | yes | yes | Retained via `proc-macro2`, `syn` |
| `unicode-segmentation 1.12.0` | yes | yes | Retained via `winit` |
| `unicode-xid 0.2.6` | yes | no | Removed from default graph |
| `unty 0.0.4` | yes | yes | Retained via `bincode` |
| `urlencoding 2.1.3` | yes | yes | Retained via `gltf` |
| `utf8parse 0.2.2` | yes | yes | Retained via `anstream`, `anstyle-parse` |
| `v_frame 0.3.9` | yes | no | Removed from default graph |
| `virtue 0.0.18` | yes | no | Removed from default graph |
| `vk-mem 0.5.0` | yes | yes | Retained via `zenith-rhi` |
| `wasm-bindgen 0.2.108` | yes | no | Removed from default graph |
| `wasm-bindgen-macro 0.2.108` | yes | no | Removed from default graph |
| `wasm-bindgen-macro-support 0.2.108` | yes | no | Removed from default graph |
| `wasm-bindgen-shared 0.2.108` | yes | no | Removed from default graph |
| `weezl 0.1.12` | yes | yes | Retained via `tiff` |
| `windows-link 0.2.1` | yes | yes | Retained via `libloading`, `parking_lot_core`, `windows-sys` |
| `windows-sys 0.52.0` | yes | yes | Retained via `winit` |
| `windows-sys 0.61.2` | yes | yes | Retained via `anstyle-query`, `anstyle-wincon` |
| `windows-targets 0.52.6` | yes | yes | Retained via `windows-sys` |
| `windows_x86_64_msvc 0.52.6` | yes | yes | Retained via `windows-targets` |
| `winit 0.30.12` | yes | yes | Retained via `zenith`, `zenith-core`, `zenith-rhi`, `zenith-sandbox` |
| `y4m 0.8.0` | yes | no | Removed from default graph |
| `zenith 0.1.0` | yes | yes | Workspace member |
| `zenith-asset 0.1.0` | yes | yes | Workspace member |
| `zenith-core 0.1.0` | yes | yes | Workspace member |
| `zenith-renderer 0.1.0` | yes | yes | Workspace member |
| `zenith-rendergraph 0.1.0` | yes | yes | Workspace member |
| `zenith-rhi 0.1.0` | yes | yes | Workspace member |
| `zenith-sandbox 0.1.0` | yes | yes | Workspace member |
| `zerocopy 0.8.39` | yes | yes | Retained via `half` |
| `zerocopy-derive 0.8.39` | yes | yes | Retained via `zerocopy` |
| `zmij 1.0.21` | yes | yes | Retained via `serde_json` |
| `zstd 0.13.3` | yes | yes | Retained via `zenith-asset` |
| `zstd-safe 7.2.4` | yes | yes | Retained via `zstd` |
| `zstd-sys 2.0.16+zstd.1.5.7` | yes | yes | Retained via `zstd-safe` |
| `zune-core 0.4.12` | yes | yes | Retained via `zune-jpeg` |
| `zune-core 0.5.1` | yes | yes | Retained via `image`, `zune-jpeg` |
| `zune-inflate 0.2.54` | yes | no | Removed from default graph |
| `zune-jpeg 0.4.21` | yes | yes | Retained via `tiff` |
| `zune-jpeg 0.5.12` | yes | yes | Retained via `image` |
