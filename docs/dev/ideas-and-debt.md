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
