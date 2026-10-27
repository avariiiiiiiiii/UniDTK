# Task: Migrate audio player from external mpv process to libmpv

## Context
- Repo: `C:/Users/admin/Documents/GitHub/unidtk`
- Current player: `backend/code/music/src/core/player.rs` spawns external `mpv.exe` per track (no IPC on Windows).
- Problems: pause = kill+respawn, seek is fake, volume applies only next track, time_pos is self-counted (drifts).
- Architecture (docs/architecture.md §6): embed libmpv (DLL) instead of bundling mpv.exe.

## Available assets (already downloaded)
- `backend/opt/libmpv/libmpv-2.dll` (120MB, PE32+ x86-64) — the runtime DLL
- `backend/opt/libmpv/libmpv.dll.a` (53KB) — import lib for linking
- `backend/opt/libmpv/include/mpv/client.h` — headers
- `backend/opt/libmpv/fetch-libmpv.ps1` — fetch script
- NOTE: these are gitignored (large binaries). The .a import lib is small — you may commit it.

## Crate choice
Use `mpv = "0.2.3"` (safe bindings, Cargo.toml of `backend/code/music`).
- It uses `extern "C"` functions from libmpv. On Windows, link against `libmpv.dll.a`.
- To make linking work, add to `backend/code/music/build.rs` (create if missing) OR to `backend/toolchain/build.rs`:
  ```rust
  println!("cargo:rustc-link-search=native={}/backend/opt/libmpv", env!("CARGO_MANIFEST_DIR")...); // adjust path
  println!("cargo:rustc-link-lib=dylib=mpv");
  ```
  Use path relative to workspace. The import lib `libmpv.dll.a` must be renamed/placed so the linker finds `mpv` — check: gcc import libs named `libmpv.dll.a` expose symbols for `-lmpv`. So `cargo:rustc-link-lib=dylib=mpv` should work with `rustc-link-search`.
- Runtime: `libmpv-2.dll` must be next to `shell.exe` (copy in build script / dev script). Add copy step to `BuildApp.vbs`/`DevApp.vbs` or use Tauri `resources` in `tauri.conf.json` (it already bundles `../../frontend/widgets/`, `fonts`, `tdjson.dll` — add libmpv-2.dll similarly). Check `backend/toolchain/tauri.conf.json` resources block and add `"../../opt/libmpv/libmpv-2.dll": "libmpv-2.dll"` (path relative to toolchain dir).

## What to change in `backend/code/music/src/core/player.rs`
Replace process-based playback with libmpv handle:

1. `PlayerInner` struct: replace `child: Option<Child>` with `mpv: Option<mpv::Mpv>` (or `mpv::create()` + `initialize()`).
2. `play_index` / `play_index_track`:
   - Kill existing (drop handle / `mpv.command("stop")`).
   - Create handle, `set_property("volume", v)`, `set_property("pause", false)`, `set_property("end", duration - crossfade)` when crossfade active.
   - `mpv.playlist_append(path)` or `mpv.command("loadfile", &[path, "replace"])`.
   - Remove `try_wait()` immediate-exit detection — replace with checking `mpv.get_property::<bool>("eof-reached")` later; for spawn errors, catch the Result from handle creation/loadfile.
3. `set_pause`: `mpv.set_property("pause", paused)` — NO kill/respawn.
4. `seek`: `mpv.seek(seconds, mpv::Seek::Absolute)` — real seek.
5. `set_volume`: `mpv.set_property("volume", v as i64)` — real, immediate.
6. `status()`: read real `time-pos` from mpv: `mpv.get_property::<f64>("time-pos")` (fallback to internal clock when mpv is dead). Keep the existing Instant clock as fallback only.
7. `check_track_finished`: instead of `child.try_wait()`, read `mpv.get_property::<bool>("eof-reached")` or `idle-active`; when true → auto-advance (same mode logic).
8. `kill_mpv` → `stop_mpv`: `mpv.command("stop")` / drop handle.
9. Update imports: remove `std::process::{Child, Command, Stdio}`, add `mpv` usage.
10. `with_pipe_name` — now pipe_name can actually be used: pass `--input-ipc-server` via `mpv.set_property("input-ipc-server", ...)` or ignore (keep comment updated).

## Keep
- All public API of PlayerManager unchanged (commands.rs depends on it): play_index, play_index_track, set_pause, seek, set_volume, next, previous, status, check_track_finished, load_tracks, jump_relative, enable/disable_ai_dj, ai_dj_*.
- AI DJ logic untouched.
- `DEFAULT_MPV_ARGS` semantics: map each to libmpv properties (`--no-config` → `config=no`, `--force-window=no` → `force-window=no` (not needed in lib), `--no-video` → `video=no`, `--audio-display=no` → `audio-display=no`, `--gapless-audio=yes` → `gapless-audio=yes`, `--replaygain=album` → `replaygain=album`, `--audio-device=auto` → `audio-device=auto`, `--audio-exclusive=no` → `audio-exclusive=no`, `--ao=wasapi` → `ao=wasapi`). Set via `mpv.set_property` or pass as option string via `mpv.set_option`.

## Validation
- `cargo check` (or `cargo build`) in `backend/toolchain` — must compile.
- If libmpv-2.dll can't be found at runtime, that's expected in dev; document that DLL must be next to exe.
- Run `cargo test` for the crate if cheap.

## Deliverable
- Modified: `backend/code/music/src/core/player.rs`, `backend/code/music/Cargo.toml`, possibly new `backend/code/music/build.rs`, `backend/toolchain/tauri.conf.json` (resources), VBS scripts for DLL copy.
- Report: what changed, validation results, remaining risks.
