# Chapter 18. Known gaps and open decisions

Each item was checked against the code on 2026-11-02. Items marked *not re-verified* are taken from the sessions and were not re-read for this rewrite.

**Migration of the page (see [H4](../history/rfcs/udtk-migration.md#h4-migration-of-the-page-to-the-new-door-former-1023-to-1035)).**

1. Four writes are still served only by Tauri: `set_module_enabled`, `ai_dj_model_download`, `telegram_set_archived`, and `explorer_sort_set`. They are not in `WS_WRITES`.
2. Read commands that are not in `WS_COMMANDS`: `get_system_volume`, `get_disabled_modules`, `telegram_chats_archive`, `telegram_messages_around`, `explorer_sort_get`, and the kernel's own readers (`log_stats`, `log_max_id`, `log_delta`, `logs`). The volume reaches the page as an event, not as a call.
3. Request-id deduplication for writes, and a refusal of automatic retries of writes, are part of the plan. They are **not** implemented in [ipc.rs](../../core/kernel/ipc.rs) or [ws.rs](../../core/kernel/ws.rs). Until they are, a write that times out on the client may still run on the server.
4. The default transport is still `tauri`. `shadow` and `ws` are opt-in, through `sessionStorage`.
5. The contract's emitter is the only path to Tauri for events in the kernel's own code. A search of [core/kernel/input.rs](../../core/kernel/input.rs) and [core/kernel/main.rs](../../core/kernel/main.rs) finds no other direct `emit`. Modules were not searched again for this rewrite (*not re-verified*).

**Diagnostics and the install.**

5. `doctor -env` checks presence only. It does not validate the PE format. The PE check lives in the module phases (11.4).
5a. The rule that a module must not depend on the kernel crate has no enforcing lint in `toolchain/build.rs`. The plan said `lint_modules_integrity` would fail on such a dependency. The function reads the registry and `Cargo.toml` only, and it does not check module dependencies on the kernel. The rule currently rests on review.
6. `SCHEDULER_TASKS` and `OBSOLETE_TASKS` are hard-coded in the CLI on purpose. They describe the installation, not the modules. A drift against a module's manifest is reported as a WARN.
7. The `cli_commands` array of `help --json` is written by hand in `help_cmd`. The application half comes from the inventory.
8. The `UniDTK` task has `Hidden` set to `false` in [UniDTKTask.xml](../../scheduler-tasks/UniDTKTask.xml), while the architecture rule says that every background task is hidden. The code and the rule disagree. Decide which is right, then change one of them.
9. The splash generator `make_splash.py` is not in the repository. The sessions say it lives outside it. The art in `splash_art.rs` cannot be regenerated from this repository alone.

**Stop and update.**

10. The question of who stops a running instance during an update is open. The uninstaller's `PREUNINSTALL` removes the autostart task on upgrade too, and the old process holds ports 26540 and 9222 until it exits. Today `udtk build` stops the app itself (Chapter 14).

**Records and counts.**

11. The old live checklist in this book used `cargo build --release --bins` and `cargo test --workspace`. The rule in [AGENTS.md](../../AGENTS.md) forbids a direct release build, and the test count changed. Chapter 17 replaces the checklist. The counts in [Part V](../history/rfcs/udtk-migration.md) are the ones recorded at the time, and they are not re-run.
12. The sessions record and [AGENTS.md](../../AGENTS.md) still refer to the older section numbers of this book. The History chapter keeps those labels in its headings, so that the references still point at the right text.
