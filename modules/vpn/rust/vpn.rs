use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use unidtk_api::paths::vpn_helper_exe;

static VPN_DB: OnceLock<Arc<dyn unidtk_api::ModuleDb>> = OnceLock::new();

const CREATE_NO_WINDOW: u32 = 0x08000000;

const SOCKS_PORT: u16 = 12908;
const TUN_NAME: &str = "wallpaper-tun";

/// Публикует текущее состояние туннеля в контракт.
///
/// Раньше модуль звал `telegram::on_vpn_state_changed()`, и telegram звал
/// `vpn::is_connected()` — замкнутый цикл между двумя модулями. Теперь vpn
/// только объявляет состояние, а telegram подписан на событие: Cargo больше не
/// даст им зависеть друг от друга.
fn publish_vpn_state() {
    unidtk_api::emit("vpn", json!({ "connected": is_connected() }));
}

// unidtk.exe itself must stay unelevated (see build.rs for why —
// an elevated wallpaper window can't be reparented under explorer.exe's
// WorkerW due to UIPI). TUN creation needs admin, so sing-box runs via a
// pre-registered elevated scheduled task; this process just starts/stops it
// with `schtasks`, which doesn't itself require elevation once the task
// exists.
const SINGBOX_TASK: &str = "UniDTKSingbox";
const VPN_STOP_TASK: &str = "UniDTKVpnStop";

/// Настройки подмодуля домашнего сервера (выключаемый split-tunnel в LAN).
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct HomeServerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_home_port")]
    pub port: u16,
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub public_key: String,
    #[serde(default)]
    pub short_id: String,
    #[serde(default)]
    pub sni: String,
    #[serde(default = "default_home_subnet")]
    pub subnet: String,
}

fn default_home_port() -> u16 {
    443
}

fn default_home_subnet() -> String {
    "192.168.31.0/24".to_string()
}

/// Каталог данных UniDTK (контракт, не локальная копия логики).
fn data_dir() -> std::path::PathBuf {
    unidtk_api::paths::data_dir()
}

/// sing-box binary to run: our bundled extended fork (XHTTP support), resolved
/// through the contract so telegram does not need this module to find it.
pub(crate) fn singbox_exe() -> std::path::PathBuf {
    unidtk_api::paths::singbox_exe()
}

fn settings_path() -> std::path::PathBuf {
    data_dir().join("vpn_settings.json")
}

fn servers_cache_path() -> std::path::PathBuf {
    data_dir().join("vpn_servers.json")
}

fn singbox_config_path() -> std::path::PathBuf {
    data_dir().join("singbox_config.json")
}

fn singbox_log_path() -> std::path::PathBuf {
    data_dir().join("singbox.log")
}

/// Written by `perform_stop()` before it fires the stop task, read by the
/// elevated helper: line 1 is the nonce, line 2 the RFC 3339 UTC instant the
/// stop was *requested*.
fn stop_cutoff_path() -> std::path::PathBuf {
    data_dir().join("vpn_stop.cutoff")
}

/// Written by the elevated helper once it has finished killing; contains
/// the nonce it was handed, so a waiter can tell this run's completion apart
/// from a previous one's.
fn stop_done_path() -> std::path::PathBuf {
    data_dir().join("vpn_stop.done")
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct VpnSettings {
    #[serde(default)]
    subscription_url: String,
    #[serde(default)]
    default_tag: String,
    #[serde(default)]
    last_tag: Option<String>,
    #[serde(default = "default_auto_connect")]
    auto_connect: bool,
    #[serde(default)]
    home_server: HomeServerConfig,
}

fn default_auto_connect() -> bool {
    false
}

fn default_settings() -> VpnSettings {
    VpnSettings {
        subscription_url: String::new(),
        default_tag: String::new(),
        last_tag: None,
        auto_connect: false,
        home_server: HomeServerConfig::default(),
    }
}

fn load_settings() -> VpnSettings {
    if let Some(db) = VPN_DB.get() {
        if let Ok(Some(bytes)) = db.kv_get("settings") {
            if let Ok(settings) = serde_json::from_slice::<VpnSettings>(&bytes) {
                return settings;
            }
        }
    }

    let legacy_path = settings_path();
    if legacy_path.exists() {
        if let Ok(raw) = std::fs::read_to_string(&legacy_path) {
            let trimmed = raw.trim_start_matches('\u{feff}');
            if let Ok(settings) = serde_json::from_str::<VpnSettings>(trimmed) {
                let _ = std::fs::remove_file(&legacy_path);
                save_settings(&settings);
                return settings;
            }
        }
        let _ = std::fs::remove_file(&legacy_path);
    }

    let settings = default_settings();
    save_settings(&settings);
    settings
}

fn load_settings_from_db(db: &dyn unidtk_api::ModuleDb) -> VpnSettings {
    if let Ok(Some(bytes)) = db.kv_get("settings") {
        if let Ok(settings) = serde_json::from_slice::<VpnSettings>(&bytes) {
            return settings;
        }
    }
    default_settings()
}

fn save_settings(settings: &VpnSettings) {
    if let Some(db) = VPN_DB.get() {
        if let Ok(bytes) = serde_json::to_vec(settings) {
            let _ = db.kv_set("settings", &bytes);
        }
    }
}

// Persist the last successfully fetched server list in redb so a later launch can still
// connect using the previous subscription when the subscription host is down.
fn save_servers_cache(servers: &[VpnServer]) {
    if let Some(db) = VPN_DB.get() {
        if let Ok(bytes) = serde_json::to_vec(servers) {
            let _ = db.kv_set("servers_cache", &bytes);
        }
    }
}

fn load_servers_cache() -> Vec<VpnServer> {
    if let Some(db) = VPN_DB.get() {
        if let Ok(Some(bytes)) = db.kv_get("servers_cache") {
            if let Ok(servers) = serde_json::from_slice::<Vec<VpnServer>>(&bytes) {
                return servers;
            }
        }
    }

    let legacy_path = servers_cache_path();
    if legacy_path.exists() {
        if let Ok(raw) = std::fs::read_to_string(&legacy_path) {
            if let Ok(servers) = serde_json::from_str::<Vec<VpnServer>>(raw.trim_start_matches('\u{feff}')) {
                let _ = std::fs::remove_file(&legacy_path);
                save_servers_cache(&servers);
                return servers;
            }
        }
        let _ = std::fs::remove_file(&legacy_path);
    }

    Vec::new()
}

fn settings_status(settings: VpnSettings) -> VpnSettingsStatus {
    VpnSettingsStatus {
        auto_connect: settings.auto_connect,
        default_tag: settings.default_tag,
        last_tag: settings.last_tag,
        subscription_url: settings.subscription_url,
        home_server: settings.home_server,
    }
}

// ── Socket surface (`unidtk_api::commands::ModuleIpcFn`) ───────────────────────
// The kernel registers this under the module id, so its router never names
// `VpnRuntime`.

fn runtime_from<'a>(
    host: Option<unidtk_api::commands::HostHandle<'a>>,
) -> Result<tauri::State<'a, VpnRuntime>, String> {
    let app = host
        .and_then(|h| h.downcast_ref::<tauri::AppHandle>())
        .ok_or_else(|| "host handle unavailable".to_string())?;
    tauri::Manager::try_state::<VpnRuntime>(app).ok_or_else(|| "vpn is not ready".to_string())
}

fn json_of<T: serde::Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

pub fn ipc_call(
    cmd: &str,
    args: &serde_json::Value,
    host: Option<unidtk_api::commands::HostHandle<'_>>,
) -> Option<Result<serde_json::Value, String>> {
    match cmd {
        "vpn_status" => Some(runtime_from(host).and_then(|runtime| json_of(status(&runtime)))),
        "vpn_list_servers" => Some(
            runtime_from(host).and_then(|runtime| list_servers(&runtime).and_then(json_of)),
        ),
        "vpn_settings" => Some(json_of(settings())),
        // The writes answer `null`, exactly like the `Result<(), String>` Tauri commands of
        // the same name: a caller that switches transports must not see a different shape.
        "vpn_connect" => Some(runtime_from(host).and_then(|runtime| {
            let tag = args
                .get("tag")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            connect(&runtime, tag).map(|_| serde_json::Value::Null)
        })),
        "vpn_disconnect" => Some(runtime_from(host).and_then(|runtime| {
            disconnect(&runtime);
            Ok(serde_json::Value::Null)
        })),
        "vpn_ping_all" => {
            Some(runtime_from(host).and_then(|runtime| ping_all(&runtime).and_then(json_of)))
        }
        "vpn_set_auto_connect" => Some(
            args.get("enabled")
                .and_then(|value| value.as_bool())
                .ok_or_else(|| "vpn_set_auto_connect requires {enabled}".to_string())
                .and_then(|enabled| json_of(set_auto_connect(enabled))),
        ),
        _ => None,
    }
}

pub fn settings() -> VpnSettingsStatus {
    settings_status(load_settings())
}

pub fn set_auto_connect(enabled: bool) -> VpnSettingsStatus {
    let mut settings = load_settings();
    settings.auto_connect = enabled;
    save_settings(&settings);
    unidtk_api::log(format!("vpn: auto-connect set to {enabled}"));
    settings_status(settings)
}

pub fn set_subscription_url(url: String) -> Result<VpnSettingsStatus, String> {
    let mut settings = load_settings();
    settings.subscription_url = url.trim().to_string();
    save_settings(&settings);
    unidtk_api::log("vpn: subscription_url updated in redb");
    Ok(settings_status(settings))
}

pub fn set_home_server_config(config: HomeServerConfig) -> Result<VpnSettingsStatus, String> {
    let mut settings = load_settings();
    settings.home_server = config;
    save_settings(&settings);
    unidtk_api::log(format!(
        "vpn: home_server config updated in redb (enabled={})",
        settings.home_server.enabled
    ));
    Ok(settings_status(settings))
}

#[derive(Clone, Serialize, Deserialize)]
pub struct VpnServer {
    pub tag: String,
    #[serde(default = "default_protocol_vless")]
    pub protocol: String,
    pub uuid: String,
    pub host: String,
    pub port: u16,
    pub network: String,
    #[serde(default = "default_security")]
    pub security: String,
    pub sni: String,
    pub fp: String,
    pub pbk: String,
    pub sid: String,
    pub flow: String,
    pub path: String,
    #[serde(rename = "serviceName")]
    pub service_name: String,
}

fn default_protocol_vless() -> String {
    "vless".into()
}

fn default_security() -> String {
    "reality".into()
}

#[derive(Clone, Serialize)]
pub struct VpnStatus {
    pub connected: bool,
    pub connecting: bool,
    pub selected_tag: Option<String>,
    #[serde(rename = "proxyLatencyMs")]
    pub proxy_latency_ms: Option<u32>,
    #[serde(rename = "proxyError")]
    pub proxy_error: Option<String>,
    /// True when post-connect public IP could not be confirmed different from
    /// pre-connect (same IP, or either lookup failed). Informational only —
    /// does NOT force `connected = false` (shared-egress multi-hop can legitimately
    /// reuse an exit IP).
    #[serde(rename = "ipUnverified")]
    pub ip_unverified: bool,
    pub error: Option<String>,
    pub auto_connect: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct VpnSettingsStatus {
    pub auto_connect: bool,
    pub default_tag: String,
    pub last_tag: Option<String>,
    pub subscription_url: String,
    pub home_server: HomeServerConfig,
}

#[derive(Default, Clone)]
pub struct VpnRuntime {
    // Arc so a detached background thread (post-connect IP verification) can
    // hold its own handle and write back after connect() has already
    // returned, without needing runtime's borrow to outlive the call.
    inner: Arc<Mutex<RuntimeInner>>,
}

#[derive(Default)]
struct RuntimeInner {
    servers: Vec<VpnServer>,
    selected_tag: Option<String>,
    proxy_latency_ms: Option<u32>,
    proxy_error: Option<String>,
    /// See `VpnStatus::ip_unverified`. Sticky from last successful connect.
    ip_unverified: bool,
    connected: bool,
    connecting: bool,
    last_error: Option<String>,
}

// ─── Subscription parsing ───

fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(byte) =
                    u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
                {
                    out.push(byte);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next()?;
            let value = parts.next().unwrap_or("");
            Some((key.to_string(), url_decode(value)))
        })
        .collect()
}

fn parse_vless_line(line: &str) -> Option<VpnServer> {
    let rest = line.strip_prefix("vless://")?;
    let (main_part, fragment) = match rest.split_once('#') {
        Some((m, f)) => (m, url_decode(f)),
        None => (rest, String::new()),
    };
    let (auth_and_host, query) = main_part.split_once('?').unwrap_or((main_part, ""));
    let (uuid, host_port) = auth_and_host.split_once('@')?;
    let (host, port_str) = host_port.rsplit_once(':')?;
    let port: u16 = port_str.parse().ok()?;
    let params = parse_query(query);

    let tag = if fragment.is_empty() {
        format!("{host}:{port}")
    } else {
        fragment
    };

    let pbk = params.get("pbk").cloned().unwrap_or_default();
    let sni = params.get("sni").cloned().unwrap_or_default();
    let sec_param = params.get("security").cloned().unwrap_or_default();
    let security = if sec_param == "reality" || !pbk.is_empty() {
        "reality".to_string()
    } else if sec_param == "tls" || !sni.is_empty() {
        "tls".to_string()
    } else {
        "none".to_string()
    };

    Some(VpnServer {
        tag,
        protocol: "vless".into(),
        uuid: uuid.to_string(),
        host: host.to_string(),
        port,
        network: params.get("type").cloned().unwrap_or_else(|| "tcp".into()),
        security,
        sni,
        fp: params.get("fp").cloned().unwrap_or_else(|| "chrome".into()),
        pbk,
        sid: params.get("sid").cloned().unwrap_or_default(),
        flow: params.get("flow").cloned().unwrap_or_default(),
        path: params.get("path").cloned().unwrap_or_else(|| "/".into()),
        service_name: params.get("serviceName").cloned().unwrap_or_default(),
    })
}

fn parse_trojan_line(line: &str) -> Option<VpnServer> {
    let rest = line.strip_prefix("trojan://")?;
    let (main_part, fragment) = match rest.split_once('#') {
        Some((m, f)) => (m, url_decode(f)),
        None => (rest, String::new()),
    };
    let (auth_and_host, query) = main_part.split_once('?').unwrap_or((main_part, ""));
    let (password, host_port) = auth_and_host.split_once('@')?;
    let (host, port_str) = host_port.rsplit_once(':')?;
    let port: u16 = port_str.parse().ok()?;
    let params = parse_query(query);

    let tag = if fragment.is_empty() {
        format!("{host}:{port}")
    } else {
        fragment
    };

    let sni = params.get("sni").cloned().unwrap_or_else(|| host.to_string());
    let sec_param = params.get("security").cloned().unwrap_or_else(|| "tls".into());

    Some(VpnServer {
        tag,
        protocol: "trojan".into(),
        uuid: password.to_string(),
        host: host.to_string(),
        port,
        network: params.get("type").cloned().unwrap_or_else(|| "tcp".into()),
        security: sec_param,
        sni,
        fp: params.get("fp").cloned().unwrap_or_else(|| "chrome".into()),
        pbk: String::new(),
        sid: String::new(),
        flow: String::new(),
        path: params.get("path").cloned().unwrap_or_else(|| "/".into()),
        service_name: params.get("serviceName").cloned().unwrap_or_default(),
    })
}

fn fetch_servers(subscription_url: &str) -> Result<Vec<VpnServer>, String> {
    let response = ureq::get(subscription_url)
        .set("User-Agent", "v2box/1.0")
        .timeout(Duration::from_secs(15))
        .call()
        .map_err(|e| format!("subscription request failed: {e}"))?;

    let raw = response
        .into_string()
        .map_err(|e| format!("subscription read failed: {e}"))?;

    use base64::Engine;
    let decoded_bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(raw.trim()))
        .map_err(|e| format!("subscription decode failed: {e}"))?;
    let decoded = String::from_utf8_lossy(&decoded_bytes);

    let servers: Vec<VpnServer> = decoded
        .lines()
        .map(str::trim)
        .filter_map(|l| {
            if l.starts_with("vless://") {
                parse_vless_line(l)
            } else if l.starts_with("trojan://") {
                parse_trojan_line(l)
            } else {
                None
            }
        })
        .collect();

    if servers.is_empty() {
        return Err("no usable VLESS/Trojan servers found in subscription".into());
    }

    Ok(servers)
}

// ─── Config builders ───

/// sing-box outbound for one subscription server (vless/trojan + TLS + transport).
///
/// Transport mapping from the subscription's Xray-style fields:
/// - `tcp` → no transport (plain TCP), `ws` → ws, `grpc` → grpc;
/// - `xhttp`/`splithttp` → the extended fork's `xhttp` transport. Upstream
///   sing-box has no such transport, and the fork requires an explicit padding
///   range (padding cannot be disabled).
fn build_singbox_outbound(server: &VpnServer, tag: &str) -> serde_json::Value {
    let mut outbound = if server.protocol == "trojan" {
        json!({
            "type": "trojan",
            "tag": tag,
            "server": server.host,
            "server_port": server.port,
            "password": server.uuid,
        })
    } else {
        let mut obj = json!({
            "type": "vless",
            "tag": tag,
            "server": server.host,
            "server_port": server.port,
            "uuid": server.uuid,
        });
        if !server.flow.is_empty() {
            obj["flow"] = json!(server.flow);
        }
        obj
    };

    let fingerprint = if server.fp.is_empty() {
        "chrome"
    } else {
        server.fp.as_str()
    };
    if server.security == "reality" {
        outbound["tls"] = json!({
            "enabled": true,
            "server_name": server.sni,
            "utls": { "enabled": true, "fingerprint": fingerprint },
            "reality": {
                "enabled": true,
                "public_key": server.pbk,
                "short_id": server.sid
            }
        });
    } else if server.security == "tls" {
        outbound["tls"] = json!({
            "enabled": true,
            "server_name": server.sni,
            "insecure": true,
            "utls": { "enabled": true, "fingerprint": fingerprint },
            "alpn": ["h2", "http/1.1"]
        });
    }

    match server.network.as_str() {
        "ws" => {
            outbound["transport"] = json!({
                "type": "ws",
                "path": server.path,
                "headers": { "Host": server.sni }
            });
        }
        "grpc" => {
            outbound["transport"] = json!({
                "type": "grpc",
                "service_name": server.service_name
            });
        }
        "xhttp" | "splithttp" => {
            outbound["transport"] = json!({
                "type": "xhttp",
                "path": server.path,
                "x_padding_bytes": "100-1000"
            });
        }
        _ => {}
    }

    outbound
}

/// Full client config for the selected `server`: TUN in, local SOCKS in
/// (readiness probe + watchdog), proxy out, and optional split-tunnel outbound
/// for the home server's LAN if the home_server submodule is enabled.
fn build_singbox_config(server: &VpnServer, settings: &VpnSettings) -> serde_json::Value {
    let singbox_log = singbox_log_path().to_string_lossy().into_owned();

    let mut outbounds = vec![
        build_singbox_outbound(server, "proxy"),
    ];

    let mut rules = vec![
        // Self-bypass: sing-box's own dials must never enter the TUN,
        // or it would route itself into its own tunnel.
        json!({ "outbound": "direct", "process_name": ["sing-box.exe"] }),
        json!({ "action": "sniff" }),
        json!({ "action": "hijack-dns", "protocol": "dns" }),
        // Keep Russian domains on the normal network interface; all
        // other TUN traffic uses `proxy`.
        json!({ "outbound": "direct", "domain_suffix": [".ru"] }),
    ];

    if settings.home_server.enabled && !settings.home_server.host.trim().is_empty() {
        let hs = &settings.home_server;
        outbounds.push(json!({
            "type": "vless",
            "tag": "lan-home",
            "server": hs.host.trim(),
            "server_port": hs.port,
            "uuid": hs.uuid.trim(),
            "tls": {
                "enabled": true,
                "server_name": hs.sni.trim(),
                "utls": { "enabled": true, "fingerprint": "chrome" },
                "reality": {
                    "enabled": true,
                    "public_key": hs.public_key.trim(),
                    "short_id": hs.short_id.trim()
                }
            }
        }));

        let subnet = if hs.subnet.trim().is_empty() {
            "192.168.31.0/24"
        } else {
            hs.subnet.trim()
        };
        rules.push(json!({
            "outbound": "lan-home",
            "ip_cidr": [subnet]
        }));
    }

    outbounds.push(json!({ "type": "direct", "tag": "direct" }));

    json!({
        // sing-box runs hidden, so route its log to a file for post-mortem
        // diagnosis of TUN/route failures. "info" (rather than "warn") makes
        // sing-box log each new connection it routes through the TUN, which is
        // the connection-level evidence for whether the tunnel is actually
        // carrying traffic.
        "log": { "level": "info", "output": singbox_log, "timestamp": true },
        "dns": {
            // Resolve through the proxy, NOT `direct`. Sending system DNS as
            // plaintext UDP straight to 8.8.8.8 gets blocked or hijacked on
            // many restricted networks, so names fail to resolve and the tunnel
            // looks "connected but nothing loads". Routing DNS over the proxy
            // keeps resolution working and avoids a DNS leak. Use DNS over TCP
            // (`type: "tcp"`) rather than plain UDP: UDP-over-VLESS relay is
            // flaky and slow to warm up, whereas TCP rides the same reliable
            // stream as the rest of the tunnel.
            //
            // `type`/`server` is the post-1.12 DNS server form; the legacy
            // `address: "tcp://…"` shape was removed in sing-box 1.14 (and
            // `sing-box check` rejects it outright).
            "servers": [{ "type": "tcp", "tag": "dns-proxy", "server": "8.8.8.8", "detour": "proxy" }]
        },
        "inbounds": [
            {
                "type": "tun",
                "tag": "tun-in",
                "interface_name": TUN_NAME,
                "address": ["172.19.0.1/30"],
                // Keep the TUN MTU conservative: Reality-over-TCP plus TUN
                // overhead can otherwise make page loads stall while large TLS
                // packets are retried.
                "mtu": 1280,
                "auto_route": true,
                "strict_route": true,
                "stack": "mixed"
            },
            {
                // Local SOCKS inbound: the watchdog, the readiness probe and
                // `probe_vpn_transport()` poll it as the "tunnel is up" signal.
                "type": "socks",
                "tag": "socks-in",
                "listen": "127.0.0.1",
                "listen_port": SOCKS_PORT
            }
        ],
        "outbounds": outbounds,
        "route": {
            "auto_detect_interface": true,
            "final": "proxy",
            "rules": rules
        }
    })
}

// ─── Process management (via elevated helper scheduled tasks) ───

// Bumped by `disconnect`/`shutdown` so an in-flight `bring_up` (which runs
// without holding the runtime mutex) cannot commit `connected=true` after the
// user cancelled, and so a late success cleans up processes it may have
// restarted after the cancel's `perform_stop`. Same class of bug as holding a
// lock across a long wait: concurrent connect/disconnect otherwise race on
// process lifetime vs. cached UI state.
static CONNECT_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// Distinguishes one fired stop from the next, so `wait_until_stopped` can't
// mistake an earlier run's done-marker for the one it is waiting on. Paired
// with the pid, which keeps it unique across app restarts too.
static STOP_NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// Set by a manual disconnect() so an in-flight auto_connect_on_startup retry
// loop stops instead of reconnecting behind the user's back. Deliberately
// process-local, not persisted: the AUTO toggle (VpnSettings::auto_connect)
// is the user's independent "reconnect on next launch" preference and must
// NOT be mutated by ordinary connect/disconnect — see connect()/disconnect().
static MANUAL_DISCONNECT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

// Last known connected flag, mirrored from RuntimeInner.connected at every
// transition (connect/disconnect/shutdown and the status() watchdog). Lets
// sibling modules react to VPN state without access to the Tauri-managed
// VpnRuntime — telegram.rs's managed "vless" proxy uses it to hand Telegram
// traffic over to the VPN TUN (TDLib "no proxy") while the VPN is up.
static CONNECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True while the wallpaper VPN tunnel is considered up.
pub fn is_connected() -> bool {
    CONNECTED.load(std::sync::atomic::Ordering::SeqCst)
}

fn schtasks(args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("schtasks")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

fn task_start(task_name: &str) -> Result<(), String> {
    unidtk_api::log(format!("vpn: schtasks /run /tn {task_name}"));
    let output =
        schtasks(&["/run", "/tn", task_name]).map_err(|e| format!("schtasks /run failed: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    unidtk_api::log(format!(
        "vpn: schtasks {task_name} exit={} stdout={stdout:?} stderr={stderr:?}",
        output.status
    ));
    if !output.status.success() {
        return Err(format!("failed to start {task_name}: {stderr}"));
    }
    Ok(())
}

fn task_stop(task_name: &str) {
    let _ = schtasks(&["/end", "/tn", task_name]);
}

fn interface_is_connected(name: &str) -> Result<bool, String> {
    let output = Command::new("cmd.exe")
        .args(["/C", "chcp 65001 >NUL & netsh interface show interface"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("netsh interface query failed: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let target = name.to_ascii_lowercase();
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().any(|line| {
        let line = line.trim();
        line.to_ascii_lowercase().contains(&target)
            && !line.contains("Отключен")
            && !line.to_ascii_lowercase().contains("disconnected")
    }))
}

// Substring match, not the exact adapter name: Happ's tun2proxy engine uses
// "happ-default-tun", its sing-box engine uses "happ-tun" (mode toggle in the
// Happ UI). A name-exact check here went stale the moment a user switched
// modes — this app's own VPN then saw "Happ not active" and raced sing-box
// against Happ's already-live TUN for the WFP capture. Same substring
// approach as server.ps1's Get-PreferredPublicGatewayRoute.
fn active_happ_tun_present() -> Result<bool, String> {
    interface_is_connected("happ")
}

fn run_diagnostic_command(label: &str, program: &str, args: &[&str]) {
    match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            unidtk_api::log(format!("vpn: diag {label} exit={}", output.status));
            for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
                unidtk_api::log(format!("vpn: diag {label}: {line}"));
            }
            for line in stderr.lines().filter(|line| !line.trim().is_empty()) {
                unidtk_api::log(format!("vpn: diag {label} stderr: {line}"));
            }
        }
        Err(error) => unidtk_api::log(format!("vpn: diag {label} failed: {error}")),
    }
}

fn log_network_snapshot(label: &str) {
    unidtk_api::log(format!("vpn: network snapshot [{label}] start"));
    run_diagnostic_command(
        "interfaces",
        "cmd.exe",
        &["/C", "chcp 65001 >NUL & netsh interface show interface"],
    );
    run_diagnostic_command(
        "routes-v4",
        "cmd.exe",
        &["/C", "chcp 65001 >NUL & route print 0.0.0.0"],
    );
    run_diagnostic_command(
        "socks-port",
        "cmd.exe",
        &["/C", &format!("netstat -ano | findstr :{SOCKS_PORT}")],
    );
    unidtk_api::log(format!("vpn: network snapshot [{label}] end"));
}

// Pull the last few lines of a core's own log file (singbox.log) into the main
// log. The core logs are otherwise easy to
// forget about — this puts the actually useful diagnostic content (why the
// SOCKS port or TUN never came up) right next to everything else instead of
// requiring a manual trip into the data dir.
fn log_tail_file(label: &str, path: &std::path::Path, max_lines: usize) {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(max_lines);
            unidtk_api::log(format!(
                "vpn: {label} log tail ({} of {} lines) [{}]",
                lines.len() - start,
                lines.len(),
                path.display()
            ));
            for line in &lines[start..] {
                unidtk_api::log(format!("vpn: {label} log: {line}"));
            }
        }
        Err(error) => unidtk_api::log(format!(
            "vpn: {label} log read failed at {}: {error}",
            path.display()
        )),
    }
}

// Dials the local sing-box SOCKS port directly instead of relying on the
// system-wide `wallpaper-tun` route. sing-box's TUN captures ALL system
// traffic (`strict_route: true`, by design, to avoid leaks) including a burst
// of Windows' own background connectivity/telemetry DNS queries that fire the
// moment the interface appears — and since sing-box's DNS is deliberately
// tunneled too (see build_singbox_config's dns comment), that burst piles up
// on the same proxy connection our probe would otherwise have to queue
// behind. Real-world effect: probes routed via the TUN consistently took
// ~12s per endpoint instead of the ~4s timeout, so "connected" reported
// "proxy: fail" even though the tunnel was carrying traffic fine. Hitting the
// SOCKS port straight from this process sidesteps that queue entirely — it
// still proves the full sing-box→server→endpoint chain works, just from the
// app's own connection rather than by trusting Windows to have routed one
// into the TUN, and `wallpaper-tun connected` already covers that half.
fn socks_agent(timeout: Duration) -> Result<ureq::Agent, String> {
    let proxy = ureq::Proxy::new(format!("socks5://127.0.0.1:{SOCKS_PORT}"))
        .map_err(|e| format!("proxy setup failed: {e}"))?;
    Ok(ureq::AgentBuilder::new()
        .proxy(proxy)
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .build())
}

fn probe_vpn_transport() -> Result<u32, String> {
    // Probe IP-literal endpoints, NOT hostnames: DNS-over-tunnel may not have
    // warmed up yet. Any HTTP response (even 4xx) proves the transport works.
    // `1.1.1.1` and `8.8.8.8` both serve valid certs for their own IP, so
    // TLS-by-IP succeeds. Single fast pass per address so a dead server caps
    // total probe time at ~8s instead of dragging out connect().
    let agent = socks_agent(Duration::from_secs(4))?;
    let endpoints = ["https://1.1.1.1/", "https://8.8.8.8/"];
    let mut last_error = None;
    for url in endpoints {
        unidtk_api::log(format!("vpn: probe http {url} start"));
        let start = Instant::now();
        match agent
            .get(url)
            .set("User-Agent", "shell-vpn-probe/1.0")
            .call()
        {
            // Any HTTP status (including the 4xx/5xx Err(Status) arm) means
            // bytes flowed end-to-end through the tunnel.
            Ok(response) => {
                let status = response.status();
                unidtk_api::log(format!("vpn: probe http {url} OK status={status}"));
                return Ok(start.elapsed().as_millis().min(u32::MAX as u128) as u32);
            }
            Err(ureq::Error::Status(status, _)) => {
                unidtk_api::log(format!("vpn: probe http {url} reachable, status={status}"));
                return Ok(start.elapsed().as_millis().min(u32::MAX as u128) as u32);
            }
            Err(error) => {
                let error = error.to_string();
                unidtk_api::log(format!("vpn: probe http {url} failed: {error}"));
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| "proxy probe failed".into()))
}

// Cloudflare's edge answers this path with `ip=<caller>` (plus timing/colo
// fields) even when hit by IP literal — same DNS-avoidance reasoning as
// probe_vpn_transport(). This is the actual "is the VPN working" signal: the
// value must differ from the pre-connect reading (see log_public_ip) once the
// tunnel is up, since traffic now exits through the VPN server, not the ISP.
fn extract_trace_ip(body: &str) -> Result<String, String> {
    body.lines()
        .find_map(|line| line.strip_prefix("ip="))
        .map(|ip| ip.to_string())
        .ok_or_else(|| "no ip= field in trace response".to_string())
}

/// No proxy up yet (pre-connect) or already torn down (post-disconnect) —
/// goes out over the normal system route.
fn fetch_public_ip_direct() -> Result<String, String> {
    let response = ureq::get("https://1.1.1.1/cdn-cgi/trace")
        .set("User-Agent", "shell-vpn-probe/1.0")
        .timeout(Duration::from_secs(4))
        .call()
        .map_err(|e| format!("trace request failed: {e}"))?;
    extract_trace_ip(&response.into_string().map_err(|e| format!("trace read failed: {e}"))?)
}

/// Proxy is up (post-connect, before disconnect stops it) — same rationale as
/// probe_vpn_transport(): go straight through the local SOCKS port instead of
/// the system-wide TUN route, so this doesn't queue behind Windows' own
/// background DNS traffic and time out even though the tunnel is fine.
fn fetch_public_ip_via_proxy() -> Result<String, String> {
    let agent = socks_agent(Duration::from_secs(4))?;
    let response = agent
        .get("https://1.1.1.1/cdn-cgi/trace")
        .set("User-Agent", "shell-vpn-probe/1.0")
        .call()
        .map_err(|e| format!("trace request failed: {e}"))?;
    extract_trace_ip(&response.into_string().map_err(|e| format!("trace read failed: {e}"))?)
}

fn log_public_ip(label: &str) {
    match fetch_public_ip_direct() {
        Ok(ip) => unidtk_api::log(format!("vpn: public ip [{label}] = {ip}")),
        Err(error) => unidtk_api::log(format!("vpn: public ip [{label}] lookup failed: {error}")),
    }
}

/// Reject a generated config before anything tries to run it. sing-box changes
/// its schema between major versions (e.g. the legacy DNS server form was
/// removed in 1.14), and a rejected config otherwise surfaces only as an
/// opaque "TUN never came up". Cheap: one short-lived `sing-box check`.
fn validate_singbox_config(path: &std::path::Path) -> Result<(), String> {
    let output = Command::new(singbox_exe())
        .args(["check", "-c"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("sing-box check failed to start: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("unknown error")
        .trim()
        .to_string();
    Err(format!("sing-box rejected the config: {reason}"))
}

fn port_is_open(port: u16) -> bool {
    use std::net::TcpStream;
    // Slightly longer connect timeout so slow-booting sing-box inbounds are not
    // falsely reported as down during the initial bind polling window.
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(500),
    )
    .is_ok()
}

fn ensure_binaries_present() -> Result<(), String> {
    let singbox = singbox_exe();
    if !singbox.exists() {
        return Err(format!(
            "sing-box.exe not found at {} (bundled build missing?)",
            singbox.display()
        ));
    }
    let helper = vpn_helper_exe();
    if !helper.exists() {
        return Err(format!(
            "vpn-helper.exe not found at {} (run `shell build` to install it)",
            helper.display()
        ));
    }
    Ok(())
}

/// Outcome of a successful bring-up. The transport probe is "soft": the tunnel
/// is considered up once the TUN interface exists, and a failed HTTP probe is
/// surfaced as a warning rather than tearing the connection down. Same for
/// the eventual `ip_unverified` signal derived from `pre_ip_handle`: IP
/// mismatch is evidence, not a hard gate.
struct BringUpResult {
    latency_ms: Option<u32>,
    probe_error: Option<String>,
    /// Started near the top of `bring_up()` (in parallel with stop/start), not
    /// yet joined. `connect()` joins it on its own detached post-commit
    /// thread so the lookup — and the fresh post-connect lookup it's compared
    /// against — can never delay the `connected = true` commit.
    pre_ip_handle: std::thread::JoinHandle<Result<String, String>>,
}

/// Fire the elevated stop task (falls back to `schtasks /end` if the helper
/// task can't be started). Does NOT touch runtime state and does NOT wait —
/// callers that need the processes actually gone must follow up with
/// `wait_until_stopped`, handing it the returned nonce.
///
/// `None` means no done-marker will ever appear for this stop (the script
/// wasn't fired, or its request file couldn't be written), so a waiter has
/// nothing to wait on beyond the port/TUN going quiet.
fn perform_stop() -> Option<String> {
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        STOP_NONCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    // Order matters: drop any previous run's marker before the script that
    // writes the new one can start, or a waiter would accept a stale file.
    let _ = std::fs::remove_file(stop_done_path());
    let request = std::fs::write(
        stop_cutoff_path(),
        format!("{nonce}\n{}\n", chrono::Utc::now().to_rfc3339()),
    );
    if let Err(e) = &request {
        // Not fatal: the script falls back to killing every matching core
        // regardless of age, i.e. the old fire-and-forget behaviour.
        unidtk_api::log(format!("vpn: write stop request failed: {e}"));
    }
    // The elevated task runs vpn-helper.exe; without it there is no way to
    // kill the elevated sing-box (this process is unelevated). Fall back to
    // ending the start task itself and let the caller's port/TUN checks decide.
    if !vpn_helper_exe().exists() {
        unidtk_api::log(format!(
            "vpn: {} missing — cannot run the elevated stop (run `shell build`)",
            vpn_helper_exe().display()
        ));
        task_stop(SINGBOX_TASK);
        return None;
    }
    match task_start(VPN_STOP_TASK) {
        Ok(()) => {
            unidtk_api::log(format!("vpn: elevated stop task started (nonce={nonce})"));
            request.ok().map(|()| nonce)
        }
        Err(e) => {
            unidtk_api::log(format!("vpn: elevated stop task failed: {e}"));
            task_stop(SINGBOX_TASK);
            None
        }
    }
}

/// Has the elevated stop script fired with this nonce finished its kill?
fn stop_marker_seen(nonce: &str) -> bool {
    std::fs::read_to_string(stop_done_path())
        .map(|content| content.trim() == nonce)
        .unwrap_or(false)
}

/// Poll until the stop fired with `stop_nonce` has reported itself done AND
/// neither the local SOCKS port nor the wallpaper TUN interface is present,
/// i.e. the previous run is truly gone. Returns `false`
/// if that hasn't happened by the deadline.
///
/// This closes the stop→start race that made connect flaky: the stop task
/// launches a helper that itself takes ~1–3 s to spawn and kill the
/// core, so checking the port immediately after firing it saw the *old*
/// sing-box still holding 12908 ("port already in use") — or an old, dying TUN
/// adapter reported "connected" and got mistaken for a fresh one.
///
/// The port/TUN checks alone were not enough, because they also pass when
/// there was nothing to stop in the first place — the usual case for the first
/// connect after an app restart. That returned instantly, bring_up() started
/// the cores, and the still-in-flight kill landed on THEM a second later:
/// sing-box was killed between logging its first line and creating the
/// adapter, so connect failed with the opaque "sing-box did not bring up the
/// wallpaper TUN interface" while singbox.log showed no error at all. Hence
/// the done-marker: it is the only signal that the kill is behind us rather
/// than ahead of us.
fn wait_until_stopped(timeout: Duration, stop_nonce: Option<&str>) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let stop_done = stop_nonce.map_or(true, stop_marker_seen);
        let socks_up = port_is_open(SOCKS_PORT);
        let tun_up = interface_is_connected(TUN_NAME).unwrap_or(false);
        if stop_done && !socks_up && !tun_up {
            return true;
        }
        if Instant::now() >= deadline {
            unidtk_api::log(format!(
                "vpn: wait_until_stopped timed out (stop_done={stop_done} socks_up={socks_up} tun_up={tun_up})"
            ));
            return false;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Bring the tunnel up for `server`. Runs entirely WITHOUT holding the runtime
/// mutex — the long port/TUN waits and HTTP probe (tens of seconds worst case)
/// must not block `status()`/`select_server()` polling on the same lock.
fn bring_up(server: &VpnServer) -> Result<BringUpResult, String> {
    ensure_binaries_present()?;
    unidtk_api::log(format!("vpn: bring_up tag={}", server.tag));
    unidtk_api::log(format!(
        "vpn: core log singbox={} exe={}",
        singbox_log_path().display(),
        singbox_exe().display()
    ));
    // Baseline reading — whatever this machine's exit IP is right now (ISP or
    // a still-live previous tunnel). Compared against the "post-connect"
    // reading after the probe; result feeds `ip_unverified` (informational).
    // Runs in parallel with stop/start so a slow lookup doesn't delay
    // perform_stop()/restart. Joined after the probe (by then startup has
    // already taken several seconds, so the join is usually free).
    let pre_ip_handle = std::thread::spawn(|| {
        let result = fetch_public_ip_direct();
        match &result {
            Ok(ip) => unidtk_api::log(format!("vpn: public ip [pre-connect] = {ip}")),
            Err(error) => {
                unidtk_api::log(format!("vpn: public ip [pre-connect] lookup failed: {error}"))
            }
        }
        result
    });

    // Always start from a clean slate: stop any prior run and wait for it to
    // actually release the SOCKS port and TUN adapter before continuing.
    let stop_nonce = perform_stop();
    if !wait_until_stopped(Duration::from_secs(10), stop_nonce.as_deref()) {
        return Err("previous VPN processes did not shut down in time; try again".into());
    }

    let happ_active = active_happ_tun_present()?;
    unidtk_api::log(format!("vpn: happ-default-tun active={happ_active}"));
    if happ_active {
        return Err("Happ VPN/TUN is active; disconnect Happ before starting wallpaper VPN".into());
    }
    if port_is_open(SOCKS_PORT) {
        return Err(format!(
            "local SOCKS port {SOCKS_PORT} is still in use after stopping; refusing to start"
        ));
    }

    let settings = load_settings();
    let singbox_cfg = build_singbox_config(server, &settings);
    std::fs::write(
        singbox_config_path(),
        serde_json::to_string_pretty(&singbox_cfg).unwrap_or_default(),
    )
    .map_err(|e| format!("write sing-box config failed: {e}"))?;
    validate_singbox_config(&singbox_config_path())?;

    // Re-check Happ right before claiming the TUN, not just once at the top
    // of bring_up(): the stop/start sequence above already took several
    // seconds, which is plenty of time for the user to have connected Happ
    // (or for Happ's own teardown to still be mid-flight) since the first
    // check. Skipping this second check let a Happ tunnel that came up *after*
    // the first check silently fight sing-box for the TUN/WFP resources
    // instead of getting the clear "Happ is active" error.
    if active_happ_tun_present()? {
        unidtk_api::log("vpn: happ-default-tun became active mid-connect; aborting");
        let _ = perform_stop();
        return Err(
            "Happ VPN/TUN is active; disconnect Happ before starting wallpaper VPN".into(),
        );
    }

    if let Err(e) = task_start(SINGBOX_TASK) {
        let _ = perform_stop();
        return Err(e);
    }
    unidtk_api::log("vpn: sing-box task started");

    let mut tun_ready = false;
    for attempt in 1..=20 {
        if interface_is_connected(TUN_NAME).unwrap_or(false) {
            unidtk_api::log(format!("vpn: {TUN_NAME} connected after {attempt} checks"));
            tun_ready = true;
            break;
        }
        if attempt == 1 || attempt == 10 || attempt == 20 {
            unidtk_api::log(format!("vpn: waiting for {TUN_NAME}, check {attempt}/20"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    if !tun_ready {
        unidtk_api::log("vpn: wallpaper TUN interface never became connected");
        log_tail_file("singbox", &singbox_log_path(), 40);
        log_network_snapshot("singbox-failed");
        // Happ can come up in the ~5s it took sing-box to fail, i.e. after our
        // last check above — surface that as the likely cause instead of the
        // generic message, since users otherwise have no way to tell "server
        // is dead" apart from "Happ is fighting you for the TUN".
        let happ_suspect = active_happ_tun_present().unwrap_or(false);
        let _ = perform_stop();
        return Err(if happ_suspect {
            "sing-box could not bring up the wallpaper TUN interface — Happ's VPN/TUN is active \
             and likely conflicting; fully quit Happ (not just its toggle) and try again"
                .into()
        } else {
            "sing-box did not bring up the wallpaper TUN interface (see singbox.log)".into()
        });
    }
    // SOCKS inbound readiness: the probe below (and the status watchdog) talk
    // to the tunnel through it, so make sure it is up before reporting success.
    let mut socks_ready = false;
    for attempt in 1..=20 {
        if port_is_open(SOCKS_PORT) {
            unidtk_api::log(format!(
                "vpn: socks port {SOCKS_PORT} opened after {attempt} checks"
            ));
            socks_ready = true;
            break;
        }
        if attempt == 1 || attempt == 10 || attempt == 20 {
            unidtk_api::log(format!("vpn: waiting for socks port {SOCKS_PORT}, check {attempt}/20"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    if !socks_ready {
        unidtk_api::log("vpn: sing-box socks port never opened");
        log_tail_file("singbox", &singbox_log_path(), 40);
        let _ = perform_stop();
        return Err("sing-box did not open its local SOCKS port in time (see singbox.log)".into());
    }

    let (latency_ms, probe_error) = match probe_vpn_transport() {
        Ok(latency_ms) => {
            unidtk_api::log(format!("vpn: proxy probe OK latency={latency_ms}ms"));
            (Some(latency_ms), None)
        }
        Err(error) => {
            unidtk_api::log(format!("vpn: proxy probe failed: {error}"));
            (None, Some(error))
        }
    };

    // Off the hot path, on a detached thread: status()'s watchdog polls the
    // SOCKS port again within ~200ms of connect() committing, so anything
    // slow here (the network snapshot alone was 1-2s of subprocess spawns,
    // and a fresh post-connect IP lookup adds up to another ~4s) was pushing
    // that poll into a window where it saw a false "port closed" and flipped
    // the UI back to disconnected right after a real success. Egress
    // verification (pre vs. post IP) is therefore NOT computed here — it's
    // finished by `connect()` on its own detached thread, after
    // `connected = true` has already been committed and returned to the
    // frontend, so it can never delay that commit. `pre_ip_handle` is handed
    // back via `BringUpResult` for that later thread to join.
    std::thread::spawn(|| {
        log_network_snapshot("connected");
    });

    unidtk_api::log("vpn: bring_up connected OK");
    Ok(BringUpResult {
        latency_ms,
        probe_error,
        pre_ip_handle,
    })
}

// ─── Per-server ping (parallel, unelevated) ───
//
// Testing a server does NOT need the TUN (and therefore no admin): a plain
// sing-box with only SOCKS inbounds is unprivileged, so we spawn a throwaway
// copy directly (no scheduled task), route one HTTP request through each SOCKS
// proxy, measure the round-trip, and kill it. Done for every server in
// parallel so the user can see which servers actually carry traffic and jump
// straight to a live one instead of blindly toggling through dead entries.

#[derive(Clone, Serialize)]
pub struct VpnPingResult {
    pub tag: String,
    #[serde(rename = "latencyMs")]
    pub latency_ms: Option<u32>,
    pub ok: bool,
    pub error: Option<String>,
}

fn ping_dir() -> std::path::PathBuf {
    let dir = data_dir().join("ping");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn port_is_available(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn find_free_port_range(count: usize) -> u16 {
    for base in (24000..50000).step_by(count + 10) {
        let mut all_free = true;
        for p in base..(base + count as u16) {
            if !port_is_available(p) {
                all_free = false;
                break;
            }
        }
        if all_free {
            return base;
        }
    }
    24000
}

/// One throwaway sing-box with N SOCKS inbounds, each routed to its own server
/// outbound via `route.rules` — so every server is dialed through its own
/// inbound and probed in parallel.
fn build_multi_ping_config(servers: &[VpnServer], start_port: u16) -> serde_json::Value {
    let mut inbounds = Vec::new();
    let mut outbounds = Vec::new();
    let mut rules = Vec::new();

    for (i, server) in servers.iter().enumerate() {
        let port = start_port + i as u16;
        let in_tag = format!("in_{i}");
        let out_tag = format!("out_{i}");

        inbounds.push(json!({
            "type": "socks",
            "tag": in_tag,
            "listen": "127.0.0.1",
            "listen_port": port
        }));

        outbounds.push(build_singbox_outbound(server, &out_tag));

        rules.push(json!({
            "inbound": [in_tag],
            "outbound": out_tag
        }));
    }

    outbounds.push(json!({ "type": "direct", "tag": "direct" }));

    json!({
        "log": { "level": "error" },
        "inbounds": inbounds,
        "outbounds": outbounds,
        "route": {
            "rules": rules,
            "final": "direct"
        }
    })
}

fn probe_proxy_get(port: u16, timeout: Duration) -> Result<u32, String> {
    let proxy = ureq::Proxy::new(format!("socks5://127.0.0.1:{port}"))
        .map_err(|e| format!("proxy config error: {e}"))?;
    let agent = ureq::AgentBuilder::new()
        .proxy(proxy)
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .build();

    // Multiple endpoints to maximize success rate across different server configs.
    // Some servers block certain endpoints, so we try several in sequence.
    let endpoints = [
        "http://www.gstatic.com/generate_204",
        "http://cp.cloudflare.com/generate_204",
        "http://connectivitycheck.gstatic.com/generate_204",
        "http://www.msftconnecttest.com/connecttest.txt",
        "http://detectportal.firefox.com/canonical.html",
        "http://captive.apple.com/hotspot-detect.html",
    ];

    let mut last_error = String::new();
    for url in endpoints {
        let req_start = Instant::now();
        match agent
            .get(url)
            .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .call()
        {
            Ok(resp) => {
                let status = resp.status();
                // Any 2xx/3xx response (including 204) means bytes flowed end-to-end
                if status == 204 || status == 200 || (200..400).contains(&status) {
                    let elapsed = req_start.elapsed().as_millis();
                    return Ok(elapsed.min(u32::MAX as u128) as u32);
                }
                last_error = format!("http {status} from {url}");
            }
            Err(ureq::Error::Status(status, _)) => {
                // Even an error status (4xx/5xx) proves the tunnel works
                if status == 204 || status == 200 || (200..400).contains(&status) {
                    let elapsed = req_start.elapsed().as_millis();
                    return Ok(elapsed.min(u32::MAX as u128) as u32);
                }
                // Server responded with non-2xx/3xx — tunnel works but endpoint blocked
                // Still counts as success since we got a response
                let elapsed = req_start.elapsed().as_millis();
                return Ok(elapsed.min(u32::MAX as u128) as u32);
            }
            Err(e) => {
                last_error = format!("{e} at {url}");
            }
        }
    }

    Err(format!("probe via proxy GET failed: {last_error}"))
}

static PING_IN_PROGRESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn ping_all(runtime: &VpnRuntime) -> Result<Vec<VpnPingResult>, String> {
    if PING_IN_PROGRESS
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        return Err("ping_all is already in progress".into());
    }
    struct PingGuard;
    impl Drop for PingGuard {
        fn drop(&mut self) {
            PING_IN_PROGRESS.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let _guard = PingGuard;

    ensure_binaries_present()?;
    let servers = {
        let mut inner = runtime.inner.lock().unwrap();
        if inner.servers.is_empty() {
            refresh_servers_locked(&mut inner)?;
        }
        inner.servers.clone()
    };
    if servers.is_empty() {
        return Err("no servers to ping".into());
    }
    unidtk_api::log(format!("vpn: ping_all start (via proxy GET), {} servers", servers.len()));

    let base_port = find_free_port_range(servers.len());
    let cfg = build_multi_ping_config(&servers, base_port);
    let cfg_path = ping_dir().join(format!("singbox_ping_{base_port}.json"));
    if let Err(e) = std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap_or_default()) {
        return Err(format!("write multi ping config failed: {e}"));
    }
    if let Err(e) = validate_singbox_config(&cfg_path) {
        let _ = std::fs::remove_file(&cfg_path);
        return Err(e);
    }

    let child = match Command::new(singbox_exe())
        .args(["run", "-c"])
        .arg(&cfg_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::remove_file(&cfg_path);
            return Err(format!("spawn sing-box for ping failed: {e}"));
        }
    };

    // RAII: the temp process and its config must die even if a probe panics.
    struct PingChildGuard {
        child: Option<std::process::Child>,
        cfg_path: std::path::PathBuf,
    }
    impl Drop for PingChildGuard {
        fn drop(&mut self) {
            if let Some(child) = self.child.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
            let _ = std::fs::remove_file(&self.cfg_path);
        }
    }
    let guard = PingChildGuard {
        child: Some(child),
        cfg_path: cfg_path.clone(),
    };

    // Give sing-box a moment to begin binding sockets before polling (a cold
    // start takes longer than the previous xray-based flow).
    std::thread::sleep(Duration::from_millis(600));

    // Wait briefly for inbounds to initialize
    for _ in 0..60 {
        let any_open = (0..servers.len()).any(|i| port_is_open(base_port + i as u16));
        if any_open {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let results: Mutex<Vec<Option<VpnPingResult>>> = Mutex::new(vec![None; servers.len()]);

    {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let next = AtomicUsize::new(0);
        let concurrency = servers.len().clamp(1, 32);

        unidtk_api::log(format!(
            "vpn: starting parallel probes for {ports} servers (concurrency={concurrency})",
            ports = servers.len()
        ));

        std::thread::scope(|scope| {
            for _ in 0..concurrency {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= servers.len() {
                        break;
                    }
                    let port = base_port + i as u16;
                    let mut r = None;
                    if port_is_open(port) {
                        if let Ok(latency_ms) = probe_proxy_get(port, Duration::from_millis(3500)) {
                            r = Some(VpnPingResult {
                                tag: servers[i].tag.clone(),
                                latency_ms: Some(latency_ms),
                                ok: true,
                                error: None,
                            });
                        }
                    }
                    // If proxy probe failed or port was not open, report real failure (no fake TCP ping)
                    if r.is_none() {
                        r = Some(VpnPingResult {
                            tag: servers[i].tag.clone(),
                            latency_ms: None,
                            ok: false,
                            error: Some("Proxy probe failed".to_string()),
                        });
                    }
                    if let Some(ref res) = r {
                        unidtk_api::log(format!(
                            "vpn: ping {} -> ok={} latency={:?}",
                            servers[i].tag, res.ok, res.latency_ms
                        ));
                    }
                    results.lock().unwrap()[i] = r;
                });
            }
        });
    }

    drop(guard);

    let out: Vec<VpnPingResult> = results
        .into_inner()
        .unwrap()
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            r.unwrap_or_else(|| VpnPingResult {
                tag: servers[i].tag.clone(),
                latency_ms: None,
                ok: false,
                error: Some("probe aborted".into()),
            })
        })
        .collect();

    let ok_count = out.iter().filter(|r| r.ok).count();
    let fail_count = out.len() - ok_count;
    unidtk_api::log(format!(
        "vpn: ping_all done — {} ok, {} failed out of {} total",
        ok_count, fail_count, out.len()
    ));
    for r in &out {
        if !r.ok {
            unidtk_api::log(format!(
                "vpn:   FAILED {} — {}",
                r.tag,
                r.error.as_deref().unwrap_or("unknown")
            ));
        }
    }
    Ok(out)
}

// ─── Public API used by Tauri commands ───

pub fn list_servers(runtime: &VpnRuntime) -> Result<Vec<VpnServer>, String> {
    let mut inner = runtime.inner.lock().unwrap();
    if inner.servers.is_empty() {
        refresh_servers_locked(&mut inner)?;
    }
    Ok(inner.servers.clone())
}

pub fn refresh_servers(runtime: &VpnRuntime) -> Result<Vec<VpnServer>, String> {
    let mut inner = runtime.inner.lock().unwrap();
    refresh_servers_locked(&mut inner)?;
    Ok(inner.servers.clone())
}

fn refresh_servers_locked(inner: &mut RuntimeInner) -> Result<(), String> {
    let settings = load_settings();
    if settings.subscription_url.is_empty() {
        return Err("VPN subscription URL is not configured".into());
    }
    match fetch_servers(&settings.subscription_url) {
        Ok(servers) => {
            unidtk_api::log(format!("vpn: fetched {} servers", servers.len()));
            save_servers_cache(&servers);
            inner.servers = servers;
            Ok(())
        }
        Err(e) => {
            unidtk_api::log(format!("vpn: fetch_servers failed: {e}"));
            // A failed subscription update must not block connecting: fall back to
            // the servers we already have, or the last set persisted to disk, so
            // the user can still reach servers from the previous subscription.
            if !inner.servers.is_empty() {
                unidtk_api::log("vpn: keeping in-memory servers after failed refresh");
                return Ok(());
            }
            let cached = load_servers_cache();
            if !cached.is_empty() {
                unidtk_api::log(format!(
                    "vpn: using {} cached servers after failed refresh",
                    cached.len()
                ));
                inner.servers = cached;
                return Ok(());
            }
            Err(e)
        }
    }
}

pub fn status(runtime: &VpnRuntime) -> VpnStatus {
    // Lightweight watchdog: if we think we're connected but the local SOCKS
    // port has closed, sing-box died under us — flip the cached flag so
    // the UI doesn't sit on a stale "On". A `connect_timeout` to localhost is
    // cheap (instant when the port is up; only the dead case costs the 200 ms
    // in `port_is_open`). We deliberately still do NOT shell out to
    // `schtasks`/`netsh` here — that process-spawn was the original source of
    // polling jank.
    let mut inner = runtime.inner.lock().unwrap();
    if inner.connected && !inner.connecting && !port_is_open(SOCKS_PORT) {
        unidtk_api::log("vpn: watchdog detected proxy port closed while connected");
        inner.connected = false;
        CONNECTED.store(false, std::sync::atomic::Ordering::SeqCst);
        inner.proxy_latency_ms = None;
        inner.proxy_error = None;
        inner.ip_unverified = false;
        if inner.last_error.is_none() {
            inner.last_error = Some("VPN stopped unexpectedly (proxy port closed)".into());
        }
        // Drop the lock before publishing: telegram subscribes to the `vpn`
        // event via the contract, and its handler may lock its own mutex.
        drop(inner);
        publish_vpn_state();
        let inner = runtime.inner.lock().unwrap();
        return VpnStatus {
            connected: inner.connected,
            connecting: inner.connecting,
            selected_tag: inner.selected_tag.clone(),
            proxy_latency_ms: inner.proxy_latency_ms,
            proxy_error: inner.proxy_error.clone(),
            ip_unverified: inner.ip_unverified,
            error: inner.last_error.clone(),
            auto_connect: load_settings().auto_connect,
        };
    }
    // NOTE (proxy health mid-session): this watchdog only checks the local
    // SOCKS port. It does NOT re-run `probe_vpn_transport()` or any commercial-
    // proxy reachability check. `proxy_error` / `proxy_latency_ms` are set once
    // at connect time and stay stale for the rest of the session. A degraded
    // commercial outbound can therefore blackhole non-.ru TUN traffic while
    // the UI still shows connected. Re-probing here is intentionally not done:
    // status() is polled every ~3s from the frontend and a full HTTP probe can
    // take multiple seconds — that would reintroduce UI jank. A future
    // background, rate-limited health probe could refresh `proxy_error`
    // without auto-disconnecting (same surface-don't-kill philosophy as
    // `ip_unverified`).
    VpnStatus {
        connected: inner.connected,
        connecting: inner.connecting,
        selected_tag: inner.selected_tag.clone(),
        proxy_latency_ms: inner.proxy_latency_ms,
        proxy_error: inner.proxy_error.clone(),
        ip_unverified: inner.ip_unverified,
        error: inner.last_error.clone(),
        auto_connect: load_settings().auto_connect,
    }
}

pub fn connect(runtime: &VpnRuntime, tag: Option<String>) -> Result<(), String> {
    // Hold the lock only long enough to reserve the "connecting" state and
    // snapshot the chosen server; the actual bring-up (tens of seconds of
    // port/TUN waits) then runs unlocked so status polling stays responsive.
    // Snapshot CONNECT_EPOCH so we can detect a concurrent disconnect/shutdown
    // that ran while bring_up was unlocked.
    let (server, epoch) = {
        let mut inner = runtime.inner.lock().unwrap();
        if inner.connecting {
            unidtk_api::log("vpn: connect ignored: already connecting");
            return Err("VPN connection is already in progress".into());
        }
        if inner.servers.is_empty() {
            refresh_servers_locked(&mut inner)?;
        }
        let settings = load_settings();
        let target = tag
            .or_else(|| inner.selected_tag.clone())
            .or(settings.last_tag.clone())
            .filter(|t| !t.is_empty())
            .or_else(|| Some(settings.default_tag.clone()))
            .ok_or_else(|| "no server selected".to_string())?;
        let server = inner
            .servers
            .iter()
            .find(|s| s.tag == target)
            .cloned()
            .or_else(|| {
                // Target tag not in the current subscription (it may have been
                // renamed/removed server-side). Fall back to the first server
                // instead of failing the whole connect.
                unidtk_api::log(format!(
                    "vpn: server '{}' not in subscription; falling back to first server",
                    target
                ));
                inner.servers.first().cloned()
            })
            .ok_or_else(|| "no servers available".to_string())?;
        let epoch = CONNECT_EPOCH.load(std::sync::atomic::Ordering::SeqCst);
        inner.connecting = true;
        inner.last_error = None;
        inner.proxy_latency_ms = None;
        inner.proxy_error = None;
        inner.ip_unverified = false;
        (server, epoch)
    };

    let result = bring_up(&server);

    let mut inner = runtime.inner.lock().unwrap();
    if CONNECT_EPOCH.load(std::sync::atomic::Ordering::SeqCst) != epoch {
        // disconnect/shutdown cancelled us. Do not overwrite the winner's
        // state. If nobody else owns the VPN, stop processes we may have
        // brought up after their perform_stop. Re-check idle after dropping
        // the lock only as a best-effort gate: a new connect may have started.
        let should_cleanup = result.is_ok() && !inner.connecting && !inner.connected;
        unidtk_api::log(format!(
            "vpn: connect superseded (epoch changed); cleanup={should_cleanup}"
        ));
        drop(inner);
        if should_cleanup {
            let still_idle = {
                let inner = runtime.inner.lock().unwrap();
                !inner.connecting && !inner.connected
            };
            if still_idle {
                let _ = perform_stop();
            }
        }
        return match result {
            Ok(_) => Err("VPN was disconnected while connecting".into()),
            Err(e) => Err(e),
        };
    }
    inner.connecting = false;
    match result {
        Ok(outcome) => {
            inner.connected = true;
            CONNECTED.store(true, std::sync::atomic::Ordering::SeqCst);
            inner.selected_tag = Some(server.tag.clone());
            inner.proxy_latency_ms = outcome.latency_ms;
            inner.proxy_error = outcome.probe_error;
            // ip_unverified stays false (set when connecting was reserved,
            // above) until the detached thread below resolves it — this
            // commit must not wait on any IP lookup.
            inner.last_error = None;
            drop(inner);
            // Hand Telegram off to the VPN TUN (managed vless proxy → direct).
            publish_vpn_state();

            // Remember this server as the auto-connect target. Whether we
            // actually reconnect on next launch is governed solely by the
            // AUTO toggle (VpnSettings::auto_connect / set_auto_connect) —
            // a manual connect here must not flip that setting on its own.
            let mut settings = load_settings();
            settings.last_tag = Some(server.tag.clone());
            save_settings(&settings);
            MANUAL_DISCONNECT.store(false, std::sync::atomic::Ordering::SeqCst);

            // Finish egress verification off the hot path: join the
            // already-in-flight pre-connect lookup (started near the top of
            // bring_up, so this join is normally instant) and take a fresh
            // post-connect reading, then write ip_unverified back — but only
            // if this connect is still the current one (epoch unchanged), so
            // a fast disconnect/reconnect can't have a stale result clobber a
            // newer connection's state.
            let inner_for_ip = Arc::clone(&runtime.inner);
            std::thread::spawn(move || {
                let pre_ip = outcome
                    .pre_ip_handle
                    .join()
                    .unwrap_or_else(|_| Err("pre-connect IP lookup thread panicked".into()));
                let post_ip = fetch_public_ip_via_proxy();
                match &post_ip {
                    Ok(ip) => unidtk_api::log(format!("vpn: public ip [post-connect] = {ip}")),
                    Err(error) => unidtk_api::log(format!(
                        "vpn: public ip [post-connect] lookup failed: {error}"
                    )),
                }
                let ip_unverified = match (&pre_ip, &post_ip) {
                    (Ok(pre), Ok(post)) if pre != post => {
                        unidtk_api::log(format!(
                            "vpn: public ip changed pre={pre} post={post} (egress verified)"
                        ));
                        false
                    }
                    (Ok(pre), Ok(post)) => {
                        unidtk_api::log(format!(
                            "vpn: public ip UNCHANGED pre={pre} post={post} (egress not confirmed different)"
                        ));
                        true
                    }
                    (pre, post) => {
                        unidtk_api::log(format!(
                            "vpn: public ip comparison inconclusive pre={pre:?} post={post:?}"
                        ));
                        true
                    }
                };
                if CONNECT_EPOCH.load(std::sync::atomic::Ordering::SeqCst) == epoch {
                    inner_for_ip.lock().unwrap().ip_unverified = ip_unverified;
                }
            });

            Ok(())
        }
        Err(e) => {
            inner.connected = false;
            CONNECTED.store(false, std::sync::atomic::Ordering::SeqCst);
            inner.last_error = Some(e.clone());
            drop(inner);
            publish_vpn_state();
            Err(e)
        }
    }
}

pub fn disconnect(runtime: &VpnRuntime) {
    unidtk_api::log("vpn: disconnect requested");
    // Reset cached state first so the UI flips to Off immediately…
    {
        let mut inner = runtime.inner.lock().unwrap();
        inner.connected = false;
        inner.connecting = false;
        inner.proxy_latency_ms = None;
        inner.proxy_error = None;
        inner.ip_unverified = false;
        inner.last_error = None;
        // Invalidate any in-flight connect so its post-bring_up commit is a no-op.
        CONNECT_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    CONNECTED.store(false, std::sync::atomic::Ordering::SeqCst);
    // Bring the managed Telegram VLESS tunnel back the moment VPN is down.
    publish_vpn_state();
    // …then actually stop and VERIFY the processes are gone. A silent stop
    // failure would otherwise leave sing-box (and its strict_route WFP
    // filters) alive with the UI showing Off — i.e. "internet broken for no
    // reason". Surface that as an error instead of hiding it.
    let stop_nonce = perform_stop();
    if !wait_until_stopped(Duration::from_secs(10), stop_nonce.as_deref()) {
        let mut inner = runtime.inner.lock().unwrap();
        inner.last_error =
            Some("VPN did not stop cleanly; some processes may still be running".into());
    } else {
        // Confirms the exit IP actually reverted to the ISP's — same "is it
        // really working" evidence as the pre/post-connect readings, but for
        // the teardown direction. Backgrounded so a slow request doesn't hold
        // up disconnect() returning to the UI.
        std::thread::spawn(|| log_public_ip("post-disconnect"));
    }
    // Does NOT touch the persisted AUTO setting — that's an independent user
    // preference (see set_auto_connect). Only stop any in-flight startup
    // auto-connect retry loop from reconnecting right back.
    MANUAL_DISCONNECT.store(true, std::sync::atomic::Ordering::SeqCst);
    unidtk_api::log("vpn: disconnect complete");
}

pub fn select_server(runtime: &VpnRuntime, tag: String) -> Result<(), String> {
    let was_connected = { runtime.inner.lock().unwrap().connected };
    let mut settings = load_settings();
    settings.last_tag = Some(tag.clone());
    save_settings(&settings);

    if was_connected {
        connect(runtime, Some(tag))
    } else {
        let mut inner = runtime.inner.lock().unwrap();
        inner.selected_tag = Some(tag);
        Ok(())
    }
}

/// Stop the VPN cores and forget the runtime state. Called on app exit: kill
/// the cores but don't touch auto-connect settings (closing the app is not the
/// same as the user opting out of the VPN). Fire-and-forget; use
/// [`shutdown_and_wait`] when the process is about to die (app quit, Windows
/// session end) and a surviving TUN would black-hole the network.
pub fn shutdown(runtime: &VpnRuntime) {
    let _ = shutdown_and_wait(runtime, Duration::ZERO);
}

/// Same as [`shutdown`], but when `budget` is non-zero also waits until the
/// elevated helper has actually killed sing-box. Ordinary exits pay nothing:
/// when no TUN/SOCKS is up the stop task is not even fired.
pub fn shutdown_and_wait(runtime: &VpnRuntime, budget: Duration) -> bool {
    unidtk_api::log("vpn: shutdown requested");
    {
        let mut inner = runtime.inner.lock().unwrap();
        inner.connected = false;
        inner.connecting = false;
        inner.proxy_latency_ms = None;
        inner.proxy_error = None;
        inner.ip_unverified = false;
        CONNECT_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    CONNECTED.store(false, std::sync::atomic::Ordering::SeqCst);
    publish_vpn_state();

    // Nothing is up: no task round-trip on an ordinary exit (VPN was off).
    let nothing_up =
        !port_is_open(SOCKS_PORT) && !interface_is_connected(TUN_NAME).unwrap_or(false);
    if nothing_up {
        return true;
    }

    let stop_nonce = perform_stop();
    if budget.is_zero() {
        return true;
    }
    let stopped = wait_until_stopped(budget, stop_nonce.as_deref());
    if !stopped {
        unidtk_api::log("vpn: shutdown wait timed out");
    }
    stopped
}

pub fn auto_connect_on_startup(app: tauri::AppHandle) {
    use tauri::Manager;
    std::thread::spawn(move || {
        let settings = load_settings();
        unidtk_api::log(format!(
            "vpn: auto-connect settings enabled={} sub_url_empty={} default_tag={:?} last_tag={:?}",
            settings.auto_connect,
            settings.subscription_url.is_empty(),
            settings.default_tag,
            settings.last_tag
        ));
        if !settings.auto_connect {
            unidtk_api::log("vpn: auto-connect skipped, disabled by last manual disconnect");
            return;
        }
        if settings.subscription_url.is_empty() {
            unidtk_api::log("vpn: auto-connect aborted, no subscription_url");
            return;
        }
        let target = settings
            .last_tag
            .filter(|t| !t.is_empty())
            .unwrap_or(settings.default_tag);
        if target.is_empty() {
            unidtk_api::log("vpn: auto-connect aborted, empty target tag");
            return;
        }
        let runtime = app.state::<VpnRuntime>();
        MANUAL_DISCONNECT.store(false, std::sync::atomic::Ordering::SeqCst);
        for attempt in 1..=3 {
            // Re-check each attempt: the AUTO toggle being switched off
            // (set_auto_connect) or a manual disconnect during an in-flight
            // try must both stop further retries instead of reconnecting
            // behind the user.
            if !load_settings().auto_connect {
                unidtk_api::log("vpn: auto-connect aborted, disabled during attempts");
                return;
            }
            if MANUAL_DISCONNECT.load(std::sync::atomic::Ordering::SeqCst) {
                unidtk_api::log("vpn: auto-connect aborted, manual disconnect during attempts");
                return;
            }
            unidtk_api::log(format!("vpn: auto-connect attempt {attempt} -> {target}"));
            match connect(&runtime, Some(target.clone())) {
                Ok(()) => {
                    unidtk_api::log("vpn: auto-connect succeeded");
                    return;
                }
                Err(e) if attempt < 3 => {
                    unidtk_api::log(format!("vpn: auto-connect attempt {attempt} failed: {e}"));
                    std::thread::sleep(Duration::from_secs(2));
                }
                Err(e) => {
                    unidtk_api::log(format!("vpn: auto-connect final failure: {e}"));
                    return;
                }
            }
        }
    });
}

/// Модуль VPN: ядро создаёт рантайм через контракт, а обёртки команд берут его
/// клон из managed-state.
pub struct VpnModule {
    runtime: VpnRuntime,
}

impl VpnModule {
    pub fn new() -> Self {
        Self {
            runtime: VpnRuntime::default(),
        }
    }

    /// Клон рантайма для managed-state ядра.
    pub fn runtime(&self) -> VpnRuntime {
        self.runtime.clone()
    }

    /// Остановка с бюджетом: `wait` разрешён только с не-главного потока
    /// (quit_app / WM_ENDSESSION), где ожидание хелпера допустимо.
    pub fn stop_with_budget(&self, wait: bool) -> bool {
        if wait {
            shutdown_and_wait(&self.runtime, std::time::Duration::from_secs(2))
        } else {
            shutdown(&self.runtime);
            false
        }
    }
}

pub static VPN_DEPENDENCIES: &[unidtk_api::manifest::ModuleDependency] = &[
    unidtk_api::manifest::ModuleDependency {
        id: "vpn:sing-box",
        description: "Sing-Box TUN/SOCKS routing core executable",
        severity: unidtk_api::manifest::DependencySeverity::Mandatory,
        kind: unidtk_api::manifest::DependencyKind::Executable {
            file_name: "sing-box.exe",
            probe_args: Some(&["version"]),
        },
    },
    unidtk_api::manifest::ModuleDependency {
        id: "vpn:helper",
        description: "Elevated native GUI-subsystem helper bridge",
        severity: unidtk_api::manifest::DependencySeverity::Mandatory,
        kind: unidtk_api::manifest::DependencyKind::Executable {
            file_name: "vpn-helper.exe",
            probe_args: None,
        },
    },
    unidtk_api::manifest::ModuleDependency {
        id: "vpn:task:singbox",
        description: "Windows Task Scheduler elevated sing-box runner",
        severity: unidtk_api::manifest::DependencySeverity::Mandatory,
        kind: unidtk_api::manifest::DependencyKind::WindowsTask {
            task_name: "UniDTKSingbox",
            require_elevated: true,
        },
    },
    unidtk_api::manifest::ModuleDependency {
        id: "vpn:task:vpnstop",
        description: "Windows Task Scheduler elevated VPN terminator",
        severity: unidtk_api::manifest::DependencySeverity::Mandatory,
        kind: unidtk_api::manifest::DependencyKind::WindowsTask {
            task_name: "UniDTKVpnStop",
            require_elevated: true,
        },
    },
];

pub fn create_module() -> Box<dyn unidtk_api::Module> {
    Box::new(VpnModule::new())
}

impl unidtk_api::Module for VpnModule {
    fn id(&self) -> &'static str {
        "vpn"
    }

    fn dependencies(&self) -> &'static [unidtk_api::manifest::ModuleDependency] {
        VPN_DEPENDENCIES
    }

    fn init(&self, ctx: unidtk_api::ModuleContext) -> Result<(), String> {
        if ctx.is_diagnostic() {
            // В режиме диагностики читаем только in-memory настройки без побочных эффектов
            let _settings = load_settings_from_db(&*ctx.db());
            return Ok(());
        }

        let _ = VPN_DB.set(ctx.db());
        let handle = ctx
            .host::<tauri::AppHandle>()
            .cloned()
            .ok_or("vpn: AppHandle missing in ModuleContext")?;
        if !ctx.enabled() {
            unidtk_api::log("vpn: disabled — auto-connect not started");
            return Ok(());
        }
        auto_connect_on_startup(handle);
        Ok(())
    }

    fn shutdown(&self, _budget: std::time::Duration) -> Result<(), String> {
        shutdown(&self.runtime);
        Ok(())
    }
}

