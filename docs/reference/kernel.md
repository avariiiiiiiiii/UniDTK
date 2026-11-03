# The Kernel

The kernel is the binary `unidtk.exe`, built from `core/kernel`. It is the invisible frame that every visible part runs on. This chapter states what belongs to it and the rules it keeps while starting and running.

## What is the kernel and what is a module

r[kernel.core-invisible]

The kernel is what cannot be switched off without the shell ceasing to be a shell. Ask of any feature: "If I turn this off, does the shell stop being a shell?" If yes, it is the kernel; if no, it is a module.

The kernel owns:

- the frame: windows, overlays, layout, lifecycle, and attaching the window to the desktop (`core/kernel/wallpaper.rs`);
- the widget system: loading, placing, dragging, adding and removing widgets;
- the command inventory and the event bus (`core/kernel/command_inventory.rs`, `core/kernel/events.rs`), and the list of modules (`core/kernel/registry.rs`, which the CLI reads; the kernel names each module type at startup);
- storage: `unidtk.redb` and `unidtk_logs.redb` (see [Storage](storage.md));
- the contract crate `unidtk-api` (`core/api`), which is all a module may depend on;
- the IPC server and the tray.

Everything the user sees as a feature is a module: the explorer, Telegram, VPN, the player, the clock, the glyph bar, micrographics.

Storage and logs once lived inside the music module, so the kernel pulled that module in only to open its own database, and music was the kernel in all but name. Since storage moved into the core, "music is just another module" is literally true.

r[kernel.menu-in-core]

The settings menu is part of the kernel, not a module. It is the control panel: which modules run, how animation behaves, which theme is active. It lives in `core/ui/menu.js`, `core/ui/menu.css`, and `#menuContainer` in `core/ui/index.html`, and it changes modules only through kernel commands.

r[kernel.stable-api]

Modules depend on the stable contract, not on kernel internals, so a kernel update does not break them. Tools that edit the system (the CLI, a future built-in AI) edit modules, configuration, and styles through the same contract, and never the kernel.

## Layers

```text
            UI (WebView2)    udtk CLI    (future) AI bridge
                  │              │              │
                  ▼              ▼              ▼
          ┌─────────────── kernel (unidtk.exe) ───────────────┐
          │  interface door     socket door 127.0.0.1:26540    │
          │  command registry · event bus · redb · window      │
          └───────────────────────┬────────────────────────────┘
                                  │ unidtk-api
          ┌──────────┬────────────┼───────────┬──────────┐
        clock    explorer     telegram       vpn     mp3-player …
```

All clients of the kernel are equal; it does not know who is calling. The two doors are described in [IPC](ipc.md).

## Starting without blocking

r[kernel.no-blocking-main-thread]

Blocking Win32 calls (`SendMessageTimeoutW`, searching for windows) never run on the Tauri main UI thread. Waiting for the desktop (`Progman`/`WorkerW`) to be ready is done on a background worker thread.

r[kernel.main-thread-once]

An attach uses the main UI thread for one short step only, through `run_on_main_thread`: to call `SetParent` and to show the window (`window.show()`). Everything before it (waiting, searching) stays on the worker. A later attach (after Explorer restarts and sends `TaskbarCreated`, or a forced attach from a second launch) follows the same split; a non-forced request is skipped while the window is already attached and visible.

The reason is the message queue. A window's thread must keep pumping messages; a thread blocked inside `SendMessageTimeoutW` stops pumping, and the window freezes for as long as the call waits, which during a cold start can be seconds.

## The repository

r[repo.flat-root]

The project lies directly in the repository root (`Cargo.toml`, `core/`, `modules/`, `opt/`, `toolchain/`, `docs/`, `assets/`, `scheduler-tasks/`, `target/`, `.cargo/`). There is no wrapper folder. All Rust scaffolding (the manifest that builds the binaries, `build.rs`, Tauri configuration) lives in `toolchain/`.
