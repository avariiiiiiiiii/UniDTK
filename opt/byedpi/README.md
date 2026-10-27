# ByeDPI (`ciadpi.exe`) — bundled DPI-desync SOCKS proxy for Telegram

Local user-space SOCKS5 with packet desync (split/disorder/fake/tlsrec):
no remote server, no admin rights, no kernel driver. The telegram module
spawns it as a hidden child (`telegram.rs::ensure_managed_proxy`) on
`127.0.0.1:12909` and points TDLib at it via `addProxy`.

- Source: https://github.com/hufrea/byedpi
- Release: `v0.17.3` (asset: `byedpi-17.3-x86_64-w64.zip`)
- Zip sha256 (release asset): `70d2c94147193cb915f9c6eb5144b8d404dacbcfa90bda2383b6b211afafa456`
- Binary sha256 (`ciadpi.exe`): `eb53ceeeb981cc6735ac24bb1e51e725280b86630e80fdf19ddc4ee4a5b54ef4`

## Runtime args (matches upstream `byedpi.bat`)

```
ciadpi.exe -i 127.0.0.1 -p 12909 --split 1 --disorder 3+s --mod-http h,d --auto torst --tlsrec 1+s
```

Single conservative desync group (split + disorder + http-mods + torst
auto-fallback + tlsrec) — works on most ISPs without tuning.

## Supply notes

- The binary is gitignored (`opt/byedpi/*.exe`); this README pins version + hashes.
- Shipped to the app via `toolchain/tauri.conf.json` resources (`ciadpi.exe`
  next to `unidtk.exe`) and copied next to dev builds by `toolchain/build.rs`
  (`copy_native_binaries`).
- Declared as an Optional dependency in the telegram module manifest
  (`telegram:byedpi`, probe `ciadpi.exe --help`); `udtk doctor -env/-modules`
  resolves it through `unidtk_api::paths`.
