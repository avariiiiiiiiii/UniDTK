# Chapter 14. `build`, step by step

`udtk build` is the only way to make a release. It builds the three binaries and the NSIS installer in one tauri pass, installs the installer silently into `C:\Program Files\UniDTK`, proves the install by hashes, restarts the app, and checks it over IPC. Do not run `cargo build` by hand, and do not copy binaries into `Program Files`. Both skip the installer, and the scheduled tasks and the PATH are registered by the installer.

```bash
udtk build          # thread count from .cargo/config.toml
udtk build -j 1    # one compiler thread, for a machine low on memory
```

The `-j N` (or `--jobs=N`) argument sets `CARGO_BUILD_JOBS` for the build. `.cargo/config.toml` sets `[build] jobs = 2` and the `RUST_MIN_STACK` stack size for the compiler. The lessons behind those numbers are in 14.3.

## 14.1 Preconditions

- The `udtk.exe` that runs the command must **not** be in `Program Files`. If it is, the command refuses with `udtk build cannot self-build from Program Files (ACL). Run it from the checkout's target\release\udtk.exe`. The reason is that the directory is read-only for the user.
- The working copy must be a checkout with a `toolchain/` directory. Otherwise the command fails with `toolchain not found (run from repo root)`.

## 14.2 The steps

Each step below is a stage in `build_cmd` in [opt/cli/main.rs](../../opt/cli/main.rs).

**Step 0. Stop the app.** `build_cmd` calls `stop_cmd()`, which is described in 13.2. The running app holds DLLs such as `tdjson.dll` and `libmpv-2.dll`. Replacing a file in use gives `os error 32`, so the app must be stopped first.

**Step 1. Set the running CLI aside.** The `udtk.exe` that runs the build is in `target\release`, and cargo must relink it. Windows will not overwrite a running executable. So the command renames its own file to `udtk.exe.held`. It deletes any old `.held` file first.

**Step 2. Build with tauri.** `run_cargo_tauri(["build", "--bundles", "nsis"], jobs)` runs `cargo tauri build --bundles nsis` in the `toolchain` directory. Tauri builds the three binaries and the NSIS installer in one pass. The environment is the `.env` file, and `CARGO_BUILD_JOBS` is set if `-j` was given. If the build fails, the command renames `udtk.exe.held` back to `udtk.exe`, so the CLI stays usable, and it returns the error.

**Step 3. Remove the set-aside copy.** After a successful build, cargo has written a fresh `udtk.exe` at the canonical path. The `.held` copy is deleted, so that the installer packs the new CLI and not the old one.

**Step 4. Copy the runtime library.** `opt/libmpv/libmpv-2.dll` is copied to `target\release`. It is compared by SHA-256 first, and an unchanged copy is left alone, because a large DLL is often held for a moment by a virus scanner. If the source is missing and no copy exists, the command fails with `... is missing and target/release has no copy`.

**Step 5. Prune stale resources.** `prune_stale_resources` removes entries in `target\release\modules` and `target\release\folders` that no longer exist in the sources. A module removed from the sources would otherwise show a second widget card. Each removal is logged as `pruned stale resource: ...`.

**Step 6. Copy the model weights.** If `opt/models/discogs-maest-129e` exists, its `model.onnx` and `model.onnx.data` are copied into `target\release\models\discogs-maest-129e`.

**Step 7. Find the installer.** `find_installer` looks for a file ending in `-setup.exe` in `target\release\bundle\nsis`. None found gives `installer not found under target/release/bundle/nsis`.

**Step 8. Install silently.** `run_installer_and_verify` starts the installer with `/S`, with no window.

- If the installer cannot start because another process holds the file (`os error 32`, typical right after a build, when a scanner reads a fresh 400 MB file), the command retries **up to 5 times, 3 seconds apart**.
- If the installer returns `740` (`ERROR_ELEVATION_REQUIRED`, because a per-machine install needs administrator rights), the command runs the installer again with `Start-Process ... -Verb RunAs -Wait`. On the author's machine UAC is off, so this succeeds without a prompt.
- Because NSIS can skip a locked file and still exit 0, the command does not trust the exit code alone. It waits up to **60 seconds** until `C:\Program Files\UniDTK\unidtk.exe` exists with the same size as `target\release\unidtk.exe`. Otherwise it fails with `installer ran but installed unidtk.exe is missing or wrong size`.

**Step 9. Verify by hashes.** For `udtk.exe`, `vpn-helper.exe`, and `libmpv-2.dll`, the command computes the SHA-256 of the installed copy and of the copy in `target\release`, using `certutil -hashfile`. A difference, or an empty hash, fails with `<name> did not update after install`. This is the proof that the installed files are the new ones.

**Step 10. Restart and check over IPC.** The command starts the task with `schtasks /run /tn UniDTK`. Task Scheduler refuses to start the task from a shell that runs at medium integrity (a token filtered by UAC). When that happens, the command starts `C:\Program Files\UniDTK\unidtk.exe` directly, with the same working directory, and notes it in the output: `restart: task start denied (medium-integrity shell) — spawned the app directly`. Then it sends `ping` every 400 ms for up to **30 seconds**. If the app does not answer, the command fails with `app did not answer on IPC within 30s after install`.

**Step 11. Report.** On success the output starts with `build to install to start to verify OK`, and then it appends the journal's ERROR records (`logs --level ERROR`). An empty list is the expected result.

## 14.3 Lessons behind the steps

These are the failures that shaped the steps. They are facts from the recorded sessions, and they are the reason for the stop-first rule.

- **Memory.** A full build with thin LTO, on the author's 15.7 GB machine, reached the system's allocation limit. The page file grew to 48 GB, and the compiler failed with `LLVM out of memory`. The fix in `.cargo/config.toml` is a low job count and a large stack. For the tests, `cargo test --release --workspace -j 2` brought down two LTO builds at once with `0xc000012d`, so the tests run with `-j 1`. These are the author's recorded limits, not a general rule.
- **Held files.** The installed app once held `tdjson.dll` and the OpenSSL DLLs **from the repository's `target\release`**, because the path to the Telegram library was stored in the settings. The consequences were an `os error 32` without a file name, and a broken Telegram after the folder was renamed. The step 0 stop and the install path prevent both.
- **Cold build.** `build.rs` requires the resources of `tauri.conf.json` (`../target/release/udtk.exe`, `vpn-helper.exe`) to exist. On an empty `target/` the build cannot succeed, because it needs the program it builds. The recorded workaround is zero-length placeholder files, which cargo overwrites. The recorded fix is to accept missing resources at build time and check them at packaging.
- **Quiet success.** A cargo run piped through a PowerShell `Select-Object` was cut off early, and the artifact silently stayed old. It was caught twice as "build succeeded, binary is old". Run cargo directly, and read the exit status.
- **Commit `cd1949b`** (`fix(cli): leave an unchanged libmpv copy alone`) is the reason for the hash check in step 4.
