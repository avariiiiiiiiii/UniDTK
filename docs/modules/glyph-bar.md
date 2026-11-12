# The Glyph Bar Module: From the Windows Volume to the LED Strip

This chapter explains the `glyph-bar` module step by step. It is a vertical strip of ten LEDs that shows the Windows master volume and changes it when clicked, dragged or scrolled. A background thread reads the default output endpoint through WASAPI and publishes each change. The page turns that event into lit segments, and a gesture travels back to the system through the kernel.

Every mechanism here is taken from the code. Line numbers refer to the files as they are in this repository.

## What you will understand by the end

- How the module starts, and what its enabled and background states mean for the poll thread.
- Why the poll thread reopens the default device every two seconds, and why it emits only on change.
- How one reading travels from `unidtk_api::emit` to the WebView and to the socket door.
- How a level from 0 to 100 becomes a count of lit LEDs.
- How a gesture becomes a `set_system_volume` or `set_system_mute` call, and why the module keeps no state of its own.

## Files involved

| File | Role |
| :--- | :--- |
| [modules/glyph-bar/rust/glyph_bar.rs](../../modules/glyph-bar/rust/glyph_bar.rs) | The WASAPI poll thread, the reads and writes of the master volume, the `Module` implementation, and the socket routes (333 lines). |
| [modules/glyph-bar/rust/Cargo.toml](../../modules/glyph-bar/rust/Cargo.toml) | The crate. It depends on `unidtk-api` and the `windows` crate, and on no other module. |
| [modules/glyph-bar/ui/widget.js](../../modules/glyph-bar/ui/widget.js) | The LED strip: layout, drawing, click, drag, wheel, and the event listener (266 lines). |
| [modules/glyph-bar/ui/glyph-bar.css](../../modules/glyph-bar/ui/glyph-bar.css) | LED styles, and the Jelly material, which draws the level as one fill (176 lines). |
| [modules/glyph-bar/ui/metadata.json](../../modules/glyph-bar/ui/metadata.json) | Declares `specialType: "glyph-bar"`, the tint slot, and `backend: true`. |
| [core/api/src/lib.rs](../../core/api/src/lib.rs) | The contract: `emit`, `subscribe`, `state`. |
| [core/kernel/main.rs](../../core/kernel/main.rs) | Starts the module through the registry, serves its commands from the Tauri door, and sets the emitter that sends events to the page. |
| [core/kernel/events.rs](../../core/kernel/events.rs) | The event broker that serves the socket door and keeps the newest `system-volume` value. |
| [core/kernel/ipc.rs](../../core/kernel/ipc.rs) | Routes the socket commands to the module. |
| [core/ui/unidtk-bridge.js](../../core/ui/unidtk-bridge.js) | Chooses, per call, between the Tauri door and the socket door. |

The module is a vertical slice, so its backend and its page share one folder ([modules.vertical-slice](../reference/modules.md#r-modules.vertical-slice)). Its crate depends on `unidtk-api` and not on the kernel ([modules.contract-only](../reference/modules.md#r-modules.contract-only)). It never calls another module; it talks only through the contract's events ([modules.no-module-to-module](../reference/modules.md#r-modules.no-module-to-module)).

---

## Step 1: Startup and the module states

The kernel creates the module in its setup code (`core/kernel/main.rs`, lines 2460-2462), calling `init` with a context whose enabled flag comes from the stored list of disabled modules. `init` decides what runs, in this order (`glyph_bar.rs`, lines 313-325):

1. In a diagnostic run it calls `get_system_volume()` once to test WASAPI, and starts no thread.
2. If the module is disabled, it logs `glyph-bar: disabled — volume poll not started` and returns.
3. Otherwise it stores the handle from `start()` in its `state` field.

`start()` spawns one thread named `system-volume`. The `VolumeState` it returns holds the stop flag and the join handle.

The metadata sets `backend: true`, so the card can be hidden while the backend keeps running ([modules.states](../reference/modules.md#r-modules.states)). Switching the module on or off goes through the kernel's module registry, under [modules.lifecycle](../reference/modules.md#r-modules.lifecycle).

---

## Step 2: The poll thread

`poll_loop` runs on the `system-volume` thread. It is compiled for Windows only; elsewhere it is empty.

### 2.1 COM and the default endpoint

The thread enters COM through `ComSession::enter`, which tolerates `RPC_E_CHANGED_MODE`. Then `default_endpoint()` does this:

```rust
// Filename: modules/glyph-bar/rust/glyph_bar.rs (lines 218-226, abridged)
let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
device.Activate(CLSCTX_ALL, None)   // IAudioEndpointVolume
```

`eRender` is the output side, and `eConsole` is the role for ordinary sound, so the result is the device the user hears by default. `IAudioEndpointVolume` exposes the master level and the mute flag.

### 2.2 The loop

```rust
// Filename: modules/glyph-bar/rust/glyph_bar.rs (lines 86-113, abridged)
while !stop.load(Ordering::Relaxed) {
    if endpoint.is_none() || polls % DEVICE_RECHECK_POLLS == 0 {
        endpoint = match default_endpoint() { /* ... */ };
    }
    polls = polls.wrapping_add(1);
    let reading = endpoint.as_ref().map(read_endpoint).expect("endpoint opened above");
    match reading {
        Ok(current) => {
            if last != Some(current) {
                last = Some(current);
                unidtk_api::emit("system-volume", serde_json::json!(current));
            }
        }
        Err(error) => { /* log, drop the endpoint, sleep 1 s */ }
    }
    thread::sleep(POLL);
}
```

- `POLL` is 120 ms and `DEVICE_RECHECK_POLLS` is 16, so the endpoint is looked up again about every two seconds. That is how a change of output device is picked up. Between lookups, one endpoint serves every poll.
- `read_endpoint` reads `GetMasterVolumeLevelScalar` and `GetMute`. The scalar is multiplied by 100, rounded and clamped to 0..100, giving `SystemVolume { level, muted }`.
- An event is emitted only when the reading differs from the last one, so a steady volume produces no traffic.

Warning: the thread runs for as long as the module is enabled, whether or not the widget is on the desk. That is one COM read every 120 ms.

---

## Step 3: From the reading to the page

`unidtk_api::emit` stores the payload as the newest value for the event, runs any Rust subscribers, and then calls the emitter that the kernel installed ([core/api/src/lib.rs](../../core/api/src/lib.rs), lines 202-223). The kernel sets the emitter once, at startup:

```rust
// Filename: core/kernel/main.rs (lines 2361-2364)
unidtk_api::set_emitter(move |event, payload| {
    let _ = handle.emit(event, payload.clone());
    events::publish(event, payload);
});
```

So each reading goes to two places:

1. **The Tauri door.** `handle.emit` sends the event to the WebView. This is the path the page uses by default.
2. **The event broker.** `events::publish` in [core/kernel/events.rs](../../core/kernel/events.rs) numbers the frame. `system-volume` is in `STATE_EVENTS`, so the broker keeps only the newest frame. A socket client that subscribes is sent that frame at once, and a short reconnect does not replay old samples.

No Rust module subscribes to `system-volume`; the micrographics page subscribes in JavaScript.

---

## Step 4: Reading the event in the page

The listener is at the bottom of `widget.js`:

```js
// Filename: modules/glyph-bar/ui/widget.js (lines 258-264)
listen("system-volume", (event) => {
  const v = event.payload;
  if (!v || typeof v.level !== "number") return;
  systemVolumeState = { level: v.level, muted: !!v.muted };
  systemVolumeReady = true;
  applyGlyphBarVisualAll();
}).catch(() => {});
```

`listen` comes from [core/ui/unidtk-bridge.js](../../core/ui/unidtk-bridge.js). The bridge's transport flag is read from the URL parameter `transport` or from `sessionStorage`, and defaults to `tauri`. Only in the `ws` or `shadow` transport does `system-volume` arrive over the socket. The state is one object, `systemVolumeState`, and the listener replaces it and redraws every strip on the page.

When a strip is first drawn and no reading has arrived, `renderGlyphBar` calls `refreshSystemVolume()`, which runs `invoke("get_system_volume")`. That read uses the Tauri door, because `get_system_volume` is not in the bridge's `WS_COMMANDS` list.

---

## Step 5: From a level to lit LEDs

`applyGlyphBarVisual` turns the state into classes:

```js
// Filename: modules/glyph-bar/ui/widget.js (lines 146-161, abridged)
const lit = glyphBarLitCount(level, muted);
card.style.setProperty("--gb-lit", String(muted ? 0 : lit));
segs.forEach((seg) => {
  const i = Number(seg.dataset.gbIndex);
  seg.classList.toggle("on", !muted && i < lit);
  seg.classList.toggle("muted-only", muted && i === 0);
});
```

`glyphBarLitCount` is the whole mapping. Muted, or a level of 0, gives 0 lit LEDs. Any other level gives `round(level / 10)`, clamped to 1..10, so a level of 4 lights one LED rather than none.

Segment 0 is the bottom one, because the container is a `column-reverse` flex column. It is the red mute segment. It lights as an ordinary LED when the level is above zero, and shows alone, as `muted-only`, when the output is muted.

`layoutGlyphBarSquares` sets the geometry. The LEDs are square: the side is the card width, clamped to 6..64, and the height is always `10 × side + 3 × 9`. The default side of 10 px gives a height of 127 px. The theme and the menu call the layout again through `card._glyphBarGeometry.layout()` (`core/ui/theme.js`, `core/ui/menu.js`). On the Jelly material, `glyph-bar.css` draws the level as one fill whose height comes from `--gb-lit`.

---

## Step 6: From a gesture to the Windows volume

All three gestures update the page at once, then call the module.

### 6.1 Click

```js
// Filename: modules/glyph-bar/ui/widget.js (lines 197-203, abridged)
// 2nd from bottom → 20%, …, 9th → 90%, 10th (top) → 100%.
const target = glyphBarVolumeForIndex(index);
systemVolumeState = { level: target, muted: false };
applyGlyphBarVisualAll();
const v = await invoke("set_system_volume", { level: target });
if (v) systemVolumeState = { level: v.level, muted: !!v.muted };
```

Clicking segment 0 instead toggles the mute with `set_system_mute`.

### 6.2 Drag

`pointerdown` starts a drag. The move handler maps the pointer's Y position to a segment and calls `dragGlyphBarTo` when the segment changes. The listeners sit on `window`, because pointer capture does not follow forwarded desktop input. Each call updates the state at once and queues the write on `glyphDragChain`; a queued write is skipped if a newer level has replaced it.

### 6.3 Wheel

`nudgeSystemVolume(±10)` runs on wheel input. If the output is muted, an upward turn unmutes and sets the level to 10, and a downward turn does nothing. Otherwise the level moves by 10 and is clamped to 0..100. A result of 0 calls `set_system_mute(true)` instead.

### 6.4 The Rust side of a write

The page's `invoke` reaches the module's command table, whose body calls a plain function in the module:

```rust
// Filename: modules/glyph-bar/rust/glyph_bar.rs (lines 130-146, abridged)
pub fn set_system_volume(level: u8) -> Result<SystemVolume, String> {
    let clamped = level.min(100);
    with_endpoint(|endpoint| unsafe {
        endpoint.SetMasterVolumeLevelScalar(clamped as f32 / 100.0, std::ptr::null())...;
        if clamped > 0 {
            endpoint.SetMute(false, std::ptr::null())...;
        }
        Ok(())
    })?;
    read_volume()
}
```

- `with_endpoint` opens COM and the default endpoint for each call. Writes are rare, so that cost is acceptable.
- A level above zero also clears the mute. A level of zero leaves the mute flag alone.
- The function returns a fresh `read_volume()`, so the page adopts the value the system reports, not the value it asked for.

The module declares the three commands in its table (`unidtk_api::module_commands!`, glyph_bar.rs), under the names the page calls. `set_system_volume` reads its `level` as a `u64` and clamps it with `volume_level` before it narrows to `u8`, so 300 sets 100, not 44.

---

## Step 7: The socket door

The same table serves every door. The socket and the WebSocket reach it through `serve_module` in [core/kernel/ipc.rs](../../core/kernel/ipc.rs), and Tauri through the kernel's `invoke_handler`. The table marks `get_system_volume` as `read_only`, served on the socket too, and the other two as writes, which the socket refuses. In the bridge, a write goes over the socket only in the `ws` transport; otherwise it goes to Tauri.

---

## Step 8: What it stores

The module stores nothing. Its code never uses `ModuleContext::store` and never opens a database ([storage.no-raw-db](../reference/storage.md#r-storage.no-raw-db)). The volume belongs to Windows, and the module keeps only the last reading, in memory, inside the values it emits.

The page stores one thing: the strip's size in the desk configuration. `renderGlyphBar` reads `config.positions["glyph-bar"]`. A stored side outside 6..64 is replaced by the default of 10, and `saveConfig()` runs only when the stored `w` and `h` differ from the computed ones ([desk.positions](../reference/desk.md#r-desk.positions)).

---

## Step 9: How it fails

- **COM cannot start.** `ComSession::enter` fails, the thread logs `glyph-bar: CoInitializeEx failed` and exits. No event is ever emitted, the page keeps level 0, and the strip is empty. The log is the only signal.
- **No default endpoint.** Each failed lookup logs `glyph-bar: volume poll error` and sleeps one second, so the log gains one line per second while the failure lasts.
- **A read fails.** The endpoint is dropped, the error is logged, and the next pass looks up the endpoint again after a one-second sleep.
- **A write fails.** The page shows a debug message (`volume: …` or `mute: …`). It does not restore its earlier state; see Known gaps.

---

## Known gaps

- **Click and drag disagree on the bottom segment.** Clicking segment 0 toggles the mute. Dragging to it calls `dragGlyphBarTo(0)`, which sets the level to `(0 + 1) × 10 = 10`, unmuted (widget.js, lines 220-222). The comment at line 13 says index 0 is "mute, not a level". A click can never set 10%, because segment 1 sets 20%.
- **Failed writes are not rolled back.** The click, drag and wheel handlers set the new state before the call, and their `catch` blocks only show a debug message. The strip keeps the optimistic value until the next `system-volume` event. If the system did not change, the strip stays wrong.
- **Shutdown ignores its budget and has no timeout.** `shutdown(_budget)` ignores the budget it is given, and `VolumeState::stop` joins the thread without a limit (glyph_bar.rs, lines 36-42 and 327-332). The thread may be asleep for up to a second in its error path.
- **Listener errors are swallowed.** `listen(...).catch(() => {})` (widget.js, line 264) hides a failed registration. The strip then receives no events and shows only the state from its one `get_system_volume` read.
- **The LED colours are hardcoded.** `glyph-bar.css` writes `rgba(255, 72, 58, 1)` for the lit mute LED (lines 81 and 88) and `rgba(255, 255, 255, …)` for the glow (lines 70-72), and uses the keyword `ease` in the segment transitions (lines 55-59). These do not come from the theme tokens ([theme.tokens-only](../reference/desk.md#r-theme.tokens-only)).

