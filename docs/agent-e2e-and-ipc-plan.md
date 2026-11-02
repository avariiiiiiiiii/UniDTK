# Implementation Plan: Agent E2E Testing, User Emulation, redb Optimization, and Removal of Duplicate IPC

> **Document:** `docs/agent-e2e-and-ipc-plan.md`  
> **Status:** Approved architecture refactoring plan  
> **Date:** 1 September 2026

---

## 1. Original User Requirements

1. **Real testing instead of a formal check ("not just for show"):**
   - Test the application the way a real user uses it: through actions in the interface (clicks, typing, switching tabs), not through artificial, isolated calls to backend functions.
2. **Eliminate total hardcoding:**
   - Stop writing a separate `ipc.rs` for each module, and stop writing hardcoded functions (`tg_cmd`, `vpn_cmd`, `vol_cmd`) in the CLI.
   - Remove the triple duplication of code (`tauri::command` $\to$ module `ipc.rs` $\to$ CLI `main.rs`).
3. **Text takes priority over Vision/screenshots:**
   - AI models work faster, cheaper, and more accurately with text. Screenshots are expensive, and many models have no vision.
   - UI inspection must be based on a compact semantic tree of elements (AOM) of 300–500 tokens, instead of 30,000 tokens of raw HTML.
4. **Action Receipts with a log delta:**
   - Any user action performed through the CLI (`shell act click ...`) must return more than a status. It must confirm the physical click, the DOM mutations, and **all new backend and frontend logs** produced by that action.
5. **One source of truth for logs (no second buffers):**
   - "Change the reader and fix the bug in redb, instead of building a second reader."
   - Do not create parallel in-memory log stores next to `redb`. Fix the algorithmic read and write bugs directly in `shell_db.rs`.
6. **Canonical command name:**
   - Use the clear name `shell commands` (not the academic `capabilities`).

---

## 2. Results of the 10-Subagent Review

During the design, 10 specialized critic subagents were called in sequence to review the plan:

1. **Critic #1 (System Reliability):**
   - *Veto* on dropping Rust IPC entirely in favor of CDP. If WebView2 crashes or hangs, the agent goes blind.
   - *Consensus:* a two-level model: L1 (Rust Core IPC) for data, daemons, and health, plus L2 (CDP) for UI emulation.
2. **Critic #2 (Security & Privilege Boundaries):**
   - *Veto* on a permanently open CDP port 9222 with `allow-origins=*` in release builds (an RCE vulnerability through the user's browser).
   - *Consensus:* CDP is enabled strictly in dev mode (`shell dev --debug`) or in builds with `debug_assertions`.
3. **Critic #3 (QA & E2E Testing):**
   - *Veto* on selectors based on CSS classes (they break on redesign) and on `sleep(200)` pauses (they conflict with the 260/320 ms `spring` animations).
   - *Consensus:* a strict selector contract (`data-testid` and ARIA roles), `waitForCondition` readiness predicates, idempotence, and state recovery (Snapshot/Restore).
4. **Critic #4 (Frontend Core & Vanilla JS):**
   - *Consensus:* add a test facade `window.__SHELL_HARNESS__` with a ring buffer of errors, `__SHELL_ERRORS__`.
   - Add a **Zero-Motion** mode (`setMotionScale(0)` in `motion.js`), which collapses animation durations to 0 ms for instant tests.
5. **Critic #5 (Performance & Low-Resource Systems):**
   - *Veto* on calling `node backend/cli/src/shell-cdp.js` (180 ms of overhead to start `node.exe`).
   - *Consensus:* a native Rust WebSocket client in the CLI (2–3 ms execution time). Use the native `el.checkVisibility()` without layout thrashing.
6. **Critic #6 (Agentic DX & CLI Ergonomics):**
   - *Consensus:* a **Diagnostic Triad** error format (WHAT, WHY, REMEDY), adaptive verbosity (a one-line status on success), exit codes 0/1/2/3/4, and an unconditional ban on interactive `stdin`.
7. **Critic #7 (Windows Subsystems & WorkerW):**
   - *Consensus:* Shell runs behind the Windows icons, under `WorkerW`. Coordinates must be multiplied by `devicePixelRatio` for 125%/150% monitors. WebView2 flags that disable DWM throttling (`--disable-backgrounding-occluded-windows`). A graceful teardown of the window to prevent zombie `WorkerW` containers.
8. **Critic #8 (Redb & Logging Pipeline):**
   - Found a fatal bug in `shell_db.rs:667`: the whole database is pulled into memory with `table.range(..).rev().collect()` before `take(limit)` is applied.
   - Found an $O(N)$ scan in `log_max_id`, where the last element of the B-tree could be taken with `next_back()` instead.
   - Found that every frontend log is written twice, at `main.rs:204`.
9. **Critic #9 (Microkernel & Dynamic Dispatch):**
   - Showed that a hand-written IPC in each module is absurd when a ready-made `tauri::command` already exists.
10. **Critic #10 (Chief Arbiter):**
    - Synthesized the consensus, formulated 10 Engineering Taboos and a 3-phase roadmap.

---

## 3. Removing Duplicate IPC

### What is wrong with the current system:
* **The frontend** calls the backend through `tauri::invoke("vpn_connect")`. This is Tauri's standard, fast, typed mechanism.
* **The modules** contain the files `vpn/ipc.rs`, `telegram/ipc.rs`, `volume/ipc.rs`, `explorer/src/ipc.rs`, and `music/src/ipc.rs`. Each one manually parses the same commands for the core's localhost TCP socket.
* **The CLI** contains 1,120 lines of the functions `vpn_cmd`, `tg_cmd`, and `vol_cmd`, which send JSON to those modules.

### Target solution:
1. **Delete the module files `ipc.rs`:**
   - Test the module's business logic through real user actions in the UI (a click on the network button $\to$ the UI calls `tauri::invoke`).
2. **The core's service IPC (`backend/core/ipc.rs`):**
   It stays as a compact system-monitoring gateway, with only what the external CLI process needs:
   - `ping`: checks that the core is reachable.
   - `logs`: reads the journal from `redb` (with `since_id` support for deltas).
   - `ps`: monitors process memory (`shell.exe`, `msedgewebview2.exe`).
   - `commands`: lists the CLI and core commands.

---

## 4. Fixing the `redb` Database (`shell_db.rs`)

Instead of building a duplicate in-memory buffer, fix the algorithmic errors in the main store, `shell.redb`:

1. **Fix `list_logs` ([`shell_db.rs:667`](file:///C:/Users/admin/Documents/GitHub/shell/backend/modules/music/src/shell_db.rs#L667)):**
   ```rust
   // BEFORE (loaded the ENTIRE database into RAM):
   let range = table.range::<u64>(..).map_err(|e| e.to_string())?;
   let rev: Vec<_> = range.rev().collect();
   for item in rev.into_iter().take(limit) { ... }

   // AFTER (lazy slice from the end, microseconds):
   for item in table.range::<u64>(..)?.rev().take(limit) { ... }
   ```
   The `collect()` call builds a `Vec` containing every entry before `take(limit)` runs. So the whole table is read into memory even when the caller needs only `limit` rows. The reversed iterator does not need that `Vec`: `take(limit)` pulls only the entries it needs from the end.
2. **Fix `log_max_id` ([`shell_db.rs:740`](file:///C:/Users/admin/Documents/GitHub/shell/backend/modules/music/src/shell_db.rs#L740)):**
   ```rust
   // BEFORE (full table scan, O(N)):
   for item in table.iter()? { ... }

   // AFTER (instant maximum lookup in the B-tree, O(1)):
   table.range::<u64>(..)?.next_back().map(|(k, _)| k.value()).unwrap_or(0)
   ```
   `next_back()` takes the last key of the range directly, instead of walking the table from the first entry. If the table is empty, `unwrap_or(0)` returns 0.
3. **A native range method for the log delta (`get_logs_since`):**
   ```rust
   pub fn get_logs_since(&self, since_id: u64) -> Result<Vec<LogEntry>, String> {
       let mut entries = Vec::new();
       let range = table.range::<u64>((std::ops::Bound::Excluded(since_id), std::ops::Bound::Unbounded))?;
       for item in range {
           let (_, v) = item?;
           if let Ok(entry) = serde_json::from_slice::<LogEntry>(v.value()) {
               entries.push(entry);
           }
       }
       Ok(entries)
   }
   ```
   `Bound::Excluded(since_id)` skips the entry the caller already has, so the result holds only the entries written after `since_id`.
4. **Remove the double write at [`main.rs:204`](file:///C:/Users/admin/Documents/GitHub/shell/backend/core/main.rs#L204):**
   In `frontend_log`, remove the duplicate `log_line(...)` call and keep the single write, `qbc_music_core::logging::log_frontend(...)`.

---

## 5. Physical User Emulation (CDP Input API)

### The 5-phase cycle for performing an action:
1. **Find the element:** strictly by the contract `data-testid="..."` or `role="..."[name="..."]`.
2. **Hit-testing:** check `element.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })` and `document.elementFromPoint(x, y)` to confirm that no modal or overlay covers the element.
3. **Coordinate mapping:** `element.getBoundingClientRect()` returns the center coordinates in **CSS pixels of the viewport**. In CDP, `Input.dispatchMouseEvent` takes these coordinates **1:1, without scaling**, because the Blink protocol expects CSS pixels. Multiplying by `devicePixelRatio` is needed only for native Win32 `SendInput`. With CDP, it makes the click miss by a factor of 1.25–1.5.
4. **Physical dispatch in Blink:**
   - `Input.dispatchMouseEvent` (`type: "mouseMoved"`, `x`, `y`)
   - `Input.dispatchMouseEvent` (`type: "mousePressed"`, `button: "left"`, `clickCount: 1`)
   - `Input.dispatchMouseEvent` (`type: "mouseReleased"`, `button: "left"`)
5. **Build the action receipt (Action Receipt):**
   - The hit coordinates and confirmation of the click.
   - DOM mutations (class and attribute changes, observed through MutationObserver).
   - The backend log delta (`redb::get_logs_since(last_id)`).
   - Errors from the ring buffer `window.__SHELL_ERRORS__`.

---

## 6. Native Rust WebSocket in the CLI and Research Findings

Based on research into the Tauri 2 GitHub ecosystem, forums, and agent automation tools, the following decisions were recorded:

1. **Rust CDP client (`tungstenite` instead of `chromiumoxide` / `headless_chrome`):**
   - `chromiumoxide` is excessive. It brings in a heavy async `tokio`, `reqwest`, and about 20 MB added to the binary.
   - A hand-written WebSocket parser in `cli/main.rs` hangs on fragmented packets.
   - **Choice:** `tungstenite = { version = "0.24", default-features = false, features = ["handshake"] }` on top of the standard `std::net::TcpStream`.
   - CLI startup time: <3 ms. Binary size: <1.5 MB. Compile time: 1.5 seconds.
   - The helper file `backend/cli/src/shell-cdp.js` and the dependency on an external `node.exe` are removed.
2. **Semantic DOM tree (Browser-Use and Playwright ARIA Snapshots pattern):**
   - The raw CDP method `Accessibility.getFullAXTree` was rejected. It returns a flat list with no coordinates, z-index, or viewport, and it requires hundreds of slow `DOM.getBoxModel` calls.
   - **Choice:** inject a lightweight JS extractor that walks the interactive nodes (`data-testid`, ARIA roles, buttons, inputs), checks `checkVisibility()` and `elementFromPoint`, and produces compact text of 300–500 tokens, with ordinal IDs `[1]`, `[2]`.
3. **Local IPC transport:**
   - For now, the lightweight localhost TCP (`127.0.0.1:26540`) stays, with JSON-over-line (`\n`), as the simplest and most cross-compatible option.
   - In the future, move to Windows Named Pipes through the `interprocess` crate (`\\.\pipe\shell-ipc`), to avoid port collisions and fully close the network stack.

---

## 7. The 10 Engineering Taboos

| No. | Engineering Taboo | Why it is forbidden | What we do instead |
|---|---|---|---|
| **1** | **No blind `sleep(N)`** | Causes races and flaky tests under varying CPU load. | `Zero-Motion` mode (`setMotionScale(0)`) plus `waitForCondition` predicates. |
| **2** | **No raw `element.click()`** | Ignores overlaps, z-index, and real visibility. | Real CDP events `Input.dispatchMouseEvent`, checked with `elementFromPoint`. |
| **3** | **No CSS selectors based on style classes** | Styles change in a redesign and break automation. | Only `data-testid` attributes and semantic ARIA roles. |
| **4** | **No parallel in-memory log databases** | Creates two sources of truth that drift apart. | Fix the B-tree algorithms in `redb` (`get_logs_since`, no `.collect()`). |
| **5** | **No open port 9222 in release** | A critical RCE vulnerability for a local browser. | CDP is available only with the flag `shell dev --debug` or in dev builds. |
| **6** | **No hardcoding of module subcommands in the CLI** | Breaks the microkernel and requires a CLI rebuild for every small change. | Universal UI commands (`shell act`, `shell dom`) and the service IPC. |
| **7** | **No feeding raw HTML (30k tokens) to an LLM** | Exhausts the model's context and causes hallucinations. | **Compact Semantic Tree** (300–500 tokens of interactive nodes only). |
| **8** | **No external runtimes (Node.js/Python/VBS)** | Breaks the self-contained nature of the `shell.exe` utility. | Native Rust code (a native WebSocket client inside `backend/cli`). |
| **9** | **No ignoring DPI under WorkerW** | On 125%/150% monitors, the click lands beside the button. | Multiply client coordinates by `window.devicePixelRatio`. |
| **10**| **No "Done" status without an Action Receipt** | You cannot take a test's word for it. | Check the receipt: the coordinates match, the log delta is clean, and there are no JS errors. |

---

## 8. Step-by-Step Implementation Roadmap

### Phase 1: Runtime Foundation, `redb` Optimization, and Native WebSocket in the CLI
1. **Optimize `shell_db.rs`:**
   - Replace `table.range(..).rev().collect()` in `list_logs` with a direct lazy iterator.
   - Replace the full table scan in `log_max_id` with `table.range(..)?.next_back()`.
   - Add the method `get_logs_since(since_id: u64)`.
   - Remove the double log write at `main.rs:204`.
2. **Native WebSocket in the CLI:**
   - Add `tungstenite` to `backend/cli/Cargo.toml`.
   - Implement a function that runs CDP commands directly, in `backend/cli/src/main.rs`.
   - Delete `backend/cli/src/shell-cdp.js`.
3. **Security and DWM in the core:**
   - Restrict `--remote-debugging-port=9222` to debug mode.
   - Add WebView2 flags against DWM throttling when the desktop is inactive.

### Phase 2: UI Test Harness, Zero-Motion, Semantic DOM, and Action Receipts
1. **Frontend test harness:**
   - Add `frontend/core/harness.js` with the ring buffer of errors `window.__SHELL_ERRORS__`.
   - Hook up the switch `setZeroMotion(true)` through `motion.js`.
2. **Semantic DOM (`shell dom`):**
   - Implement the export of a compact interactive tree (300–500 tokens).
3. **Physical input and action receipts (`shell act`):**
   - Implement `shell act click <selector>` with an `elementFromPoint` check, DPI normalization, and capture of the log delta.

### Phase 3: Removing Duplicate IPC and Cleaning Up the CLI
1. **Deleting the module `ipc.rs` files:**
   - Delete `telegram/ipc.rs`, `vpn/ipc.rs`, `volume/ipc.rs`, `explorer/src/ipc.rs`, and `music/src/ipc.rs`.
   - Simplify `backend/core/ipc.rs`: keep only `ping`, `logs`, `ps`, and `commands`.
2. **Cleaning up the CLI `backend/cli/src/main.rs`:**
   - Delete the module-specific functions `tg_cmd`, `vpn_cmd`, and `vol_cmd`.
   - Keep a lean set: `shell doctor`, `shell logs`, `shell ps`, `shell dom`, `shell act`, `shell commands`.
3. **Verification against the full checklist:**
   - `cargo check`, `cargo test`, `cargo build`, and real E2E actions at runtime.
