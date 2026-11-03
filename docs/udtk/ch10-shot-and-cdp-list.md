# Chapter 10. `shot` and `cdp-list`

```bash
udtk shot [path] [--json|-j] [-q|--quiet] [-o|--output <path>]
udtk cdp-list
```

`shot` asks CDP for a PNG (`Page.captureScreenshot`, from the surface, optimised for speed). The default file is `unidtk_YYYY-MM-DD_HH-MM-SS.png` in the **Screenshots** folder of the user, found through `SHGetKnownFolderPath`. If that folder cannot be created, it tries `Pictures\Screenshots`, then `Pictures`, and then the system temporary directory. A name that already exists gets `_1` to `_100` appended. A path without an extension gets `.png`. A relative path is taken from the current directory.

Before writing, the file is checked: it must start with the PNG signature and be at least **1024 bytes**. Otherwise the error is `corrupted screenshot received (size: N bytes, expected valid PNG)`. If writing to the chosen place fails, the file goes to the temporary directory, and the output gives the path that was actually used.

`-q` prints only the absolute path. `--json` prints `{"ok": true, "path": ..., "bytes": N}`.

When CDP is not reachable, the error carries a hint, and the hint tells two cases apart: `UniDTK application is not running. Start it via 'udtk run' or 'schtasks /run /tn UniDTK'` when the socket also fails, and `UniDTK core is running, but WebView2 CDP (port N) is not responding.` when only CDP fails.

`cdp-list` prints the JSON list of CDP targets from `GET /json`.
