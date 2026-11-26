# Ideas and Technical Debt

This is a working list, not a rule book: things that may be built, and known debt to pay. When an idea is decided, its rule goes to [the reference](../reference/README.md) and it leaves this page.

## Built-in AI

- **Option A (first iteration):** the AI edits only modules, configs, styles, tokens, and logs — safe, no rebuild. NOT the core.
- **Option B (later):** the AI may edit the Rust core and run a rebuild — a full developer agent (separate, not now).
- **MCP contract:** the same API as CLI/modules.
- **AI center (business):** an open-source product + its own AI knowledge base (analogue of opencode/hermes).

## Business model (future)

1. **Hand-made setup** — a paid service: configuring the shell for a specific user (modules, styles, animations, AI).
2. **Own AI center** — an analogue of opencode/hermes: open source + own AI knowledge base/subscription; the built-in AI connects to it.
3. **Module ecosystem** — a module marketplace (later).

## Ideas (future modules)

- **Terminal** — an embedded terminal as a module.
- **Taskbar** — as a module.
- **Start menu** — as a module.
- **Multi-messenger** — the Telegram widget may later support WhatsApp, Discord, Slack too. One messenger module with different backends (the architecture allows it: TDLib → common adapter).

## Tech debt

- Dead Telegram code: `activeTopicThreadId`, the `data-topic-id` branch, `hasBackend`.
- Repo hygiene: `Cargo.lock`, `Server - SSH.cmd` (uncommitted), `.obsidian/` (decide: commit or gitignore).
- `.tmp_analyze.py`, `.tmp_split.py` — removed (leftovers from a worker).
- The player sync does not track child `yt-dlp` processes: on exit an active download is left orphaned (not a regression; at the next change, `shutdown` should get a registry of `Child` processes and kill them).
- **The native shell names modules.** Move the preset sizes keyed by module (`PRESET_GEOMETRY` in `theme.rs`, `sizes_json` in `theme_record.rs`) and the `size_managed` skip list into each module's metadata. Read `mgLayout` and `avatarColor` from a map of module `lookOptions` with per-preset defaults. Build `ROLL_ROLES` from the union of the modules' `tints[].role`. Use metadata for whether Explorer appears in the desk catalog. Move the app-name icon table `shortcut_glyphs.rs` into data.
- **WebView2 VPN settings.** Restore the confirmed home server switch value when the settings command reports a write failure.
- **Long Telegram commands on the UI door.** `telegram_chats` (10 s chat load), `telegram_get_chat_pinned_message` (15 s), the message reads and the send/download/account commands wait on TDLib for seconds. When Telegram's panel becomes native, declare them `long` ([ipc.long-commands-off-thread](../reference/ipc.md#r-ipc.long-commands-off-thread)) and give the panel matching deadlines, or one slow chat load will hold the host's other commands again. One-of-a-name-at-a-time then needs a look: two message reads for two chats must not refuse each other.
- **The settings contract, next steps** ([modules.settings-contract](../reference/modules.md#r-modules.settings-contract)): mp3-player's `config.toml`; Telegram's configuration. An open settings pane does not yet hear `settings-changed`: a change made elsewhere (the panel's "АВТО") shows when the pane is opened again, and a write from the old value is refused and the pane reads again.
- **Found by the session 152 critics, not that plan's to fix:**
  - **The interface door is open to any local process while CDP is on** ([ipc.cdp-always-on](../reference/ipc.md#r-ipc.cdp-always-on)): `Runtime.evaluate` on the page can call any Tauri command, `settings` and writes included, and read the WebSocket token. The sentence in [ipc.socket-read-only](../reference/ipc.md#r-ipc.socket-read-only) that only the page holds the interface door holds only with CDP off.
  - **Telegram's configuration is shadowed.** `telegram_save_config` writes `telegram_config.json`, while `load_config` reads the redb key `config` first and falls back to the file; once the key exists, a save is not what the next load returns (read from the code, not run).
  - **mp3-player keeps its configuration in `config.toml`,** written with `std::fs`, beside [storage.no-raw-db](../reference/storage.md#r-storage.no-raw-db)'s "declared through `StoreSpec`". The settings contract (session 152 plan) is its next home.
  - **libmpv is linked `raw-dylib`.** Whether a missing `libmpv-2.dll` stops the kernel from loading at all, rather than leaving the player in an error state, is unchecked (the import table of the built binary).
  - **The visualizer's capture thread wakes about 150 times a second** (`audio.rs`, `TICK / 5` = 6.6 ms) whenever the visualizer is on, playing or not. Event-driven WASAPI capture (`AUDCLNT_STREAMFLAGS_EVENTCALLBACK`) would wake it only for data.
