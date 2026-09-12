Ash and ash-window are vendored from https://github.com/ash-rs/ash at revision f4c2ca3e4f6b998d5254ad101a32f024d87cdec2 (Vulkan 1.4.352 bindings).

Archive: https://codeload.github.com/ash-rs/ash/tar.gz/f4c2ca3e4f6b998d5254ad101a32f024d87cdec2

Archive SHA-256: f01d49542dbdeee9b81d39f4b2c6dc299e32c8cc92c3912c9979fd998920babd

The source archive was used because GitHub's Git transport was unavailable. The workspace patch applies the same ash version to VMA and window integration. Upstream licenses are included alongside the source. Third-party generated code retains upstream documentation.

vk-mem is vendored from the published 0.5.0 crate (https://github.com/gwihlidal/vk-mem-rs), including VMA 3.3.0. Its C++ allocator is unchanged. Rust compatibility edits replace ash::prelude::VkResult with ash::VkResult, remove obsolete AllocationCallbacks lifetimes, and adapt the extension-chain trait and push method to the pinned ash revision.
The vk-mem source identifies upstream revision 967541e623ac1e811569f0983c279ac043613b74. Its MIT and Apache license texts are included. The defragmentation return type also has an explicit elided lifetime for current Rust diagnostics.

Shader compilation invokes the Slang SDK's `slangc` executable. No Slang Rust bindings or libclang are needed to build Zenith. The compiler's descriptor heap stride options are supplied directly on the command line.
