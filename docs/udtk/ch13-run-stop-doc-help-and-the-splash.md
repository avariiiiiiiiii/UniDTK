# Chapter 13. `run`, `stop`, `doc`, `help`, and the splash

## 13.1 `udtk run`

```bash
udtk run [args...]
```

`run` starts **only** `unidtk.exe` from the directory of the CLI (`install_dir()`). It loads the `.env` file, sets the working directory to that directory, forwards the arguments, and starts the process with `CREATE_NO_WINDOW`. It does not wait for the app. It prints `launched <path>` and returns. If `unidtk.exe` is not next to the CLI, the error is `unidtk.exe not found next to udtk.exe (...). Run 'udtk build' or install the app first.` `run` never searches the repository for another binary. A stale copy once started from a deleted `target` directory, and that is why the rule exists.

`--debug` and `-d` are refused with `--debug was removed: CDP is always on in release builds`.

## 13.2 `udtk stop`

```bash
udtk stop
```

`stop` makes the app's files replaceable. It runs in three steps:

1. If CDP is reachable, it asks the page to quit through its harness (`h.invoke('quit_app')`). The command `quit_app` is defined in [core/kernel/main.rs](../../core/kernel/main.rs). In that case it allows **3500 ms** for the quit.
2. It then polls for processes whose executable lies under the install directory, excluding itself. The total wait is the grace period plus **2500 ms**. The poll uses PowerShell and `Win32_Process`.
3. If processes remain after the deadline, it runs `taskkill /F` on each. Finally it runs `schtasks /end` for the elevated tasks (`UniDTKSingbox`, `UniDTKVpnStop`).

It then looks at the install directory again, for up to two more seconds, because a killed process takes a moment to leave the list. If nothing is left it prints `stopped` (or `stopped (N process(es) had to be killed)`) and exits 0. If something is still running it fails with `still running after quit and kill: pid ...`, and `udtk build` and the installer's pre-install hook stop there instead of failing later with an opaque `os error 32`.

## 13.3 `udtk doc`

```bash
udtk doc
```

`doc` renders [README.md](../../README.md), [AGENTS.md](../../AGENTS.md), and every page in `docs/` into `%LOCALAPPDATA%\UniDTK\cache\book`, and opens `index.html` in Explorer. The source is the installed `book` folder if it exists, and otherwise the checkout. The output goes to the cache because `Program Files` is read-only.

## 13.4 `udtk help`

`udtk help` prints the text help. `udtk help --json` prints the manifest of Chapter 7: `protocol`, `cli_commands`, `app_commands`, and a note. The offline manifest works when the app is not running, because the inventory is compiled into the CLI.

## 13.5 The splash

`udtk` with no arguments prints the splash, from [opt/cli/splash.rs](../../opt/cli/splash.rs) and the generated art in [opt/cli/splash_art.rs](../../opt/cli/splash_art.rs). The art is generated, so it must not be edited by hand. The generator is not in the repository. The sessions record says it lives next to the design skill, outside the repository, under the name `make_splash.py`, and that the repository keeps only the output.

How the layout is chosen:

- The width comes from `COLUMNS`, and the height from `LINES`.
- The emblem is the largest of three precomputed sizes that fits **both** the width (a threshold computed from the art's geometry) **and** the row budget. The budget is `clamp(height - 2, 10, 28)` (`rows_budget`). Narrower or shorter windows fall back to smaller emblems, then to a stacked layout, then to text.
- Colour is used only on a terminal. It is disabled by `NO_COLOR` and by `TERM=dumb`, and it is forced by `FORCE_COLOR` or `CLICOLOR_FORCE`.
- On Windows, the console is set to VT mode and to code page 65001, and the result is **verified**. If that fails, the output is plain text instead of raw escape sequences.
- The right column is aligned to the computed width of the emblem, not to a constant. Otherwise the lines drift.

The command map at the bottom wraps at narrow widths, with an indent.
