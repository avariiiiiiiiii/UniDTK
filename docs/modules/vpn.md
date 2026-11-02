# The VPN Module: From a Subscription Link to a Running Tunnel

This chapter explains the `vpn` module step by step. It shows how a subscription link becomes a sing-box configuration, why an elevated helper process is needed, how the app waits for the old tunnel to disappear before it starts a new one, and what happens at shutdown. The last part keeps the earlier guide for the third-party Happ Desktop client, because this module has to coexist with Happ.

Every mechanism here is taken from the code. Line numbers refer to the files as they are in this repository.

## What you will understand by the end

- Why the module needs a second process, and why that process is a GUI-subsystem binary.
- How a subscription is decoded (base64 or plain text) and parsed into servers.
- What the generated sing-box configuration routes, and in which order.
- How connect, disconnect, and shutdown wait for the elevated helper, and what the timeouts are.
- How auto-connect and the shutdown budget work.

## Files involved

| File | Role |
| :--- | :--- |
| [modules/vpn/rust/vpn.rs](../../modules/vpn/rust/vpn.rs) | The module: subscription, configuration, connect, disconnect, status, shutdown (2,240 lines). |
| [modules/vpn/helper/main.rs](../../modules/vpn/helper/main.rs) | `vpn-helper.exe`: starts and stops `sing-box.exe` with the rights of its scheduled task (207 lines). |
| [scheduler-tasks/UniDTKSingboxTask.xml](../../scheduler-tasks/UniDTKSingboxTask.xml) | Task `UniDTKSingbox`, which runs `vpn-helper.exe start`. |
| [scheduler-tasks/UniDTKVpnStopTask.xml](../../scheduler-tasks/UniDTKVpnStopTask.xml) | Task `UniDTKVpnStop`, which runs `vpn-helper.exe stop`. |
| [core/api/src/paths.rs](../../core/api/src/paths.rs) | Where binaries and state files live. |
| [core/kernel/main.rs](../../core/kernel/main.rs) and [core/kernel/shutdown.rs](../../core/kernel/shutdown.rs) | App shutdown and the Windows session-end listener. |

Binaries live next to the program (`sing-box.exe`, `vpn-helper.exe`), in the install directory returned by `paths::install_dir()`. Mutable state lives in the data directory, `%LOCALAPPDATA%\UniDTK`:

| State file | Written by | Meaning |
| :--- | :--- | :--- |
| `vpn_settings.json` | the module | Subscription URL, auto-connect flag, last server tag, home-server settings. |
| `vpn_servers.json` | the module | Cache of the parsed server list. |
| `singbox_config.json` | the module | The configuration sing-box runs with. |
| `singbox.log` | sing-box | Log file of the running core. |
| `vpn_core.pid` | the helper | PID of the running `sing-box.exe`. |
| `vpn_stop.cutoff` | the module | The stop request: a nonce on line 1. |
| `vpn_stop.done` | the helper | The nonce of the stop that has finished. |
| `logs/vpn_helper.log` | the helper | Log of helper actions. |

---

## Step 1: Why there are two processes

A TUN adapter, which is a virtual network interface that takes over all traffic, needs administrator rights to create. The app itself must stay unelevated. The comment at the top of `vpn.rs` gives the reason: an elevated wallpaper window cannot be reparented under the desktop's `WorkerW` window, because of Windows' privilege isolation (UIPI).

So the module does not elevate itself. It asks Windows Task Scheduler to run a pre-registered task that has the highest available rights:

```rust
const SINGBOX_TASK: &str = "UniDTKSingbox";
const VPN_STOP_TASK: &str = "UniDTKVpnStop";
```

Each task runs `vpn-helper.exe` hidden, with a fixed argument:

```xml
<Hidden>true</Hidden>
<Command>@INSTALL_DIR@\vpn-helper.exe</Command>
<Arguments>start</Arguments>
```

The `@INSTALL_DIR@` placeholder is replaced when the task is registered. The principal is `HighestAvailable`. The app starts a task with `schtasks /run /tn <name>`, which does not require the app itself to be elevated once the task exists:

```rust
fn task_start(task_name: &str) -> Result<(), String> {
    let output = schtasks(&["/run", "/tn", task_name])?;
    // ... logs stdout and stderr, fails if the exit code is not success
}
```

The helper has two modes. `start` launches sing-box. `stop` kills it. Everything else the module does runs without elevation.

---

## Step 2: From subscription text to server list

### 2.1 Fetching

`fetch_servers` downloads the subscription with `ureq`, using the User-Agent `v2box/1.0` and a 15-second timeout. It then decodes the body and keeps only lines that start with `vless://` or `trojan://`. If nothing usable remains, it fails with `no usable VLESS/Trojan servers found in subscription`.

### 2.2 Decoding: plain text or base64

Subscriptions come in two shapes. Some providers send the list of links as plain text, others send the same text encoded in base64. `decode_subscription` handles both, in this order:

```rust
fn decode_subscription(raw: &str) -> Result<String, String> {
    let body = raw.trim();
    if body.lines().any(|l| l.trim_start().starts_with("vless://") || l.trim_start().starts_with("trojan://")) {
        return Ok(body.to_string());
    }
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(body))
        .map_err(|e| format!("subscription decode failed: {e}"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
```

The steps are:

1. If any line already looks like a link, the body is plain text and is used as it is.
2. Otherwise the body is decoded as standard base64.
3. If standard base64 fails, the body is decoded as URL-safe base64.
4. If both fail, the function returns an error. An HTML error page therefore fails cleanly.

The unit tests in `vpn.rs` pin these three cases: `plain_text_subscription_is_taken_as_is`, `base64_subscription_is_decoded`, and `garbage_is_an_error`.

### 2.3 Parsing one link

A VLESS link has the form `vless://<uuid>@<host>:<port>?<query>#<name>`. `parse_vless_line` splits it in this order:

- The part after `#` becomes the server tag. If it is empty, the tag is `host:port`.
- The query gives `type` (transport, default `tcp`), `fp` (fingerprint, default `chrome`), `path` (default `/`), `serviceName`, `flow`, and `sid`.
- Security is decided from the query. If `pbk` is present or `security=reality`, the security is `reality`. Otherwise, if `sni` is present or `security=tls`, it is `tls`. Otherwise it is `none`.

A Trojan link is parsed the same way, except that the password takes the place of the UUID. Its default security is `tls`, and its SNI defaults to the host name.

The parsed list is kept in `vpn_servers.json`, so a restart does not need the network to show servers.

---

## Step 3: The sing-box configuration

`build_singbox_config` builds one JSON document. Each part does one job, and the order of the rules matters, because sing-box checks rules from the first to the last.

### 3.1 Inbounds: the TUN and the local SOCKS port

```rust
"inbounds": [
    {
        "type": "tun",
        "tag": "tun-in",
        "interface_name": TUN_NAME,          // "wallpaper-tun"
        "address": ["172.19.0.1/30"],
        "mtu": 1280,
        "auto_route": true,
        "strict_route": true,
        "stack": "mixed"
    },
    {
        "type": "socks",
        "tag": "socks-in",
        "listen": "127.0.0.1",
        "listen_port": SOCKS_PORT            // 12908
    }
]
```

- The TUN inbound takes over the system's routes (`auto_route`) and blocks traffic that would bypass it (`strict_route`). The MTU is 1280, a deliberately small value; the comment says large TLS packets otherwise stall page loads.
- The SOCKS inbound on `127.0.0.1:12908` is the signal the app uses to know the tunnel is alive. The readiness check, the status watchdog, and the transport probe all talk to it.

### 3.2 Outbounds

The `proxy` outbound comes from `build_singbox_outbound`. It maps the subscription's transport to sing-box's transport: `tcp` means no transport, `ws` and `grpc` map directly, and `xhttp` or `splithttp` map to the `xhttp` transport of the bundled sing-box fork, which requires a padding range.

`direct` is a plain outbound. If the home-server option is enabled, a third outbound `lan-home` is added: a VLESS connection with REALITY to the user's own server.

### 3.3 DNS

DNS goes through the proxy, over TCP:

```rust
"servers": [{ "type": "tcp", "tag": "dns-proxy", "server": "8.8.8.8", "detour": "proxy" }]
```

The comment explains both choices. Plain UDP DNS to 8.8.8.8 is often blocked or hijacked, so names stop resolving and the tunnel looks broken. Routing DNS through the proxy also avoids a DNS leak. TCP is used because UDP over VLESS is slow to warm up.

### 3.4 Route rules

The rules are checked in this order:

1. `process_name: sing-box.exe` goes to `direct`. This is the self-bypass rule: sing-box's own connections must never enter the TUN, or it would route itself into its own tunnel.
2. `action: sniff` reads the domain name from the connection, so that domain rules work.
3. `action: hijack-dns` takes DNS queries into sing-box's own DNS.
4. `domain_suffix: [".ru"]` goes to `direct`. Russian domains use the normal network.
5. If the home server is enabled, `ip_cidr: [<subnet>]` goes to `lan-home`. The default subnet is `192.168.31.0/24`.

Everything else goes to `final: "proxy"`.

### 3.5 Validation before use

The configuration is written as pretty JSON to `singbox_config.json`, and then `validate_singbox_config` runs `sing-box check -c <file>`. If sing-box rejects it, the first line of its stderr becomes the error message: `sing-box rejected the config: ...`. A broken configuration never reaches the helper.

---

## Step 4: The stop protocol: nonce and done marker

Stopping sing-box takes place in another process, in another security context, and it is asynchronous. The module therefore needs a way to know that *this* stop has finished, and not an earlier one.

### 4.1 The request

`perform_stop` does four things:

1. It builds a nonce from the app's PID and a counter: `"<pid>-<n>"`.
2. It deletes `vpn_stop.done`, so no marker from an earlier stop can be mistaken for this one.
3. It writes `vpn_stop.cutoff`: line 1 is the nonce, line 2 is the time of the request in RFC 3339 format.
4. It starts `UniDTKVpnStop`. If the helper is missing, it falls back to ending the start task.

The helper reads only line 1 of the cutoff file. Line 2 is written but not used by the helper.

### 4.2 The helper's side

`stop()` in the helper reads the nonce and then:

- Reads the PID from `vpn_core.pid`.
- Checks that the process at that PID really is `sing-box.exe`. The check asks Windows for the image path (`QueryFullProcessImageNameW`) and tests whether it ends with `sing-box.exe`. If the PID has been reused by another program, it is not killed.
- Terminates the process and waits at most 2,000 ms for it to exit.
- Removes `vpn_core.pid`.
- Writes the nonce to `vpn_stop.done`. This happens even when nothing was killed, because the app's waiter treats the marker as "the stop has run".

### 4.3 The waiter

`wait_until_stopped` polls every 150 ms until all three conditions are true:

1. the done marker contains this stop's nonce (or there is no nonce to wait for),
2. nothing listens on `127.0.0.1:12908`,
3. the `wallpaper-tun` interface is gone.

The comment in the code explains why the marker is needed. Checking the port alone is not enough, because it also passes when there was nothing to stop, which is the usual case after an app restart. The start would then run while the old kill was still in flight, and the kill would land on the new sing-box.

---

## Step 5: Bringing the tunnel up

`connect()` and `bring_up()` split the work so that the lock on shared state is never held during the slow parts.

### 5.1 `connect()`: reserve, then work unlocked

`connect()` holds the runtime lock only long enough to:

1. refuse if a connect is already running (`connecting`),
2. choose the server: the requested tag, else the selected tag, else the last tag, else the default tag, else the first server,
3. read the current `CONNECT_EPOCH` and set `connecting = true`.

It then calls `bring_up()` without the lock. The comment says the waits can take tens of seconds, and status polling must stay responsive during them.

When `bring_up()` returns, `connect()` takes the lock again and checks the epoch. `disconnect()` and `shutdown()` increment `CONNECT_EPOCH`. If the epoch changed, the connect was cancelled while it ran, and its result must not overwrite the newer state. In that case the connect stops any processes it may have started.

If the epoch is unchanged, `connect()`:

1. sets `connected = true` and the atomic `CONNECTED` flag,
2. publishes the new state to other modules (Step 10),
3. saves `last_tag` in the settings,
4. starts a detached thread that checks the exit IP (Step 5.3).

### 5.2 `bring_up()`: the sequence

`bring_up()` runs these steps in order:

1. `ensure_binaries_present()` checks that `sing-box.exe` and `vpn-helper.exe` exist.
2. A thread reads the current public IP without the tunnel. This is the baseline for later comparison. It runs in parallel with everything below.
3. `perform_stop()` fires the stop protocol, and `wait_until_stopped(10 s)` waits for it. If the wait times out, the connect fails with `previous VPN processes did not shut down in time`.
4. `active_happ_tun_present()` refuses to continue if a Happ tunnel is active (Step 7).
5. If port 12908 is still open, the connect is refused.
6. The configuration is built, written, and validated (Step 3).
7. Happ is checked a second time, because the steps above can take several seconds.
8. `task_start("UniDTKSingbox")` starts sing-box through the helper.
9. The module polls for the `wallpaper-tun` interface: 20 checks, 250 ms apart. That is at most about 5 seconds. If the interface never appears, it logs the last lines of `singbox.log`, runs `perform_stop()`, and returns an error. The error says whether Happ is the likely cause.
10. The module polls for port 12908 the same way, 20 checks, 250 ms apart.
11. `probe_vpn_transport()` sends an HTTPS request through the SOCKS port to `https://1.1.1.1/`, then to `https://8.8.8.8/` if the first fails (4-second timeout each). It uses IP literals on purpose, because DNS over the tunnel may not be warm yet. Any HTTP response, even a 4xx, counts as proof that the transport works. A failure is recorded as a warning, not as a failed connect. The comment says the tunnel counts as up once the TUN interface exists.

### 5.3 The egress check

Connecting does not wait for the IP check. After `connected = true` is committed, a detached thread joins the baseline thread, reads the public IP through the proxy, and compares the two:

- If the IPs differ, the log says `public ip changed ... (egress verified)`.
- If they are equal, `ip_unverified` is set and the UI can show that the exit was not confirmed as different.
- If either reading fails, the result is marked inconclusive.

The check is written back only if the epoch has not changed, so a fast reconnect cannot be overwritten by an old result.

---

## Step 6: The helper binary

The helper is a small Rust program, not a script. Its first line is the crate attribute that makes it a GUI-subsystem binary:

```rust
#![windows_subsystem = "windows"]
```

With this attribute Windows does not create a console window for the process, so a hidden task cannot flash a black window. The header comment of `helper/main.rs` gives the history: the old chain was `wscript.exe` → a VBS file → `powershell.exe` → a `.ps1` script. Starting that chain used WMI and .NET and took seconds, which did not fit into the time Windows allows for shutdown. The Rust helper starts and stops in milliseconds.

### 6.1 `start`

```rust
match Command::new(&exe)
    .args(["run", "-c"])
    .arg(&config)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
    .spawn()
```

The helper starts sing-box detached, writes its PID to `vpn_core.pid`, and exits at once. The comment says why it does not stay resident: the core must outlive the helper, and a resident helper would keep the `UniDTKSingbox` task marked as running forever.

Before it spawns, `start` checks whether the recorded PID is still a sing-box process. If it is, the helper does nothing and returns success.

### 6.2 `stop`

As described in Step 4. The helper always writes the done marker, because a hung kill would otherwise stall every connect and disconnect for its full timeout.

### 6.3 `probe`

The modes `probe`, `--probe`, and `check` return 0 at once. They let the app check that the binary runs.

---

## Step 7: Coexisting with Happ Desktop

Happ is a different VPN client that also creates a TUN adapter. Two TUN adapters would fight over the same routes. The module therefore checks for Happ before it starts.

```rust
fn active_happ_tun_present() -> Result<bool, String> {
    interface_is_connected("happ")
}
```

The check runs `netsh interface show interface` and looks for any line that contains `happ` and does not say disconnected. The match is a substring on purpose. The comment explains that Happ uses different adapter names depending on its mode (`happ-default-tun` for one engine, `happ-tun` for the other), so an exact name would go stale when the user switches modes.

The check runs twice: before the helper is started, and again just before the TUN is claimed. If Happ appears in between, the connect is aborted, and the error tells the user to fully quit Happ rather than only toggling it off.

---

## Step 8: Auto-connect at startup

When the module is enabled, `init` calls `auto_connect_on_startup`. That function runs on its own thread:

1. It stops if `auto_connect` is off in the settings.
2. It stops if there is no subscription URL.
3. It takes the target server: `last_tag` if set, otherwise `default_tag`.
4. It tries `connect()` up to three times. Before each try it checks again that `auto_connect` is still on and that `MANUAL_DISCONNECT` is not set. Between failed tries it sleeps for 2 seconds.

Two flags are kept apart on purpose. `auto_connect` is the user's preference, "reconnect on next launch". `MANUAL_DISCONNECT` is process-local: a manual disconnect stops an auto-connect that is still retrying, but it does not change the preference. A manual connect does not change the preference either.

---

## Step 9: The shutdown budget

The kernel's `app_shutdown(app, wait_for_vpn)` has two modes, and each one gives the VPN a different amount of time:

| Caller | Call | What the VPN does |
| :--- | :--- | :--- |
| The `quit_app` command, and the Windows session-end message `WM_ENDSESSION` (see [core/kernel/shutdown.rs](../../core/kernel/shutdown.rs)) | `app_shutdown(app, true)` → `stop_with_budget(true)` | Waits up to 2 seconds for the helper. |
| The exit request of the main event loop ([core/kernel/main.rs](../../core/kernel/main.rs), around line 2568) | `app_shutdown(app, false)` → `Module::shutdown(1,500 ms)` | Does not wait. `VpnModule::shutdown` ignores the budget and fires the stop without awaiting it. |

The main loop must never sleep, which is why it passes `false`. The source comment says so directly.

The waiting path is `shutdown_and_wait(runtime, 2 s)`. It first checks whether anything is up: if port 12908 is closed and the TUN is gone, it returns at once, and no scheduled task is run. Otherwise it fires the stop and waits up to 2 seconds for the done marker, and for the port and TUN to disappear.

The 2-second wait exists because of the session-end case. A window that is a child of `WorkerW` never receives `WM_ENDSESSION`, so [core/kernel/shutdown.rs](../../core/kernel/shutdown.rs) creates a hidden top-level window on its own thread for that message. The file states the reason: if the app dies while sing-box keeps its TUN up, a strict route can black-hole the network after the next boot.

---

## Step 10: Telling other modules about the state

The VPN module does not call other modules. When the state changes, `publish_vpn_state` emits an event on the contract bus:

```rust
fn publish_vpn_state() {
    unidtk_api::emit("vpn", json!({ "connected": is_connected() }));
}
```

The Telegram module subscribes to `vpn` at startup and reads the last value with `unidtk_api::state("vpn")`. This keeps the two modules independent of each other: Cargo does not allow a dependency from one module to the other. The same mechanism is described in [docs/architecture.md](../architecture.md).

---

## Appendix A: Happ Desktop for Windows (third-party client)

The rest of this chapter is the earlier guide for configuring the Happ Desktop client. The module does not read these settings. It only refuses to start while a Happ TUN is active (Step 7). The guide is kept because users who run both clients need it.

> The Happ interface is in Russian. Labels are given in English, and the original Russian label is shown in parentheses in each path.

### A.1 Architecture concept (split tunneling)

The goal is to separate network flows:

1. **Direct traffic (Direct / Bypass):** Russian resources (banks, Gosuslugi government services, marketplaces Ozon/WB, Yandex services, VK, Kinopoisk, local networks) go directly through the home ISP at maximum speed, with no blocking based on a foreign IP.
2. **Proxied traffic (Proxy):** blocked or throttled services (YouTube, Discord, Instagram, X/Twitter, foreign resources) are routed through the protected tunnel (VLESS / Reality / Trojan / Shadowsocks).

### A.2 System requirements and TUN mode

- **Run as administrator:** Happ creates a virtual adapter (Wintun). In the properties of the `happ.exe` shortcut (*Compatibility* tab), enable *"Run this program as an administrator"*.
- **System proxy:** `Off` (must not run in parallel with TUN).
- **TUN:** `On` (intercepts all TCP/UDP traffic of the system, including games and consoles).
- **TUN provider:** `sing-box`.
- **TUN mode:** `Mixed`. **TUN configuration:** `Default`.
- **MTU:** `1400` is recommended if connections drop with large packets. The module's own sing-box uses 1280 (Step 3.1). These are two different clients with two different settings.

### A.3 Routing rules (Routing / Direct RU)

Path: `Settings` (Настройки) → `Routing rules` (Правила маршрутизации) → `geo` → `Edit rules` (Редактировать правила) → `Direct`.

All of these must be in the **Direct URL or IP** field:

```text
# Local subnets (LAN)
10.0.0.0/8
172.16.0.0/12
192.168.0.0/16
169.254.0.0/16
224.0.0.0/4
255.255.255.255

# Russian IP ranges and services
geoip:ru
geosite:category-ru
geosite:ru

# Russian domain zone masks
regexp:.*\.ru$
regexp:.*\.рф$
```

- `geoip:ru`: all IP addresses registered in Russia.
- `geosite:category-ru`: a preset list of Russian services (banks, marketplaces, government services, streaming).
- `geosite:ru`: the domain database for the `.ru` zone.
- `regexp:.*\.ru$` and `regexp:.*\.рф$`: any domain under those top-level zones.
- Final action (Default Outbound) for unmatched traffic: **`Proxy`**.

### A.4 DNS

Path: `Settings` (Настройки) → `Routing rules` (Правила маршрутизации) → `geo`.

- **Enable Fake DNS:** `Off`.
- **Domain strategy:** `IPIfNonMatch`. The client first matches the name by geosite, then resolves the IP and checks it against geoip. This is the optimal mode.
- **Remote DNS:** type `DoH`, URL `https://cloudflare-dns.com/dns-query` (or `https://1.1.1.1/dns-query`), IP `1.1.1.1`. Requests to foreign sites are encrypted and pass inside the tunnel.
- **Local DNS:** type `DoH` or `UDP`, URL `https://dns.yandex.ru/dns-query`, IP `77.88.8.8`. Do not use Google DNS (`8.8.8.8`) for local traffic in Russia: it often returns non-optimal foreign CDN nodes.
- **Other settings** (Другие настройки): Use local DNS `On`; Use DNS from JSON `Off`; Enable server resolving `On`; DNS IP for server resolving `77.88.8.8`; Enable DNS for TUN `On`; DNS address for TUN `1.1.1.1`.

### A.5 Advanced core parameters (Anti-DPI / Xray)

Path: `Settings` (Настройки) → `Advanced settings` (Расширенные настройки).

| Parameter | Value | Rationale |
| :--- | :--- | :--- |
| **Enable fragmentation** | `On` | Splits the first TLS handshake packet to bypass SNI filtering. |
| **Type** | `Xray` | The standard fragment handler. |
| **Fragment packets** | `tlshello` | Splits only the `ClientHello` packet. |
| **Fragment length (min-max)** | `50-100` | Optimal chunk size. |
| **Fragment interval** | `10-20` | Delay in ms between parts. |
| **Max fragment split** | `100-200` | Split boundary. |
| **Enable noise** | `Off` | Requires server-side support. |
| **Enable multiplexer (Mux)** | `Off` | Mux worsens UDP ping (Discord, games) and cuts video speed. |
| **Preferred IP type** | `IPv4` | Avoids delays from non-working IPv6 routes. |
| **Packet sniffing** | `On` | Required to recognize domains inside TUN traffic and for geosite to work. |

### A.6 Applying changes and verification

1. On the Happ main screen, click **Disconnect**.
2. Click **Connect** to rebuild the virtual adapter and the routing table.
3. Restart the web browser to reset the local socket and DNS cache.

Verification:

- **Russian segment:** open `2ip.ru`. The real IP and the Russian ISP should show. Gosuslugi and bank sites should open without a captcha.
- **Foreign segment:** open YouTube or Discord. Traffic should go through the IP of the VPN server.

### A.7 Troubleshooting

- **TUN fails to start / adapter is not created:** run Happ as administrator, and check whether the antivirus or Windows Defender blocks the Wintun device.
- **Conflicts with GoodbyeDPI / Zapret / ByeDPI:** do not run Happ in TUN mode at the same time as utilities based on the WinDivert driver.
- **Discord voice does not work:** make sure **TUN** mode is enabled. Discord voice uses UDP, and a regular system proxy does not intercept it.

---

## Summary

- The app runs as the normal user. The tiny `vpn-helper.exe`, started through Task Scheduler, starts and stops the elevated `sing-box.exe`, which is the process that creates the TUN.
- A subscription is either plain text or base64; both are accepted, and anything else is an error.
- The sing-box configuration sends Russian domains and the helper's own traffic directly, and everything else through the proxy. DNS goes through the proxy over TCP. The configuration is checked with `sing-box check` before it is used.
- Stop requests carry a nonce. The helper writes it back, and the app waits for that nonce, the closed port, and the vanished TUN, so an old stop can never be mistaken for the current one.
- Shutdown waits at most 2 seconds, and only on the paths that must not leave a TUN behind.
