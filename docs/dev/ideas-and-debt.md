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
- **The native shell names modules** (the user, 2026-11-17: "the menu's settings are hard-coded per module, remove that"; audited in session 143). Against `r[shell.widget-contract]`, the shell (`core/desktop`) still knows modules by id; the page does the same, and the port copied it. To move to the modules: (1) the VPN pane (`core/desktop/menu_vpn.rs`, `View::Vpn`, the `settingsView == "vpn"` branch in `menu_desk.rs`, five `vpn_*` kernel calls) into `modules/vpn/native/` through a settings hook on `Widget` (a pane the module draws, opened as `View::ModuleSettings(id)`), with the menu's controls exposed to native crates; (2) the preset sizes keyed by module (`PRESET_GEOMETRY` in `theme.rs`, `sizes_json` in `theme_record.rs`) into a `presetSize` in each module's `metadata.json`, and the `size_managed` skip list (`micrographics`, `vpn` and the dead ids `glyph`, `audio-circle`) into `"presetSize": false`; (3) `mgLayout` and `avatarColor` from fixed theme fields to a generic map of each module's `lookOptions`, with per-preset defaults in the metadata; (4) the colour roles `ROLL_ROLES` from the union of the modules' `tints[].role`; (5) `explorer` hidden from the desk by a metadata field instead of its id in `core/api/src/modules.rs`; (6) the app-name icon table `shortcut_glyphs.rs` (about 50 names, some personal) out of the binary into data. Tests that name modules change with it.
