# Chapter 9. When Things Fail

This chapter explains what UniDTK does when something breaks, and where the evidence is left. A failure can happen in three places: in Rust code that panics, in backend code that reports a problem through the log, or in the JavaScript of the page. The shutdown path has its own record too. After this chapter, you can say which file holds the record for each kind of failure, follow a log line from the call that writes it to the text file and the database, explain why the usual filter `--level ERROR` shows nothing today, and walk through a short investigation with the `udtk` commands.

## Four records

Each kind of failure has its own record. Knowing which one to read saves most of the work.

| Kind of failure | Where it is recorded | How to read it |
| :--- | :--- | :--- |
| A Rust panic, on any thread | `%LOCALAPPDATA%\UniDTK\crash.log`, plus a summary line in the log | Open the file. No `udtk` command reads it. |
| A backend message, such as a failed start | `unidtk_logs.redb`, and the text file `logs\unidtk.log` | `udtk logs` |
| A JavaScript error in the page | `unidtk_logs.redb` as a `frontend` line, for the errors the page reports; otherwise `window.__UNIDTK_ERRORS__` in memory | `udtk logs --source frontend`, `udtk gui js:` |
| The shutdown sequence | The same log, with `shutdown:` lines | `udtk logs` |

The paths are the ones in [Install and Runtime Layout](../reference/install.md). The rule is [install.data-in-localappdata](../reference/install.md#r-install.data-in-localappdata): all of them live under `%LOCALAPPDATA%\UniDTK`, and none of them is written next to the program.

## The panic hook

Rust turns a panic into a call to a hook function, set with `std::panic::set_hook`. UniDTK installs its hook as the first action of `main()`, so a panic in any later code, on any thread, reaches it.

Filename: core/kernel/main.rs (abridged, the start of `main`)

```rust
fn main() {
    panic_hook::install_panic_hook();
    unidtk_api::set_logger(|message| log_line(message));
    load_env_into_process();
    log_line("main: start");
    // ...
}
```

The hook itself is short. It builds a report, sends a summary to the log, and writes the whole report to disk.

Filename: core/kernel/panic_hook.rs (abridged)

```rust
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let report = format_panic(info);
        let summary = report
            .lines()
            .filter(|line| !line.is_empty())
            .take(4)
            .collect::<Vec<_>>()
            .join(" | ");
        crate::logging::log_backend("panic", summary);
        crate::logging::flush();
        write_crash_log(&report);
        default_hook(info);
    }));
}
```

The order of the four steps is the point. The process may die right after the hook returns, and a log line that is still in a queue would die with it. So the hook first sends the summary into the queue, then calls `flush()`, which takes everything queued and writes it to the database synchronously. Then `write_crash_log()` appends the full report to `crash.log` and calls `sync_all()`, which asks the operating system to write the file to disk. The report contains the thread name, the file and line of the panic, the payload, and a backtrace. The default hook runs last, so a development run still prints to the console.

Note: the header comment of `panic_hook.rs` says the hook is for `panic = "abort"` builds. The release profile in `Cargo.toml` sets `panic = "unwind"`. The hook runs in both cases, because the standard library calls the hook before it starts to unwind. The comment is out of date and is not a reason to skip the hook.

The `crash.log` path is built with `dirs::data_local_dir()` directly, not with `unidtk_api::paths`. It is the same folder, `%LOCALAPPDATA%\UniDTK`, so a reader who knows the data folder finds the file.

## Where a log line goes

Every backend log line, from the kernel or a module, follows one path. The producer does not write to the database. It puts an entry into a queue, and a single thread writes the queue in batches.

Filename: core/kernel/logging.rs (abridged, `enqueue`)

```rust
fn enqueue(entry: LogEntry) {
    let Some(queue) = QUEUE.get() else {
        return; // early startup — the file sink covers that window
    };
    let critical = matches!(entry.level.as_str(), "WARN" | "ERROR");
    if critical {
        let _ = queue.tx.send(entry);
    } else if let Err(TrySendError::Full(_)) = queue.tx.try_send(entry) {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}
```

The queue is a bounded channel of 4096 entries. A `WARN` or `ERROR` entry waits for room, so it is never lost. An `INFO` or `DEBUG` entry is dropped when the queue is full, and the drop is counted. The flusher thread waits for the first entry, then keeps collecting for up to 200 ms or 2048 entries, and writes the batch. The write is two steps. `LogStore::insert_batch()` puts the batch into `unidtk_logs.redb` in one transaction, with eventual durability, which means the commit does not wait for the disk. Then `append_file()` writes one text line per entry to `logs\unidtk.log`. The line has the form `[timestamp] message`, and a frontend entry has the prefix `[frontend]`.

The rule is [storage.log-queue](../reference/storage.md#r-storage.log-queue), and the full mechanism is in [Logging: One Queue, Batched Commits, Two Sinks](../dev/logging.md).

Note: before the logging pipeline is ready, the queue does not exist. The function `log_line()` in `main.rs` then writes straight to the text file, so the first lines of a start are kept.

Note: the database is `unidtk_logs.redb`, a separate file from `unidtk.redb`, so a burst of log writes never waits on a user-data transaction. The rule is [storage.two-files](../reference/storage.md#r-storage.two-files).

The default retention policy keeps seven days and at most 5000 entries. The flusher runs a sweep once when it starts, and the sweep removes what the policy no longer allows.

## Levels: the trap in the filter

The database stores a `level` for each entry, and `udtk logs --level ERROR` filters on it. The queue has a blocking path for `WARN` and `ERROR`, so it looks as though an error would be kept and found.

Warning: in the code as it is today, no producer writes `WARN` or `ERROR`. `log_backend()` always sets the level to `INFO`, and so does `log_frontend()`. The function `log_line()` calls `log_backend()`, and the Tauri command `frontend_log` calls `log_frontend()`. A panic summary is written by `log_backend("panic", ...)`, so it is `INFO` as well. The blocking path exists, but nothing takes it. The logging document says this in its own words: "A frontend level is not implemented."

The consequence is that an empty result from `udtk logs --level ERROR` does not mean the application is healthy. A panic or a failed start can be present in the log while that filter returns nothing. Search by content instead, and check `crash.log` for panics.

## Frontend errors reach the log

JavaScript errors in the page take a different route, because the page cannot write to the database. It calls a command. The command `frontend_log` takes one `message` string and sends it to `log_frontend()`.

Filename: core/kernel/main.rs (abridged)

```rust
#[tauri::command(async)]
fn frontend_log(message: String) {
    crate::logging::log_frontend(&message);
}
```

Three parts of the page send messages through this command.

- `core/ui/boot-log.js` listens for `error` and `unhandledrejection` on `window`. It sends a line that begins with `BOOT ERROR:` or `BOOT UNHANDLED:`. The file is loaded as a module script, from `core/ui/index.html`, so its listeners are installed after the document has been parsed. An error that happens earlier, while the classic scripts run, is not reported by this file.
- `core/ui/unidtk-core.js` has a helper, `rlog()`, that writes to the console and sends the message. It also sends specific failures, such as `MUSIC RENDER FAIL` and `MICROGRAPHICS RENDER FAIL`, with the stack when there is one.
- `core/ui/harness.js` keeps its own record. It listens for the same two events and pushes each one into `window.__UNIDTK_ERRORS__`, a ring buffer of the last 50 errors. This record stays in the page and does not go to the log. It is read with the CLI, as shown below.

The error listener in `unidtk-core.js` shows the error on the screen, through `showDebugMessage()`. It does not send it to the log.

Warning: `frontend_log` is declared as a write command in `core/kernel/command_inventory.rs`. The socket door refuses writes, so `udtk invoke frontend_log` is refused. The rule is [ipc.socket-read-only](../reference/ipc.md#r-ipc.socket-read-only). A message that the page sends to the log reaches it only through the interface door.

## When the application stops

There are three ways the application can stop, and all three call the same function, `app_shutdown()` in `core/kernel/main.rs`.

1. The event loop reports `RunEvent::ExitRequested`. The handler calls `app_shutdown(app, false)`, and it must not block the event loop, so it does not wait for the VPN helper.
2. The command `quit_app` is called from the page. It calls `app_shutdown(app, true)` and waits, bounded, for the elevated helper that stops sing-box.
3. Windows ends the session. The window that the desktop attaches to is a child window, and Windows sends the session-end messages only to top-level windows. So `core/kernel/shutdown.rs` creates a hidden top-level window with its own message loop, and in `WM_ENDSESSION` it calls `app_shutdown(app, true)` under a shutdown block reason.

The reason for the third path is stated in the header of `shutdown.rs`. A surviving sing-box keeps its TUN adapter up, and a surviving TUN with `strict_route` black-holes the user's network on the next boot. The VPN is therefore stopped inside the time Windows allows.

`app_shutdown()` runs once. Its first line swaps a flag, `SHUTDOWN_DONE`, and a second call returns at once. It then stops the modules in a fixed order: the music player, the glyph bar, Telegram, the VPN, and the micrographics module. It removes the hotkeys, resets the wallpaper attachment, flushes the log queue, and writes `shutdown: done`.

Filename: core/kernel/main.rs (abridged, `app_shutdown`)

```rust
pub(crate) fn app_shutdown(app: &tauri::AppHandle, wait_for_vpn: bool) {
    if SHUTDOWN_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    log_line("shutdown: begin");
    // ... modules stopped in a fixed order ...
    hotkeys::shutdown();
    let _ = wallpaper::reset();
    crate::logging::flush();
    log_line("shutdown: done");
}
```

Note: the lines `shutdown: begin` and `shutdown: done` are a quick test of a stop. A stop that has a `begin` line and no `done` line did not finish. The missing step is the first place to look.

## Investigating a failure

Suppose a widget is blank, or the application seems to have stopped. The steps below go from the cheapest check to the most detailed. Each one is a `udtk` command or a file. The CLI reference is in [the udtk book](../udtk/README.md).

1. Check that the application answers. Run `udtk doctor -ping`. It asks the socket door for a reply and reports the time the reply took. If there is no answer, the process is not running, or it is stuck. A panic leaves its report in `crash.log`, so that file is the next step.

2. Read the backend lines. Run `udtk logs --limit 100`. Look for a `panic` module, for `shutdown:` lines, and for messages from the module in question. The default limit of the CLI is 50, so raise it when you need more.

3. Read the crash report, if a panic is suspected. Open `%LOCALAPPDATA%\UniDTK\crash.log` in a text editor. The newest report is at the end. The report names the thread and the file and line of the panic.

4. Read the errors of the page. Run `udtk logs --source frontend --limit 30`. This lists the lines sent by `frontend_log`. A missing line does not prove there was no error, because only the errors that the three page parts report are sent.

5. Look at the in-page record. Run `udtk gui "js:window.__UNIDTK_ERRORS__"`, which evaluates the expression in the page. The result is the ring buffer from `harness.js`, which holds the last 50 errors with their stacks. The command runs over CDP, so it is not a write through the socket door.

6. Look at the page itself. Run `udtk shot` to take a screenshot. The CLI saves it to `Pictures\Screenshots` and prints its path, and you can open the file to see the screen. Run `udtk gui tree` to print the tree of the interactive elements on the page. A widget with no interactive element does not appear in that tree, so the screenshot is the check for what is drawn.

7. Check the level filter last. Run `udtk logs --level ERROR`. As explained above, this filter is empty for every failure in the code as it stands, so an empty result tells you nothing.

Warning: the commands that inspect the page, `udtk gui`, `udtk shot`, and `udtk cdp-list`, need CDP to answer. The CLI learns the CDP port from the application over the socket door, in the reply to the ping. So `udtk doctor -ping` must answer first, and a page that is not loaded gives no useful result from the other commands.

Note: `udtk doctor` accepts section flags, such as `-kernel`, `-modules`, `-env`, `-check`, and `-simulate`. `-simulate` runs simulated user actions in the live page. Use it when the other checks look healthy but the behaviour is wrong.

## Summary

A failure in UniDTK leaves its evidence in one of four places. A panic leaves a full report in `crash.log`, and a short summary in the log, written before the process can die. A backend or frontend message goes through one queue, and is written in batches to `unidtk_logs.redb` and to the text file `unidtk.log`. A page error that the page reports is sent through `frontend_log`, and the errors that stay in the page are kept in `window.__UNIDTK_ERRORS__`. The shutdown writes its own begin and end lines. The filter `--level ERROR` is empty today because every producer writes `INFO`, so the search has to be by content.

The terms used in this chapter are explained in [Appendix A: Glossary](appendix-a-glossary.md).
