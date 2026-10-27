# sing-box (extended fork) — bundled VPN core

Upstream sing-box cannot dial **XHTTP** (Xray's `xhttp`/`splithttp` transport).
The user's subscription contains XHTTP servers, so we bundle a fork that can.

- Source: https://github.com/shtorm-7/sing-box-extended
- Release: `v1.14.0-extended-2.7.1` (asset: `sing-box-1.14.0-extended-2.7.1-windows-amd64.zip`)
- Asset sha256 (zip): `5a1e40d2e4ae6648d141f9bf048fdc9a9e133fe679c3e4a58b5ad6b79faf2707`
- Binary sha256 (`sing-box.exe`): `d4fe7f5e0b5e6a2e4250aed33e083cb01e9a80432063b3f714c9d5ec4e480864`

## Why a fork

- Upstream sing-box (1.12.12 installed with Happ; docs as of 1.14) supports
  HTTP / WebSocket / QUIC / gRPC / HTTPUpgrade transports — **no xhttp**.
  Verified empirically: `sing-box check` → `unknown transport type: xhttp`.
- The fork adds `"transport": { "type": "xhttp", ... }`. Verified live against the
  subscription's XHTTP server: `curl --socks5 → HTTP 204`.

## Usage notes

- `x_padding_bytes` is mandatory in the fork's xhttp transport (padding cannot be
  disabled): use `"100-1000"`.
- The plain `sing-box.exe` from the `.zip` runs unelevated. The `SFW-*.exe` release
  variant ships a `requireAdministrator` manifest and is **not** used here.
- The binary is gitignored (89 MB); this README pins version + hashes.

## Live verification (2026-09-11, real subscription)

| Transport | Result |
|---|---|
| tcp + reality + `xtls-rprx-vision` (#11, default server) | HTTP 204 |
| xhttp + reality (Обход#13) | HTTP 204 |
| xhttp #9 (СПб) | server-side failure — identical via xray |
| gRPC servers | server-side failure — identical via xray |
