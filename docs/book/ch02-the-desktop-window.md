# Chapter 2. The desktop window

This chapter explains how the kernel puts its window behind the desktop icons. The window is an ordinary WebView2 window at first, and it has to become a child of a window that Windows keeps for the desktop. After reading it you can follow the attach from the first call in `setup` to the final `ShowWindow`, name the thread that runs each step, and explain what happens when the display changes or the shell restarts.

## The problem

A normal top-level window sits above the desktop, and the desktop icons sit inside the desktop's own window. To make UniDTK look like part of the desktop, its window must become a child of the window that paints the wallpaper, and that window must sit below the icons.

Windows keeps the desktop in a window of class `Progman`. The code sends `Progman` one message, `0x052C`, and Explorer answers by creating a second window of class `WorkerW`. After that, the desktop is split: the icons live in one `WorkerW` with a child window named `SHELLDLL_DefView`, and the wallpaper is drawn by a sibling `WorkerW` next to it. The kernel parents its window to that sibling. The comment in `find_worker_w_internal()` puts it this way: messages `0x052C` with parameters `(0xD, 1)` or `(0xD, 0)` split the desktop into `WorkerW`.

Two facts shape the design. The wait for Explorer can be long, and some of the Win32 calls that wait block the thread that makes them. So the search runs on a worker thread, and only the final reparenting runs on the UI thread. The rule is [kernel.no-blocking-main-thread](../reference/kernel.md#r-kernel.no-blocking-main-thread).

## Step 1: setup asks for the window

The Tauri application starts in `main()` in `core/kernel/main.rs`. Tauri calls the `setup` closure once the event loop exists. Near the end of that closure, the kernel stores the application handle and asks for the window to be shown.

Filename: core/kernel/main.rs (abridged, the end of `setup`)

```rust
let _ = APP_HANDLE.set(app.handle().clone());

request_wallpaper_visible(app.handle());

log_line("setup: done");
```

`request_wallpaper_visible()` is a one-line wrapper around `request_wallpaper_attach(app, false)`. That function is the entry point of the attach. It first takes a flag, `ATTACH_IN_PROGRESS`, with a compare-and-swap, so that two requests cannot run two attach loops at once. A request that finds the flag set returns without doing anything. Otherwise it spawns a thread and returns at once. The `setup` closure therefore does not wait, and the window is shown later.

Note: the `false` argument means "not forced". A forced attach, passed as `true`, runs even when the window is already visible. The second-launch handler in the single-instance plugin uses the forced form.

## Step 2: the worker thread waits for the desktop

The spawned thread runs a loop of up to forty attempts. Each attempt calls `wallpaper::resolve_desktop_host()`, which runs the whole search described in the next step. If the search finds a host, the thread hands the result to the UI thread and stops. If not, it sleeps and tries again.

Filename: core/kernel/main.rs (abridged, the loop in `request_wallpaper_attach`)

```rust
thread::spawn(move || {
    // ...
    for attempt in 0..40 {
        if is_shutting_down() { return; }
        let allow_progman = attempt >= 25;
        if let Ok(host) = wallpaper::resolve_desktop_host(allow_progman) {
            if run_on_main_thread(&handle, move |app| {
                // ... show_and_attach_wallpaper(app, host) ...
            }).is_ok() {
                return;
            }
        }
        let sleep_ms = if attempt < 4 { 500 } else if attempt < 25 { 500 } else { 1000 };
        thread::sleep(Duration::from_millis(sleep_ms));
    }
    log_line("request_wallpaper_visible: desktop host unavailable after 40 attempts");
});
```

The first twenty-five attempts do not accept `Progman` as a host. The comment in the code says this avoids a black screen covering the wallpaper during logon. From attempt twenty-five on, the loop accepts `Progman` as a fallback. The sleep is half a second per attempt up to attempt twenty-five and one second after it, so the fallback begins after about twelve and a half seconds, not the fifteen the comment states. The first four attempts sleep the same half second, which the code spells out separately. The loop is bounded at forty attempts either way.

Note: the thread is a plain `thread::spawn`, not a Tauri task. It only sends work to the UI thread and waits for the answer, so it does not need the event loop.

## Step 3: finding the desktop host

`resolve_desktop_host()` in `core/kernel/wallpaper.rs` calls `find_worker_w_internal(true, allow_progman_fallback)`. The search has four parts, in this order.

First, it finds `Progman`. It asks `FindWindowW` for the class, falls back to `GetShellWindow()`, and finally to a search by title. Then it sends `Progman` the three `0x052C` messages with `SendMessageTimeoutW`, each with a 100 ms limit and the `SMTO_ABORTIFHUNG` flag. The flag stops the call from waiting out the full timeout when Explorer is hung, which matters during logon.

Second, it enumerates the top-level windows with `EnumWindows(enum_window, ...)`. The callback `enum_window()` looks for a window that has a child `SHELLDLL_DefView`, and then takes the next top-level `WorkerW` after it. That sibling must be at least 640 by 480 pixels.

Filename: core/kernel/wallpaper.rs (abridged, the match in `enum_window`)

```rust
let shell_dll_def_view = WindowsAndMessaging::FindWindowExW(
    window, HWND::default(), w!("SHELLDLL_DefView"), None,
).unwrap_or_default();

if !HWND::is_invalid(&shell_dll_def_view) {
    let worker_w = WindowsAndMessaging::FindWindowExW(
        HWND::default(), window, w!("WorkerW"), None,
    ).unwrap_or_default();
    // ... size check ...
    if width >= 640 && height >= 480 {
        *(ref_worker_w.0 as *mut HWND) = worker_w;
        return BOOL(0);
    }
}
```

Third, if no sibling matches, it scans every top-level `WorkerW` that does not contain `SHELLDLL_DefView`, again with the size test. Fourth, if `allow_progman_fallback` is set and `Progman` itself contains `SHELLDLL_DefView`, it returns `Progman`. If none of these succeeds, the function returns `Err("desktop host not ready yet")`, and the worker sleeps.

Note: the function is called with `verbose` set to `true` from the worker, so each step writes a line to the log. When the desktop is not ready, that log is the first place to look.

## Step 4: the hand-off to the UI thread

When the search succeeds, the worker needs the UI thread for the window operations. The helper `run_on_main_thread()` does that hand-off.

Filename: core/kernel/main.rs (abridged, `run_on_main_thread`)

```rust
fn run_on_main_thread<F>(app: &tauri::AppHandle, task: F) -> Result<(), String>
where
    F: FnOnce(tauri::AppHandle) -> Result<(), String> + Send + 'static,
{
    let app_for_task = app.clone();
    let (tx, rx) = mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(task(app_for_task));
    })
    .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_millis(3000)).map_err(|e| e.to_string())?
}
```

Tauri queues the closure onto the thread that runs the event loop, and the closure runs there. The worker blocks on a channel for at most three seconds. The UI thread's answer is the `Result` of the task, so an error from the attach comes back to the worker, which then tries again on its next attempt.

The reason for this shape is the Win32 message queue. A window belongs to the thread that created it, and that thread must keep pumping messages. If the UI thread were blocked inside `SendMessageTimeoutW`, the window would stop responding for as long as the call waited. This is the reason the rule [kernel.main-thread-once](../reference/kernel.md#r-kernel.main-thread-once) says the UI thread is used exactly once per attach.

Warning: the three-second limit is a guard, not a promise. If the UI thread is busy for longer, `recv_timeout` fails, the worker reports an error, and the closure may still run later. The attach is written to be safe to repeat for that reason: a second attach that finds the window already visible does nothing.

## Step 5: attaching on the UI thread

The closure passed to `run_on_main_thread` is `show_and_attach_wallpaper()`. It keeps the window hidden until the attach has succeeded, so the user never sees the window in the wrong place.

Filename: core/kernel/main.rs (abridged, `show_and_attach_wallpaper`)

```rust
fn show_and_attach_wallpaper(app: tauri::AppHandle, host: (isize, u32)) -> Result<(), String> {
    let window = app.get_webview_window("wallpaper")...;
    let _ = window.set_decorations(false);
    let _ = window.set_shadow(false);
    let _ = window.set_skip_taskbar(true);
    let _ = window.hide();

    let hwnd = wallpaper_hwnd(&app)?;
    if let Err(error) = wallpaper::attach_to_host(hwnd, ATTACH_OPTS, host) {
        return Err(error);            // the window stays hidden
    }
    window.show().map_err(|e| format!("show wallpaper window: {e}"))?;
    Ok(())
}
```

The window is declared in `toolchain/tauri.conf.json` with the label `wallpaper`, and it starts with `"visible": false`. Its decorations are switched off here, in code, so the window has no frame when it becomes a child.

The work itself is `attach_to_host()` in `core/kernel/wallpaper.rs`. Its steps, in order, are these. It checks that the window that owns the host is still the same process as the one the worker found, using the process id saved in step 3. It saves the original wallpaper path once, so that it can be restored on exit. It checks that the host's class is `WorkerW` or `Progman`, and that its rectangle is still at least 640 by 480. Then it reparents the window.

Filename: core/kernel/wallpaper.rs (abridged, `attach_to_host`)

```rust
WindowsAndMessaging::SetParent(hwnd, host_w).map_err(|e| e.to_string())?;
make_child_window(hwnd);            // strips popup/caption styles, adds WS_CHILD | WS_VISIBLE

let primary = primary_monitor_rect()...;
let child_x = primary.left - ww_rect.left;
let child_y = primary.top - ww_rect.top;
let child_w = primary.right - primary.left;
let child_h = primary.bottom - primary.top;
WindowsAndMessaging::SetWindowPos(hwnd, None, child_x, child_y, child_w, child_h, ...);
```

`SetParent` is the call that makes the window a child of the host. A child's coordinates are relative to its parent, so the code subtracts the host's origin from the primary monitor's origin. The primary monitor is read with `MonitorFromPoint` at the point (0, 0), with physical pixels, so the size matches the host's rectangle under per-monitor DPI.

Then the code does something that looks odd. It sets the window one pixel shorter, and then back to full height. The comment says this guarantees two size messages, so that the WebView2 composition surface is refitted to the full client area. Without it, the surface can stay offset by the old frame and leave a strip at the top edge. The same trick is used in `update_geometry()`.

Note: the final `SetWindowPos` passes `HWND_TOP`, but on the `WorkerW` path it also passes `SWP_NOZORDER`, which tells Windows to ignore that argument, so the window keeps its place in the stacking order. The `Progman` fallback omits `SWP_NOZORDER`, so the window is raised to the top. The comment explains why: `Progman` also owns the icons, and `HWND_BOTTOM` would put the application behind its painted surface.

## Step 6: input forwarding

At the end of `attach_to_host()`, the function calls `start_input_forwarding(hwnd, ...)` in `core/kernel/input.rs`. The comment in `main.rs` above `ATTACH_OPTS` says that the reparented child never receives real wheel input, and that Chromium ignores a posted `WM_MOUSEWHEEL`, so scrolling is delivered another way.

To fix this, the kernel creates a hidden window of its own, the raw input window, and registers it for raw mouse and touchpad input with the `RIDEV_INPUTSINK` flag. That flag lets the window receive input even when it is not the foreground window. The raw input window is created on the thread that calls `start_input_forwarding()`, which is the UI thread, because `attach_to_host()` runs there. Wheel events come from a low-level hook, `WH_MOUSE_LL`, installed on a separate thread that owns its own message loop.

Note: `ATTACH_OPTS` in `core/kernel/main.rs` sets `forward_mouse` and `forward_keyboard` to `true`. Keyboard forwarding is what lets a text field in a widget accept typing while the desktop is in front.

## Step 7: display changes and shell restarts

Monitors come and go, resolutions change, and the DPI can change. The raw input window also receives the messages Windows sends for those events. In the window procedure in `core/kernel/input.rs`, three messages lead to the same action.

Filename: core/kernel/input.rs (abridged, the change check in the window procedure)

```rust
if msg == WindowsAndMessaging::WM_DISPLAYCHANGE
    || msg == WindowsAndMessaging::WM_SETTINGCHANGE
    || msg == WindowsAndMessaging::WM_DPICHANGED
{
    crate::trigger_display_change();
}
```

`trigger_display_change()` in `core/kernel/main.rs` starts a short debounce. It takes a sequence number, sleeps for 150 ms, and continues only if no newer change has arrived in the meantime. Then it calls `wallpaper::update_geometry(hwnd)` and emits the event `display-changed` through the event contract, which the page and the modules can subscribe to.

`update_geometry()` deliberately does not search for the desktop. Its comment says that geometry follows the host the window is already attached to, because a search would send blocking messages to Explorer on every resize. It reads the parent with `GetParent()`, computes the primary monitor's rectangle again, and applies the same one-pixel refit as the attach.

A shell restart is a separate case. When Explorer restarts, it broadcasts the registered message `TaskbarCreated`. The same window procedure catches it and calls `request_wallpaper_visible()` again, so the whole attach runs once more with the new desktop. The `refresh_display_layout` command does the same geometry update when the page asks for it.

Note: the window procedure runs on the thread that owns the raw input window, which is the UI thread. The debounce keeps that work off the message loop: the procedure only spawns a thread and returns.

## Step 8: leaving the desktop

When the application quits, the `quit_app` command runs its cleanup on the UI thread. It calls `wallpaper::detach()`, which calls `SetParent(hwnd, None)` and stops input forwarding. Then it calls `wallpaper::reset()`, which restores the wallpaper the user had before UniDTK started.

Filename: core/kernel/wallpaper.rs (abridged, `reset`)

```rust
pub fn reset() -> Result<(), String> {
    let original = ORIGINAL_WALLPAPER.lock().unwrap().clone();
    if let Some(path) = original {
        if !path.is_empty() {
            return set_wallpaper(&path);
        }
    }
    Ok(())
}
```

The original path is saved once, by `save_original_wallpaper()`, the first time the window is attached. `set_wallpaper()` sends the wallpaper change with `SPIF_SENDCHANGE` only when the path differs from the current one. That broadcast makes Explorer repaint the desktop, which briefly hides the icons, so the code avoids it when nothing has changed.

Note: after `detach()`, the window has no parent and is no longer part of the desktop. Only then does the application exit, through `app_shutdown()`.

## Summary

The kernel parents its window to the wallpaper `WorkerW` that Explorer creates when it receives the `0x052C` messages. The search for that window runs on a background thread, with a bounded number of attempts. The reparenting, the sizing, and the show run once on the UI thread, which Tauri provides through `run_on_main_thread`, and the window stays hidden until they succeed. Input comes from raw input registrations on the same thread. Display changes and shell restarts run the same geometry and attach code again. The next chapter explains how a command gets from the page, or from the CLI, into the functions that do the work: [Chapter 3, The kernel and its commands](ch03-the-kernel-and-its-commands.md).
