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
