# libmpv in the Player: In-Process Playback Through a Raw-dylib FFI

This chapter explains how the music player plays audio through libmpv, the client library of the mpv media player, which runs inside the app. It shows how the Rust code calls the DLL without an import library, how one player instance is created and driven, and which threads touch it.

The first version of this work was a task note. It planned to use the `mpv` crate and a `build.rs` link directive, and it described the player as spawning `mpv.exe`. The code now does something different, and this chapter describes the code. The differences are listed at the end.

## What you will understand by the end

- How Rust calls functions in `libmpv-2.dll` without a `.lib` or `.dll.a` import library.
- How a safe `Libmpv` wrapper turns C strings, C doubles, and C flags into Rust calls.
- What the player does to start, pause, seek, change volume, and stop a track.
- Which threads touch the libmpv handle, and how the player learns that a track has ended.
- Which parts of the old design are gone, and which settings are left over.

## Files involved

| File | Role |
| :--- | :--- |
| [modules/mp3-player/rust/ffi/libmpv.rs](../../modules/mp3-player/rust/ffi/libmpv.rs) | The FFI declarations and the `Libmpv` wrapper (252 lines). |
| [modules/mp3-player/rust/playback/player.rs](../../modules/mp3-player/rust/playback/player.rs) | The player: options, track start, pause, seek, volume, the end-of-track poll (1,084 lines). |
| [modules/mp3-player/rust/Cargo.toml](../../modules/mp3-player/rust/Cargo.toml) | The crate's dependencies. The `mpv` crate is not among them. |
| [modules/mp3-player/rust/library/config.rs](../../modules/mp3-player/rust/library/config.rs) | Holds an `mpv_path` setting that nothing reads (Step 9). |
| [opt/libmpv/fetch-libmpv.ps1](../../opt/libmpv/fetch-libmpv.ps1) | Downloads the DLL and its headers. |
| [toolchain/build.rs](../../toolchain/build.rs) | Copies `opt/libmpv` next to the built binaries. |
| [toolchain/tauri.conf.json](../../toolchain/tauri.conf.json) | Lists `libmpv-2.dll` as an installer resource. |

---

## Step 1: Getting the DLL

[fetch-libmpv.ps1](../../opt/libmpv/fetch-libmpv.ps1) downloads one archive from the mpv-winbuild-cmake releases, release `20260828`, commit `git-182fa6ca49`. It extracts only `libmpv-2.dll` and the headers with 7-Zip, and it copies the DLL to `opt/libmpv/libmpv-2.dll`. The script skips the download when the DLL is already there.

The repository's copy has these properties:

| Item | Value |
| :--- | :--- |
| `opt/libmpv/libmpv-2.dll` | 120,323,072 bytes |
| SHA-256 of `libmpv-2.dll` | `82be8edd8e61bd7a02458efaf648d6414e262d59e9873d516a2e107579618fe2` |
| `opt/libmpv/extracted/` | Headers (`include/mpv/client.h`, `render.h`, `render_gl.h`, `stream_cb.h`), the DLL, and `libmpv.dll.a` |

The `libmpv.dll.a` import library is in `extracted/`. The Rust code does not use it (Step 2). It is kept only because the archive contained it.

---

## Step 2: Calling the DLL without an import library

Usually a program that calls a DLL links against an import library (`.lib`, or `.dll.a` with MinGW). The import library tells the linker where each function lives. This code instead uses a Rust attribute that tells the compiler to read the DLL directly:

```rust
#[link(name = "libmpv-2", kind = "raw-dylib")]
extern "C" {
    pub fn mpv_create() -> *mut mpv_handle;
    pub fn mpv_initialize(ctx: *mut mpv_handle) -> c_int;
    pub fn mpv_destroy(ctx: *mut mpv_handle);
    pub fn mpv_set_option_string(ctx: *mut mpv_handle, name: *const c_char, data: *const c_char) -> c_int;
    pub fn mpv_set_property(ctx: *mut mpv_handle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
    pub fn mpv_set_property_string(ctx: *mut mpv_handle, name: *const c_char, data: *const c_char) -> c_int;
    pub fn mpv_get_property(ctx: *mut mpv_handle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
    pub fn mpv_command(ctx: *mut mpv_handle, args: *const *const c_char) -> c_int;
}
```

With `raw-dylib`, the compiler builds the needed import stubs by itself from the DLL's export table. No import library is needed at build time. The DLL is still needed at run time. It must sit next to the executable, or the process fails to start with a missing-DLL error.

Only eight functions are declared. Nothing else from the libmpv API is used. In particular there is no `mpv_wait_event`, no `mpv_observe_property`, and no wakeup callback. Step 7 explains what this means for the threads.

Because `raw-dylib` needs no link directive, `build.rs` has no `rustc-link-search` line for libmpv. A search for `rustc-link` in the toolchain and in the module finds none.

---

## Step 3: The safe wrapper

`Libmpv` wraps the raw handle. It is the only type that the rest of the module touches.

### 3.1 Creating the handle

```rust
pub fn new() -> Result<Self, String> {
    let handle = unsafe { mpv_create() };
    if handle.is_null() { return Err("mpv_create failed (out of memory?)".to_string()); }
    let ret = unsafe { mpv_initialize(handle) };
    if ret != MPV_ERROR_SUCCESS {
        unsafe { mpv_destroy(handle) };
        return Err(format!("mpv_initialize failed: {ret}"));
    }
    Ok(Libmpv { handle })
}
```

`mpv_create` allocates a context, and `mpv_initialize` starts it. If initialization fails, the context is destroyed at once, so no half-built handle leaks.

### 3.2 Setting options and properties

libmpv has three kinds of settings, and the wrapper has a method for each:

- **Options** are set once, before initialization or on a fresh handle, with `set_option(name, value)`. They take strings.
- **Properties** can be set at any time. `set_property_str` takes a string. `set_property_double` takes a 64-bit float with the `MPV_FORMAT_DOUBLE` tag.
- **Flags** are booleans. `set_property_flag` first tries the string `yes` or `no`. If that fails, it sends the value again with the `MPV_FORMAT_FLAG` tag and an `int`. The fallback exists because some properties accept only one of the two forms.

Reading works the same way. `get_property_double` and `get_property_flag` ask libmpv for a value in a given format and return an error if libmpv refuses.

### 3.3 Commands

A command is a list of strings, such as `loadfile <path> replace` or `seek <seconds> absolute`. The wrapper builds a C array of pointers, with a null pointer at the end, as `mpv_command` requires:

```rust
let mut ptrs: Vec<*const c_char> = cstrs.iter().map(|c| c.as_ptr()).collect();
ptrs.push(ptr::null());
let ret = unsafe { mpv_command(self.handle, ptrs.as_ptr()) };
```

Every `CString` must be kept alive until the call returns. The `cstrs` vector does that, because it lives until the end of the function.

### 3.4 Releasing the handle

```rust
impl Drop for Libmpv {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { mpv_destroy(self.handle) };
            self.handle = ptr::null_mut();
        }
    }
}
```

When the `Libmpv` value is dropped, the context is destroyed, which stops playback and frees the memory. The wrapper also has `unsafe impl Send`, so the handle can be moved between threads. The module keeps it inside a `Mutex`, so only one thread uses it at a time.

---

## Step 4: The options of a new player

`create_mpv` builds a fresh handle for each track. It applies the options in `DEFAULT_MPV_OPTIONS`:

```rust
const DEFAULT_MPV_OPTIONS: &[(&str, &str)] = &[
    ("config", "no"),
    ("force-window", "no"),
    ("video", "no"),
    ("audio-display", "no"),
    ("gapless-audio", "yes"),
    ("replaygain", "album"),
    ("audio-device", "auto"),
    ("audio-exclusive", "no"),
    ("ao", "wasapi"),
    ("keep-open", "yes"),
];
```

What they do, briefly:

- `config=no` ignores any mpv configuration file on the machine, so the player behaves the same everywhere.
- `video=no` and `audio-display=no` turn off video and cover art. The player is audio-only.
- `gapless-audio=yes` and `replaygain=album` set playback behaviour.
- `ao=wasapi` sends audio to Windows' WASAPI output. `audio-device=auto` and `audio-exclusive=no` leave the device choice to the system.
- `keep-open=yes` keeps the handle at the last frame when a track ends, instead of unloading it. The player reads `eof-reached` after this, so the flag stays readable.
- `force-window=no` asks mpv not to open a video window. It is kept from the earlier list. Video is already off, so it has no practical effect here.

After the options, `create_mpv` sets `log-file` to `qbc-music-mpv.log` in the system temp folder. The name is an old one from the project's previous name, and the file is for debugging only.

---

## Step 5: Starting a track

`play_index_track_at` starts one track, and it always starts with a clean slate:

1. `stop_mpv()` sends `stop` to the old handle and drops it.
2. `create_mpv()` creates a new handle with the options above. If this fails, the state becomes `error`, and the error text is kept in `last_error`.
3. The volume is set (`volume`). `end` is set to `no`, so libmpv does not cut the track at an estimated end.
4. If the track should resume at a position, `start` is set to that number of seconds. The position is used only when it is inside the track.
5. `loadfile <path> replace` loads the file. If this fails, the error message names the file and the log path.
6. The volume is set once more after loading, and the shared state is updated.

Each track gets a new handle, not a reused one. The cost is a short setup for every track. The gain is that no state from the previous track (its position, end, or error) can leak into the next one.

---

## Step 6: Pause, seek, volume, and status

These operations never restart the process. The old design killed and respawned `mpv.exe` for a pause, and its seek and volume were approximations. The current code uses properties and commands on the live handle:

| Operation | libmpv call | Source |
| :--- | :--- | :--- |
| Pause | `set_property_flag("pause", true)` (in `pause_mpv`) | `player.rs` |
| Resume | `set_property_flag("pause", false)` (in `resume_mpv`) | `player.rs` |
| Seek | `command(["seek", <seconds>, "absolute"])` | `player.rs`, `seek` |
| Volume | `set_property_double("volume", v)` | `player.rs`, `set_volume` |
| Position and duration | `get_property_double("time-pos")` and `("duration")` on each `status()` call | `player.rs`, `status` |

`set_pause(false)` has one special case. If the handle is missing, idle, or at the end of the file, the player does not just unpause. It reads the track's saved position and starts the track again from there with `play_index_track_at`. The position is used only if it is inside the track; otherwise playback starts from zero.

`status()` reads `time-pos` and `duration` each time it is called. If libmpv gives no position, the player falls back to its own clock, which counts elapsed time while playing.

---

## Step 7: Threads and how the end of a track is found

### 7.1 Who touches the handle

The handle is inside `PlayerInner`, behind a `Mutex`. Three kinds of thread take that lock:

1. **Command threads.** Each call from the UI (play, pause, seek, volume, next) runs on a thread of the app's runtime. It takes the lock, calls libmpv, and releases the lock.
2. **The end-of-track monitor.** A background thread, `player-eof-monitor`, runs for as long as the player exists (Step 7.2).
3. **libmpv's own threads.** libmpv starts threads for decoding and audio output. The app does not manage them and does not see their events.

### 7.2 The end-of-track monitor

`PlayerManager::new` starts the monitor:

```rust
let _ = std::thread::Builder::new()
    .name("player-eof-monitor".to_string())
    .spawn(move || {
        while Arc::strong_count(&pm.inner) > 1 {
            std::thread::sleep(Duration::from_millis(500));
            pm.maybe_auto_advance_eof();
        }
    });
```

The loop ends when every other `PlayerManager` clone is gone, which is when the count of the shared `inner` drops to one. The loop runs every 500 milliseconds, and `maybe_auto_advance_eof` also rate-limits itself to one probe per 500 milliseconds.

Each probe:

1. Does nothing if no track is loaded, or the player is paused or stopped.
2. Reads the `eof-reached` flag from libmpv.
3. If the flag is set, it moves on. With AI DJ on, the mode decides the next track: the AI DJ picks one (Smart), a random track (Random), or the next in the queue (Sequential). With AI DJ off, it advances to the next queue entry. At the end of the queue it sends `stop`, sets `running` to false and `paused` to true, sets the position to the duration, sets the status to `stopped`, and clears the current track.

`check_track_finished` makes the same check on demand, for callers that ask for it.

### 7.3 What this means

The app does not receive an end-of-track event from libmpv. It learns about the end by asking. The delay is at most about half a second, which is the monitor's interval. The design is simple, and it is the reason the FFI needs only eight functions. The cost is a small delay, and a poll that runs even when nothing has changed.

---

## Step 8: Known risks

- If `libmpv-2.dll` is missing next to the executable, the process cannot start, or the first playback fails with an error.
- A test in `ffi/libmpv.rs` panics with `libmpv-2.dll missing?` when the DLL cannot be created. The test is written to fail loudly in that case.
- The end-of-track check depends on `eof-reached`, which is kept readable by `keep-open=yes`. Removing that option would break auto-advance.
- Each track creates a new libmpv context. The setup cost is paid on every track change.

---

## Step 9: What is left over from the old design

- `mpv_path` is in `library/config.rs`, with the default `mpv.exe`. The config can read and write the field. No other code in the module reads it, so the playback path does not depend on `mpv.exe`.
- The name `qbc-music-mpv.log` is an old project name.
- The `with_pipe_name` constructor still exists. It ignores its argument, because libmpv runs in-process and no IPC pipe is needed.

---

## Differences from the original task note

| Task note said | What the code does |
| :--- | :--- |
| Use the `mpv` crate (version 0.2.3). | The crate is not used. The code declares the C functions itself, with `raw-dylib`. |
| Link through `rustc-link-search` and `rustc-link-lib=dylib=mpv`, using `libmpv.dll.a`. | No link directive. `raw-dylib` needs no import library. |
| Copy the DLL with a VBS script or a Tauri resource. | `build.rs` stages `opt/libmpv` next to the built binaries. The installer reads the resource list from `tauri.conf.json`. |
| Kill and respawn for pause; approximate seek and volume. | Pause, seek, and volume are property and command calls on the live handle. |
| Read `eof-reached` through a timer. | Done, with a 500-millisecond monitor thread. |
| Use `--input-ipc-server`. | Not used. The pipe name argument is ignored. |

---

## Summary

- The DLL is called through `raw-dylib`, which needs no import library. Eight functions are declared.
- A `Libmpv` value owns one context, creates it with options, and destroys it on drop.
- Each track gets a fresh context. Pause, seek, and volume are changed on the live context.
- The app learns that a track ended by checking the `eof-reached` flag every 500 milliseconds. It does not receive libmpv events.
- The old `mpv.exe` design and its settings are gone from the playback path, except for an unused `mpv_path` field in the config.
