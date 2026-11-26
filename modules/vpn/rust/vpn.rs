use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use unidtk_api::paths::vpn_helper_exe;

mod hwid;
mod settings;
mod subscription;
use settings::{store, SettingsStore, Snapshot, VpnSettings};
use unidtk_api::settings::{ModuleSettings, SettingKind, SettingSpec};

/// What the VPN shows in the menu's declared fields (`r[modules.settings-contract]`). The
/// home server is a record its own pane section draws.
const DECLARED_SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        key: "subscriptions",
        label: "Подписки",
        summary: "Ссылки на подписки; серверы всех подписок в одном списке",
        kind: SettingKind::TextList { placeholder: "https://…", max_items: 8, max_chars: 4096 },
        secret: true,
    },
    SettingSpec {
        key: "auto_connect",
        label: "Подключаться при запуске",
        summary: "VPN включится сам, когда запустится UniDTK",
        kind: SettingKind::Switch { default: false },
        secret: false,
    },
    SettingSpec { key: "home_server", label: "Домашний сервер", summary: "", kind: SettingKind::Record, secret: true },
];

/// The kernel's keeper of [`DECLARED_SETTINGS`], from the module's context.
static DECLARED: std::sync::OnceLock<Arc<dyn ModuleSettings>> = std::sync::OnceLock::new();

/// Moves what the old record held for the menu into the settings contract, once each (a
/// stored version means it moved, or the person has set it since): the subscription
/// address, auto-connect and the home server; then the remembered server becomes a key of
/// the first subscription. Never stops start-up: an error is logged and start-up goes on.
fn migrate_settings(store: &SettingsStore, declared: &dyn ModuleSettings) {
    let record = store.raw_record().unwrap_or(serde_json::Value::Null);
    let mut moved = Vec::new();
    let mut move_value = |key: &str, value: serde_json::Value| match declared.get(key) {
        Ok(current) if current.version == 0 => match declared.set(key, value, Some(0)) {
            Ok(_) => moved.push(key.to_string()),
            Err(error) => unidtk_api::log(format!("vpn: {key} not moved into the settings contract: {error}")),
        },
        Ok(_) => {}
        Err(error) => unidtk_api::log(format!("vpn: {key} not moved into the settings contract: {error}")),
    };
    if let Some(url) = record["subscription_url"].as_str().map(str::trim).filter(|url| !url.is_empty()) {
        match subscription::normalize_link(url) {
            Ok(url) if !url.is_empty() => move_value("subscriptions", json!([url])),
            Ok(_) => {}
            Err(error) => unidtk_api::log(format!("vpn: the stored subscription was not moved: {error}")),
        }
    }
    if record["auto_connect"] == true {
        move_value("auto_connect", json!(true));
    }
    if record["home_server"].is_object() {
        // A record written from `HomeServerConfig::default()` holds port 0 (the derive, not
        // the serde default): that is "never set", and the module's check refuses it.
        let mut home = record["home_server"].clone();
        if home["port"].as_u64().unwrap_or(0) == 0 {
            home["port"] = json!(443);
        }
        move_value("home_server", home);
    }
    if !moved.is_empty() {
        unidtk_api::log(format!("vpn: moved into the settings contract: {}", moved.join(", ")));
    }
    // A remembered server from the one-subscription days is a plain tag; it becomes the
    // key of the first subscription's server of that name.
    if let Ok((_, subscriptions)) = store.subscriptions() {
        if let Some(first) = subscriptions.first() {
            let plain = |tag: &str| !subscriptions.iter().any(|sub| tag.starts_with(&format!("{}/", sub.id)));
            let _ = store.update(|settings| {
                if let Some(tag) = settings.last_tag.take() {
                    settings.last_tag = Some(if plain(&tag) { format!("{}/{tag}", first.id) } else { tag });
                }
                if !settings.default_tag.is_empty() && plain(&settings.default_tag) {
                    settings.default_tag = format!("{}/{}", first.id, settings.default_tag);
                }
            });
        }
    }
}

/// The remembered server's subscription, when it is one of the current ones.
fn subscription_of<'a>(key: &str, subscriptions: &'a [settings::Subscription]) -> Option<&'a settings::Subscription> {
    subscriptions.iter().find(|sub| key.starts_with(&format!("{}/", sub.id)))
}

/// Connect at start-up; off when the contract cannot be read.
fn auto_connect_on() -> bool {
    DECLARED.get().and_then(|declared| declared.get("auto_connect").ok()).is_some_and(|stored| stored.value == true)
}


const CREATE_NO_WINDOW: u32 = 0x08000000;

const SOCKS_PORT: u16 = 12908;
const TUN_NAME: &str = "wallpaper-tun";

/// Publishes the current tunnel state to the contract.
///
/// The module used to call `telegram::on_vpn_state_changed()`, and telegram called
/// `vpn::is_connected()`, forming a cycle between two modules. Now vpn
/// only declares its state, and telegram subscribes to the event: Cargo no longer
/// lets them depend on each other.
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

/// Home-server submodule settings (a switchable split tunnel on the LAN).
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

/// UniDTK data directory (comes from the contract, not a local copy of the logic).
fn data_dir() -> std::path::PathBuf {
    unidtk_api::paths::data_dir()
}

/// sing-box binary to run: our bundled extended fork (XHTTP support), resolved
/// through the contract so telegram does not need this module to find it.
pub(crate) fn singbox_exe() -> std::path::PathBuf {
    unidtk_api::paths::singbox_exe()
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

// ── Commands ────────────────────────────────────────────────────────────────
// The writes answer `null` or the settings, exactly as the Tauri commands of the
// same names did, so the interface sees the same shapes.
// The Rune picker supplies an optional revision through the same contract (r[ipc.one-contract]).

unidtk_api::module_commands! {
    impl VpnModule {
        read_only "vpn_status" "VPN runtime status" |this| Ok(status(&this.runtime));
        read_only(long) "vpn_list_servers" "VPN servers" |this, subscription_revision: Option<u64>| list_servers(&this.runtime, subscription_revision);
        read_ui "vpn_groups" "Each subscription: its provider's words, its server count and this machine's device id for it" |this| Ok(groups_status(&this.runtime));
        write(long) "vpn_refresh_servers" "Fetch the subscription again" |this| refresh_servers(&this.runtime);
        write(long) "vpn_connect" "Connect to a server (or the selected one)" |this, key: Option<String>, subscription_revision: Option<u64>| {
            unidtk_api::log(format!("vpn: command connect key={key:?}"));
            connect(&this.runtime, key, subscription_revision)
        };
        write(long) "vpn_disconnect" "Drop the current connection" |this| {
            disconnect(&this.runtime);
            Ok(())
        };
        write "vpn_select_server" "Select the server to connect to" |this, key: String| select_server(&this.runtime, key);
        // Not a read, though it answers with measurements: it takes a global
        // in-progress guard and stores the results.
        write(long) "vpn_ping_all" "Measure latency of every server" |this, subscription_revision: Option<u64>| ping_all(&this.runtime, subscription_revision);
    }
}

fn load_settings() -> Result<VpnSettings, String> {
    store()?.read()
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
    /// `<subscription id>/<tag>`: what connect, the check and the panel name it by.
    #[serde(default)]
    pub key: String,
    /// Its subscription's id.
    #[serde(default)]
    pub group: String,
}

/// A server as the list commands show it: no id, password or key. `vpn_list_servers` is
/// served on the socket, where any local process may read it; the full record stays in
/// the module and its cache.
#[derive(Clone, Serialize)]
pub struct ServerView {
    pub key: String,
    pub tag: String,
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub network: String,
    pub security: String,
}

impl From<&VpnServer> for ServerView {
    fn from(server: &VpnServer) -> Self {
        Self {
            key: server.key.clone(),
            tag: server.tag.clone(),
            protocol: server.protocol.clone(),
            host: server.host.clone(),
            port: server.port,
            network: server.network.clone(),
            security: server.security.clone(),
        }
    }
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
    /// The selected server's key (`<subscription id>/<tag>`).
    pub selected_tag: Option<String>,
    /// Its name, for the panel to show.
    pub selected_name: Option<String>,
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
    /// The current subscription, when the settings snapshot is available.
    pub subscription_revision: Option<u64>,
    /// Subscription entries that are not VLESS or Trojan, or do not parse.
    #[serde(rename = "skippedServers")]
    pub skipped_servers: usize,
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
    servers_revision: u64,
    selected_tag: Option<String>,
    proxy_latency_ms: Option<u32>,
    proxy_error: Option<String>,
    /// See `VpnStatus::ip_unverified`. Sticky from last successful connect.
    ip_unverified: bool,
    connected: bool,
    /// The revision used to start the live tunnel. Meaningful while connected.
    connected_revision: u64,
    connecting: bool,
    last_error: Option<String>,
    /// Entries of the last subscription this module could not use.
    skipped_servers: usize,
    /// Each subscription's notes and failure from the last fetch for this revision.
    groups: Vec<Group>,
    /// The provider answered for this revision, servers or not: a list of placeholders
    /// only is an answer, and fetching it again on every call would only repeat it.
    fetched: bool,
}

impl RuntimeInner {
    fn use_subscription(&mut self, revision: u64) {
        if self.servers_revision != revision {
            self.servers_revision = revision;
            self.servers.clear();
            self.skipped_servers = 0;
            self.groups.clear();
            self.fetched = false;
        }
    }

    /// Why there is no server to use: a provider's refusal, else a note, else a failure.
    fn no_servers(&self) -> String {
        let groups = &self.groups;
        groups
            .iter()
            .find_map(|g| g.refusal.clone())
            .or_else(|| groups.iter().find_map(|g| g.comments.first().map(|note| format!("В подписке нет серверов: {note}"))))
            .or_else(|| groups.iter().find_map(|g| g.error.clone()))
            .unwrap_or_else(|| "no servers available".to_string())
    }

    fn reserve_connect(
        &mut self,
        settings_store: &SettingsStore,
        snapshot: &Snapshot,
        tag: Option<&str>,
    ) -> Result<Option<(VpnServer, u64)>, String> {
        // A URL write can commit while the runtime waits on a subscription fetch.
        // Order reservation against that write, then release both locks for bring-up.
        settings_store.with_current_subscription(snapshot.revision, || {
            if self.servers.is_empty() {
                return Err(self.no_servers());
            }
            let settings = &snapshot.settings;
            let remembered = self
                .selected_tag
                .as_deref()
                .or(settings.last_tag.as_deref())
                .filter(|tag| !tag.is_empty())
                .or(Some(settings.default_tag.as_str()));
            let (server, fell_back) = pick_server(&self.servers, tag, remembered)?;
            if fell_back {
                unidtk_api::log(format!(
                    "vpn: remembered server {remembered:?} not in subscription; connecting to '{}' instead",
                    server.tag
                ));
            }
            // Two subscriptions can use the same tag for different servers.
            if self.connected
                && self.connected_revision == snapshot.revision
                && self.selected_tag.as_deref() == Some(server.key.as_str())
            {
                unidtk_api::log(format!("vpn: connect to '{}' ignored: already connected to it", server.tag));
                return Ok(None);
            }
            let epoch = CONNECT_EPOCH.load(std::sync::atomic::Ordering::SeqCst);
            self.connecting = true;
            self.last_error = None;
            self.proxy_latency_ms = None;
            self.proxy_error = None;
            self.ip_unverified = false;
            Ok(Some((server, epoch)))
        })
    }
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
        key: String::new(),
        group: String::new(),
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
        key: String::new(),
        group: String::new(),
    })
}

/// A subscription body is base64 of a link list, or the plain list itself.
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

/// The servers of a decoded subscription that this module can use, and how many
/// entries it had to leave out (other schemes such as ss:// or vmess://, and
/// links of a supported scheme that do not parse). Blank lines and comments do not count.
fn parse_subscription(decoded: &str) -> (Vec<VpnServer>, usize) {
    let mut servers = Vec::new();
    let mut skipped = 0;
    for line in decoded
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let parsed = if line.starts_with("vless://") {
            parse_vless_line(line)
        } else if line.starts_with("trojan://") {
            parse_trojan_line(line)
        } else {
            None
        };
        match parsed {
            Some(server) => servers.push(server),
            None => skipped += 1,
        }
    }
    (servers, skipped)
}

/// Which server a connect request means. A request that names a tag the
/// subscription does not have is an error: connecting somewhere else without a word
/// would hand the user a different exit. A remembered tag that is gone falls back to
/// the first server, and the flag says so, so the caller can report it.
fn pick_server(
    servers: &[VpnServer],
    requested: Option<&str>,
    remembered: Option<&str>,
) -> Result<(VpnServer, bool), String> {
    if let Some(tag) = requested.filter(|t| !t.is_empty()) {
        return servers
            .iter()
            .find(|s| s.key == tag)
            .cloned()
            .map(|s| (s, false))
            .ok_or_else(|| format!("server '{tag}' is not in the subscription"));
    }
    if let Some(tag) = remembered.filter(|t| !t.is_empty()) {
        if let Some(server) = servers.iter().find(|s| s.key == tag) {
            return Ok((server.clone(), false));
        }
    }
    servers
        .first()
        .cloned()
        .map(|s| (s, true))
        .ok_or_else(|| "no servers available".to_string())
}

// ─── Config builders ───

/// sing-box outbound for one subscription server (vless/trojan + TLS + transport).
///
/// Transport mapping from the subscription's Xray-style fields:
/// - `tcp` → no transport (plain TCP), `ws` → ws, `grpc` → grpc;
/// - `xhttp`/`splithttp` → the extended fork's `xhttp` transport. Upstream
///   sing-box has no such transport, and the fork requires an explicit padding
///   range (padding cannot be disabled).
/// The name a server presents in TLS and in the `Host` header: the sni when the
/// link names one, else the host itself. Once the dial address is an IP literal,
/// this is the only domain left in the outbound.
fn tls_name(server: &VpnServer) -> &str {
    if server.sni.is_empty() {
        &server.host
    } else {
        &server.sni
    }
}

// ─── Addresses: the dial literal and a free TUN subnet ───
//
// #538: the generated config asked sing-box to resolve the proxy's own hostname
// through a DNS server whose `detour` is that very proxy. sing-box detects the
// cycle and every dial fails with "DNS resolution loop detected", so the TUN and
// the SOCKS port came up while no traffic ever left the machine. The dial
// address is therefore resolved here, at config-build time, and written as a
// literal; the domain stays in `server_name` (and in the ws `Host` header), so
// TLS/Reality and SNI still see the name the link asked for.
//
// The TUN address used to be the literal `172.19.0.1/30`, which Happ's
// `happ-xray` adapter holds while it runs: an elevated sing-box then died at
// `set ipv4 address: The object already exists`. A free /30 is picked per
// connect instead, and any /30 that overlaps an adapter's or a route's range is
// skipped.

/// How long a system DNS lookup may take before the connect reports an honest
/// failure instead of hanging.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(4);

/// How many /30 candidates are tried in each range before the next range is
/// scanned; a full range is millions of subnets, and a scan must stay cheap.
const TUN_CANDIDATES_PER_RANGE: u32 = 2048;

/// A /30 range the TUN must not claim, as a network address and a prefix length.
type V4Range = (u32, u8);

fn parse_v4(text: &str) -> Option<u32> {
    let mut parts = text.split('.');
    let mut value = 0u32;
    for _ in 0..4 {
        let part: u32 = parts.next()?.parse().ok()?;
        if part > 255 {
            return None;
        }
        value = (value << 8) | part;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(value)
}

fn show_v4(value: u32) -> String {
    format!(
        "{}.{}.{}.{}",
        (value >> 24) & 0xff,
        (value >> 16) & 0xff,
        (value >> 8) & 0xff,
        value & 0xff
    )
}

/// A subnet's first and last address, both inclusive.
fn range_bounds((net, prefix): V4Range) -> (u32, u32) {
    let prefix = prefix.min(32);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    let start = net & mask;
    (start, start | !mask)
}

/// Whether two ranges share an address.
fn ranges_overlap(a: V4Range, b: V4Range) -> bool {
    let (a_start, a_end) = range_bounds(a);
    let (b_start, b_end) = range_bounds(b);
    a_start <= b_end && b_start <= a_end
}

/// The IPv4 `/30` subnets this machine already reaches: every adapter's own
/// subnet (on-link) and every route's destination. A TUN address inside one of
/// them would either fail to be set or shadow a LAN under `strict_route`.
#[cfg(windows)]
fn occupied_v4_ranges() -> Vec<V4Range> {
    use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
    use windows::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetAdaptersAddresses, GetIpForwardTable2, GAA_FLAG_INCLUDE_PREFIX,
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        IP_ADAPTER_ADDRESSES_LH, MIB_IPFORWARD_TABLE2,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    let mut ranges: Vec<V4Range> = Vec::new();

    // Adapters: their unicast addresses with the on-link prefix length.
    let flags = GAA_FLAG_INCLUDE_PREFIX
        | GAA_FLAG_SKIP_ANYCAST
        | GAA_FLAG_SKIP_MULTICAST
        | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 16 * 1024;
    // `u64` backing so the header is 8-byte aligned; `size` is the byte count.
    let mut buffer: Vec<u64> = Vec::new();
    for _ in 0..4 {
        buffer.resize((size as usize).div_ceil(8).max(1), 0);
        let adapter = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let result = unsafe {
            GetAdaptersAddresses(AF_INET.0 as u32, flags, None, Some(adapter), &mut size)
        };
        // GetAdaptersAddresses reports its errors as plain Win32 codes (`u32`).
        if result == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if result != NO_ERROR.0 {
            unidtk_api::log(format!("vpn: GetAdaptersAddresses failed: {result}"));
            break;
        }
        let mut current = adapter;
        while !current.is_null() {
            let entry = unsafe { &*current };
            let mut unicast = entry.FirstUnicastAddress;
            while !unicast.is_null() {
                let address = unsafe { &(*unicast).Address };
                if address.iSockaddrLength >= std::mem::size_of::<SOCKADDR_IN>() as i32
                    && !address.lpSockaddr.is_null()
                {
                    let sockaddr = unsafe { &*(address.lpSockaddr as *const SOCKADDR_IN) };
                    if sockaddr.sin_family == AF_INET {
                        let ip = u32::from_be(unsafe { sockaddr.sin_addr.S_un.S_addr });
                        let prefix = unsafe { (*unicast).OnLinkPrefixLength }.min(32);
                        ranges.push((ip, prefix));
                    }
                }
                unicast = unsafe { (*unicast).Next };
            }
            current = entry.Next;
        }
        break;
    }

    // Routes: destination prefixes, so a /30 inside a routed (not on-link) subnet
    // cannot take the TUN over a corporate or VPN range.
    unsafe {
        let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
        if GetIpForwardTable2(AF_INET, &mut table) == NO_ERROR && !table.is_null() {
            let count = (*table).NumEntries as usize;
            let rows = (*table).Table.as_ptr();
            for index in 0..count {
                let row = &*rows.add(index);
                let destination = row.DestinationPrefix.Prefix;
                if destination.si_family == AF_INET {
                    let ip = u32::from_be(destination.Ipv4.sin_addr.S_un.S_addr);
                    // The default route covers everything and says nothing about a
                    // LAN; an aggregate shorter than /8 is a carrier or tunnel
                    // advertisement, not an on-link subnet. Only a real subnet can
                    // collide with the TUN, and only a real subnet is worth the scan.
                    let prefix = row.DestinationPrefix.PrefixLength;
                    if (8..=30).contains(&prefix) {
                        ranges.push((ip, prefix));
                    }
                }
            }
            FreeMibTable(table.cast());
        }
    }

    // Ranges no TUN may ever use, whatever the adapters say.
    ranges.push((parse_v4("0.0.0.0").unwrap_or(0), 8));
    ranges.push((parse_v4("127.0.0.0").unwrap_or(0), 8));
    ranges.push((parse_v4("169.254.0.0").unwrap_or(0), 16));
    ranges.push((parse_v4("224.0.0.0").unwrap_or(0), 4));
    ranges.push((parse_v4("240.0.0.0").unwrap_or(0), 4));
    ranges.push((parse_v4("100.64.0.0").unwrap_or(0), 10));
    ranges
}

#[cfg(not(windows))]
fn occupied_v4_ranges() -> Vec<V4Range> {
    vec![(parse_v4("0.0.0.0").unwrap_or(0), 8), (parse_v4("127.0.0.0").unwrap_or(0), 8)]
}

/// The `/30` the TUN takes: the address written into sing-box, and the literal
/// the readiness check expects on the adapter. `172.19.0.0/30` stays first, so a
/// machine without a conflicting client keeps the address it always had.
fn pick_tun_address(occupied: &[V4Range]) -> (String, String) {
    let scans: [V4Range; 5] = [
        // The address every earlier connect used, tried first so an ordinary
        // machine keeps it.
        (parse_v4("172.19.0.0").unwrap_or(0), 30),
        // Its own /16 next: one subnet over from the old address keeps logs and
        // packet captures comparable to the ones support already has.
        (parse_v4("172.19.0.0").unwrap_or(0), 16),
        (parse_v4("172.16.0.0").unwrap_or(0), 12),
        (parse_v4("10.0.0.0").unwrap_or(0), 8),
        (parse_v4("192.168.0.0").unwrap_or(0), 16),
    ];
    for scan in scans {
        let (start, end) = range_bounds(scan);
        let mut base = start;
        let mut tried = 0u32;
        while base <= end && tried < TUN_CANDIDATES_PER_RANGE {
            let candidate = (base, 30);
            if !occupied.iter().any(|held| ranges_overlap(candidate, *held)) {
                return (format!("{}/30", show_v4(base + 1)), show_v4(base + 1));
            }
            base = base.saturating_add(4);
            tried += 1;
        }
    }
    // Every scan was exhausted (a machine with a subnet over most of the private
    // space). The old fixed address is still the best guess; the connect reports
    // its own failure if it is taken.
    unidtk_api::log("vpn: no free TUN /30 found; falling back to 172.19.0.1/30");
    let fallback = parse_v4("172.19.0.1").unwrap_or(0);
    (format!("{}/30", show_v4(fallback)), show_v4(fallback))
}

/// The IPv4 literal sing-box dials: the server's host when it already is one, else
/// the system resolver's first IPv4 answer, bounded by [`RESOLVE_TIMEOUT`].
/// `getaddrinfo` cannot be cancelled, so the lookup runs on a thread that is
/// abandoned when the deadline passes; the result is read through a channel.
fn resolve_ipv4(host: &str) -> Result<String, String> {
    let host = host.trim();
    if parse_v4(host).is_some() {
        return Ok(host.to_string());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let target = host.to_string();
    std::thread::spawn(move || {
        use std::net::ToSocketAddrs;
        // A name that does not resolve and a name with only IPv6 answers are different
        // faults; reading both as "no IPv4" called a deleted server IPv6-only.
        let answer = (target.as_str(), 0).to_socket_addrs().map(|mut addresses| {
            addresses.find_map(|address| match address.ip() {
                std::net::IpAddr::V4(v4) => Some(v4.to_string()),
                std::net::IpAddr::V6(_) => None,
            })
        });
        let _ = tx.send(answer);
    });
    match rx.recv_timeout(RESOLVE_TIMEOUT) {
        Ok(Ok(Some(address))) => Ok(address),
        Ok(Ok(None)) => Err(format!(
            "the VPN server '{host}' has no IPv4 address; a tunnel over IPv6 only is not supported"
        )),
        Ok(Err(error)) => Err(format!("the VPN server '{host}' does not resolve: {error}")),
        Err(_) => Err(format!(
            "could not resolve the VPN server '{host}' within {} s; check the network or the subscription",
            RESOLVE_TIMEOUT.as_secs()
        )),
    }
}

/// Every server's dial literal for one ping run: one lookup per distinct host, so
/// `vpn_ping_all` does not pay the timeout once per server. A host that cannot be
/// dialled (no name, or IPv6 only) keeps its reason, which its row reports.
fn resolve_dials(servers: &[VpnServer]) -> std::collections::HashMap<String, Result<String, String>> {
    use std::collections::HashMap;
    let mut dials = HashMap::new();
    for server in servers {
        if dials.contains_key(&server.host) {
            continue;
        }
        let dial = resolve_ipv4(&server.host);
        if let Err(error) = &dial {
            unidtk_api::log(format!("vpn: ping cannot dial '{}': {error}", server.host));
        }
        dials.insert(server.host.clone(), dial);
    }
    dials
}

fn build_singbox_outbound(server: &VpnServer, tag: &str, dial: &str) -> serde_json::Value {
    let mut outbound = if server.protocol == "trojan" {
        json!({
            "type": "trojan",
            "tag": tag,
            "server": dial,
            "server_port": server.port,
            "password": server.uuid,
        })
    } else {
        let mut obj = json!({
            "type": "vless",
            "tag": tag,
            "server": dial,
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
    let name = tls_name(server);
    if server.security == "reality" {
        outbound["tls"] = json!({
            "enabled": true,
            "server_name": name,
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
            "server_name": name,
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
                "headers": { "Host": name }
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
///
/// `dial` is the IPv4 literal sing-box connects to (see [`resolve_ipv4`]: the
/// proxy's own hostname must never be resolved through the proxy, `#538`), and
/// `tun` the TUN address chosen for this connect ([`pick_tun_address`]).
fn build_singbox_config(
    server: &VpnServer,
    home: &HomeServerConfig,
    dial: &str,
    tun: &str,
) -> serde_json::Value {
    let singbox_log = singbox_log_path().to_string_lossy().into_owned();

    let mut outbounds = vec![
        build_singbox_outbound(server, "proxy", dial),
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

    if home.enabled && !home.host.trim().is_empty() {
        let hs = home;
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
            //
            // Only the user's traffic resolves here. The proxy's own address is
            // already the literal `dial`, so the cycle sing-box used to detect
            // (`dns-proxy` -> `proxy` -> needs the proxy's hostname) cannot form.
            "servers": [{ "type": "tcp", "tag": "dns-proxy", "server": "8.8.8.8", "detour": "proxy" }]
        },
        "inbounds": [
            {
                "type": "tun",
                "tag": "tun-in",
                "interface_name": TUN_NAME,
                "address": [tun],
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

// One process-lifecycle operation at a time: a connect's bring-up, a disconnect's
// stop, the watchdog's cleanup, a server check and a shutdown. They share the stop
// task's done-marker, and the helper's stop kills every bundled sing-box, so two
// overlapping ones failed each other: a stop during a server check killed the check's
// core, and a disconnect between a connect's epoch check and its start left a tunnel
// up under an "Off" switch. The UI door runs these commands off its thread
// (r[ipc.long-commands-off-thread]), so waiting here holds no other command.
static LIFECYCLE: Mutex<()> = Mutex::new(());

fn lifecycle() -> std::sync::MutexGuard<'static, ()> {
    LIFECYCLE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

// One subscription fetch at a time. The fetch runs without the runtime lock, so
// `vpn_status` never waits on the network; this keeps two callers that both found an
// empty list from fetching twice.
static FETCH: Mutex<()> = Mutex::new(());

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

/// Whether the adapter `name` actually carries `address` (the literal chosen for
/// this connect). A stale adapter from a dead core still reports "connected",
/// and a TUN sing-box failed to address would otherwise read as ready.
fn interface_has_address(name: &str, address: &str) -> bool {
    let output = Command::new("cmd.exe")
        .args(["/C", "chcp 65001 >NUL & netsh interface ipv4 show addresses"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let Ok(output) = output else { return false };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let target = name.to_ascii_lowercase();
    // `netsh` prints one block per adapter, its header at column 0 and the fields
    // indented; the header is localized, so a block is recognised by indentation
    // rather than by its wording.
    let mut in_adapter = false;
    for line in stdout.lines() {
        if !line.starts_with(' ') && !line.starts_with('\t') && !line.trim().is_empty() {
            in_adapter = line.to_ascii_lowercase().contains(&target);
            continue;
        }
        if in_adapter && line.contains(address) {
            return true;
        }
    }
    false
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
    match read_tail(path, TAIL_BYTES) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(max_lines);
            unidtk_api::log(format!(
                "vpn: {label} log tail ({} lines) [{}]",
                lines.len() - start,
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

/// How much of a core's log the tail reads: the whole file reached 15 MB, read into
/// memory after every failed connect for its last 40 lines.
const TAIL_BYTES: u64 = 64 * 1024;

/// The last `bytes` of `path`, from the first whole line in them.
fn read_tail(path: &std::path::Path, bytes: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(bytes)))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let text = String::from_utf8_lossy(&buffer).into_owned();
    Ok(match (len > bytes, text.split_once('\n')) {
        (true, Some((_, rest))) => rest.to_string(),
        _ => text,
    })
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

/// How long a fresh tunnel gets to carry one request before connect gives up on it.
const PROBE_BUDGET: Duration = Duration::from_secs(10);

/// One connect's whole bring-up: the old core's stop, the TUN, the SOCKS port and the
/// probe share it, so a connect answers within it however its steps go. The host waits
/// longer than this for `vpn_connect` (a server check may hold the lifecycle first).
const CONNECT_BUDGET: Duration = Duration::from_secs(30);

/// Whether the tunnel carries traffic, retried until `deadline`. Any HTTP response
/// (even 4xx) proves the transport works. One pass over two IP literals used to decide
/// it, and a cold tunnel fails exactly those (a TLS reset from 1.1.1.1 and 8.8.8.8 while
/// a hostname's plain 204 answers), so the endpoints mix IP literals with hostnames
/// (DNS-over-tunnel may not be warm yet) and plain HTTP with TLS, and the rounds repeat.
fn probe_vpn_transport(deadline: Instant) -> Result<u32, String> {
    let agent = socks_agent(Duration::from_secs(4))?;
    let endpoints = [
        "https://1.1.1.1/",
        "http://cp.cloudflare.com/generate_204",
        "http://1.1.1.1/cdn-cgi/trace",
        "https://8.8.8.8/",
        "http://www.gstatic.com/generate_204",
    ];
    let mut last_error = None;
    for url in endpoints.iter().cycle() {
        let start = Instant::now();
        let Some(timeout) = endpoint_timeout(deadline, start).map(|left| left.min(Duration::from_secs(4))) else {
            break;
        };
        unidtk_api::log(format!("vpn: probe http {url} start"));
        match agent
            .get(url)
            .timeout(timeout)
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
                // A refused or reset request returns at once; don't spin on it.
                std::thread::sleep(Duration::from_millis(150));
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

/// Outcome of a successful bring-up: the tunnel carried a request. A tunnel whose probe
/// never got an answer is refused, not reported as connected with a warning (the switch
/// used to say "on" while nothing passed). The `ip_unverified` signal derived from
/// `pre_ip_handle` stays evidence, not a hard gate.
struct BringUpResult {
    latency_ms: Option<u32>,
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
fn bring_up(
    server: &VpnServer,
    home: &HomeServerConfig,
    epoch: u64,
    dial: Result<String, String>,
    deadline: Instant,
) -> Result<BringUpResult, String> {
    ensure_binaries_present()?;
    let superseded = || CONNECT_EPOCH.load(std::sync::atomic::Ordering::SeqCst) != epoch;
    // A core this bring-up started is stopped again before it reports anything: the
    // disconnect that superseded it may have run its own stop before the start.
    let abandon = |message: &str| -> String {
        unidtk_api::log(format!("vpn: {message}; stopping the core this connect started"));
        let nonce = perform_stop();
        let _ = wait_until_stopped(Duration::from_secs(5), nonce.as_deref());
        message.to_string()
    };
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
    let stop_wait = deadline.saturating_duration_since(Instant::now()).min(Duration::from_secs(10));
    if !wait_until_stopped(stop_wait, stop_nonce.as_deref()) {
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

    // The proxy's own hostname cannot be resolved through the proxy (`#538`):
    // the literal was resolved by the caller, in parallel with the stop above.
    // A missed deadline here is a cancellation, not a failure to report.
    if CONNECT_EPOCH.load(std::sync::atomic::Ordering::SeqCst) != epoch {
        return Err("VPN was disconnected while connecting".into());
    }
    let dial = dial?;

    // The TUN is chosen now, after the old adapter is gone (its address must not
    // block the candidate) and before the config is written. The home-server
    // subnet is excluded too: `strict_route` would take it away from its own
    // split-tunnel outbound.
    let mut occupied = occupied_v4_ranges();
    if home.enabled {
        let subnet = home.subnet.trim();
        if let Some((network, prefix)) = subnet.split_once('/') {
            if let (Some(network), Ok(prefix)) = (parse_v4(network), prefix.parse::<u8>()) {
                occupied.push((network, prefix.min(32)));
            }
        }
    }
    let (tun, tun_literal) = pick_tun_address(&occupied);
    unidtk_api::log(format!("vpn: tun address {tun} ({} held ranges)", occupied.len()));

    let singbox_cfg = build_singbox_config(server, home, &dial, &tun);
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

    // A disconnect that landed during the stop/validation above already owns the
    // state; starting the core now would fight it (`#538` widened this window).
    if CONNECT_EPOCH.load(std::sync::atomic::Ordering::SeqCst) != epoch {
        unidtk_api::log("vpn: connect superseded before the core started; aborting");
        return Err("VPN was disconnected while connecting".into());
    }

    if let Err(e) = task_start(SINGBOX_TASK) {
        let _ = perform_stop();
        return Err(e);
    }
    unidtk_api::log("vpn: sing-box task started");

    let mut tun_ready = false;
    for attempt in 1..=20 {
        if superseded() {
            return Err(abandon("VPN was disconnected while connecting"));
        }
        // The adapter must carry the address chosen for this connect: a leftover
        // `wallpaper-tun` from a dead core answers "connected" too, and the
        // config then could not have set its address.
        if interface_is_connected(TUN_NAME).unwrap_or(false)
            && interface_has_address(TUN_NAME, &tun_literal)
        {
            unidtk_api::log(format!("vpn: {TUN_NAME} connected at {tun_literal} after {attempt} checks"));
            tun_ready = true;
            break;
        }
        if attempt == 1 || attempt == 10 || attempt == 20 {
            unidtk_api::log(format!("vpn: waiting for {TUN_NAME} at {tun_literal}, check {attempt}/20"));
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
        if superseded() {
            return Err(abandon("VPN was disconnected while connecting"));
        }
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

    let probe_deadline = deadline.min(Instant::now() + PROBE_BUDGET);
    let latency_ms = match probe_vpn_transport(probe_deadline) {
        Ok(latency_ms) => {
            unidtk_api::log(format!("vpn: proxy probe OK latency={latency_ms}ms"));
            latency_ms
        }
        Err(error) => {
            unidtk_api::log(format!("vpn: proxy probe failed: {error}"));
            log_tail_file("singbox", &singbox_log_path(), 20);
            return Err(abandon(&format!("the server carries no traffic: {error}")));
        }
    };
    if superseded() {
        return Err(abandon("VPN was disconnected while connecting"));
    }

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
        latency_ms: Some(latency_ms),
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
    pub key: String,
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
fn build_multi_ping_config(
    servers: &[VpnServer],
    start_port: u16,
    dials: &std::collections::HashMap<String, Result<String, String>>,
) -> serde_json::Value {
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

        // Same literal as a connect uses: the throwaway config has no DNS block,
        // so a hostname here would be resolved by the system and could disagree
        // with what the real tunnel dials.
        let dial = match dials.get(&server.host) {
            Some(Ok(literal)) => literal.as_str(),
            _ => server.host.as_str(),
        };
        outbounds.push(build_singbox_outbound(server, &out_tag, dial));

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

/// One server's whole check, every endpoint included. Endpoints used to get 3.5 s to
/// connect plus 3.5 s to read each, six in a row, so a dead server cost ~20 s and the
/// whole check outlived the host's wait (the picker stayed empty).
const SERVER_CHECK_BUDGET: Duration = Duration::from_secs(5);

/// The endpoint request's own share of `budget` left at `now`: none once less than a
/// useful slice remains.
fn endpoint_timeout(deadline: Instant, now: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(now)
        .filter(|left| *left >= Duration::from_millis(200))
}

fn probe_proxy_get(port: u16, budget: Duration) -> Result<u32, String> {
    let proxy = ureq::Proxy::new(format!("socks5://127.0.0.1:{port}"))
        .map_err(|e| format!("proxy config error: {e}"))?;

    // Multiple endpoints to maximize success rate across different server configs.
    // Some servers block certain endpoints, so we try several in sequence, within one
    // budget for the server.
    let endpoints = [
        "http://www.gstatic.com/generate_204",
        "http://cp.cloudflare.com/generate_204",
        "http://connectivitycheck.gstatic.com/generate_204",
        "http://www.msftconnecttest.com/connecttest.txt",
        "http://detectportal.firefox.com/canonical.html",
        "http://captive.apple.com/hotspot-detect.html",
    ];

    let deadline = Instant::now() + budget;
    let mut last_error = String::new();
    for url in endpoints {
        let req_start = Instant::now();
        let Some(timeout) = endpoint_timeout(deadline, req_start) else {
            last_error = format!("no answer within {} s ({last_error})", budget.as_secs());
            break;
        };
        // ureq bounds the SOCKS handshake by the agent's `timeout_connect` (30 s unless
        // set), not by the request's `.timeout()`: an agent per endpoint carries what is
        // left of the budget into the handshake, where a dead server's probe waits.
        let agent = ureq::AgentBuilder::new().proxy(proxy.clone()).timeout_connect(timeout).build();
        match agent
            .get(url)
            .timeout(timeout)
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

/// Puts a child the kernel itself starts (the server check's sing-box) into a job that
/// closes with the kernel: its `Drop` guard does not run when the process is killed or
/// exits, and an orphaned core kept its ports and ~70 MB until the next stop's sweep.
#[cfg(windows)]
fn dies_with_kernel(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    // The job's handle stays open for the life of the process; the OS closes it on exit.
    static JOB: std::sync::OnceLock<Option<isize>> = std::sync::OnceLock::new();
    let job = JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(None, windows::core::PCWSTR::null()).ok()?;
        let mut limit = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limit.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limit as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .ok()?;
        Some(job.0 as isize)
    });
    if let Some(job) = job {
        if let Err(error) = unsafe { AssignProcessToJobObject(HANDLE(*job as _), HANDLE(child.as_raw_handle())) } {
            unidtk_api::log(format!("vpn: the check's core is not tied to the kernel: {error}"));
        }
    }
}

pub fn ping_all(
    runtime: &VpnRuntime,
    subscription_revision: Option<u64>,
) -> Result<Vec<VpnPingResult>, String> {
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

    let settings_store = store()?;
    let snapshot = settings_store.snapshot(subscription_revision)?;
    ensure_binaries_present()?;
    ensure_servers(runtime, settings_store, &snapshot)?;
    let (servers, no_servers, revision) = {
        let inner = runtime.inner.lock().unwrap();
        (inner.servers.clone(), inner.no_servers(), snapshot.revision)
    };
    // A stop now would kill this check's sing-box with the tunnel's (the helper sweeps
    // every bundled core), so the check waits its turn with connects and disconnects.
    let _lifecycle = lifecycle();
    if servers.is_empty() {
        return Err(no_servers);
    }
    unidtk_api::log(format!("vpn: ping_all start (via proxy GET), {} servers", servers.len()));

    let base_port = find_free_port_range(servers.len());
    let dials = resolve_dials(&servers);
    let cfg = build_multi_ping_config(&servers, base_port, &dials);
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
        Ok(child) => {
            dies_with_kernel(&child);
            child
        }
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
                    // A server whose name cannot be dialled is not probed: its reason is
                    // the answer, and probing it only burned the budget.
                    let unreachable = match dials.get(&servers[i].host) {
                        Some(Err(reason)) => Some(reason.clone()),
                        _ => None,
                    };
                    let mut r = None;
                    if unreachable.is_none() && port_is_open(port) {
                        if let Ok(latency_ms) = probe_proxy_get(port, SERVER_CHECK_BUDGET) {
                            r = Some(VpnPingResult {
                                key: servers[i].key.clone(), tag: servers[i].tag.clone(),
                                latency_ms: Some(latency_ms),
                                ok: true,
                                error: None,
                            });
                        }
                    }
                    // If proxy probe failed or port was not open, report real failure (no fake TCP ping)
                    if r.is_none() {
                        r = Some(VpnPingResult {
                            key: servers[i].key.clone(), tag: servers[i].tag.clone(),
                            latency_ms: None,
                            ok: false,
                            error: Some(unreachable.unwrap_or_else(|| "Proxy probe failed".to_string())),
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
                key: servers[i].key.clone(), tag: servers[i].tag.clone(),
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
    settings_store.with_current_subscription(revision, || Ok(out))
}

// ─── Public API used by Tauri commands ───

/// One subscription in the lists: its provider's words, and a transport failure if it
/// had one. Its servers carry its id in `group`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Group {
    pub id: String,
    pub host: String,
    pub title: Option<String>,
    pub refusal: Option<String>,
    pub comments: Vec<String>,
    /// A transport failure, cut back to the host; the subscription's cached servers, if
    /// any, stay listed.
    pub error: Option<String>,
}

/// The lists as the panel draws them: each subscription with its notes and its servers.
fn groups_view(inner: &RuntimeInner) -> serde_json::Value {
    let groups: Vec<serde_json::Value> = inner
        .groups
        .iter()
        .map(|group| {
            let servers: Vec<ServerView> = inner.servers.iter().filter(|s| s.group == group.id).map(ServerView::from).collect();
            json!({
                "id": group.id, "host": group.host, "title": group.title, "refusal": group.refusal,
                "comments": group.comments, "error": group.error, "servers": servers,
            })
        })
        .collect();
    json!({ "groups": groups })
}

pub fn list_servers(runtime: &VpnRuntime, subscription_revision: Option<u64>) -> Result<serde_json::Value, String> {
    let settings_store = store()?;
    let snapshot = settings_store.snapshot(subscription_revision)?;
    ensure_servers(runtime, settings_store, &snapshot)?;
    let inner = runtime.inner.lock().unwrap();
    settings_store.with_current_subscription(snapshot.revision, || Ok(groups_view(&inner)))
}

pub fn refresh_servers(runtime: &VpnRuntime) -> Result<serde_json::Value, String> {
    let settings_store = store()?;
    let snapshot = settings_store.snapshot(None)?;
    runtime.inner.lock().unwrap().use_subscription(snapshot.revision);
    fetch_into(runtime, settings_store, &snapshot, true)?;
    Ok(groups_view(&runtime.inner.lock().unwrap()))
}

/// The subscriptions as the settings pane shows them, without fetching: per provider its
/// words, its server count, and the device id this machine sends it (so the person can find
/// this device in the provider's list). No address.
fn groups_status(runtime: &VpnRuntime) -> serde_json::Value {
    let snapshot = store().and_then(|store| store.snapshot(None)).ok();
    let inner = runtime.inner.lock().unwrap();
    let current = snapshot.as_ref().is_some_and(|snapshot| snapshot.revision == inner.servers_revision);
    let groups: Vec<serde_json::Value> = snapshot
        .iter()
        .flat_map(|snapshot| snapshot.subscriptions.iter())
        .map(|sub| {
            let host = subscription::provider_host(&sub.url);
            let group = inner.groups.iter().find(|g| current && g.id == sub.id);
            json!({
                "id": sub.id,
                "host": host,
                "title": group.and_then(|g| g.title.clone()),
                "refusal": group.and_then(|g| g.refusal.clone()),
                "error": group.and_then(|g| g.error.clone()),
                "fetched": group.is_some(),
                "servers": inner.servers.iter().filter(|s| current && s.group == sub.id).count(),
                "device_id": hwid::id_for(&host),
            })
        })
        .collect();
    json!({ "subscription_revision": snapshot.as_ref().map(|s| s.revision), "groups": groups })
}

fn ensure_servers(runtime: &VpnRuntime, settings_store: &SettingsStore, snapshot: &Snapshot) -> Result<(), String> {
    let mut inner = runtime.inner.lock().unwrap();
    inner.use_subscription(snapshot.revision);
    if inner.fetched || !inner.servers.is_empty() {
        return Ok(());
    }
    drop(inner);
    fetch_into(runtime, settings_store, snapshot, false)
}

/// A subscription's servers with their keys (`<subscription id>/<tag>`): two providers
/// may name a server the same.
fn keyed(servers: Vec<VpnServer>, id: &str) -> Vec<VpnServer> {
    servers
        .into_iter()
        .map(|mut server| {
            server.key = format!("{id}/{}", server.tag);
            server.group = id.to_string();
            server
        })
        .collect()
}

/// Fetches every subscription at once and publishes them for `snapshot`'s revision. The
/// network part and the cache writes hold neither the runtime lock nor the settings lock:
/// a status poll during a 15-second fetch answers at once. A subscription that failed on
/// the way keeps its cached servers and says why; one that answered (servers or not)
/// replaces its cache. `force` fetches even when another caller filled the lists while
/// this one waited for [`FETCH`].
fn fetch_into(runtime: &VpnRuntime, settings_store: &SettingsStore, snapshot: &Snapshot, force: bool) -> Result<(), String> {
    if snapshot.subscriptions.is_empty() {
        return Err("Нет подписок: добавьте ссылку в настройках VPN".into());
    }
    let _fetch = FETCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if !force {
        let inner = runtime.inner.lock().unwrap();
        if inner.servers_revision == snapshot.revision && (inner.fetched || !inner.servers.is_empty()) {
            return Ok(());
        }
    }
    let answers: Vec<(settings::Subscription, Result<subscription::Fetched, String>)> = std::thread::scope(|scope| {
        let pending: Vec<_> = snapshot
            .subscriptions
            .iter()
            .map(|sub| (sub.clone(), scope.spawn(move || subscription::fetch(&sub.url))))
            .collect();
        pending
            .into_iter()
            .map(|(sub, handle)| (sub, handle.join().unwrap_or_else(|_| Err("subscription fetch panicked".into()))))
            .collect()
    });
    let (mut servers, mut groups, mut skipped, mut answered) = (Vec::new(), Vec::new(), 0, true);
    for (sub, answer) in answers {
        let mut group = Group { id: sub.id.clone(), host: subscription::provider_host(&sub.url), ..Group::default() };
        match answer {
            Ok(subscription::Fetched { servers: fetched, notes, skipped: left_out }) => {
                let fetched = keyed(fetched, &sub.id);
                unidtk_api::log(format!(
                    "vpn: {} fetched {} servers, {} notes, {left_out} entries skipped{}",
                    group.host,
                    fetched.len(),
                    notes.comments.len(),
                    notes.refusal.as_deref().map(|refusal| format!("; provider: {refusal}")).unwrap_or_default()
                ));
                if let Err(error) = settings_store.save_servers_cache(&sub.id, &fetched) {
                    unidtk_api::log(format!("vpn: could not cache the servers of {}: {error}", group.host));
                }
                group.title = notes.title;
                group.refusal = notes.refusal;
                group.comments = notes.comments;
                skipped += left_out;
                servers.extend(fetched);
            }
            // The error is already cut back to the host (`subscription::redact_urls`).
            Err(error) => {
                answered = false;
                unidtk_api::log(format!("vpn: fetch_servers failed: {error}"));
                let cached = settings_store.load_servers_cache(&sub.id);
                if !cached.is_empty() {
                    unidtk_api::log(format!("vpn: using {} cached servers of {}", cached.len(), group.host));
                }
                group.error = Some(error);
                servers.extend(cached);
            }
        }
        groups.push(group);
    }
    settings_store.prune_caches(&snapshot.subscriptions.iter().map(|sub| sub.id.clone()).collect::<Vec<_>>());
    let mut inner = runtime.inner.lock().unwrap();
    // Publication uses the revision the fetch was for, and only while it is still the
    // current one (the guard refuses a fetch a newer list of subscriptions replaced).
    settings_store.with_current_subscription(snapshot.revision, || {
        inner.use_subscription(snapshot.revision);
        let failed = (servers.is_empty() && groups.iter().all(|g| g.error.is_some()))
            .then(|| groups.iter().find_map(|g| g.error.clone()))
            .flatten();
        inner.servers = servers;
        inner.groups = groups;
        inner.skipped_servers = skipped;
        inner.fetched = answered;
        match failed {
            Some(error) => Err(error),
            None => Ok(()),
        }
    })
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
        // A core that died can leave its TUN adapter and strict-route filters
        // behind: the UI says Off while the network stays cut. Clean up through
        // the elevated stop, unless a new connect has started in the meantime.
        let runtime_for_cleanup = runtime.clone();
        std::thread::spawn(move || {
            let _lifecycle = lifecycle();
            let busy = {
                let inner = runtime_for_cleanup.inner.lock().unwrap();
                inner.connecting || inner.connected
            };
            if busy {
                return;
            }
            let nonce = perform_stop();
            if !wait_until_stopped(Duration::from_secs(10), nonce.as_deref()) {
                unidtk_api::log("vpn: cleanup after a crashed core did not finish");
            }
        });
        inner = runtime.inner.lock().unwrap();
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
    let settings = store().and_then(|store| store.snapshot(None)).ok();
    let subscription_revision = settings.as_ref().map(|snapshot| snapshot.revision);
    VpnStatus {
        connected: inner.connected,
        connecting: inner.connecting,
        selected_tag: inner.selected_tag.clone(),
        selected_name: inner
            .selected_tag
            .as_deref()
            .and_then(|key| inner.servers.iter().find(|s| s.key == key))
            .map(|s| s.tag.clone()),
        proxy_latency_ms: inner.proxy_latency_ms,
        proxy_error: inner.proxy_error.clone(),
        ip_unverified: inner.ip_unverified,
        error: inner.last_error.clone(),
        auto_connect: auto_connect_on(),
        subscription_revision,
        skipped_servers: if subscription_revision == Some(inner.servers_revision) {
            inner.skipped_servers
        } else {
            0
        },
    }
}

pub fn connect(
    runtime: &VpnRuntime,
    tag: Option<String>,
    subscription_revision: Option<u64>,
) -> Result<(), String> {
    // Hold the lock only long enough to reserve the "connecting" state and
    // snapshot the chosen server; the actual bring-up (tens of seconds of
    // port/TUN waits) then runs unlocked so status polling stays responsive.
    // Snapshot CONNECT_EPOCH so we can detect a concurrent disconnect/shutdown
    // that ran while bring_up was unlocked.
    let settings_store = store()?;
    let already_connecting = || -> Result<(), String> {
        unidtk_api::log("vpn: connect ignored: already connecting");
        Err("VPN connection is already in progress".into())
    };
    if runtime.inner.lock().unwrap().connecting {
        return already_connecting();
    }
    // The subscription fetch, when the list is empty, runs without the runtime lock.
    let snapshot = settings_store.snapshot(subscription_revision)?;
    ensure_servers(runtime, settings_store, &snapshot)?;
    let (server, epoch) = {
        let mut inner = runtime.inner.lock().unwrap();
        if inner.connecting {
            return already_connecting();
        }
        let Some((server, epoch)) = inner.reserve_connect(settings_store, &snapshot, tag.as_deref())?
        else {
            return Ok(());
        };
        (server, epoch)
    };

    // The dial literal is resolved here, before bring_up's long stop/start: the
    // lookup then overlaps the stop instead of extending the wait, and it runs
    // unlocked (r[kernel.no-blocking-main-thread]). A failure is a real error:
    // a config that cannot dial must never be written (`#538`).
    let lookup_host = server.host.clone();
    let dial = std::thread::spawn(move || resolve_ipv4(&lookup_host))
        .join()
        .unwrap_or_else(|_| Err("the resolver thread panicked".into()));

    // Held to the end: a disconnect or a server check waits for this bring-up (and its
    // cleanup below) instead of running its stop in the middle of it.
    let _lifecycle = lifecycle();
    let result = bring_up(&server, &settings_store.home_server(), epoch, dial, Instant::now() + CONNECT_BUDGET);

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
            inner.connected_revision = snapshot.revision;
            CONNECTED.store(true, std::sync::atomic::Ordering::SeqCst);
            inner.selected_tag = Some(server.key.clone());
            inner.proxy_latency_ms = outcome.latency_ms;
            inner.proxy_error = None;
            // ip_unverified stays false (set when connecting was reserved,
            // above) until the detached thread below resolves it — this
            // commit must not wait on any IP lookup.
            inner.last_error = None;
            // Keep the remembered tag ordered with the live selection. A later
            // subscription change can supersede this preference during bring-up.
            let remember = settings_store.remember_connected(snapshot.revision, server.key.clone());
            MANUAL_DISCONNECT.store(false, std::sync::atomic::Ordering::SeqCst);
            drop(inner);
            publish_vpn_state();
            if let Err(error) = remember {
                unidtk_api::log(format!("vpn: could not remember connected server: {error}"));
            }

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
        MANUAL_DISCONNECT.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    CONNECTED.store(false, std::sync::atomic::Ordering::SeqCst);
    // Bring the managed Telegram VLESS tunnel back the moment VPN is down.
    publish_vpn_state();
    // A connect still bringing its core up sees the new epoch and stops; this stop
    // runs after it, so the core it may have started cannot outlive the switch.
    let _lifecycle = lifecycle();
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
    unidtk_api::log("vpn: disconnect complete");
}

pub fn select_server(runtime: &VpnRuntime, tag: String) -> Result<(), String> {
    let mut inner = runtime.inner.lock().unwrap();
    store()?.update(|settings| settings.last_tag = Some(tag.clone()))?;
    if inner.connected {
        drop(inner);
        connect(runtime, Some(tag), None)
    } else {
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
    let in_flight;
    {
        let mut inner = runtime.inner.lock().unwrap();
        // A connect that is still bringing the cores up has already asked the
        // elevated helper to start sing-box, before the SOCKS port or the TUN exist.
        in_flight = inner.connecting || inner.connected;
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
    let nothing_up = !in_flight
        && !port_is_open(SOCKS_PORT)
        && !interface_is_connected(TUN_NAME).unwrap_or(false);
    if nothing_up {
        return true;
    }

    if budget.is_zero() {
        // The main thread cannot wait for a bring-up in flight; that bring-up sees the
        // new epoch and stops its own core (`bring_up`'s `abandon`).
        let _ = perform_stop();
        return true;
    }
    let start = Instant::now();
    let _lifecycle = loop {
        match LIFECYCLE.try_lock() {
            Ok(guard) => break Some(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => break Some(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) if start.elapsed() < budget / 2 => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(std::sync::TryLockError::WouldBlock) => break None,
        }
    };
    let stop_nonce = perform_stop();
    let stopped = wait_until_stopped(budget.saturating_sub(start.elapsed()), stop_nonce.as_deref());
    if !stopped {
        unidtk_api::log("vpn: shutdown wait timed out");
    }
    stopped
}

pub fn auto_connect_on_startup(runtime: VpnRuntime) {
    std::thread::spawn(move || {
        let settings = match load_settings() {
            Ok(settings) => settings,
            Err(error) => {
                unidtk_api::log(format!("vpn: auto-connect could not read settings: {error}"));
                return;
            }
        };
        let subscriptions = store().and_then(|store| store.subscriptions()).map(|(_, list)| list).unwrap_or_default();
        unidtk_api::log(format!(
            "vpn: auto-connect settings enabled={} subscriptions={} default={:?} last={:?}",
            auto_connect_on(),
            subscriptions.len(),
            settings.default_tag,
            settings.last_tag
        ));
        if !auto_connect_on() {
            unidtk_api::log("vpn: auto-connect skipped, disabled by last manual disconnect");
            return;
        }
        let Some(first) = subscriptions.first() else {
            unidtk_api::log("vpn: auto-connect aborted, no subscription");
            return;
        };
        // The remembered server is passed as remembered, not requested: when its
        // subscription is gone, the connect falls back to the first server.
        let remembered = settings.last_tag.clone().filter(|t| !t.is_empty()).unwrap_or(settings.default_tag.clone());
        let waited_for = remembered.is_empty().then_some(first).or_else(|| subscription_of(&remembered, &subscriptions)).unwrap_or(first);
        MANUAL_DISCONNECT.store(false, std::sync::atomic::Ordering::SeqCst);
        // Re-checked before each step: the AUTO toggle being switched off
        // (the `auto_connect` setting) or a manual disconnect during an in-flight try must
        // both stop further retries instead of reconnecting behind the user.
        let cancelled = |when: &str| {
            if !auto_connect_on() {
                unidtk_api::log(format!("vpn: auto-connect aborted, disabled {when}"));
                return true;
            }
            if MANUAL_DISCONNECT.load(std::sync::atomic::Ordering::SeqCst) {
                unidtk_api::log(format!("vpn: auto-connect aborted, manual disconnect {when}"));
                return true;
            }
            false
        };
        // At logon the network often comes up after the app: a connect then is refused
        // for carrying no traffic, so wait until the subscription's host resolves.
        let host = subscription_host(&waited_for.url);
        let waited = Instant::now();
        while let Err(error) = resolve_ipv4(&host) {
            if waited.elapsed() >= AUTO_CONNECT_NETWORK_WAIT {
                unidtk_api::log(format!("vpn: auto-connect: no network after {} s ({error}); trying anyway", waited.elapsed().as_secs()));
                break;
            }
            if cancelled("while waiting for the network") {
                return;
            }
            std::thread::sleep(Duration::from_secs(3));
        }
        let pauses = [2, 5, 10];
        for attempt in 1..=pauses.len() + 1 {
            if cancelled("during attempts") {
                return;
            }
            unidtk_api::log(format!("vpn: auto-connect attempt {attempt} -> {remembered:?}"));
            match connect(&runtime, None, None) {
                Ok(()) => {
                    unidtk_api::log("vpn: auto-connect succeeded");
                    return;
                }
                Err(e) if attempt <= pauses.len() => {
                    unidtk_api::log(format!("vpn: auto-connect attempt {attempt} failed: {e}"));
                    std::thread::sleep(Duration::from_secs(pauses[attempt - 1]));
                }
                Err(e) => {
                    unidtk_api::log(format!("vpn: auto-connect final failure: {e}"));
                    return;
                }
            }
        }
    });
}

/// How long auto-connect at logon waits for the network before its first attempt.
const AUTO_CONNECT_NETWORK_WAIT: Duration = Duration::from_secs(90);

/// The host part of a subscription URL: what must resolve before a connect can work.
fn subscription_host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    host_port.split(':').next().unwrap_or_default().to_string()
}

/// VPN module: it owns the runtime, and its commands read it from here.
pub struct VpnModule {
    runtime: VpnRuntime,
}

impl VpnModule {
    pub fn new() -> Self {
        Self {
            runtime: VpnRuntime::default(),
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
            // In diagnostics mode, read only the in-memory settings, without side effects
            settings::read_record(ctx.db().as_ref())?;
            return Ok(());
        }

        settings::bind(ctx.db());
        // Before the switched-off gate, so a disabled module's setting moves too.
        match ctx.settings() {
            Ok(declared) => {
                if let Ok(store) = store() {
                    store.declare(declared.clone());
                    migrate_settings(store, declared.as_ref());
                }
                let _ = DECLARED.set(declared);
            }
            Err(error) => unidtk_api::log(format!("vpn: {error}")),
        }
        if !ctx.enabled() {
            unidtk_api::log("vpn: disabled — auto-connect not started");
            return Ok(());
        }
        auto_connect_on_startup(self.runtime.clone());
        Ok(())
    }

    fn commands(&self) -> &'static [unidtk_api::commands::CommandSpec] {
        Self::COMMANDS
    }

    fn dispatch(&self, cmd: &str, args: &serde_json::Value) -> Option<Result<serde_json::Value, String>> {
        self.dispatch_command(cmd, args)
    }

    /// The connection state, not a log of transitions.
    fn state_events(&self) -> &'static [&'static str] {
        &["vpn"]
    }

    fn settings(&self) -> &'static [SettingSpec] {
        DECLARED_SETTINGS
    }

    /// Every subscription row is a link this module can fetch (an app's link is stored as
    /// the address it carries), and none twice; the home server is a whole record.
    fn check_setting(&self, key: &str, _old: &serde_json::Value, new: serde_json::Value) -> Result<serde_json::Value, String> {
        match key {
            "subscriptions" => {
                let mut kept: Vec<String> = Vec::new();
                for (index, row) in new.as_array().into_iter().flatten().enumerate() {
                    let url = subscription::normalize_link(row.as_str().unwrap_or_default())
                        .map_err(|reason| format!("Подписка {}: {reason}", index + 1))?;
                    if url.is_empty() {
                        continue;
                    }
                    if kept.contains(&url) {
                        return Err(format!("Подписка {}: такая уже есть", index + 1));
                    }
                    kept.push(url);
                }
                Ok(json!(kept))
            }
            "home_server" => {
                let config: HomeServerConfig = serde_json::from_value(new).map_err(|_| "Домашний сервер: запись не читается".to_string())?;
                if config.port == 0 {
                    return Err("Порт должен быть числом от 1 до 65535".into());
                }
                serde_json::to_value(config).map_err(|error| error.to_string())
            }
            _ => Ok(new),
        }
    }

    /// A subscription removed under a live or starting tunnel takes the tunnel with it.
    fn setting_changed(&self, key: &str, _version: u64) {
        if key != "subscriptions" {
            return;
        }
        let ids: Vec<String> = store().and_then(|store| store.subscriptions()).map(|(_, list)| list.into_iter().map(|s| s.id).collect()).unwrap_or_default();
        let orphaned = {
            let inner = self.runtime.inner.lock().unwrap();
            (inner.connected || inner.connecting)
                && inner.selected_tag.as_deref().is_some_and(|key| !ids.iter().any(|id| key.starts_with(&format!("{id}/"))))
        };
        if orphaned {
            unidtk_api::log("vpn: the live server's subscription was removed; disconnecting");
            disconnect(&self.runtime);
        }
    }

    /// A non-zero budget waits for the elevated helper to confirm that sing-box
    /// is gone: the exit paths off the main thread (`quit_app`, session end) pass
    /// one, because a surviving TUN black-holes the network after the next boot.
    fn shutdown(&self, budget: std::time::Duration) -> Result<(), String> {
        shutdown_and_wait(&self.runtime, budget);
        Ok(())
    }

    /// Switched off by the user: the cores stop, nobody waits for the helper.
    fn suspend(&self) -> Result<(), String> {
        shutdown(&self.runtime);
        Ok(())
    }
}

#[cfg(test)]
mod subscription_tests {
    use super::{
        build_singbox_config, build_singbox_outbound, decode_subscription, endpoint_timeout,
        parse_subscription, parse_vless_line, pick_server, pick_tun_address, probe_proxy_get,
        read_tail, resolve_ipv4, subscription_host, VpnServer,
    };
    use base64::Engine;
    use std::time::{Duration, Instant};

    #[test]
    fn the_dial_address_is_a_literal_and_the_name_stays_the_domain() {
        // #538: a config whose outbound server is the hostname asks sing-box to
        // resolve it through the proxy's own DNS detour, which sing-box refuses
        // ("DNS resolution loop detected"). The literal is dialed; the domain
        // stays in server_name and in the ws Host header.
        let server = parse_vless_line(
            "vless://id@ess.example:443?security=reality&sni=ess.example&pbk=k&sid=s#ess",
        )
        .unwrap();
        let outbound = build_singbox_outbound(&server, "proxy", "64.188.82.140");
        assert_eq!(outbound["server"], "64.188.82.140");
        assert_eq!(outbound["tls"]["server_name"], "ess.example");
        assert!(parse_vless_line("vless://id@ess.example:443?security=tls#ess").is_some());
    }

    #[test]
    fn a_tls_link_without_an_sni_names_the_host() {
        let server = parse_vless_line("vless://id@h.example:443?security=tls&type=ws#ws").unwrap();
        let outbound = build_singbox_outbound(&server, "proxy", "10.9.8.7");
        assert_eq!(outbound["tls"]["server_name"], "h.example");
        assert_eq!(outbound["transport"]["headers"]["Host"], "h.example");
    }

    #[test]
    fn an_ip_host_needs_no_lookup_and_a_bad_name_reports_itself() {
        assert_eq!(resolve_ipv4("158.160.199.68").unwrap(), "158.160.199.68");
        assert_eq!(resolve_ipv4(" 1.1.1.1 ").unwrap(), "1.1.1.1");
        let error = resolve_ipv4("no-such-host.invalid").unwrap_err();
        assert!(error.contains("no-such-host.invalid"), "{error}");
    }

    #[test]
    fn the_tun_skips_a_held_range_and_never_overlaps_it() {
        // Happ's adapter holds exactly 172.19.0.1/30 — the address the config used
        // to hardcode. The picker has to move on, and the address it returns must
        // not land inside any held range.
        let occupied = [(super::parse_v4("172.19.0.0").unwrap(), 30), (super::parse_v4("192.168.31.0").unwrap(), 24)];
        let (address, literal) = pick_tun_address(&occupied);
        assert_ne!(literal, "172.19.0.1", "the held /30 must not be chosen");
        let chosen = (super::parse_v4(&literal).unwrap(), 30);
        assert!(
            !occupied.iter().any(|held| super::ranges_overlap(chosen, *held)),
            "{address} overlaps a held range"
        );
        assert_eq!(address, format!("{literal}/30"));
        // Nothing held: the address every earlier connect used stays.
        assert_eq!(pick_tun_address(&[]).1, "172.19.0.1");
    }

    #[test]
    fn the_machines_own_adapters_never_overlap_the_chosen_tun() {
        // Environment-tolerant by design: on any machine, whatever adapters and
        // routes exist right now, the chosen /30 must be outside every range the
        // machine reports. On this machine Happ holds 172.19.0.1/30, which is the
        // address the config used to hardcode (`#538`).
        let occupied = super::occupied_v4_ranges();
        assert!(!occupied.is_empty(), "every machine has at least the reserved ranges");
        let (address, literal) = pick_tun_address(&occupied);
        assert!(literal.ends_with(|c: char| c.is_ascii_digit()), "{literal}");
        let chosen = (super::parse_v4(&literal).unwrap(), 30);
        assert!(
            !occupied.iter().any(|held| super::ranges_overlap(chosen, *held)),
            "{address} overlaps a range this machine holds: {occupied:?}"
        );
        println!("tun={address} held={} ranges", occupied.len());
    }

    #[test]
    fn the_built_config_dials_the_literal_and_uses_the_chosen_tun() {
        let server = parse_vless_line(
            "vless://id@ess.example:443?security=reality&sni=ess.example&pbk=k&sid=s#ess",
        )
        .unwrap();
        let config = build_singbox_config(&server, &super::HomeServerConfig::default(), "64.188.82.140", "172.31.0.1/30");
        let outbounds = config["outbounds"].as_array().unwrap();
        assert_eq!(outbounds[0]["server"], "64.188.82.140");
        assert_eq!(outbounds[0]["tls"]["server_name"], "ess.example");
        let tun = &config["inbounds"][0];
        assert_eq!(tun["type"], "tun");
        assert_eq!(tun["address"][0], "172.31.0.1/30");
        // The user's own traffic still resolves through the tunnel.
        assert_eq!(config["dns"]["servers"][0]["detour"], "proxy");
    }

    #[test]
    fn plain_text_subscription_is_taken_as_is() {
        let body = "vless://a@h:1#x\ntrojan://b@h:2#y\n";
        assert_eq!(decode_subscription(body).unwrap(), body.trim());
    }

    #[test]
    fn base64_subscription_is_decoded() {
        let body = base64::engine::general_purpose::STANDARD.encode("vless://a@h:1#x");
        assert_eq!(decode_subscription(&body).unwrap(), "vless://a@h:1#x");
    }

    fn server(tag: &str) -> VpnServer {
        let mut server = parse_vless_line(&format!("vless://id@{tag}.example:443?security=tls&sni={tag}.example#{tag}")).unwrap();
        server.key = tag.to_string();
        server
    }

    #[test]
    fn a_subscription_counts_what_it_cannot_use() {
        let body = "vless://id@a.example:443#a\nss://abcdef@b.example:1#b\n\n# comment\n\
                    vmess://xyz\ntrojan://pw@c.example:443#c\nvless://broken";
        let (servers, skipped) = parse_subscription(body);
        assert_eq!(servers.iter().map(|s| s.tag.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert_eq!(skipped, 3); // ss://, vmess://, and the vless link that does not parse
    }

    #[test]
    fn a_requested_tag_that_is_gone_is_an_error_not_another_server() {
        let servers = [server("a"), server("b")];
        assert!(pick_server(&servers, Some("gone"), Some("a")).is_err());
        let (picked, fell_back) = pick_server(&servers, Some("b"), Some("a")).unwrap();
        assert_eq!((picked.tag.as_str(), fell_back), ("b", false));
    }

    #[test]
    fn a_remembered_tag_that_is_gone_falls_back_and_says_so() {
        let servers = [server("a"), server("b")];
        let (picked, fell_back) = pick_server(&servers, None, Some("gone")).unwrap();
        assert_eq!((picked.tag.as_str(), fell_back), ("a", true));
        let (picked, fell_back) = pick_server(&servers, None, Some("b")).unwrap();
        assert_eq!((picked.tag.as_str(), fell_back), ("b", false));
        assert!(pick_server(&[], None, Some("a")).is_err());
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(decode_subscription("<html>not a subscription</html>").is_err());
    }

    #[test]
    fn auto_connect_waits_for_the_subscription_host() {
        assert_eq!(subscription_host("https://bot.example.ru/sub/abc?x=1"), "bot.example.ru");
        assert_eq!(subscription_host("https://user:pw@host.example:8443/p"), "host.example");
        assert_eq!(subscription_host("host.example/p"), "host.example");
    }

    // A dead server's check used to cost six endpoints of 3.5 s + 3.5 s each; the whole
    // check now shares one budget per server.
    #[test]
    fn an_endpoint_gets_only_what_is_left_of_the_budget() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(5);
        assert_eq!(endpoint_timeout(deadline, now), Some(Duration::from_secs(5)));
        assert_eq!(endpoint_timeout(deadline, now + Duration::from_millis(4900)), None);
        assert_eq!(endpoint_timeout(deadline, now + Duration::from_secs(6)), None);
    }

    // Live, 2026-11-26: France answered the check after 15.0 s with a 5 s budget. The
    // budget was the request's `.timeout()`, which ureq applies after the connection; the
    // SOCKS handshake through sing-box waits on the agent's `timeout_connect` (30 s by
    // default), so a dead server held its probe until sing-box gave up dialling it.
    #[test]
    fn a_proxy_that_never_answers_costs_only_the_budget() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            // Accept and hold every connection without a byte of the SOCKS greeting.
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                held.push(stream);
            }
        });
        let start = Instant::now();
        assert!(probe_proxy_get(port, Duration::from_millis(800)).is_err());
        assert!(start.elapsed() < Duration::from_secs(3), "the probe took {:?}", start.elapsed());
    }

    #[test]
    fn a_name_that_does_not_resolve_is_not_called_ipv6_only() {
        let error = resolve_ipv4("no-such-host.invalid").unwrap_err();
        assert!(!error.contains("IPv6"), "{error}");
    }

    #[test]
    fn the_log_tail_reads_only_the_end_and_starts_on_a_whole_line() {
        let path = std::env::temp_dir().join(format!("unidtk-tail-{}.log", std::process::id()));
        let body: String = (0..2000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, &body).unwrap();
        let tail = read_tail(&path, 100).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(tail.len() <= 100, "{}", tail.len());
        assert!(tail.starts_with("line "), "{tail:?}");
        assert!(tail.ends_with("line 1999\n"), "{tail:?}");
    }
}

#[cfg(test)]
mod settings_contract_tests {
    use std::sync::Arc;

    use serde_json::json;
    use unidtk_api::settings::{InMemorySettings, ModuleSettings};
    use unidtk_api::{InMemoryModuleDb, ModuleDb};

    use super::settings::SettingsStore;
    use super::{migrate_settings, DECLARED_SETTINGS};

    // The one-subscription record moves into the contract once, its app link unwrapped,
    // and the remembered server becomes a key of that subscription.
    #[test]
    fn the_old_record_moves_into_the_contract_once() {
        let db = Arc::new(InMemoryModuleDb::new());
        db.kv_set(
            "settings",
            br#"{"subscription_url":"happ://add/https://x.example/T+k","auto_connect":true,"home_server":{"enabled":true,"host":"h.example","port":443},"last_tag":"Estonia"}"#,
        )
        .unwrap();
        let store = SettingsStore::new(db.clone(), std::env::temp_dir().join("unidtk-vpn-migrate-none"));
        let declared = Arc::new(InMemorySettings::new(DECLARED_SETTINGS));
        store.declare(declared.clone());
        migrate_settings(&store, declared.as_ref());
        assert_eq!(declared.get("subscriptions").unwrap().value, json!(["https://x.example/T+k"]));
        assert_eq!(declared.get("auto_connect").unwrap().value, json!(true));
        assert_eq!(declared.get("home_server").unwrap().value["host"], "h.example");

        // Live, session 152: the old record of a never-used home server had port 0, and
        // the check refused the whole record.
        let zero = Arc::new(InMemoryModuleDb::new());
        zero.kv_set("settings", br#"{"home_server":{"enabled":false,"host":"","port":0}}"#).unwrap();
        let zero_store = SettingsStore::new(zero, std::env::temp_dir().join("unidtk-vpn-migrate-none"));
        let zero_declared = Arc::new(InMemorySettings::new(DECLARED_SETTINGS));
        zero_store.declare(zero_declared.clone());
        migrate_settings(&zero_store, zero_declared.as_ref());
        assert_eq!(zero_declared.get("home_server").unwrap().value["port"], 443);
        let (_, subscriptions) = store.subscriptions().unwrap();
        let key = store.read().unwrap().last_tag.unwrap();
        assert_eq!(key, format!("{}/Estonia", subscriptions[0].id));

        // Again: nothing moves twice, the key stays a key.
        declared.set("auto_connect", json!(false), None).unwrap();
        migrate_settings(&store, declared.as_ref());
        assert_eq!(declared.get("auto_connect").unwrap().value, json!(false));
        assert_eq!(declared.get("subscriptions").unwrap().version, 1);
        assert_eq!(store.read().unwrap().last_tag.unwrap(), key);
    }

    #[test]
    fn the_module_checks_its_subscriptions_and_its_home_server() {
        let module = super::create_module();
        let checked = module
            .check_setting("subscriptions", &json!([]), json!(["happ://add/https://a.example/s", "https://b.example/s"]))
            .unwrap();
        assert_eq!(checked, json!(["https://a.example/s", "https://b.example/s"]));
        assert!(module.check_setting("subscriptions", &json!([]), json!(["https://a.example/s", "https://a.example/s"])).unwrap_err().contains("уже есть"));
        assert!(module.check_setting("subscriptions", &json!([]), json!(["happ://crypt4/x"])).unwrap_err().starts_with("Подписка 1:"));
        assert!(module.check_setting("home_server", &json!({}), json!({ "host": "h", "port": 0 })).is_err());
        assert_eq!(module.check_setting("home_server", &json!({}), json!({ "host": "h", "port": 8443 })).unwrap()["port"], 8443);
    }
}
