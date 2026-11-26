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
| [modules/vpn/rust/settings.rs](../../modules/vpn/rust/settings.rs) | Settings updates, legacy import and the subscription cache. |
| [modules/vpn/helper/main.rs](../../modules/vpn/helper/main.rs) | `vpn-helper.exe`: starts and stops `sing-box.exe` with the rights of its scheduled task. |
| [modules/vpn/native/lib.rs](../../modules/vpn/native/lib.rs) | The native half: the VPN panel on the Rune desk, in its own card and compact in a host's nesting zone ([shell.widget-contract](../reference/shell.md#r-shell.widget-contract)). |
| [modules/vpn/native/settings.rs](../../modules/vpn/native/settings.rs) | The native menu pane for the subscription, auto-connect, and the home server. |
| [scheduler-tasks/UniDTKSingboxTask.xml](../../scheduler-tasks/UniDTKSingboxTask.xml) | Task `UniDTKSingbox`, which runs `vpn-helper.exe start`. |
| [scheduler-tasks/UniDTKVpnStopTask.xml](../../scheduler-tasks/UniDTKVpnStopTask.xml) | Task `UniDTKVpnStop`, which runs `vpn-helper.exe stop`. |
| [core/api/src/paths.rs](../../core/api/src/paths.rs) | Where binaries and state files live. |
| [core/kernel/main.rs](../../core/kernel/main.rs) and [core/kernel/shutdown.rs](../../core/kernel/shutdown.rs) | App shutdown and the Windows session-end listener. |

On the Rune desk, open Settings → Desk → VPN → Configure VPN. The pane loads the stored values before showing the form. It sends one write at a time and changes the switches when the kernel acknowledges the command. The home server switch sends the last confirmed configuration with the new `enabled` value. Save Server sends the form's edits. A blank port means 443 and a filled port must be in 1–65535.

The VPN stores settings and the server cache in its `ModuleDb` slice. Each settings update reads the latest record and commits under one module lock. A command reports a storage error to the caller. The saved subscription URL and the empty server cache share one redb transaction when the URL changes. List, refresh, ping and connect use a subscription revision to identify their source. A fetch or ping that finishes after a subscription change returns a retryable error. The kernel reserves a connection against the current revision before starting the stop and start sequence with its settings snapshot. Asking to connect to the active tag and revision leaves the running tunnel in place.

The Rune panel reads `subscription_revision` from `vpn_status` on open and on its three-second poll while visible. A new revision closes the picker, discards its previous tags and latency measurements, and prompts a visible panel to fetch the current list. List, ping and connect requests from Rune pass the revision in `subscriptionRevision`. The kernel checks it before acting, and the panel ignores replies for revisions it has discarded. Older clients can omit the argument and continue to receive the existing response shapes.

Binaries live next to the program (`sing-box.exe`, `vpn-helper.exe`), in the install directory returned by `paths::install_dir()`. Mutable state lives in the data directory, `%LOCALAPPDATA%\UniDTK`:

| State file | Written by | Meaning |
| :--- | :--- | :--- |
| `vpn_settings.json` | legacy import | Subscription URL, auto-connect flag, last server tag and home-server settings. The module imports it when the redb record is absent and removes the file after commit. |
| `vpn_servers.json` | legacy import | Cache of the parsed server list. An explicit empty redb list supersedes this file after a subscription change. |
| `singbox_config.json` | the module | The configuration sing-box runs with. |
| `singbox.log` | sing-box | Log file of the running core. |
| `vpn_core.pid` | the helper | PID of the running `sing-box.exe`. |
| `vpn_stop.cutoff` | the module | The stop request: a nonce on line 1. |
| `vpn_stop.done` | the helper | The nonce of the stop that has finished. |
| `logs/vpn_helper.log` | the helper | Log of helper actions, one timestamped line each; moved to `.1` past 1 MB. |
| `logs/singbox_stderr.log` | sing-box | What the core prints outside its own log (a panic); moved to `.1` past 1 MB. |

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

### 2.0 The link a person pastes

`vpn_set_subscription_url` stores the address, not whatever was pasted. `subscription::normalize_link` takes an `http(s)` address as it is and unwraps an app's own link to the address it carries: the value of `url=` (percent-encoded, as Clash and sing-box write it; a `+` stays a plus, because tokens carry it) or a path that continues with the address (`happ://add/<url>`, `incy://add/<url>`, `v2raytun://import/<url>`, `hiddify://import/<url>`). The fragment (`#name`) is the app's label and is dropped. It refuses, with a Russian reason that never repeats the link (the link is a credential): an encrypted link (`happ://crypt…`, `incy://crypt…`, opened only by its own app), a link wrapped twice, a user and password in the address, and an address on this computer (loopback, unspecified, link-local). A home server on the local network stays allowed. The settings field then shows what was stored.

### 2.1 Fetching

`subscription::fetch` downloads the subscription with `ureq`, using the User-Agent `v2box/1.0` (it selects the base64 list; a Clash User-Agent gets YAML), a 15-second timeout and a 4 MiB cap on the body. It sends the device headers a provider's device limit counts (Remnawave, [docs.rw, HWID device limit](https://docs.rw/docs/features/hwid-device-limit/)): `x-hwid`, `x-device-os: Windows`, `x-ver-os` (from the registry's `CurrentVersion`), `x-device-model` (the BIOS product name, printable ASCII, "PC" for a firmware placeholder). `x-hwid` (`hwid.rs`) is the first 32 hex characters of SHA-256 over the machine's `MachineGuid` (read from the 64-bit registry view), a fixed salt and the provider's host, so two providers cannot tell they serve one machine and the id survives a reinstall. When the machine's id cannot be read, a random id made once and kept in the module's store (`device_secret`) takes its place, never a hash of nothing. The settings pane shows the id under the link, so the person can find this device in the provider's list. Without the headers, the user's provider answered with three placeholders and `X-Hwid-Not-Supported: true`; with them, with `X-Hwid-Max-Devices-Reached: true` (session 152).

The answer is split into servers and the provider's notes (`subscription::Notes`): its `profile-title` and `announce` headers (plain or `base64:` UTF-8), and its placeholders, entries that cannot be dialled (`0.0.0.0`, `::`, port 0 or 1) or carry an all-zero id, whose names are the provider's message ("❌ Ваше приложение не поддерживается", "📱 Лимит устройств достигнут"). `x-hwid-max-devices-reached` or `x-hwid-not-supported` becomes the notes' refusal. Placeholders never enter the server list, so a connect, the check and the fallback to the first server never pick one. A provider's answer is accepted even with no servers: only a transport failure is an error, and only a transport failure falls back to the cached list (a provider that now refuses this device would otherwise be hidden behind servers that no longer let it in). An answer marks the revision as fetched, so a list of placeholders is not fetched again on every call; the panel's "ОБНОВИТЬ" (`vpn_refresh_servers`) asks again. A connect or a check with no servers answers with the refusal or the first note. Every URL in a fetch error is cut back to its scheme and host, a `user:password@` included (`subscription::redact_urls`), before it is logged or returned.

`vpn_subscription_notes` (read_ui) answers the notes for the current revision and the device id for its provider, without the address. `vpn_list_servers` is served on the socket and answers `ServerView` records: tag, protocol, host, port, transport and security, never the id, password or keys, which stay in the module and its cache. The panel draws the refusal and the notes as rows nobody can pick, above the servers; with no servers, its button reads "ОБНОВИТЬ" instead of "ПРОВЕРИТЬ ВСЕ", and its picker shows the provider's words instead of "Выберите сервер". The subscription field and the home server's id and keys are secure fields shown in plain text (`.secure().revealed(true)`): `udtk host tree` cannot read them, and they do not copy or undo. The page (WebView2) is frozen until G5 and shows none of this.

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

The parsed list is kept in the module's store (`servers_cache`), so a restart does not need the network to show servers.

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
        "address": [<a free /30>],          // 172.19.0.1/30 unless another client holds it
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

The TUN address is not a constant any more (#538). `172.19.0.1/30` was the address for every connect, and Happ's `happ-xray` adapter holds exactly that subnet while it runs: an elevated sing-box then died with `set ipv4 address: The object already exists`. `pick_tun_address` now runs per connect, after the previous adapter is gone, and takes the first /30 that overlaps nothing the machine already reaches: every adapter's on-link prefix and every route's real subnet (longer than /8 and shorter than /31), from `occupied_v4_ranges` (`GetAdaptersAddresses` and `GetIpForwardTable2`). The scan starts at `172.19.0.0/30`, then the rest of its /16, then `172.16/12`, `10/8` and `192.168/16`, and stops after 2,048 candidates in each range. The home server's own subnet is excluded too, because `strict_route` would take it from its split-tunnel outbound. The readiness loop reads the address back from the adapter (`netsh interface ipv4 show addresses`): a leftover `wallpaper-tun` from a dead core answers "connected", and only the chosen address on it proves that this run is up.

### 3.2 Outbounds

The `proxy` outbound comes from `build_singbox_outbound`. It maps the subscription's transport to sing-box's transport: `tcp` means no transport, `ws` and `grpc` map directly, and `xhttp` or `splithttp` map to the `xhttp` transport of the bundled sing-box fork, which requires a padding range.

Its `server` is an IPv4 literal, resolved once per connect by `resolve_ipv4` (a 4-second bound on a thread of its own, alongside the stop). The domain stays in `tls.server_name` and in the ws `Host` header, so REALITY and SNI still see the name the link asked for. The reason is in 3.3. A host that does not resolve is an honest error, and no configuration is written.

`direct` is a plain outbound. If the home-server option is enabled, a third outbound `lan-home` is added: a VLESS connection with REALITY to the user's own server.

### 3.3 DNS

DNS goes through the proxy, over TCP:

```rust
"servers": [{ "type": "tcp", "tag": "dns-proxy", "server": "8.8.8.8", "detour": "proxy" }]
```

The comment explains both choices. Plain UDP DNS to 8.8.8.8 is often blocked or hijacked, so names stop resolving and the tunnel looks broken. Routing DNS through the proxy also avoids a DNS leak. TCP is used because UDP over VLESS is slow to warm up.

This server now serves only the user's traffic. As long as the outbound's `server` was the proxy's own hostname, sing-box had to resolve that hostname through this very server, whose detour is the outbound waiting for it. It detected the cycle and every dial failed with `DNS resolution loop detected: dns-proxy -> dns-proxy`, while the TUN and the SOCKS port came up and the connect reported success (#538). The literal removes the cycle; the entry is still needed for everything the user's own traffic looks up.

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

The helper reads both lines. Line 1 is the nonce. Line 2 tells it who asked: a request written within the last 15 seconds is logged as `requested by the app, nonce <pid>-<n>`, an older one or none as `NOT requested by the app`. That is how a stop someone ran by hand (the task, or the helper itself) is told apart from the app's own (session 151: a stop nobody logged killed a live core).

### 4.2 The helper's side

`stop()` in the helper reads the nonce and then:

- Reads the PID from `vpn_core.pid`.
- Checks that the process at that PID really is `sing-box.exe`. The check asks Windows for the image path (`QueryFullProcessImageNameW`) and tests whether it ends with `sing-box.exe`. If the PID has been reused by another program, it is not killed.
- Terminates the process and waits at most 2,000 ms for it to exit. The log says `terminated` only when `TerminateProcess` succeeded; otherwise it names the Win32 error.
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

One process-wide lifecycle lock (`LIFECYCLE`) orders everything that starts or stops a core: a connect's bring-up and its cleanup, a disconnect's stop, the watchdog's cleanup, a server check (below) and an awaited shutdown. They share the done marker, and the helper's stop kills every bundled sing-box, so two overlapping ones used to fail each other: a stop during a server check killed the check's core, and a disconnect between a connect's epoch check and its start left a tunnel up under an "Off" switch. The commands that take this lock are `long` on the UI door ([ipc.long-commands-off-thread](../reference/ipc.md#r-ipc.long-commands-off-thread)), so waiting for it holds no other command.

**The server check** (`vpn_ping_all`, the picker's "ПРОВЕРИТЬ ВСЕ") starts one sing-box with a SOCKS inbound per server and probes them all at once. Each server has one budget for all its endpoints (`SERVER_CHECK_BUDGET`, 5 s, each request bounded by what is left). The bound has to reach the SOCKS handshake: ureq applies a request's `.timeout()` only after the connection, and the handshake waits on the agent's `timeout_connect` (30 s unless set), so each endpoint gets an agent whose `timeout_connect` is what is left. Before that, a dead server held its probe until sing-box gave up dialling it (France: 15.0 s live). The probes start after the names are resolved (at most 4 s) and the core is up (about 1.5 s), so the whole check ends in about 10 s even when servers are dead; it used to give every endpoint 3.5 s to connect plus 3.5 s to read, six in a row, and one dead server took 20 s. A server whose name does not resolve, or resolves only to IPv6, is not probed: its row reports that reason (`resolve_ipv4` tells the two apart; it used to call a name that no longer exists "IPv6 only"). The check's core is put into a job that closes with the kernel (`dies_with_kernel`), so it cannot outlive a killed kernel. As the page does, the panel then connects to the fastest server, unless the person pressed the switch or picked a server while the check ran.

The subscription fetch never holds the runtime lock: `fetch_into` fetches unlocked (one fetch at a time, `FETCH`) and publishes under the settings revision, so `vpn_status` answers at once during a 15-second fetch.

### 5.1 `connect()`: reserve, then work unlocked

`connect()` holds the runtime lock only long enough to:

1. refuse if a connect is already running (`connecting`),
2. choose the server: the requested tag, else the selected tag, else the last tag, else the default tag, else the first server,
3. read the current `CONNECT_EPOCH` and set `connecting = true`.

It then takes the lifecycle lock and calls `bring_up()` without the runtime lock, with one budget for the whole bring-up (`CONNECT_BUDGET`, 30 s): the old core's stop, the TUN, the SOCKS port and the probe share it.

When `bring_up()` returns, `connect()` takes the lock again and checks the epoch. `disconnect()` and `shutdown()` increment `CONNECT_EPOCH`. If the epoch changed, the connect was cancelled while it ran, and its result must not overwrite the newer state. In that case the connect stops any processes it may have started.

If the epoch is unchanged, `connect()`:

1. sets `connected = true` and the atomic `CONNECTED` flag,
2. publishes the new state to other modules (Step 10),
3. saves `last_tag` in the settings,
4. starts a detached thread that checks the exit IP (Step 5.3).

### 5.2 `bring_up()`: the sequence

`bring_up()` runs these steps in order:

1. `ensure_binaries_present()` checks that `sing-box.exe` and `vpn-helper.exe` exist.
2. A thread reads the current public IP without the tunnel. This is the baseline for later comparison. It runs in parallel with everything below. The proxy's own hostname is resolved on a second thread beside it (`resolve_ipv4`, 4-second bound); its failure is the connect's error, and no configuration is written.
3. `perform_stop()` fires the stop protocol, and `wait_until_stopped` waits for it, at most 10 s of the budget. If the wait times out, the connect fails with `previous VPN processes did not shut down in time`.
4. `active_happ_tun_present()` refuses to continue if a Happ tunnel is active (Step 7).
5. If port 12908 is still open, the connect is refused.
6. A disconnect or shutdown that ran during the wait above ends the connect here: the epoch in `CONNECT_EPOCH` no longer matches, so nothing is started.
7. The TUN address is picked for this run (3.1), the configuration is built, written, and validated (Step 3).
8. Happ is checked a second time, because the steps above can take several seconds.
9. `task_start("UniDTKSingbox")` starts sing-box through the helper.
10. The module polls for the `wallpaper-tun` interface: 20 checks, 250 ms apart. That is at most about 5 seconds. Every check first reads the epoch: a disconnect that arrived meanwhile stops the core this connect started (`abandon`) and ends the connect, so its core cannot outlive the switch even when the disconnect's own stop ran earlier. Each check also reads the adapter's address and accepts only the /30 chosen in step 7. If the interface never appears, it logs the last lines of `singbox.log`, runs `perform_stop()`, and returns an error. The error says whether Happ is the likely cause.
11. The module polls for port 12908 the same way, 20 checks, 250 ms apart.
12. `probe_vpn_transport()` asks through the SOCKS port until one request is answered or 10 s pass (`PROBE_BUDGET`, within the connect's budget). It cycles through IP literals and hostnames, plain HTTP and TLS (`https://1.1.1.1/`, `http://cp.cloudflare.com/generate_204`, `http://1.1.1.1/cdn-cgi/trace`, `https://8.8.8.8/`, `http://www.gstatic.com/generate_204`), each request bounded by what is left: a cold tunnel fails the two IP-literal HTTPS endpoints while a hostname's plain 204 answers, and DNS over the tunnel may not be warm yet. Any HTTP response, even a 4xx, counts as proof that the transport works. When none answers, the connect is refused: the core is stopped and `vpn_status.error` reads `the server carries no traffic: <the last error>`. Before session 151 a failed probe was only a warning, and the switch said "on" while nothing passed.

The reason a connect gave travels to both panels: `vpn_status.error` carries it, the Rune panel draws it under the switch, and the WebView panel writes it into `.vpn-error`. Before #538 nothing drew that field, so a refused connect flipped the switch back without a word.

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

With this attribute Windows does not create a console window for the process, so a hidden task cannot flash a black window. The helper runs elevated but writes into the user's own data directory, so before every write (its log, the PID file, the done marker, the core's start) it refuses a directory that is a junction or a symlink (`safe_dir`): a link there would point an administrator's write anywhere on the disk. The header comment of `helper/main.rs` gives the history: the old chain was `wscript.exe` → a VBS file → `powershell.exe` → a `.ps1` script. Starting that chain used WMI and .NET and took seconds, which did not fit into the time Windows allows for shutdown. The Rust helper starts and stops in milliseconds.

### 6.1 `start`

```rust
match Command::new(&exe)
    .args(["run", "-c"])
    .arg(&config)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(stderr)
    .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
    .spawn()
```

The helper moves `singbox.log` to `.1` past 5 MB (it had reached 15 MB, and the kernel reads its tail after a failed connect; that tail is now the last 64 KB only), sends the core's stderr to `logs/singbox_stderr.log`, starts sing-box detached, writes its PID to `vpn_core.pid`, and exits at once. The comment says why it does not stay resident: the core must outlive the helper, and a resident helper would keep the `UniDTKSingbox` task marked as running forever.

Before it spawns, `start` checks whether the recorded PID is still a sing-box process. If it is, the helper does nothing and returns success.

### 6.2 `stop`

As described in Step 4. The helper always writes the done marker, because a hung kill would otherwise stall every connect and disconnect for its full timeout.

After the recorded kill it sweeps for orphaned cores: every running process whose image is exactly our bundled `sing-box.exe` is terminated (`sweep_orphaned_cores`). A core can outlive its PID file when the helper dies between the spawn and the write, or when the app is killed first; its TUN then stays up with no record left to kill, and every later connect would fail on `previous VPN processes did not shut down in time`. The image path is compared in full, so another program called `sing-box` is never touched.

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
4. It waits, up to 90 seconds (`AUTO_CONNECT_NETWORK_WAIT`), until the subscription's host resolves: at logon the network often comes up after the app, and a connect before it would be refused for carrying no traffic (5.2).
5. It tries `connect()` up to four times, pausing 2, 5 and 10 seconds after a failure. Before each step it checks again that `auto_connect` is still on and that `MANUAL_DISCONNECT` is not set.

Two flags are kept apart on purpose. `auto_connect` is the user's preference, "reconnect on next launch". `MANUAL_DISCONNECT` is process-local: a manual disconnect stops an auto-connect that is still retrying, but it does not change the preference. A manual connect does not change the preference either.

---

## Step 9: The shutdown budget

The kernel's `app_shutdown(app, wait_for_vpn)` has two modes, and each one gives the VPN a different amount of time:

| Caller | Call | What the VPN does |
| :--- | :--- | :--- |
| The `quit_app` command, and the Windows session-end message `WM_ENDSESSION` (see [core/kernel/shutdown.rs](../../core/kernel/shutdown.rs)) | `app_shutdown(app, true)` → `Module::shutdown(2 s)` | Waits up to 2 seconds for the helper. |
| The exit request of the main event loop ([core/kernel/main.rs](../../core/kernel/main.rs), the `RunEvent::ExitRequested` arm) | `app_shutdown(app, false)` → `Module::shutdown(0)` | Does not wait. A zero budget fires the stop without awaiting it. |

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

The Telegram module subscribes to `vpn` at startup and reads the last value with `unidtk_api::state("vpn")`. This keeps the two modules independent of each other: Cargo does not allow a dependency from one module to the other. The same mechanism is described in [docs/architecture.md](../reference/README.md).

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
