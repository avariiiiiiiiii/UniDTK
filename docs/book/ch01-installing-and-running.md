# Chapter 1. Installing and running

This chapter shows where UniDTK lives on disk after an install, how the application starts, which four `udtk` commands tell you that it is alive, and what `udtk build` does to produce a release. After reading it you can start the shell, check its health, read its log, take a screenshot, and explain each step of a release build.

## Two directories

UniDTK keeps its program and its data apart. The program directory is written once, by the installer, and the application only reads from it. The data directory is where everything that changes at runtime goes.

Filename: install layout (abridged, from `C:\Program Files\UniDTK`)

```text
unidtk.exe           the kernel: window, IPC, storage
udtk.exe             the command-line bridge
vpn-helper.exe       GUI-subsystem helper that stops the VPN
libmpv-2.dll         embedded player library
modules/             module manifests
scheduler-tasks/     task templates, with @INSTALL_DIR@ placeholders
book/                this book, rendered on demand by udtk doc
// ...
```

The rule is [install.program-files-read-only](../reference/install.md#r-install.program-files-read-only): the application writes nothing next to itself. The rule that places the mutable state is [install.data-in-localappdata](../reference/install.md#r-install.data-in-localappdata). In code, the two directories come from two small functions in `core/api/src/paths.rs`. `install_dir()` returns the folder that contains the running executable, so `udtk` and `unidtk` always agree on where the program is. `data_dir()` asks Windows for the local application data folder and appends `UniDTK`, which gives `%LOCALAPPDATA%\UniDTK`.

That folder holds the two databases (`unidtk.redb` and `unidtk_logs.redb`, see [Chapter 5](ch05-storage.md)), `logs/`, `cache/`, and the Telegram session. The cache folder is where `udtk doc` renders the book, as you will see below.

Note: the install directory is not writable for an ordinary user. This is why the program and the data are separate, and why `udtk build` cannot run from inside `Program Files`.

## Starting the application

There are three ways to start UniDTK, and they all run the same file, `unidtk.exe`, from the install directory.

The first is the logon task. The template `scheduler-tasks/UniDTKTask.xml` has a `LogonTrigger`, so Windows starts the task when the user signs in. Its action is the command line `@INSTALL_DIR@\unidtk.exe` with the working directory `@INSTALL_DIR@`. The placeholder is never committed as a real path: `prepare_task_xml()` in `opt/cli/main.rs` replaces `@INSTALL_DIR@` with the real install directory when the task is registered. The installer triggers that registration through `udtk tasks register all` (see `toolchain/windows/hooks.nsh`), so a fresh install starts at logon without anyone running a script by hand. You can check or change the logon task with `udtk autostart status`, `udtk autostart enable`, and `udtk autostart disable`.

The second is `udtk run`. It is the manual start, and it is the one to use when you want the application now. The function `run_cmd()` in `opt/cli/main.rs` does four things. It looks for `unidtk.exe` next to the CLI, and stops with an error if the file is missing. It rejects the old `--debug` flag, because the debug port is always on. It starts the program with the install directory as its working directory and with the variables from the `.env` file that `find_env_file()` locates. Finally it starts the process with the `CREATE_NO_WINDOW` flag, so no console window appears. Then it returns `launched <path>` at once. It does not wait for the window to appear, which is why the health checks below exist.

The third is the development entry. From a checkout, `cargo tauri dev` inside `toolchain/` runs the kernel without an installer. This is the path for changing code, and it has no scheduler task.

Warning: `udtk run` launches the copy that sits next to the CLI. Run it from `target\release` in a checkout and you start the development build; run it from `Program Files` and you start the installed build. It never searches for some other copy.

## The first checks

Once the application runs, four commands tell you whether it is healthy. Each one talks to the running process over the socket door on `127.0.0.1:26540`, the door described in [Chapter 3](ch03-the-kernel-and-its-commands.md).

`udtk doctor -ping` is the quickest check. It opens the socket and sends the `ping` command. The reply is built by `handle_ping()` in `core/kernel/ipc.rs`, and it contains the protocol version, the list of capabilities, and two ports. The `cdpPort` is the port of the WebView2 debugging protocol, which `udtk shot` and `udtk gui` use. The `wsPort` is the port of the page's own door. The round trip is printed in milliseconds, as in `[OK] IPC ping (3ms): app is running`.

Warning: plain `udtk doctor` with no flags runs every section, including a `cargo check` of the backend. That takes tens of seconds. Use `-ping` for a live check, and `udtk doctor -check` only when you want the compiler's verdict.

`udtk logs --level ERROR` prints the most recent error records from the log. `udtk logs --source frontend --limit 30` prints the last thirty records written by the page. The default limit is fifty. The command calls the socket command `logs`, which reads the journal in `unidtk_logs.redb`, the second database file described in [Chapter 5](ch05-storage.md). An empty result from `--level ERROR` is the healthy answer.

`udtk shot` takes a picture of the window. It asks the page for a screenshot over the debugging protocol and saves it under `Pictures\Screenshots` by default, then prints the absolute path. Use it after any visual change, because the log cannot show a broken layout.

`udtk doc` renders this book. It reads the Markdown from the install's `book/` folder, or from the checkout if the install has none, writes HTML under `%LOCALAPPDATA%\UniDTK\cache\book`, and opens the index in Explorer. Run `udtk doc --check` inside a checkout to validate the book's links and the rule identifiers it cites.

## Reading the ping reply

The reply to `ping` is the first piece of the system you can read from outside. The function `handle_ping()` in `core/kernel/ipc.rs` builds it, and its result looks like this.

Filename: core/kernel/ipc.rs (abridged, the result of `handle_ping`)

```rust
Ok(serde_json::json!({
    "pong": true,
    "proto": PROTOCOL,
    "capabilities": ["envelope-v1", "log-delta", "door-policy", "gui", "ws-door"],
    "cdpPort": crate::cdp_port(),
    "wsPort": crate::ws::port(),
}))
```

The `proto` field is the wire version of the socket protocol. The CLI compares it with its own version and refuses to guess when they differ, because the installed bridge and the installed kernel are replaced separately. The `capabilities` list says which features this build has, so a client can check for a feature before it uses it.

The `cdpPort` is not a constant. The kernel prefers port 9222 for the debugging protocol, but it takes another port if 9222 is already in use, which happens when a stale instance still holds it. The CLI never assumes 9222; it reads the port from this reply. The `wsPort` is the port of the page's own door, described in [Chapter 3](ch03-the-kernel-and-its-commands.md).

Note: the reply does not include the token of the page's door. The comment in the code says the reply travels over the socket, which any local process can open, so the token is handed out only to the page.

## First run and the data folder

The first time the program starts, the data folder may not exist yet. `data_dir()` in `core/api/src/paths.rs` creates `%LOCALAPPDATA%\UniDTK` if it is missing, so every caller gets an existing directory. The kernel then opens its two databases inside that folder, `unidtk.redb` for the user's data and `unidtk_logs.redb` for the journal, as [Chapter 5](ch05-storage.md) explains. Nothing needs to be created by hand.

Warning: the data folder is per Windows user. A second Windows account has its own `%LOCALAPPDATA%` and therefore its own databases, its own Telegram session, and its own log. Copying the install directory to another account does not copy the data.

If a check in this chapter does not behave as described, the troubleshooting path is in [Chapter 9](ch09-when-things-fail.md).

## Building a release

The only command that builds a release is `udtk build`. The rule is [install.one-build-command](../reference/install.md#r-install.one-build-command): plain `cargo build` and hand copies into `Program Files` are not allowed, because a hand copy does not register the scheduled tasks. The function is `build_cmd()` in `opt/cli/main.rs`. Its steps run in this order.

Filename: opt/cli/main.rs (abridged, the order of `build_cmd`)

```rust
fn build_cmd(args: &[String]) -> Result<String, String> {
    // ...
    if exe.to_string_lossy().to_ascii_lowercase().contains("program files") {
        return Err("udtk build cannot self-build from Program Files (ACL). ...".into());
    }
    let _ = stop_cmd();                              // 1. stop the running app
    let held = exe.with_file_name("udtk.exe.held");
    let _ = std::fs::rename(&exe, &held);            // 2. free the name for cargo
    let build_res = run_cargo_tauri(&["build", "--bundles", "nsis"], jobs); // 3.
    // ...
    run_installer_and_verify(&installer, &release_dir, &programs_dir)?; // 4. install, hash
    let _ = std::process::Command::new("schtasks")
        .args(["/run", "/tn", "UniDTK"]).output();  // 5. restart
    // ... poll ping for up to 30 seconds ...
}
```

The first step stops the running application, so its DLLs are not locked. The second renames the running `udtk.exe` to `udtk.exe.held`. Windows will not let cargo overwrite a running executable, and the rename frees the name. If the build fails, the rename is undone, so the CLI still runs.

The third step is one call to `cargo tauri build --bundles nsis`, run in `toolchain/`. Tauri builds all three binaries and then the NSIS installer in a single pass. The `-j N` option of `udtk build` sets the environment variable `CARGO_BUILD_JOBS`, which overrides the thread count in `.cargo/config.toml`. Thin LTO needs a lot of memory, so the thread count matters on a small machine.

The fourth step runs the installer silently with `/S` and waits for it. A freshly written installer is often held for a few seconds by a virus scanner, so the call retries on a sharing violation. After the install, the function compares the SHA-256 hash of `udtk.exe`, `vpn-helper.exe`, and `libmpv-2.dll` in `Program Files` against the copies in `target\release`. The hashes come from `certutil`. A mismatch is an error, not a warning.

The fifth step restarts the application through the same scheduled task the logon uses. If Windows refuses to start the task from a shell that is not elevated, the function starts `unidtk.exe` directly. Then it sends `ping` every half second for up to thirty seconds. The build is not finished until the ping answers, which is the same gate the first checks use.

Warning: do not repeat these steps by hand. Copying `unidtk.exe` into `Program Files` skips the installer, so the scheduled tasks and the `PATH` entry are not registered, and the hash check never runs.

Note: the copy in `Program Files` is made by the installer. The build function does not copy the binaries there itself; it only checks, after the install, that the installed files match the build output.

## Summary

UniDTK installs into a read-only program directory and keeps its state in `%LOCALAPPDATA%\UniDTK`. The logon task and `udtk run` both start `unidtk.exe` from that directory. `udtk doctor -ping`, `udtk logs`, `udtk shot`, and `udtk doc` are the first checks, and each one talks to the running kernel. `udtk build` is the single release path: it stops the app, builds, installs silently, verifies by hash, restarts, and waits for a live answer. The next chapter opens the window that the kernel draws behind the desktop icons: [Chapter 2, The desktop window](ch02-the-desktop-window.md).
