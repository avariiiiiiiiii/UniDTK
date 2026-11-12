# Where this code comes from

The crates in this directory are a copy of part of Rune, a native GPU UI framework by Anomaly, published under the MIT licence (see `LICENSE` next to this file).

- **Repository:** `https://github.com/anomalyco/rune`
- **Branch and commit:** `dev` at `2b7dbf223b84b58bcfb1e5a9e92577b9244540dd` (2026-11-11)
- **Why a copy and not a dependency:** UniDTK owns its renderer. From the import on, this is UniDTK code; upstream changes are read and taken by hand, never pulled. The plan is [the native desktop host on Rune](../../docs/dev/plan-rune-host.md).

## What was taken

| Crate | What it is |
| :--- | :--- |
| `rune` | The framework: elements, runtime, layout, text, renderer, windows, input |
| `rune-motion` | The animation runtime: springs, retargetable tracks, presence, matched geometry |
| `rune-taffy` | The compact layout store over Taffy |
| `rune-staging` | Per-frame GPU uploads through reused staging memory |
| `rune-fx` | Effect layers: glow, pulses, motion blur, distortion |
| `rune-light` | Lighting: soft shadows, bevels, god rays, global illumination |
| `rune-notify` | System notifications (toast on Windows) |
| `rune-i18n` | Message catalogs checked at compile time |
| `procwatch` | The process's own memory and CPU, watched from inside |
| `rune-perf` | Bounded local performance recording |

Not taken: `rune-kit`, `rune-mermaid`, `rune-highlights`, `rune-luau`, `rune-knobs`, `rune-mac`, `rune-ref`, `rune-register`, the themes, the examples, the site, the research notes, and the vendored `wgpu-hal` patch.

## Changes made to the copy

1. **Import.** Each crate's `Cargo.toml`, `src/` and embedded assets were copied unchanged. The `tests/`, `examples/` and `benches/` directories were not copied.
2. **Workspace.** The crates are members of the UniDTK workspace. They inherit `edition`, `license` and the lint table from the root `Cargo.toml`, which repeats upstream's values; `clippy.toml` sits in this directory so that it applies to these crates only.
3. **Tests removed.** The `[[test]]`, `[[example]]` and dev-dependency sections were removed from the manifests, every in-source test file (`*_tests.rs`, `tests.rs`, `src/**/tests/`) was deleted together with the `mod` line that declared it (178 files), and each crate sets `[lib] test = false, doctest = false`, so the inline `#[cfg(test)]` blocks that remain are never built. Many of these tests needed a real GPU adapter or fonts from upstream's `opencode` theme, which is not taken.
4. **Other platforms removed.** UniDTK runs only on `x86_64-pc-windows-msvc`, so the `rune` crate lost every module that is never compiled there: a module was removed when the `#[cfg]` on its `mod` line is false for `target_os = "windows"`, `target_arch = "x86_64"`, `target_vendor = "pc"` with `test` off. That is 50 `mod` declarations in 25 files (106 lines with their attributes) and the 86 source files behind them: the macOS, iOS and Apple-vendor code (the native Metal backend `gpu/metal/`, AppKit windows, menus, popup menus, clipboard, drag-out, native surfaces, system materials, screen recording, video through AVFoundation, spelling, shortcuts, display link, embedding, the NSAccessibility adapter), the Linux code (X11, Wayland layer shell and shortcuts, the D-Bus tray, the desktop portals for file dialogs and appearance, the AT-SPI adapter) and the wasm32 code (`platform/web/`, the web clipboard, task and cursor images, the page clock), and `tooling/test_alloc.rs` under `#[cfg(test)]`; modules gated `any(test, <another platform>)` went too, since tests are never built. Also removed: the two `include!`d tables only those modules read (`gpu/metal/builtin_table.rs`, `render/uber/windfoil_hash.rs`), the three `wasm-bindgen` JavaScript files of `platform/web/`, and the seven precompiled Apple shader libraries `src/*.metallib` (about 1.8 MB), whose remaining `include_bytes!` sit behind `cfg(target_os = "macos")`. In all 98 files and 17,178 lines of source. In `Cargo.toml` the six dependency sections for `cfg(target_vendor = "apple")`, `cfg(target_os = "macos")`, `cfg(target_os = "linux")`, `cfg(target_arch = "wasm32")`, `cfg(all(unix, not(target_os = "linux")))` and `cfg(all(unix, not(target_os = "macos"), ...))` were removed; the features `metal-backend`, `ogg-audio` and `dbus`, whose optional dependencies were all in those sections, were removed and dropped from `default`; `accessibility` keeps only `accesskit_windows`, and `native-file-dialog` keeps only `rfd`. Inline code for other platforms in the files that stay was not rewritten: 616 `#[cfg(...)]` attributes in 107 files (316 of them `#[cfg(target_os = "macos")]`) and 75 `cfg!(...)` expressions are false on Windows, so their blocks, some of which name removed modules and dependencies, are compiled out. The cfg predicates that tested `feature = "metal-backend"` or `feature = "dbus"` named a feature the manifest no longer declares, so each predicate was replaced with `any()`, which is always false — the value the absent feature had — so every `all`, `any` and `not` around it evaluates exactly as before on Windows, and `cargo check` reports no `unexpected_cfgs`. The three `private_interfaces` warnings that remain come from upstream unchanged.
5. **Line ends.** The repository's `.gitattributes` marks `core/rune/**` as `-text`, so a Windows checkout with `core.autocrlf` keeps these files as upstream wrote them, with LF. The renderer takes its precompiled DX12 uber shader (`uber.vs.dxbc`, `uber.fs.dxbc`) only while the WGSL source hashes as `render/uber/dxbc_hash.rs` records, and that hash is of the LF text: with CRLF it compiled the shader through FXC on every launch, 56.6 s on an Iris Xe laptop before the first frame (`renderer.begin` to `renderer.ready`, session 138).
6. **Two methods gated.** `CommitMeter::handler` and `CommitMeter::frame_ends` (`tooling/perf_warn/commits.rs`) are called only under `#[cfg(debug_assertions)]`, so a release build of the app warned that they are never used; they carry the same `#[cfg(debug_assertions)]` as their callers now.
