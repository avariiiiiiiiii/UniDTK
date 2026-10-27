use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    ffi::{c_void, CStr, CString},
    os::windows::process::CommandExt,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError}, Arc, Mutex, OnceLock,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Skip re-attempting `TdJson::load` for this long after a failure (status is
/// polled every ~750ms while unauthorized; without a cooldown that storms
/// LoadLibrary + log lines indefinitely).
const TD_LOAD_FAILURE_COOLDOWN: Duration = Duration::from_secs(20);

/// Overall wall-clock budget for the `loadChats` round loop in `chats()`.
const CHATS_LOAD_DEADLINE: Duration = Duration::from_secs(10);

/// Cap in-memory history per chat (oldest→newest Vec; drop from the front).
const MAX_CACHED_MESSAGES_PER_CHAT: usize = 1000;

/// Treat in-flight `downloadFile` entries older than this as stale so a lost
/// `updateFile` (TDLib error, client restart, etc.) cannot block retries forever.
const DOWNLOAD_STALE_SECS: u64 = 30;

/// Cooldown between failed attempts to (re)start the managed ciadpi proxy.
/// Without it a dead ciadpi would be re-spawned (blocking up to ~5 s under the
/// inner lock) on every status/chat poll.
const PROXY_ENSURE_COOLDOWN: Duration = Duration::from_secs(30);

/// The stuck-connection heal ignores the first two minutes after the client
/// starts: a cold MTProto handshake through a desync proxy can legitimately
/// take tens of seconds, and resetting it mid-handshake makes things worse.
const PROXY_HEAL_GRACE: Duration = Duration::from_secs(120);

/// Backoff steps (seconds) between stuck-connection proxy re-applications.
const PROXY_HEAL_BACKOFF_SECS: [u64; 3] = [30, 120, 300];

/// Budget for one `getChatPinnedMessage` / pinned-filter search. The pinned
/// message usually sits outside the local history window (old "chat rules"
/// posts), so TDLib performs a server round-trip; through a slow proxy that
/// legitimately outlives the previous 2.5s budget, and folding the timeout
/// into "no pinned message" is what hid pin bars in group chats.
const CHAT_PINNED_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Minimum spacing between `get_chat_pinned_message` failure log lines per
/// chat: the frontend retries the request on every render, so without a
/// throttle a dead transport floods the journal with identical lines.
const CHAT_PINNED_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(300);

static TELEGRAM_DB: OnceLock<Arc<dyn unidtk_api::ModuleDb>> = OnceLock::new();

const DEFAULT_TELEGRAM_API_ID: &str = "38708022";
const DEFAULT_TELEGRAM_API_HASH: &str = "0d82dab78dc08e1393ee1e609c49b41d";

// ─── Managed ByeDPI proxy (proxy_type "byedpi") ───
//
// ByeDPI (`ciadpi.exe` by hufrea) is a local user-space SOCKS5 proxy with a
// DPI desync engine (split/disorder/fake/tlsrec) — no remote server, no admin,
// no kernel driver. It replaces the old managed VLESS tunnel (dedicated
// sing-box → home server), which no longer exists as a backend mode.
// TDLib (SOCKS5-only) is pointed at the local ciadpi listener; ciadpi sends
// desynced packets straight to Telegram. While the wallpaper VPN is up the
// instance is not run at all — TDLib then goes direct and the VPN TUN carries
// Telegram (see sync_vpn_proxy).
const CREATE_NO_WINDOW: u32 = 0x08000000;
/// Local SOCKS5 port of the ByeDPI instance. The main VPN sing-box
/// owns 12908, so this one must not collide with it.
const MANAGED_PROXY_PORT: u16 = 12909;

/// Desync profile shipped with ByeDPI's own windows bundle (`byedpi.bat`):
/// split + disorder + http-mods + auto-fallback on torst + tlsrec split.
/// Conservative (single TLS group, no custom fakes) so it works on most ISPs
/// without tuning; users can still pick manual socks5/http/none instead.
const BYEDPI_ARGS: &[&str] = &[
    "--split", "1",
    "--disorder", "3+s",
    "--mod-http", "h,d",
    "--auto", "torst",
    "--tlsrec", "1+s",
];

/// Live child handle of the managed ByeDPI instance; `None` when not running
/// (or when a leftover instance from a previous app run is being reused).
static MANAGED_PROXY_CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// ByeDPI executable: install dir (shipped with the app) or the vendored
/// checkout copy (`opt/byedpi/ciadpi.exe`) in dev. No PATH lookup — the app
/// must ship its own copy.
fn byedpi_exe() -> Option<std::path::PathBuf> {
    let install = unidtk_api::paths::byedpi_exe();
    if install.is_file() {
        return Some(install);
    }
    unidtk_api::paths::resolve_binary("ciadpi.exe")
}

/// True when `127.0.0.1:port` actually speaks SOCKS5 (greeting `05 01 00` →
/// `05 00`). A bare TCP connect is not enough: a foreign process sitting on the
/// port would be mistaken for the managed ciadpi and TDLib would be pointed at
/// a service that cannot proxy anything.
fn managed_proxy_port_open() -> bool {
    socks5_probe(MANAGED_PROXY_PORT, Duration::from_millis(300))
}

fn socks5_probe(port: u16, timeout: Duration) -> bool {
    use std::io::{Read, Write};
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&addr, timeout) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    if stream.write_all(&[0x05, 0x01, 0x00]).is_err() {
        return false;
    }
    let mut reply = [0u8; 2];
    if stream.read_exact(&mut reply).is_err() {
        return false;
    }
    reply == [0x05, 0x00]
}

/// Ensure the managed ByeDPI proxy is up and its SOCKS port is accepting
/// connections. Reuses a still-running child, or a leftover instance from a
/// previous app run listening on the same port (same args, same port).
fn ensure_managed_proxy() -> Result<(), String> {
    {
        let mut guard = MANAGED_PROXY_CHILD.lock().unwrap();
        let mut reusable = false;
        if let Some(child) = guard.as_mut() {
            match child.try_wait() {
                Ok(None) => {
                    if managed_proxy_port_open() {
                        reusable = true;
                    } else {
                        // Alive but not listening — restart cleanly rather
                        // than waiting forever.
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                }
                _ => {}
            }
        }
        if reusable {
            return Ok(());
        }
        *guard = None;
    }
    // A leftover (or foreign) SOCKS5 listener already owns the port.
    if managed_proxy_port_open() {
        unidtk_api::log(format!(
            "telegram: reusing existing SOCKS5 listener on 127.0.0.1:{MANAGED_PROXY_PORT}"
        ));
        return Ok(());
    }

    let Some(exe) = byedpi_exe() else {
        return Err(
            "ciadpi.exe not found (bundled build missing?)".to_string(),
        );
    };

    let mut child = Command::new(&exe)
        .args(["-i", "127.0.0.1", "-p", &MANAGED_PROXY_PORT.to_string()])
        .args(BYEDPI_ARGS)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| format!("spawn ciadpi failed: {error}"))?;

    for _ in 0..50 {
        // Check the child first: if ciadpi died instantly, do not adopt a
        // foreign listener that won the race for the port.
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("ciadpi exited early ({status})"));
        }
        if managed_proxy_port_open() {
            *MANAGED_PROXY_CHILD.lock().unwrap() = Some(child);
            unidtk_api::log(format!(
                "telegram: managed proxy byedpi up on 127.0.0.1:{MANAGED_PROXY_PORT}"
            ));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(format!(
        "ciadpi did not open 127.0.0.1:{MANAGED_PROXY_PORT} in time"
    ))
}

/// Kill the managed ByeDPI instance if we started one. Called when the proxy
/// type is switched away from "byedpi", when the wallpaper VPN takes over
/// Telegram traffic, and on app exit.
pub fn stop_managed_proxy() {
    if let Some(mut child) = MANAGED_PROXY_CHILD.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(windows)]
use windows::{
    core::{PCSTR, PCWSTR},
    Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_WITH_ALTERED_SEARCH_PATH,
    },
};

extern "system" {
    fn FreeLibrary(hLibModule: windows::Win32::Foundation::HMODULE) -> i32;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramAccount {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub avatar_path: Option<String>,
    #[serde(default)]
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramStatus {
    pub mode: String,
    pub authorized: bool,
    pub auth_state: String,
    pub auth_hint: String,
    pub active_account_id: String,
    pub accounts: Vec<TelegramAccount>,
    /// TDLib connection state, canonical: "ready" | "connecting" |
    /// "connecting_to_proxy" | "updating" | "waiting_for_network" | …
    #[serde(default)]
    pub connection_state: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramChat {
    pub id: String,
    pub title: String,
    pub initials: String,
    pub favorite: bool,
    pub order: i64,
    pub unread: u32,
    pub kind: String,
    pub preview: String,
    pub time: String,
    pub muted: bool,
    pub photo_file_id: Option<i32>,
    pub photo_path: Option<String>,
    /// Id of `last_message` when known — used as a mark-read fallback when the
    /// in-memory history window is empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_id: Option<String>,
    pub user_id: Option<String>,
    pub status: Option<String>,
    pub is_forum: Option<bool>,
    pub member_count: Option<i32>,
    pub linked_community_id: Option<String>,
    pub has_subchats: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramEntity {
    pub kind: String,
    pub offset: i32,
    pub length: i32,
    pub url: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramReactionItem {
    pub reaction_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_emoji_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_emoji_path: Option<String>,
    pub count: u32,
    pub is_chosen: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramMessage {
    pub id: String,
    pub from: String,
    /// Channel post signature, when Telegram provides an author label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_signature: Option<String>,
    pub text: String,
    pub entities: Option<Vec<TelegramEntity>>,
    pub time: String,
    pub reactions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reaction_items: Option<Vec<TelegramReactionItem>>,
    pub attachment: Option<String>,
    pub attachment_kind: Option<String>,
    pub attachment_file_id: Option<i32>,
    pub attachment_local_path: Option<String>,
    pub attachment_width: Option<i32>,
    pub attachment_height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_duration: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_performer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_waveform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_file_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_local_path: Option<String>,
    pub sticker: Option<String>,
    pub reply_to_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_sender: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_text: Option<String>,
    pub reply_markup: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i64>,
    /// TDLib album identifier shared by messages sent together.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_album_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_pinned: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramSendRequest {
    pub chat_id: String,
    pub text: String,
    /// Optional TDLib text entities built by the frontend. Today only
    /// `mentionName` (user picked from the @ autocomplete who has no public
    /// username) is translated; anything else is dropped.
    pub entities: Option<Vec<Value>>,
    pub reply_to_message_id: Option<String>,
    #[serde(alias = "threadId", alias = "topicId")]
    pub message_thread_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramMember {
    pub user_id: String,
    pub name: String,
    pub username: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramSendAttachmentRequest {
    pub chat_id: String,
    pub path: String,
    pub caption: Option<String>,
    pub reply_to_message_id: Option<String>,
    #[serde(alias = "threadId", alias = "topicId")]
    pub message_thread_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramArchiveRequest {
    pub chat_id: String,
    pub archived: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramEditMessageRequest {
    pub chat_id: String,
    pub message_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramDeleteMessagesRequest {
    pub chat_id: String,
    pub message_ids: Vec<String>,
    #[serde(default = "default_true")]
    pub revoke: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramReactionRequest {
    pub chat_id: String,
    pub message_id: String,
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub custom_emoji_id: Option<String>,
    #[serde(default)]
    pub reaction_type: Option<String>,
    #[serde(default)]
    pub remove: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramPinMessageRequest {
    pub chat_id: String,
    pub message_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramForwardMessagesRequest {
    pub to_chat_id: String,
    pub from_chat_id: String,
    pub message_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramDeleteChatHistoryRequest {
    pub chat_id: String,
    #[serde(default)]
    pub remove_from_chat_list: bool,
    #[serde(default = "default_true")]
    pub revoke: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramCallbackQueryRequest {
    pub chat_id: String,
    pub message_id: String,
    pub data: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramForumTopicsRequest {
    pub chat_id: String,
    pub query: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramAuthCode {
    pub code: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramPhone {
    pub phone_number: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramPassword {
    pub password: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramEmail {
    pub email_address: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramEmailCode {
    pub code: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramRegistration {
    pub first_name: String,
    pub last_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TelegramConfig {
    pub api_id: String,
    pub api_hash: String,
    pub tdjson_path: String,
    pub proxy_type: Option<String>,
    pub proxy_host: Option<String>,
    pub proxy_port: Option<u16>,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    #[serde(default)]
    pub active_account_id: Option<String>,
    #[serde(default)]
    pub accounts: Vec<TelegramAccount>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramDebugInfo {
    pub mode: String,
    pub authorized: bool,
    pub auth_state: String,
    pub auth_hint: String,
    pub config_path: String,
    pub config_exists: bool,
    pub tdjson_path: String,
    pub tdjson_exists: bool,
    pub api_id_configured: bool,
    pub api_hash_configured: bool,
    pub proxy_configured: bool,
    pub tdlib_data_dir: String,
}

#[derive(Clone)]
pub struct TelegramRuntime {
    inner: Arc<Mutex<TelegramInner>>,
    app_handle: Option<tauri::AppHandle>,
}

/// Process-wide handle so vpn.rs can re-sync the managed proxy on
/// connect/disconnect without holding a Tauri State reference.
static TELEGRAM_FOR_VPN: OnceLock<Arc<Mutex<TelegramInner>>> = OnceLock::new();

/// Состояние VPN из контракта.
///
/// Раньше здесь стоял прямой вызов `vpn::is_connected()`, из-за чего telegram и
/// vpn зависели друг от друга. Теперь vpn публикует событие `vpn`, а telegram
/// читает последнее известное значение — зависимости нет ни в одну сторону.
fn vpn_connected_from_contract() -> bool {
    unidtk_api::state("vpn")
        .and_then(|payload| payload.get("connected").and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// Подписаться на изменения VPN. Вызывает ядро один раз при старте.
///
/// Заменяет прежний обратный вызов `vpn -> telegram::on_vpn_state_changed()`.
pub fn subscribe_vpn_events() {
    unidtk_api::subscribe("vpn", |_payload| {
        on_vpn_state_changed();
    });
}

impl Default for TelegramRuntime {
    fn default() -> Self {
        let inner = Arc::new(Mutex::new(TelegramInner::mock()));
        // First (only) runtime wins; subsequent defaults are tests/no-ops.
        let _ = TELEGRAM_FOR_VPN.set(Arc::clone(&inner));
        Self {
            inner,
            app_handle: None,
        }
    }
}

impl TelegramRuntime {
    pub fn set_app_handle(&mut self, handle: tauri::AppHandle) {
        self.app_handle = Some(handle.clone());
        self.inner.lock().unwrap().app_handle = Some(handle);
        // Updates are drained by the dedicated TDLib receiver thread spawned in
        // ensure_started() — no 10ms poll loop here.
    }
}

struct TelegramInner {
    mode: String,
    authorized: bool,
    auth_state: String,
    auth_hint: String,
    td: Option<Arc<TdJson>>,
    /// Instant of the last failed `TdJson::load` attempt; used to backoff
    /// retries while the frontend polls `status()` every ~750ms.
    last_td_load_failure: Option<Instant>,
    chats: HashMap<String, TelegramChat>,
    /// Chats currently in TDLib's Archive list. This is kept separate from Main
    /// so list updates can move a chat without losing its message/topic state.
    archived_chats: HashMap<String, TelegramChat>,
    /// Message history cache. Keyed by `chat_id` for the main chat and by
    /// `chat_id::<thread_id>` for forum topic threads so topic history never
    /// mixes with the general chat (see `cache_key_for`).
    messages: HashMap<String, Vec<TelegramMessage>>,
    /// user_id → display name ("first_name last_name", trimmed).
    users: HashMap<i64, String>,
    /// user_id → status string ("online", "recently", "last_week", "last_month", "was_online:<ts>").
    user_statuses: HashMap<i64, String>,
    /// chat_id → title (secondary to `chats`; used for channel senders).
    chat_titles_by_id: HashMap<i64, String>,
    chats_exhausted: bool,
    archive_chats_exhausted: bool,
    next_extra: u64,
    app_handle: Option<tauri::AppHandle>,
    /// File ids with an in-flight async `downloadFile` (synchronous: false),
    /// mapped to when the request was queued. Prevents duplicate TDLib requests
    /// from chat-parse + frontend retries; entries older than
    /// [`DOWNLOAD_STALE_SECS`] are treated as stale and allow a fresh request.
    downloading_files: HashMap<i32, Instant>,
    /// `file_id` → (`chat_id`, `message_id`) for message-attachment downloads
    /// kicked off via `download_file`, so `handle_file_update` can emit
    /// `telegram-attachment-ready` without relying solely on a full messages scan.
    attachment_file_owners: HashMap<i32, (String, String, Option<i64>)>,
    /// @extra → oneshot sender for in-flight `td_request` waiters.
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Value>>>>,
    /// Last VPN state seen by `sync_vpn_proxy`, so the managed proxy
    /// is only re-applied on an actual connect/disconnect transition.
    vpn_connected: bool,
    /// Last TDLib connection state, canonical: "ready" | "connecting" |
    /// "connecting_to_proxy" | "updating" | "waiting_for_network".
    /// Fed by `updateConnectionState`; used to surface network trouble and to
    /// re-apply the proxy when the connection gets stuck connecting.
    connection_state: String,
    /// When the connection last entered a connecting state — used to detect a
    /// stuck connection and re-apply the managed proxy (see
    /// `on_connection_state_changed`).
    connecting_since: Option<Instant>,
    /// id of the TDLib proxy record created for the current client (from the
    /// `addedProxy` reply). Reset whenever the client is recreated or the
    /// account changes — stale ids answer `Unknown proxy identifier`.
    active_proxy_id: Option<i32>,
    /// Endpoint identity of `active_proxy_id`; `enableProxy` is only allowed
    /// while it matches the configured endpoint.
    active_proxy_key: Option<String>,
    /// `@extra`/kind/sent-at of the in-flight proxy command, so its reply can
    /// be correlated on the receiver thread without ever blocking it.
    proxy_cmd_extra: Option<u64>,
    proxy_cmd_kind: Option<&'static str>,
    proxy_cmd_sent_at: Option<Instant>,
    /// Stuck-connection heal bookkeeping (see `on_connection_state_changed`).
    proxy_heal_attempt: u32,
    proxy_heal_next_at: Option<Instant>,
    /// When the TDLib client was created; gates the heal grace period.
    client_started_at: Option<Instant>,
    /// Set after a failed managed-proxy (re)start; retries wait out the cooldown.
    proxy_ensure_failed_at: Option<Instant>,
    /// Chat currently opened via TDLib `openChat` (required for read receipts
    /// and live updates to sync to Telegram servers / other devices).
    open_chat_id: Option<String>,
    active_account_id: String,
    my_user_id: Option<i64>,
    /// custom_emoji_id (string) -> local path to thumbnail / sticker image file
    custom_emojis: HashMap<String, String>,
    /// file_id (i32) -> list of custom_emoji_ids waiting for this download to finish
    custom_emoji_file_waiters: HashMap<i32, Vec<String>>,
    /// custom_emoji_id -> Instant when download was requested (debounce in-flight requests)
    custom_emojis_in_flight: HashMap<String, Instant>,
    /// @extra -> list of custom_emoji_ids requested in this batch
    custom_emoji_requests: HashMap<u64, Vec<String>>,
    /// chat_id → Instant of the last throttled pin-request error log line.
    pinned_error_log_at: HashMap<String, Instant>,
    /// Set to stop the dedicated TDLib receiver thread.
    receiver_shutdown: Arc<AtomicBool>,
    receiver_handle: Option<JoinHandle<()>>,
}

impl TelegramInner {
    fn mock() -> Self {
        let config = load_config();
        let active_account_id = config.active_account_id.unwrap_or_else(|| "default".into());
        Self {
            mode: "mock".into(),
            authorized: false,
            auth_state: "mock".into(),
            auth_hint:
                "TDLib is not started. Set TDJSON_PATH, TELEGRAM_API_ID and TELEGRAM_API_HASH."
                    .into(),
            td: None,
            last_td_load_failure: None,
            chats: HashMap::new(),
            archived_chats: HashMap::new(),
            messages: HashMap::new(),
            users: HashMap::new(),
            user_statuses: HashMap::new(),
            chat_titles_by_id: HashMap::new(),
            chats_exhausted: false,
            archive_chats_exhausted: false,
            next_extra: 1,
            app_handle: None,
            downloading_files: HashMap::new(),
            attachment_file_owners: HashMap::new(),
            pending: Arc::new(Mutex::new(HashMap::new())),
            vpn_connected: false,
            connection_state: "initializing".into(),
            connecting_since: None,
            active_proxy_id: None,
            active_proxy_key: None,
            proxy_cmd_extra: None,
            proxy_cmd_kind: None,
            proxy_cmd_sent_at: None,
            proxy_heal_attempt: 0,
            proxy_heal_next_at: None,
            client_started_at: None,
            proxy_ensure_failed_at: None,
            open_chat_id: None,
            active_account_id,
            my_user_id: None,
            custom_emojis: HashMap::new(),
            custom_emoji_file_waiters: HashMap::new(),
            custom_emojis_in_flight: HashMap::new(),
            custom_emoji_requests: HashMap::new(),
            pinned_error_log_at: HashMap::new(),
            receiver_shutdown: Arc::new(AtomicBool::new(false)),
            receiver_handle: None,
        }
    }

    /// True if a non-stale download is already in flight for `file_id`.
    fn download_in_flight(&self, file_id: i32) -> bool {
        match self.downloading_files.get(&file_id) {
            Some(started) => started.elapsed() < Duration::from_secs(DOWNLOAD_STALE_SECS),
            None => false,
        }
    }

    fn emit(&self, event: &str, payload: impl Serialize + Clone) {
        // Through the contract rather than straight to Tauri: the kernel's emitter feeds
        // both doors, so these events reach the WebSocket client too. One helper covers
        // every Telegram event.
        unidtk_api::emit(
            event,
            serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null),
        );
    }

    fn status(&self) -> TelegramStatus {
        let mut config = load_config();
        for acc in &mut config.accounts {
            acc.is_active = acc.id == self.active_account_id;
        }
        TelegramStatus {
            mode: self.mode.clone(),
            authorized: self.authorized,
            auth_state: self.auth_state.clone(),
            auth_hint: self.auth_hint.clone(),
            active_account_id: self.active_account_id.clone(),
            accounts: config.accounts,
            connection_state: self.connection_state.clone(),
        }
    }

    fn remember_error(&mut self, error: impl Into<String>) {
        let error = error.into();
        self.mode = "tdlib".into();
        self.authorized = false;
        self.auth_state = "error".into();
        self.auth_hint = error.clone();
        unidtk_api::log(format!("telegram: {error}"));
    }

    /// Inform TDLib the user opened `chat_id`. Closes any previously open chat.
    /// TDLib docs: openChat/closeChat + viewMessages are required for read
    /// receipts to reach Telegram servers (and therefore other devices).
    fn open_chat(&mut self, chat_id: &str) {
        let cid = parse_chat_id(chat_id);
        if cid == 0 {
            return;
        }
        if self.open_chat_id.as_deref() == Some(chat_id) {
            return;
        }
        if let Some(prev) = self.open_chat_id.take() {
            if let Err(error) = self.send(json!({
                "@type": "closeChat",
                "chat_id": parse_chat_id(&prev)
            })) {
                unidtk_api::log(format!("telegram: closeChat({prev}) failed: {error}"));
            }
        }
        if let Err(error) = self.send(json!({
            "@type": "openChat",
            "chat_id": cid
        })) {
            unidtk_api::log(format!("telegram: openChat({chat_id}) failed: {error}"));
            return;
        }
        self.open_chat_id = Some(chat_id.to_string());
    }

    fn close_chat(&mut self, chat_id: Option<&str>) {
        let Some(open) = self.open_chat_id.clone() else {
            return;
        };
        if let Some(want) = chat_id {
            if open != want {
                return;
            }
        }
        if let Err(error) = self.send(json!({
            "@type": "closeChat",
            "chat_id": parse_chat_id(&open)
        })) {
            unidtk_api::log(format!("telegram: closeChat({open}) failed: {error}"));
        }
        self.open_chat_id = None;
    }

    /// Mark inbox messages as read and push the receipt to Telegram servers.
    /// Uses `openChat` + `viewMessages(source=chatHistory, force_read=true)`.
    fn mark_messages_read(&mut self, chat_id: &str, messages: &[TelegramMessage]) {
        let cid = parse_chat_id(chat_id);
        if cid == 0 {
            return;
        }

        // Collect valid server/local message ids from our cache. Prefer the
        // highest id (viewMessages marks everything up to max_id as read).
        let mut message_ids: Vec<i64> = messages
            .iter()
            .filter_map(|message| message.id.parse::<i64>().ok())
            .filter(|id| *id > 0)
            .collect();
        message_ids.sort_unstable();
        message_ids.dedup();

        // If the cache is empty/stale, still try the chat's last_message id so
        // "Mark as read" / open-chat without a history load still syncs.
        if let Some(last_id) = self
            .chats
            .get(chat_id)
            .and_then(|chat| chat.last_message_id.as_ref())
            .and_then(|id| id.parse::<i64>().ok())
            .filter(|id| *id > 0)
        {
            if !message_ids.contains(&last_id) {
                message_ids.push(last_id);
                message_ids.sort_unstable();
            }
        }

        if message_ids.is_empty() {
            unidtk_api::log(format!(
                "telegram: mark_read skipped for {chat_id}: no message ids"
            ));
            return;
        }

        // Keep only the newest few ids — enough for max_id / force_read, avoids
        // huge vectors when the per-chat cache holds hundreds of messages.
        let view_ids: Vec<i64> = if message_ids.len() > 20 {
            message_ids[message_ids.len() - 20..].to_vec()
        } else {
            message_ids
        };

        // Open the chat first so TDLib treats the view as chat-history source
        // even if `source` is guessed; force_read still covers closed chats.
        self.open_chat(chat_id);

        if let Err(error) = self.send(json!({
            "@type": "viewMessages",
            "chat_id": cid,
            "message_ids": view_ids,
            "source": { "@type": "messageSourceChatHistory" },
            "force_read": true
        })) {
            unidtk_api::log(format!(
                "telegram: viewMessages({chat_id}) failed: {error}"
            ));
        }

        // Clear the manual "marked as unread" flag if the user had set it.
        if let Err(error) = self.send(json!({
            "@type": "toggleChatIsMarkedAsUnread",
            "chat_id": cid,
            "is_marked_as_unread": false
        })) {
            unidtk_api::log(format!(
                "telegram: toggleChatIsMarkedAsUnread({chat_id}) failed: {error}"
            ));
        }
    }

    /// Insert a just-sent message if updateNewMessage has not already stored it.
    fn store_sent_message(
        &mut self,
        chat_id: String,
        message: TelegramMessage,
    ) -> TelegramMessage {
        let list = self.messages.entry(chat_id).or_default();
        if !message.id.is_empty() && list.iter().any(|existing| existing.id == message.id) {
            return message;
        }
        list.push(message.clone());
        trim_messages_to_cap(list, MAX_CACHED_MESSAGES_PER_CHAT);
        message
    }

    /// Drop in-flight proxy-command bookkeeping (reply arrived or client died).
    fn clear_proxy_cmd(&mut self) {
        self.proxy_cmd_extra = None;
        self.proxy_cmd_kind = None;
        self.proxy_cmd_sent_at = None;
    }

    /// True when `value` is the reply to the currently tracked proxy command.
    fn is_proxy_cmd_reply(&self, value: &Value) -> bool {
        let extra = value
            .get("@extra")
            .and_then(|e| e.as_u64().or_else(|| e.as_i64().map(|v| v as u64)));
        matches!((extra, self.proxy_cmd_extra), (Some(a), Some(b)) if a == b)
    }

    /// Forget everything that belongs to the previous TDLib client.
    fn reset_proxy_client_state(&mut self) {
        self.active_proxy_id = None;
        self.active_proxy_key = None;
        self.clear_proxy_cmd();
        self.connecting_since = None;
        self.proxy_heal_attempt = 0;
        self.proxy_heal_next_at = None;
        self.proxy_ensure_failed_at = None;
        self.client_started_at = None;
    }

    /// Cooldown-guarded managed-proxy start. A dead ciadpi must not be
    /// re-spawned (blocking up to ~5 s under the inner lock) on every poll.
    fn ensure_managed_proxy_cached(&mut self) -> Result<(), String> {
        if let Some(failed_at) = self.proxy_ensure_failed_at {
            if failed_at.elapsed() < PROXY_ENSURE_COOLDOWN {
                return Err("managed proxy restart in cooldown".into());
            }
        }
        match ensure_managed_proxy() {
            Ok(()) => {
                self.proxy_ensure_failed_at = None;
                Ok(())
            }
            Err(error) => {
                self.proxy_ensure_failed_at = Some(Instant::now());
                Err(error)
            }
        }
    }

    /// Fire-and-forget proxy command with `@extra` correlation. Never waits:
    /// `apply_proxy_config` runs both on the receiver thread and under the
    /// inner mutex, so a synchronous `td_request` here would self-deadlock.
    fn send_proxy_cmd(&mut self, kind: &'static str, mut request: Value) -> Result<(), String> {
        if self.proxy_cmd_extra.is_some() {
            let fresh = self
                .proxy_cmd_sent_at
                .map(|t| t.elapsed() < Duration::from_secs(10))
                .unwrap_or(false);
            if fresh {
                // One proxy command is already in flight; its reply decides the
                // next step. Dropping the duplicate avoids add/enable churn.
                return Ok(());
            }
            self.clear_proxy_cmd();
        }
        let extra = self.alloc_extra();
        request["@extra"] = json!(extra);
        self.proxy_cmd_extra = Some(extra);
        self.proxy_cmd_kind = Some(kind);
        self.proxy_cmd_sent_at = Some(Instant::now());
        if let Err(error) = self.send(request) {
            self.clear_proxy_cmd();
            return Err(error);
        }
        Ok(())
    }

    /// `disableProxy` is safe to repeat and needs no correlation.
    fn send_disable_proxy(&mut self) -> Result<(), String> {
        self.send(json!({"@type": "disableProxy"}))
    }

    /// Prefer enabling the known proxy record (no new entry, no churn); add (or
    /// re-add) it when there is no matching record. TDLib `addProxy` is
    /// idempotent for an identical endpoint — it returns the same id.
    fn enable_or_add_proxy(
        &mut self,
        server: &str,
        port: u16,
        kind: Value,
        key: String,
    ) -> Result<(), String> {
        if let (Some(id), Some(active_key)) =
            (self.active_proxy_id, self.active_proxy_key.as_deref())
        {
            if active_key == key {
                return self
                    .send_proxy_cmd("enableProxy", json!({"@type": "enableProxy", "proxy_id": id}));
            }
        }
        self.send_proxy_cmd("addProxy", add_proxy_request(server, port, kind, true))
    }

    /// Apply the effective proxy mode to TDLib. Never blocks: every request is
    /// fire-and-forget, outcomes are handled asynchronously in `handle_update`.
    fn apply_proxy_config(&mut self, config: &TelegramConfig) -> Result<(), String> {
        let proxy_type = normalize_proxy_type(config.proxy_type.as_deref());
        if proxy_type == "byedpi" {
            // Managed ByeDPI: bring up the local ciadpi SOCKS5 on
            // 127.0.0.1:MANAGED_PROXY_PORT (desync, no remote server) and
            // point TDLib at it. Host/port/username fields in the config are
            // ignored in this mode.
            //
            // While the wallpaper VPN (vpn.rs) is up, Telegram traffic
            // already exits through its TUN, so our own instance would be
            // redundant: TDLib goes direct ("no proxy") and the managed
            // instance stays down. sync_vpn_proxy() flips this back the moment
            // the VPN disconnects.
            if vpn_connected_from_contract() {
                stop_managed_proxy();
                unidtk_api::log(
                    "telegram: proxy mode byedpi + VPN up → disableProxy (traffic via TUN)",
                );
                return self.send_disable_proxy();
            }
            if let Err(error) = self.ensure_managed_proxy_cached() {
                // No local SOCKS listener → pointing TDLib at it would blackhole
                // the account. Fall back to a direct connection instead.
                let _ = self.send_disable_proxy();
                if error != "managed proxy restart in cooldown" {
                    unidtk_api::log(format!(
                        "telegram: managed proxy unavailable: {error}; TDLib goes direct"
                    ));
                }
                return Err(error);
            }
            unidtk_api::log(format!(
                "telegram: proxy mode byedpi + VPN down → SOCKS 127.0.0.1:{MANAGED_PROXY_PORT}"
            ));
            let key = proxy_endpoint_key("socks5", "", "127.0.0.1", MANAGED_PROXY_PORT);
            return self.enable_or_add_proxy(
                "127.0.0.1",
                MANAGED_PROXY_PORT,
                socks5_proxy_type("", ""),
                key,
            );
        }
        // Any other mode does not need the managed instance running.
        stop_managed_proxy();
        if proxy_type == "none" {
            unidtk_api::log("telegram: proxy mode none → disableProxy");
            return self.send_disable_proxy();
        }

        let host = config
            .proxy_host
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string();
        let Some(port) = config.proxy_port else {
            return Ok(());
        };
        if host.is_empty() || port == 0 {
            return Ok(());
        }

        let username = config.proxy_username.as_deref().unwrap_or("").to_string();
        let password = config.proxy_password.as_deref().unwrap_or("").to_string();
        let (kind, key) = match proxy_type.as_str() {
            "http" => (
                http_proxy_type(&username, &password),
                proxy_endpoint_key("http", &username, &host, port),
            ),
            "socks5" | "socks" => (
                socks5_proxy_type(&username, &password),
                proxy_endpoint_key("socks5", &username, &host, port),
            ),
            _ => return Ok(()),
        };
        self.enable_or_add_proxy(&host, port, kind, key)
    }

    fn send(&mut self, mut request: Value) -> Result<(), String> {
        if request.get("@extra").is_none() {
            request["@extra"] = json!(self.alloc_extra());
        }
        let td = self
            .td
            .as_ref()
            .ok_or_else(|| "TDLib is not loaded".to_string())?;
        td.send(&request.to_string())
    }

    fn handle_update(&mut self, value: Value) {
        match value.get("@type").and_then(Value::as_str) {
            Some("updateAuthorizationState") => self.handle_auth(value.get("authorization_state")),
            // Connection health: TDLib reports Ready / Connecting /
            // ConnectingToProxy / Updating / WaitingForNetwork. The state is
            // normalized and surfaced to the UI so messages stop silently
            // hanging; a stuck Connecting triggers the proxy heal (with backoff).
            Some("updateConnectionState") => {
                let raw = value
                    .get("state")
                    .and_then(|s| s.get("@type"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let state = normalize_connection_state(raw);
                let previous = self.connection_state.clone();
                self.connection_state = state.to_string();
                unidtk_api::log(format!(
                    "telegram: connection state {} -> {}",
                    previous, state
                ));
                self.emit("telegram-connection-state", json!({ "state": state }));
                self.on_connection_state_changed(state);
            }
            // Only these update types actually change chat-list data (title,
            // last message, unread count, order, mute state). TDLib emits many
            // other update types continuously even at rest (presence pings,
            // connection state, options, file progress, ...); emitting
            // "telegram-chat-update" for those too forced the frontend to
            // rebuild the whole widget roughly once a second, which reset the
            // avatar cache (unscrollable list, composer losing focus) and
            // fought the message scroll position on every open chat.
            Some("updateNewChat")
            | Some("updateChatTitle")
            | Some("updateChatLastMessage")
            | Some("updateChatReadInbox")
            | Some("updateChatPosition")
            | Some("updateChatNotificationSettings")
            | Some("updateChatViewAsTopics")
            | Some("updateCommunity")
            | Some("updateCommunityFullInfo") => {
                self.handle_chat_update(value);
                self.emit("telegram-chat-update", ());
            }
            Some("updateChatAddedToList") | Some("updateChatRemovedFromList") => {
                self.handle_chat_list_update(value);
                self.emit("telegram-chat-update", ());
            }
            Some("updateNewMessage") => {
                if let Some(message) = value.get("message") {
                    if let Some(chat_id) = message.get("chat_id").map(td_id_to_string) {
                        let from = self.sender_name(message);
                        let (parsed, missing) = parse_message_with(message, from, &self.custom_emojis);
                        if !missing.is_empty() {
                            self.request_custom_emojis(&missing);
                        }
                        let is_forum = self
                            .chats
                            .get(&chat_id)
                            .or_else(|| self.archived_chats.get(&chat_id))
                            .and_then(|c| c.is_forum)
                            .unwrap_or(false);
                        let thread_id = if is_forum {
                            parsed.message_thread_id.filter(|id| *id != 0)
                        } else {
                            None
                        };
                        let cache_key = cache_key_for(&chat_id, thread_id);
                        // Outgoing send already stored the same id from the
                        // sendMessage response — skip to avoid transient duplicates.
                        let already = parsed.from == "me"
                            && !parsed.id.is_empty()
                            && self
                                .messages
                                .get(&cache_key)
                                .map(|list| list.iter().any(|m| m.id == parsed.id))
                                .unwrap_or(false);
                        if !already {
                            let list = self.messages.entry(cache_key).or_default();
                            list.push(parsed.clone());
                            trim_messages_to_cap(list, MAX_CACHED_MESSAGES_PER_CHAT);
                            if !parsed.id.is_empty() {
                                if let Some(chat) = self
                                    .chats
                                    .get_mut(&chat_id)
                                    .or_else(|| self.archived_chats.get_mut(&chat_id))
                                {
                                    chat.last_message_id = Some(parsed.id.clone());
                                }
                            }
                            self.emit(
                                "telegram-message-new",
                                json!({ "chatId": chat_id, "threadId": thread_id, "message": parsed }),
                            );
                        }
                    }
                }
            }
            Some("updateMessageContent") => {
                // Message body edited (text/caption/entities). new_content has the
                // same shape as message.content (e.g. messageText / messagePhoto).
                let chat_id = value.get("chat_id").map(td_id_to_string).unwrap_or_default();
                let message_id = value
                    .get("message_id")
                    .map(td_id_to_string)
                    .unwrap_or_default();
                if let Some(new_content) = value.get("new_content") {
                    if !chat_id.is_empty() && !message_id.is_empty() {
                        let keys = message_cache_keys_for_chat(&self.messages, &chat_id);
                        let mut updates = Vec::new();
                        for (key, thread_id) in keys {
                            let updated = self
                                .messages
                                .get_mut(&key)
                                .and_then(|list| list.iter_mut().find(|m| m.id == message_id))
                                .map(|msg| {
                                    // message_text / parse_entities expect { "content": ... }.
                                    let wrapper = json!({ "content": new_content });
                                    msg.text = message_text(&wrapper);
                                    msg.entities = parse_entities(&wrapper);
                                    msg.clone()
                                });
                            if let Some(updated) = updated {
                                updates.push((thread_id, updated));
                            }
                        }
                        for (thread_id, updated) in updates {
                            self.emit(
                                "telegram-message-edited",
                                json!({
                                    "chatId": chat_id,
                                    "threadId": thread_id,
                                    "messageId": message_id,
                                    "message": updated
                                }),
                            );
                        }
                    }
                }
            }
            Some("updateDeleteMessages") => {
                let chat_id = value.get("chat_id").map(td_id_to_string).unwrap_or_default();
                let message_ids: Vec<String> = value
                    .get("message_ids")
                    .and_then(Value::as_array)
                    .map(|ids| ids.iter().map(td_id_to_string).collect())
                    .unwrap_or_default();
                if !chat_id.is_empty() && !message_ids.is_empty() {
                    let keys = message_cache_keys_for_chat(&self.messages, &chat_id);
                    let mut changed_threads = Vec::new();
                    for (key, thread_id) in keys {
                        if let Some(list) = self.messages.get_mut(&key) {
                            let old_len = list.len();
                            list.retain(|m| !message_ids.iter().any(|id| id == &m.id));
                            if list.len() != old_len {
                                changed_threads.push(thread_id);
                            }
                        }
                    }
                    for thread_id in changed_threads {
                        self.emit(
                            "telegram-message-deleted",
                            json!({
                                "chatId": chat_id,
                                "threadId": thread_id,
                                "messageIds": message_ids
                            }),
                        );
                    }
                }
            }
            Some("updateMessageSendSucceeded") => {
                // Replaces the temp/optimistic id from sendMessage with the final one.
                if let Some(message) = value.get("message") {
                    let chat_id = message.get("chat_id").map(td_id_to_string).unwrap_or_default();
                    let old_message_id = value
                        .get("old_message_id")
                        .map(td_id_to_string)
                        .unwrap_or_default();
                    if !chat_id.is_empty() && !old_message_id.is_empty() {
                        let from = self.sender_name(message);
                        let (parsed, missing) = parse_message_with(message, from, &self.custom_emojis);
                        if !missing.is_empty() {
                            self.request_custom_emojis(&missing);
                        }
                        let mut replaced_thread = None;
                        let keys = message_cache_keys_for_chat(&self.messages, &chat_id);
                        for (key, thread_id) in keys {
                            let replaced = self
                                .messages
                                .get_mut(&key)
                                .and_then(|list| {
                                    list.iter()
                                        .position(|m| m.id == old_message_id)
                                        .map(|idx| list[idx] = parsed.clone())
                                })
                                .is_some();
                            if replaced {
                                replaced_thread = Some(thread_id);
                                break;
                            }
                        }
                        let replaced = replaced_thread.is_some();
                        // No match → already replaced / deduped via updateNewMessage race.
                        if replaced {
                            let is_forum = self
                                .chats
                                .get(&chat_id)
                                .or_else(|| self.archived_chats.get(&chat_id))
                                .and_then(|c| c.is_forum)
                                .unwrap_or(false);
                            let thread_id = if is_forum {
                                replaced_thread.flatten().or(parsed.message_thread_id).filter(|id| *id != 0)
                            } else {
                                None
                            };
                            self.emit(
                                "telegram-message-sent",
                                json!({
                                    "chatId": chat_id,
                                    "threadId": thread_id,
                                    "oldMessageId": old_message_id,
                                    "message": parsed
                                }),
                            );
                        }
                    }
                }
            }
            Some("updateMessageSendFailed") => {
                // Leave the temp message in place so the UI can show failed/retry.
                if let Some(message) = value.get("message") {
                    let chat_id = message.get("chat_id").map(td_id_to_string).unwrap_or_default();
                    let thread_id = thread_id_from_message(message);
                    let old_message_id = value
                        .get("old_message_id")
                        .map(td_id_to_string)
                        .unwrap_or_default();
                    let error_message = value
                        .get("error")
                        .and_then(|err| err.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("send failed")
                        .to_string();
                    if !chat_id.is_empty() && !old_message_id.is_empty() {
                        self.emit(
                            "telegram-message-send-failed",
                            json!({
                                "chatId": chat_id,
                                "threadId": thread_id,
                                "oldMessageId": old_message_id,
                                "error": error_message
                            }),
                        );
                    }
                }
            }
            Some("updateMessageInteractionInfo") => {
                // Reactions / views changed after the message was already stored.
                let chat_id = value.get("chat_id").map(td_id_to_string).unwrap_or_default();
                let message_id = value
                    .get("message_id")
                    .map(td_id_to_string)
                    .unwrap_or_default();
                if !chat_id.is_empty() && !message_id.is_empty() {
                    let (reactions, reaction_items, missing) = parse_reactions_full(
                        value.get("interaction_info"),
                        &self.custom_emojis,
                    );
                    if !missing.is_empty() {
                        self.request_custom_emojis(&missing);
                    }
                    let keys = message_cache_keys_for_chat(&self.messages, &chat_id);
                    let mut updated_threads = Vec::new();
                    for (key, thread_id) in keys {
                        let found = self
                            .messages
                            .get_mut(&key)
                            .and_then(|list| list.iter_mut().find(|m| m.id == message_id))
                            .map(|msg| {
                                msg.reactions = reactions.clone();
                                msg.reaction_items = reaction_items.clone();
                            })
                            .is_some();
                        if found {
                            updated_threads.push(thread_id);
                        }
                    }
                    for thread_id in updated_threads {
                        self.emit(
                            "telegram-message-reactions",
                            json!({
                                "chatId": chat_id,
                                "threadId": thread_id,
                                "messageId": message_id,
                                "reactions": reactions,
                                "reactionItems": reaction_items,
                            }),
                        );
                    }
                }
            }
            Some("updateMessageIsPinned") => {
                let chat_id = value.get("chat_id").map(td_id_to_string).unwrap_or_default();
                let message_id = value
                    .get("message_id")
                    .map(td_id_to_string)
                    .unwrap_or_default();
                let is_pinned = value
                    .get("is_pinned")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !chat_id.is_empty() && !message_id.is_empty() {
                    let keys = message_cache_keys_for_chat(&self.messages, &chat_id);
                    for (key, _thread_id) in keys {
                        if let Some(list) = self.messages.get_mut(&key) {
                            for m in list.iter_mut() {
                                if m.id == message_id {
                                    m.is_pinned = Some(is_pinned);
                                }
                            }
                        }
                    }
                    self.emit(
                        "telegram-message-pinned",
                        json!({
                            "chatId": chat_id,
                            "messageId": message_id,
                            "isPinned": is_pinned
                        }),
                    );
                }
            }
            Some("updateOption") => {
                let name = value.get("name").and_then(Value::as_str).unwrap_or("");
                if name == "my_id" {
                    if let Some(opt_val) = value.get("value") {
                        if let Some(int_val) = opt_val.get("value").and_then(Value::as_i64) {
                            self.my_user_id = Some(int_val);
                        } else if let Some(str_val) = opt_val.get("value").and_then(Value::as_str) {
                            self.my_user_id = str_val.parse::<i64>().ok();
                        }
                    }
                    if let Some(uid) = self.my_user_id {
                        let _ = self.send(json!({
                            "@type": "getUser",
                            "user_id": uid,
                            "@extra": 1001
                        }));
                    }
                }
            }
            Some("updateUser") => {
                if let Some(user) = value.get("user") {
                    self.cache_user(user);
                    self.update_account_with_user(user);
                }
            }
            Some("updateUserStatus") => {
                let user_id = value.get("user_id").and_then(Value::as_i64).unwrap_or(0);
                if user_id != 0 {
                    if let Some(status_obj) = value.get("status") {
                        let status = parse_user_status(status_obj);
                        self.user_statuses.insert(user_id, status.clone());
                        self.emit(
                            "telegram-user-status",
                            json!({
                                "userId": user_id.to_string(),
                                "status": status
                            }),
                        );
                    }
                }
            }
            // Direct getUser response (fire-and-forget send has no waiter).
            Some("user") => {
                let extra = value.get("@extra").and_then(|e| e.as_u64().or_else(|| e.as_i64().map(|v| v as u64)));
                if extra == Some(1001) {
                    if let Some(uid) = value.get("id").and_then(Value::as_i64) {
                        self.my_user_id = Some(uid);
                    }
                }
                self.cache_user(&value);
                self.update_account_with_user(&value);
            }
            Some("updateFile") => {
                if let Some(file) = value.get("file") {
                    self.handle_file_update(file);
                }
            }
            Some("stickers") => {
                self.handle_custom_emoji_stickers(&value);
            }
            // Reply to a tracked `addProxy` command: remember the record id so
            // later heal/VPN toggles can re-enable it in place.
            Some("addedProxy") => {
                if self.is_proxy_cmd_reply(&value) {
                    if let Some((id, key)) = parse_added_proxy(&value) {
                        let is_enabled = value
                            .get("is_enabled")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        self.active_proxy_id = Some(id);
                        self.active_proxy_key = Some(key.clone());
                        unidtk_api::log(format!(
                            "telegram: proxy enabled id={id} {key} (is_enabled={is_enabled})"
                        ));
                    } else {
                        unidtk_api::log("telegram: addProxy reply without a usable proxy record");
                    }
                    self.clear_proxy_cmd();
                }
            }
            // `enableProxy` answers `ok`; nothing to store, just clear the guard.
            Some("ok") => {
                if self.is_proxy_cmd_reply(&value) {
                    if let Some(kind) = self.proxy_cmd_kind {
                        unidtk_api::log(format!("telegram: {kind} ok"));
                    }
                    self.clear_proxy_cmd();
                }
            }
            Some("error") => {
                let message = value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("TDLib error")
                    .to_string();
                if self.is_proxy_cmd_reply(&value) {
                    // Proxy command failed: log it in context instead of
                    // clobbering the authorization hint.
                    let kind = self.proxy_cmd_kind.unwrap_or("proxy command");
                    unidtk_api::log(format!("telegram: {kind} failed: {message}"));
                    if message.contains("Unknown proxy identifier") {
                        // Stale id (client recreated / account switched): forget
                        // it and re-add the proxy from the current config.
                        self.active_proxy_id = None;
                        self.active_proxy_key = None;
                        self.clear_proxy_cmd();
                        let config = load_config();
                        if let Err(error) = self.apply_proxy_config(&config) {
                            unidtk_api::log(format!("telegram: proxy re-add failed: {error}"));
                        }
                    } else {
                        self.clear_proxy_cmd();
                    }
                } else {
                    self.auth_hint = message.clone();
                    unidtk_api::log(format!("telegram: tdlib error: {message}"));
                }
            }
            _ => {}
        }
    }

    /// Resolve message sender to a display name using users/chats caches.
    fn sender_name(&mut self, message: &Value) -> String {
        if message
            .get("is_outgoing")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return "me".into();
        }
        let sender = message.get("sender_id");
        match sender.and_then(|s| s.get("@type")).and_then(Value::as_str) {
            Some("messageSenderUser") => {
                let user_id = sender
                    .and_then(|s| s.get("user_id"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                if let Some(name) = self.users.get(&user_id) {
                    if !name.is_empty() {
                        return name.clone();
                    }
                }
                if user_id != 0 {
                    // Fire-and-forget; cache fills via updateUser / user response.
                    let _ = self.send(json!({
                        "@type": "getUser",
                        "user_id": user_id
                    }));
                }
                "user".into()
            }
            Some("messageSenderChat") => {
                let chat_id = sender
                    .and_then(|s| s.get("chat_id"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let key = chat_id.to_string();
                if let Some(chat) = self.chats.get(&key) {
                    if !chat.title.is_empty() {
                        return chat.title.clone();
                    }
                }
                if let Some(title) = self.chat_titles_by_id.get(&chat_id) {
                    if !title.is_empty() {
                        return title.clone();
                    }
                }
                "channel".into()
            }
            _ => parse_sender(message),
        }
    }

    fn parse_message(&mut self, message: &Value) -> TelegramMessage {
        let from = self.sender_name(message);
        let (msg, missing) = parse_message_with(message, from, &self.custom_emojis);
        if !missing.is_empty() {
            self.request_custom_emojis(&missing);
        }
        msg
    }

    fn parse_messages(&mut self, response: &Value) -> Option<Vec<TelegramMessage>> {
        let messages = response.get("messages")?.as_array()?;
        let mut parsed: Vec<_> = messages
            .iter()
            .map(|m| self.parse_message(m))
            .collect();
        parsed.reverse();
        Some(parsed)
    }

    fn cache_user(&mut self, user: &Value) {
        let id = match user.get("id").and_then(Value::as_i64) {
            Some(id) if id != 0 => id,
            _ => return,
        };
        let first = user
            .get("first_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let last = user
            .get("last_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let name = format!("{first} {last}").trim().to_string();
        if !name.is_empty() {
            self.users.insert(id, name);
        }
        if let Some(status_obj) = user.get("status") {
            let status = parse_user_status(status_obj);
            self.user_statuses.insert(id, status);
        }
    }

    fn update_account_with_user(&mut self, user: &Value) {
        let user_id = match user.get("id").and_then(Value::as_i64) {
            Some(id) if id != 0 => id,
            _ => return,
        };
        if self.my_user_id != Some(user_id) {
            return;
        }

        let first = user
            .get("first_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let last = user
            .get("last_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let name = format!("{first} {last}").trim().to_string();
        let phone = user
            .get("phone_number")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string());
        let username = user
            .get("usernames")
            .and_then(|u| u.get("active_usernames"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .or_else(|| user.get("username").and_then(Value::as_str))
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string());
        let avatar_path = user
            .get("profile_photo")
            .and_then(|p| p.get("small"))
            .and_then(|s| s.get("local"))
            .and_then(|l| l.get("path"))
            .and_then(Value::as_str)
            .filter(|p| !p.trim().is_empty())
            .map(|s| s.to_string());

        let mut config = load_config();
        let active_id = self.active_account_id.clone();
        if let Some(acc) = config.accounts.iter_mut().find(|a| a.id == active_id) {
            if !name.is_empty() {
                acc.name = name;
            }
            if phone.is_some() {
                acc.phone = phone;
            }
            if username.is_some() {
                acc.username = username;
            }
            if avatar_path.is_some() {
                acc.avatar_path = avatar_path;
            }
            acc.is_active = true;
        } else {
            config.accounts.push(TelegramAccount {
                id: active_id.clone(),
                name: if name.is_empty() { "Telegram Account".into() } else { name },
                phone,
                username,
                avatar_path,
                is_active: true,
            });
        }
        let _ = save_raw_config(&config);
    }

    /// Track connection health and heal a stuck managed proxy with a backoff.
    ///
    /// `Updating` is normal catch-up after a reconnect (can last minutes) and
    /// never triggers a heal; `WaitingForNetwork` means there is no network at
    /// all — swapping the proxy cannot help, TDLib retries by itself.
    fn on_connection_state_changed(&mut self, state: &'static str) {
        if state == "ready" {
            self.connecting_since = None;
            self.proxy_heal_attempt = 0;
            self.proxy_heal_next_at = None;
            return;
        }
        if state != "connecting" && state != "connecting_to_proxy" {
            return;
        }
        let since = *self.connecting_since.get_or_insert_with(Instant::now);
        if self.td.is_none() || vpn_connected_from_contract() {
            return;
        }
        // Grace after client start: a cold MTProto handshake through a desync
        // proxy can legitimately take tens of seconds.
        if let Some(started) = self.client_started_at {
            if started.elapsed() < PROXY_HEAL_GRACE {
                return;
            }
        }
        if let Some(next_at) = self.proxy_heal_next_at {
            if Instant::now() < next_at {
                return;
            }
        }
        // Only heal after the base backoff step of being stuck.
        if since.elapsed() < Duration::from_secs(PROXY_HEAL_BACKOFF_SECS[0]) {
            return;
        }
        if self.proxy_cmd_extra.is_some() {
            // A proxy command is already in flight; wait for its reply.
            return;
        }
        let config = load_config();
        match self.apply_proxy_config(&config) {
            Ok(()) => {
                unidtk_api::log("telegram: connection stuck — re-applied proxy config");
            }
            Err(error) => {
                unidtk_api::log(format!("telegram: proxy heal failed: {error}"));
            }
        }
        self.proxy_heal_attempt = self.proxy_heal_attempt.saturating_add(1);
        let step = PROXY_HEAL_BACKOFF_SECS
            [(self.proxy_heal_attempt as usize - 1).min(PROXY_HEAL_BACKOFF_SECS.len() - 1)];
        self.proxy_heal_next_at = Some(Instant::now() + Duration::from_secs(step));
    }

    fn handle_auth(&mut self, state: Option<&Value>) {
        let state_type = state
            .and_then(|value| value.get("@type"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        self.auth_state = state_type
            .trim_start_matches("authorizationState")
            .to_string();
        self.authorized = state_type == "authorizationStateReady";
        self.auth_hint = match state_type {
            "authorizationStateWaitTdlibParameters" => {
                let config = load_config();
                let api_id = std::env::var("TELEGRAM_API_ID")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| config.api_id.clone())
                    .parse::<i32>()
                    .unwrap_or_default();
                let api_hash = std::env::var("TELEGRAM_API_HASH")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| config.api_hash.clone());
                let dir = tdlib_account_dir(&self.active_account_id);
                let _ = std::fs::create_dir_all(&dir);
                let _ = self.send(json!({
                    "@type": "setTdlibParameters",
                    "api_id": api_id,
                    "api_hash": api_hash,
                    "database_directory": dir.join("db").to_string_lossy(),
                    "files_directory": dir.join("files").to_string_lossy(),
                    "use_file_database": true,
                    "use_chat_info_database": true,
                    "use_message_database": true,
                    "use_secret_chats": false,
                    "system_language_code": "en",
                    "device_model": "Windows",
                    "system_version": "Windows",
                    "application_version": "0.1.0",
                    "database_encryption_key": ""
                }));
                let proxy_hint = match self.apply_proxy_config(&config) {
                    Ok(()) => "TDLib parameters sent.".to_string(),
                    Err(error) => {
                        unidtk_api::log(format!("telegram: proxy setup failed: {error}"));
                        format!("TDLib parameters sent, but proxy setup failed: {error}")
                    }
                };
                proxy_hint
            }
            "authorizationStateWaitPhoneNumber" => "Enter phone number.".into(),
            "authorizationStateWaitCode" => "Enter Telegram login code.".into(),
            "authorizationStateWaitPassword" => "Enter 2FA password.".into(),
            "authorizationStateWaitEmailAddress" => "Enter email address.".into(),
            "authorizationStateWaitEmailCode" => "Enter the code sent to your email.".into(),
            "authorizationStateWaitRegistration" => "Enter your name to finish registration.".into(),
            "authorizationStateWaitEncryptionKey" => "Setting up local storage…".into(),
            "authorizationStateWaitOtherDeviceConfirmation" => {
                // QR login link (tg://…) updates frequently; surface it via auth_hint.
                state
                    .and_then(|value| value.get("link"))
                    .and_then(Value::as_str)
                    .filter(|link| !link.is_empty())
                    .map(|link| link.to_string())
                    .unwrap_or_else(|| "Confirm this login on another device.".into())
            }
            "authorizationStateReady" => {
                // The initial in-memory rows are only for browser preview.
                // Never merge them with a real, authorized TDLib account.
                self.chats.clear();
                self.archived_chats.clear();
                self.messages.clear();
                self.users.clear();
                self.chat_titles_by_id.clear();
                self.chats_exhausted = false;
                self.archive_chats_exhausted = false;
                let _ = self.send(json!({
                    "@type": "loadChats",
                    "chat_list": null,
                    "limit": 100
                }));
                let _ = self.send(json!({
                    "@type": "getMe",
                    "@extra": 1001
                }));
                "Authorized.".into()
            }
            "authorizationStateClosed" => {
                // Client is dead after logOut. We are on the receiver thread, so
                // only signal shutdown and drop our Arc — never join here.
                // The receiver loop exits on the flag; destroy runs when its
                // Arc drops. Next ensure_started() creates a fresh client.
                self.receiver_shutdown.store(true, Ordering::SeqCst);
                self.td = None;
                self.reset_proxy_client_state();
                let _ = self.receiver_handle.take(); // detach; do not join self
                self.chats.clear();
                self.archived_chats.clear();
                self.messages.clear();
                self.chats_exhausted = false;
                self.archive_chats_exhausted = false;
                self.authorized = false;
                self.auth_state = "saved_config".into();
                "Logged out. Reconnecting...".into()
            }
            _ => state_type.into(),
        };
    }

    fn handle_chat_update(&mut self, value: Value) {
        if let Some(chat) = value.get("chat") {
            // updateNewChat fires before the chat has a position in any list
            // (that arrives later via a separate updateChatPosition update), so
            // gating the insert on is_main_chat here would drop it permanently —
            // the position-update branch below only updates existing entries,
            // it never inserts. Always insert; archive/leave position updates
            // remove it if it leaves the main chat list.
            let mut parsed = parse_chat(chat);
            if parsed.photo_file_id.is_some() && parsed.photo_path.is_none() {
                let fid = parsed.photo_file_id.unwrap();
                // Fire-and-forget; skip if this file_id is already downloading
                // (e.g. frontend also requested it). Completion via updateFile.
                // Stale entries (>30s) are ignored so a lost updateFile cannot
                // permanently block retries.
                if !self.download_in_flight(fid) {
                    unidtk_api::log(format!(
                        "telegram avatar: chat={} file={} queue from updateNewChat",
                        parsed.id, fid
                    ));
                    match self.send(json!({
                            "@type": "downloadFile",
                            "file_id": fid,
                            "priority": 1,
                            "offset": 0,
                            "limit": 0,
                            "synchronous": false
                        })) {
                        Ok(()) => {
                            self.downloading_files.insert(fid, Instant::now());
                        }
                        Err(error) => unidtk_api::log(format!(
                            "telegram avatar: chat={} file={} queue failed: {error}",
                            parsed.id, fid
                        )),
                    }
                }
            }
            if let Ok(id) = parsed.id.parse::<i64>() {
                self.chat_titles_by_id.insert(id, parsed.title.clone());
            }
            let chat_id = parsed.id.clone();
            let archive_only = chat
                .get("positions")
                .and_then(Value::as_array)
                .map(|positions| {
                    positions.iter().any(is_archive_position)
                        && !positions.iter().any(is_main_position)
                })
                .unwrap_or(false);
            if archive_only {
                if let Some(order) = chat
                    .get("positions")
                    .and_then(Value::as_array)
                    .and_then(|positions| positions.iter().find(|p| is_archive_position(p)))
                    .and_then(|position| json_i64(position.get("order")))
                {
                    parsed.order = order;
                }
            }
            self.chats.insert(chat_id.clone(), parsed);
            if archive_only {
                if let Some(chat) = self.chats.remove(&chat_id) {
                    self.archived_chats.insert(chat_id.clone(), chat);
                }
            }
            return;
        }
        if let Some(chat_id) = value.get("chat_id").map(td_id_to_string) {
            // updateChatPosition carries a single `position`; updateChatLastMessage
            // and updateNewChat carry a `positions` array. Either can move a chat
            // out of the main list (into the archive, or removed entirely).
            let single_archive = value
                .get("position")
                .map(is_archive_position)
                .unwrap_or(false);
            let array_archive = value
                .get("positions")
                .and_then(Value::as_array)
                .map(|positions| {
                    positions.iter().any(is_archive_position)
                        && !positions.iter().any(is_main_position)
                })
                .unwrap_or(false);
            if single_archive || array_archive {
                if let Some(mut chat) = self
                    .chats
                    .remove(&chat_id)
                    .or_else(|| self.archived_chats.remove(&chat_id))
                {
                    if let Some(position) = value
                        .get("position")
                        .filter(|p| is_archive_position(p))
                        .or_else(|| {
                            value
                                .get("positions")
                                .and_then(Value::as_array)
                                .and_then(|ps| ps.iter().find(|p| is_archive_position(p)))
                        })
                    {
                        chat.order = json_i64(position.get("order")).unwrap_or(chat.order);
                    }
                    self.archived_chats.insert(chat_id.clone(), chat);
                }
            }
            let single_removes = value
                .get("position")
                .map(is_removed_from_main)
                .unwrap_or(false);
            let array_removes = value
                .get("positions")
                .and_then(Value::as_array)
                .map(|positions| {
                    !positions.is_empty()
                        && positions.iter().any(is_removed_from_main)
                        && !positions.iter().any(is_main_position)
                })
                .unwrap_or(false);
            if single_removes || array_removes {
                self.chats.remove(&chat_id);
                return;
            }
            let main_position = value
                .get("position")
                .filter(|position| is_main_position(position))
                .or_else(|| {
                    value
                        .get("positions")
                        .and_then(Value::as_array)
                        .and_then(|positions| positions.iter().find(|p| is_main_position(p)))
                });
            if let Some(position) = main_position {
                if let Some(mut chat) = self.archived_chats.remove(&chat_id) {
                    if let Some(order) = json_i64(position.get("order")) {
                        chat.order = order;
                    }
                    chat.favorite = position
                        .get("is_pinned")
                        .and_then(Value::as_bool)
                        .unwrap_or(chat.favorite);
                    self.chats.insert(chat_id.clone(), chat);
                }
            }
            if let Some(chat) = self
                .chats
                .get_mut(&chat_id)
                .or_else(|| self.archived_chats.get_mut(&chat_id))
            {
                if let Some(title) = value.get("title").and_then(Value::as_str) {
                    chat.title = title.to_string();
                    chat.initials = initials(title);
                    if let Ok(id) = chat_id.parse::<i64>() {
                        self.chat_titles_by_id.insert(id, title.to_string());
                    }
                }
                if let Some(unread) = value.get("unread_count").and_then(Value::as_u64) {
                    chat.unread = unread as u32;
                }
                if let Some(last) = value.get("last_message") {
                    chat.preview = preview_from_message(last);
                    chat.time = time_from_message(last);
                    chat.last_message_id = last
                        .get("id")
                        .map(td_id_to_string)
                        .filter(|value| !value.is_empty());
                }
                // Refresh the sort order from whichever position shape this update
                // carries. Without handling `positions` (plural), a chat bumped to
                // the top by a new message via updateChatLastMessage keeps its stale
                // order and the list never reorders.
                let main_position = value
                    .get("position")
                    .filter(|position| is_main_position(position))
                    .or_else(|| {
                        value
                            .get("positions")
                            .and_then(Value::as_array)
                            .and_then(|positions| positions.iter().find(|p| is_main_position(p)))
                    });
                if let Some(position) = main_position {
                    if let Some(order) = json_i64(position.get("order")) {
                        chat.order = order;
                    }
                    chat.favorite = position
                        .get("is_pinned")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                }
                if let Some(settings) = value.get("notification_settings") {
                    chat.muted = is_muted(settings);
                }
                if let Some(view_as_topics) = value.get("view_as_topics").and_then(Value::as_bool) {
                    chat.is_forum = Some(view_as_topics);
                }
                if let Some(pinned_val) = value.get("pinned_message_id") {
                    let id_str = td_id_to_string(pinned_val);
                    chat.pinned_message_id = if id_str.is_empty() || id_str == "0" {
                        None
                    } else {
                        Some(id_str)
                    };
                }
                if let Some(comm_id) = value.get("linked_community_id").map(td_id_to_string) {
                    if !comm_id.is_empty() && comm_id != "0" {
                        chat.linked_community_id = Some(comm_id);
                    }
                }
            }
        }
    }

    fn handle_chat_list_update(&mut self, value: Value) {
        let Some(chat_id) = value.get("chat_id").map(td_id_to_string) else {
            return;
        };
        let Some(list_type) = value
            .get("chat_list")
            .and_then(|list| list.get("@type"))
            .and_then(Value::as_str)
        else {
            return;
        };
        let added = value.get("@type").and_then(Value::as_str) == Some("updateChatAddedToList");
        let (chat, was_archived) = match self.chats.remove(&chat_id) {
            Some(chat) => (Some(chat), false),
            None => (self.archived_chats.remove(&chat_id), true),
        };
        let Some(chat) = chat else {
            return;
        };
        match (list_type, added) {
            ("chatListArchive", true) => {
                self.chats.remove(&chat_id);
                self.archived_chats.insert(chat_id, chat);
            }
            ("chatListMain", true) => {
                self.archived_chats.remove(&chat_id);
                self.chats.insert(chat_id, chat);
            }
            ("chatListArchive", false) => {
                // Membership updates can be delivered as remove-old then
                // add-new. Preserve the chat in Main during that short gap.
                self.archived_chats.remove(&chat_id);
                self.chats.insert(chat_id, chat);
            }
            ("chatListMain", false) => {
                self.chats.remove(&chat_id);
                // Archiving may arrive as remove-Main followed by add-Archive.
                self.archived_chats.insert(chat_id, chat);
            }
            _ => {
                if was_archived {
                    self.archived_chats.insert(chat_id, chat);
                } else {
                    self.chats.insert(chat_id, chat);
                }
            }
        }
    }

    fn handle_file_update(&mut self, file: &Value) {
        let file_id = file.get("id").and_then(Value::as_i64).map(|v| v as i32);
        if file_id.is_none() {
            return;
        }
        let fid = file_id.unwrap();
        let local = file.get("local");
        let completed = local
            .and_then(|l| l.get("is_downloading_completed"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let is_downloading = local
            .and_then(|l| l.get("is_downloading_active"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !completed {
            // Idle without completion → failed/cancelled; allow a later retry.
            if !is_downloading {
                unidtk_api::log(format!(
                    "telegram avatar: file={fid} download stopped before completion"
                ));
                self.downloading_files.remove(&fid);
                self.attachment_file_owners.remove(&fid);
            }
            return;
        }
        self.downloading_files.remove(&fid);
        let path = local
            .and_then(|l| l.get("path"))
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string());
        if path.is_none() {
            unidtk_api::log(format!(
                "telegram avatar: file={fid} completed but TDLib supplied no local path"
            ));
            self.attachment_file_owners.remove(&fid);
            return;
        }
        let local_path = path.unwrap();
        let mut chat_matches = 0usize;
        for chat in self.chats.values_mut() {
            if chat.photo_file_id == Some(fid) && chat.photo_path.is_none() {
                chat.photo_path = Some(local_path.clone());
                chat_matches += 1;
            }
        }
        // Message attachments: update cached messages and notify the frontend
        // with a dedicated event (telegram-chat-update only refreshes the chat
        // list, so placeholders would otherwise stay stuck).
        let mut attachment_updates: Vec<(String, String, Option<i64>)> = Vec::new();
        if let Some((chat_id, message_id, thread_id)) = self.attachment_file_owners.remove(&fid) {
            let cache_key = cache_key_for(&chat_id, thread_id);
            if let Some(messages) = self.messages.get_mut(&cache_key) {
                if let Some(msg) = messages.iter_mut().find(|m| m.id == message_id) {
                    if msg.attachment_local_path.is_none() {
                        msg.attachment_local_path = Some(local_path.clone());
                        attachment_updates.push((chat_id, message_id, thread_id));
                    }
                }
            }
        }
        for (cache_key, messages) in self.messages.iter_mut() {
            for msg in messages.iter_mut() {
                if msg.attachment_file_id == Some(fid) && msg.attachment_local_path.is_none() {
                    msg.attachment_local_path = Some(local_path.clone());
                    let (chat_id, thread_id) = split_message_cache_key(cache_key);
                    attachment_updates.push((chat_id.to_string(), msg.id.clone(), thread_id));
                }
            }
        }
        if chat_matches > 0 {
            unidtk_api::log(format!(
                "telegram avatar: file={fid} completed, path assigned to {chat_matches} chat(s)"
            ));
            self.emit("telegram-chat-update", ());
        }
        let attachment_count = attachment_updates.len();
        for (chat_id, message_id, thread_id) in attachment_updates {
            unidtk_api::log(format!(
                "telegram attachment: file={fid} ready chat={chat_id} message={message_id}"
            ));
            self.emit(
                "telegram-attachment-ready",
                json!({
                    "chatId": chat_id,
                    "threadId": thread_id,
                    "messageId": message_id,
                    "path": local_path.clone(),
                }),
            );
        }
        let mut custom_emoji_matches = 0usize;
        if let Some(custom_ids) = self.custom_emoji_file_waiters.remove(&fid) {
            for cid in custom_ids {
                self.record_custom_emoji_path(cid, local_path.clone());
                custom_emoji_matches += 1;
            }
        }
        if chat_matches == 0 && attachment_count == 0 && custom_emoji_matches == 0 {
            unidtk_api::log(format!(
                "telegram file: file={fid} completed but matched no pending chat, attachment, or emoji"
            ));
        }
    }

    fn record_custom_emoji_path(&mut self, custom_emoji_id: String, path: String) {
        if self.custom_emojis.len() > 2000 {
            let keys_to_remove: Vec<_> = self.custom_emojis.keys().take(500).cloned().collect();
            for k in keys_to_remove {
                self.custom_emojis.remove(&k);
            }
        }
        self.custom_emojis.insert(custom_emoji_id.clone(), path.clone());

        for messages in self.messages.values_mut() {
            for msg in messages.iter_mut() {
                if let Some(ref mut items) = msg.reaction_items {
                    for item in items.iter_mut() {
                        if item.custom_emoji_id.as_deref() == Some(&custom_emoji_id) {
                            item.custom_emoji_path = Some(path.clone());
                        }
                    }
                }
            }
        }

        self.emit(
            "telegram-custom-emoji-ready",
            json!({
                "customEmojiId": custom_emoji_id,
                "path": path,
            }),
        );
    }

    fn request_custom_emojis(&mut self, ids: &[String]) {
        if self.td.is_none() || !self.authorized {
            return;
        }
        let now = Instant::now();
        self.custom_emojis_in_flight.retain(|_, start| start.elapsed() < Duration::from_secs(60));

        let mut to_request: Vec<String> = Vec::new();
        for id in ids {
            let id = id.trim();
            if id.is_empty() {
                continue;
            }
            if self.custom_emojis.contains_key(id) {
                continue;
            }
            if let Some(started) = self.custom_emojis_in_flight.get(id) {
                if started.elapsed() < Duration::from_secs(15) {
                    continue;
                }
            }
            self.custom_emojis_in_flight.insert(id.to_string(), now);
            to_request.push(id.to_string());
        }

        if to_request.is_empty() {
            return;
        }

        for chunk in to_request.chunks(50) {
            let extra = self.alloc_extra();
            self.custom_emoji_requests.insert(extra, chunk.to_vec());
            let _ = self.send(json!({
                "@type": "getCustomEmojiStickers",
                "custom_emoji_ids": chunk,
                "@extra": extra
            }));
        }
    }

    fn handle_custom_emoji_stickers(&mut self, value: &Value) {
        let extra = value.get("@extra").and_then(|e| e.as_u64().or_else(|| e.as_i64().map(|v| v as u64)));
        let requested_ids = extra.and_then(|ex| self.custom_emoji_requests.remove(&ex));
        let Some(stickers) = value.get("stickers").and_then(Value::as_array) else {
            return;
        };

        for (idx, sticker) in stickers.iter().enumerate() {
            let custom_id = sticker
                .get("full_type")
                .and_then(|ft| ft.get("custom_emoji_id"))
                .map(td_id_to_string)
                .filter(|s| !s.is_empty() && s != "0")
                .or_else(|| {
                    sticker
                        .get("custom_emoji_id")
                        .map(td_id_to_string)
                        .filter(|s| !s.is_empty() && s != "0")
                })
                .or_else(|| {
                    requested_ids.as_ref().and_then(|ids| ids.get(idx).cloned())
                });

            let Some(cid) = custom_id else {
                continue;
            };

            let is_webp = sticker
                .get("format")
                .and_then(|f| f.get("@type"))
                .and_then(Value::as_str)
                == Some("stickerFormatWebp");

            let target_file = sticker
                .get("thumbnail")
                .and_then(|t| t.get("file"))
                .or_else(|| if is_webp { sticker.get("sticker") } else { None });

            let Some(file_obj) = target_file else {
                continue;
            };

            let (file_id, local_path) = file_info(file_obj);
            if let Some(path) = local_path {
                self.record_custom_emoji_path(cid, path);
            } else if let Some(fid) = file_id {
                self.custom_emoji_file_waiters
                    .entry(fid)
                    .or_default()
                    .push(cid);
                if !self.download_in_flight(fid) {
                    let _ = self.send(json!({
                        "@type": "downloadFile",
                        "file_id": fid,
                        "priority": 1,
                        "offset": 0,
                        "limit": 0,
                        "synchronous": false
                    }));
                    self.downloading_files.insert(fid, Instant::now());
                }
            }
        }
    }

    fn alloc_extra(&mut self) -> u64 {
        let extra = self.next_extra;
        self.next_extra += 1;
        extra
    }

    fn logout(&mut self) -> Result<TelegramStatus, String> {
        self.send(json!({"@type": "logOut"}))?;
        self.chats.clear();
        self.archived_chats.clear();
        self.messages.clear();
        self.user_statuses.clear();
        self.downloading_files.clear();
        self.attachment_file_owners.clear();
        self.chats_exhausted = false;
        self.archive_chats_exhausted = false;
        self.open_chat_id = None;
        Ok(self.status())
    }
}

/// Dedicated TDLib receive loop. TDLib requires a single receiver — this is the
/// only place that calls `td.receive`. Responses with a matching `@extra` are
/// routed to `pending`; everything else is handled as an update under a brief
/// inner-mutex lock (must not call `td_request` from handle_update).
fn receiver_loop(
    td: Arc<TdJson>,
    inner: Arc<Mutex<TelegramInner>>,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Value>>>>,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::SeqCst) {
        let text = match td.receive(0.5) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(_) => break,
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                unidtk_api::log(format!("telegram: TDLib JSON parse failed: {error}"));
                continue;
            }
        };
        let extra = value
            .get("@extra")
            .and_then(|e| e.as_u64().or_else(|| e.as_i64().map(|v| v as u64)));
        if let Some(extra) = extra {
            let sender = pending.lock().unwrap().remove(&extra);
            if let Some(tx) = sender {
                let _ = tx.send(value);
                continue;
            }
        }
        if let Ok(mut locked) = inner.lock() {
            locked.handle_update(value);
        }
    }
}

/// Stop the receiver thread, join it, then drop TdJson (so destroy is not called
/// while the thread is blocked in receive).
fn shutdown_client(inner: &Arc<Mutex<TelegramInner>>) {
    let (handle, td) = {
        let mut g = inner.lock().unwrap();
        g.receiver_shutdown.store(true, Ordering::SeqCst);
        g.open_chat_id = None;
        if let Some(ref td) = g.td {
            // Wake a blocked receive (0.5s timeout also works; this speeds exit).
            let _ = td.send(
                &json!({
                    "@type": "getOption",
                    "name": "version",
                    "@extra": 0
                })
                .to_string(),
            );
        }
        g.pending.lock().unwrap().clear();
        g.reset_proxy_client_state();
        (g.receiver_handle.take(), g.td.take())
    };
    if let Some(handle) = handle {
        let _ = handle.join();
    }
    drop(td);
}

/// Load TDLib if needed and spawn the single receiver thread. Never holds the
/// inner mutex across TDLib waits.
fn ensure_started(inner: &Arc<Mutex<TelegramInner>>) -> Result<TelegramStatus, String> {
    {
        let g = inner.lock().unwrap();
        if g.td.is_some() {
            return Ok(g.status());
        }
        // After a failed load, skip re-trying LoadLibrary until the cooldown
        // elapses (or save_config clears last_td_load_failure). Status/error
        // already live in mode/auth_state/auth_hint from the prior attempt.
        if let Some(failed_at) = g.last_td_load_failure {
            if failed_at.elapsed() < TD_LOAD_FAILURE_COOLDOWN {
                return Ok(g.status());
            }
        }
    }

    let config = load_config();
    let api_id = std::env::var("TELEGRAM_API_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| config.api_id.clone())
        .parse::<i32>()
        .ok();
    let api_hash = std::env::var("TELEGRAM_API_HASH")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| config.api_hash.clone());
    if api_id.is_none() || api_hash.trim().is_empty() {
        let mut g = inner.lock().unwrap();
        g.mode = "mock".into();
        g.auth_state = "missing_config".into();
        g.auth_hint = "Enter Telegram API ID, API hash and tdjson.dll path.".into();
        return Ok(g.status());
    }

    let tdjson_path = std::env::var("TDJSON_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| config.tdjson_path.clone())
        .trim()
        .to_string();
    let td = match TdJson::load(if tdjson_path.trim().is_empty() {
        bundled_tdjson_path().map(|path| path.to_string_lossy().to_string())
    } else {
        Some(tdjson_path)
    }) {
        Ok(td) => td,
        Err(error) => {
            inner.lock().unwrap().last_td_load_failure = Some(Instant::now());
            return Err(error);
        }
    };
    let td = Arc::new(td);

    {
        let mut g = inner.lock().unwrap();
        if g.td.is_some() {
            return Ok(g.status());
        }
        g.last_td_load_failure = None;
        let shutdown = Arc::new(AtomicBool::new(false));
        g.receiver_shutdown = shutdown.clone();
        g.pending.lock().unwrap().clear();
        let pending = g.pending.clone();
        let td_thread = td.clone();
        let inner_thread = Arc::clone(inner);
        g.td = Some(td.clone());
        // Fresh client: ids of proxy records from a previous client/account are
        // not ours to re-enable anymore.
        g.reset_proxy_client_state();
        g.client_started_at = Some(Instant::now());
        g.mode = "tdlib".into();
        g.auth_state = "starting".into();
        g.auth_hint = "TDLib loaded; waiting for authorization state.".into();
        // TDLib defaults to verbose stderr logging; the app journal is the
        // source of truth, so keep TDLib at warnings/errors by default.
        // Override with UNIDTK_TD_LOG_LEVEL (1..=5) when debugging TDLib itself.
        let verbosity: i64 = std::env::var("UNIDTK_TD_LOG_LEVEL")
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(2);
        let _ = td.send(
            &json!({"@type": "setLogVerbosityLevel", "new_verbosity_level": verbosity})
                .to_string(),
        );
        g.receiver_handle = Some(std::thread::spawn(move || {
            receiver_loop(td_thread, inner_thread, pending, shutdown);
        }));
        g.send(json!({"@type": "getAuthorizationState"}))?;
    }

    // Let the receiver process WaitTdlibParameters (and follow-ups) without
    // holding the mutex — replaces the old drain_updates(200ms) after start.
    std::thread::sleep(Duration::from_millis(200));

    // If already authorized (existing session), fetch user info to restore
    // account metadata (phone, username) that may have been lost in config.
    {
        let mut g = inner.lock().unwrap();
        if g.authorized && g.td.is_some() {
            let _ = g.send(json!({
                "@type": "getMe",
                "@extra": 1001
            }));
        }
    }
    std::thread::sleep(Duration::from_millis(100));
    Ok(inner.lock().unwrap().status())
}

/// Send a TDLib request and wait for the matching `@extra` response **without**
/// holding the TelegramInner mutex across the wait.
fn td_request(
    inner: &Arc<Mutex<TelegramInner>>,
    mut request: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let (extra, rx, pending) = {
        let mut g = inner.lock().unwrap();
        let extra = g.alloc_extra();
        request["@extra"] = json!(extra);
        let td = g
            .td
            .as_ref()
            .ok_or_else(|| "TDLib is not loaded".to_string())?
            .clone();
        let (tx, rx) = mpsc::channel();
        let pending = g.pending.clone();
        pending.lock().unwrap().insert(extra, tx);
        td.send(&request.to_string())?;
        (extra, rx, pending)
    };
    match rx.recv_timeout(timeout) {
        Ok(value) => {
            if value.get("@type").and_then(Value::as_str) == Some("error") {
                let code = value.get("code").and_then(Value::as_i64).unwrap_or(0);
                let message = value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("TDLib request failed");
                return Err(format!("{code}: {message}"));
            }
            Ok(value)
        }
        Err(RecvTimeoutError::Timeout) => {
            pending.lock().unwrap().remove(&extra);
            Err("TDLib request timed out".into())
        }
        // The pending map was cleared (client shutdown/restart dropped the
        // sender). The request did not time out on the wire — reporting it as
        // a timeout would poison cooldown/backoff logic and the journal.
        Err(RecvTimeoutError::Disconnected) => {
            Err("TDLib client stopped".into())
        }
    }
}

/// After a successful auth request, wait briefly for `updateAuthorizationState`
/// (applied by the receiver thread) without holding the inner mutex.
fn await_auth_progress(inner: &Arc<Mutex<TelegramInner>>, previous_state: &str, timeout: Duration) {
    let until = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < until {
        {
            let g = inner.lock().unwrap();
            if g.auth_state != previous_state {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ── Socket surface (`unidtk_api::commands::ModuleIpcFn`) ───────────────────────
// The kernel registers this under the module id, so its router never names
// `TelegramRuntime`.

fn runtime_from<'a>(
    host: Option<unidtk_api::commands::HostHandle<'a>>,
) -> Result<tauri::State<'a, TelegramRuntime>, String> {
    let app = host
        .and_then(|h| h.downcast_ref::<tauri::AppHandle>())
        .ok_or_else(|| "host handle unavailable".to_string())?;
    tauri::Manager::try_state::<TelegramRuntime>(app)
        .ok_or_else(|| "telegram is not ready".to_string())
}

fn json_of<T: serde::Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

fn chats_json(runtime: &TelegramRuntime) -> Result<serde_json::Value, String> {
    json_of(chats(runtime))
}

fn archive_chats_json(runtime: &TelegramRuntime) -> Result<serde_json::Value, String> {
    json_of(archive_chats(runtime))
}

pub fn ipc_call(
    cmd: &str,
    args: &serde_json::Value,
    host: Option<unidtk_api::commands::HostHandle<'_>>,
) -> Option<Result<serde_json::Value, String>> {
    match cmd {
        "telegram_status" => Some(runtime_from(host).and_then(|runtime| json_of(status(&runtime)))),
        "telegram_chats" => Some(runtime_from(host).and_then(|runtime| {
            // Same shape as the Tauri command: the array itself. The socket version used
            // to wrap it in `{count, items}`.
            chats_json(&runtime)
        })),
        "telegram_chats_archive" => Some(runtime_from(host).and_then(|runtime| {
            archive_chats_json(&runtime)
        })),
        "telegram_set_archived" => Some(runtime_from(host).and_then(|runtime| {
            let request = args
                .get("request")
                .ok_or_else(|| "telegram_set_archived requires {request}".to_string())
                .and_then(|value| {
                    serde_json::from_value::<TelegramArchiveRequest>(value.clone())
                        .map_err(|e| format!("telegram_set_archived: bad request: {e}"))
                })?;
            set_chat_archived(&runtime, request).map(|_| serde_json::Value::Null)
        })),
        "telegram_messages_around" => Some(runtime_from(host).and_then(|runtime| {
            let chat_id = args
                .get("chat_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "telegram_messages_around requires chat_id".to_string())?
                .to_string();
            let message_id = args
                .get("message_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "telegram_messages_around requires message_id".to_string())?
                .to_string();
            let thread_id = args.get("thread_id").and_then(|v| v.as_i64());
            let limit = args.get("limit").and_then(|v| v.as_u64()).map(|v| v as u32);
            let list = messages_around(&runtime, chat_id, message_id, thread_id, limit)?;
            json_of(list)
        })),
        _ => None,
    }
}

pub fn status(runtime: &TelegramRuntime) -> TelegramStatus {
    if let Err(error) = ensure_started(&runtime.inner) {
        let mut inner = runtime.inner.lock().unwrap();
        inner.remember_error(error);
        return inner.status();
    }
    sync_vpn_proxy(&runtime.inner);
    runtime.inner.lock().unwrap().status()
}

/// Set by the single app-shutdown path: from then on [`sync_vpn_proxy`] only
/// stops the managed proxy and never restarts it. `vpn::shutdown()` fires
/// `on_vpn_state_changed()` mid-exit, which used to race a fresh proxy up
/// while the app was already tearing down.
static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Called once on the app-shutdown path, before the VPN is stopped.
pub fn begin_shutdown() {
    SHUTTING_DOWN.store(true, std::sync::atomic::Ordering::SeqCst);
    stop_managed_proxy();
}

/// Canonical proxy mode. Empty/missing defaults to "byedpi" (managed local
/// desync). Legacy "vless" (pre-ByeDPI managed tunnel) maps onto "byedpi".
fn normalize_proxy_type(raw: Option<&str>) -> String {
    let mode = raw.unwrap_or("").trim().to_lowercase();
    if mode.is_empty() {
        return "byedpi".to_string();
    }
    if mode == "vless" {
        return "byedpi".to_string();
    }
    mode
}

/// Canonical connection-state key derived from TDLib's `connectionState…`
/// object name. TDLib reports `Ready`/`Connecting`/`ConnectingToProxy`/
/// `Updating`/`WaitingForNetwork` (after the prefix is trimmed). Older code
/// compared against `online`/`connecting`/`waitingForNetwork`, which never
/// matched — the stuck-connection heal was effectively dead.
fn normalize_connection_state(raw: &str) -> &'static str {
    match raw.trim_start_matches("connectionState") {
        "Ready" | "ready" | "online" => "ready",
        "Connecting" | "connecting" => "connecting",
        "ConnectingToProxy" | "connectingToProxy" => "connecting_to_proxy",
        "Updating" | "updating" => "updating",
        "WaitingForNetwork" | "waitingForNetwork" => "waiting_for_network",
        _ => "unknown",
    }
}

/// Identity of a proxy endpoint used to decide between re-enabling an existing
/// TDLib proxy record (`enableProxy`) and adding one (`addProxy`).
fn proxy_endpoint_key(kind: &str, username: &str, server: &str, port: u16) -> String {
    format!("{kind}://{username}@{server}:{port}")
}

fn proxy_kind_key(kind: Option<&str>) -> &'static str {
    match kind {
        Some("proxyTypeSocks5") => "socks5",
        Some("proxyTypeHttp") => "http",
        Some("proxyTypeMtproto") => "mtproto",
        _ => "unknown",
    }
}

/// Build the modern TDLib `addProxy` request:
/// `addProxy(proxy: Proxy, enable: Bool, comment: String)`.
///
/// The legacy flat shape (top-level `server`/`port`/`type`) is rejected by this
/// TDLib build with `400 Proxy must be non-empty` — that bug kept the managed
/// ByeDPI proxy disabled and pushed Telegram onto a DPI-throttled direct
/// connection (reconnect storms, request timeouts, heavy CPU churn).
fn add_proxy_request(server: &str, port: u16, kind: Value, enable: bool) -> Value {
    json!({
        "@type": "addProxy",
        "proxy": {
            "@type": "proxy",
            "server": server,
            "port": port,
            "type": kind,
        },
        "enable": enable,
        "comment": "",
    })
}

fn socks5_proxy_type(username: &str, password: &str) -> Value {
    json!({"@type": "proxyTypeSocks5", "username": username, "password": password})
}

fn http_proxy_type(username: &str, password: &str) -> Value {
    json!({"@type": "proxyTypeHttp", "username": username, "password": password, "http_only": false})
}

/// Extract `(id, endpoint key)` from an `addedProxy` response, if it carries a
/// usable proxy record.
fn parse_added_proxy(value: &Value) -> Option<(i32, String)> {
    let id = value.get("id").and_then(Value::as_i64)? as i32;
    let proxy = value.get("proxy")?;
    let server = proxy.get("server").and_then(Value::as_str)?;
    let port = proxy.get("port").and_then(Value::as_i64)? as u16;
    let kind = proxy_kind_key(
        proxy
            .get("type")
            .and_then(|t| t.get("@type"))
            .and_then(Value::as_str),
    );
    let username = proxy
        .get("type")
        .and_then(|t| t.get("username"))
        .and_then(Value::as_str)
        .unwrap_or("");
    Some((id, proxy_endpoint_key(kind, username, server, port)))
}

/// Keep the managed "byedpi" proxy in step with the wallpaper VPN: VPN up →
/// TDLib goes direct (Telegram traffic exits via the VPN TUN) and the
/// managed ByeDPI proxy is stopped; VPN down → the proxy comes back
/// up and TDLib proxies through it.
///
/// Called from:
/// - `status()` (safety net if the frontend still polls),
/// - `on_vpn_state_changed()` whenever vpn.rs flips CONNECTED (connect,
///   disconnect, watchdog) — required because the frontend stops polling
///   telegram status once authorized, so toggles used to leave the proxy stuck.
///
/// Manual proxy modes (socks5/http/none) are the user's explicit choice and
/// are left untouched. Empty/missing proxyType defaults to "byedpi" (auto).
fn sync_vpn_proxy(inner: &Arc<Mutex<TelegramInner>>) {
    if SHUTTING_DOWN.load(std::sync::atomic::Ordering::SeqCst) {
        stop_managed_proxy();
        return;
    }
    let vpn_connected = vpn_connected_from_contract();
    let config = load_config();
    let proxy_type = normalize_proxy_type(config.proxy_type.as_deref());

    let mut g = inner.lock().unwrap();
    let transition = g.vpn_connected != vpn_connected;
    g.vpn_connected = vpn_connected;

    if proxy_type != "byedpi" {
        if transition {
            unidtk_api::log(format!(
                "telegram: vpn connected={vpn_connected}; proxy mode '{proxy_type}' is manual — not auto-switched"
            ));
        }
        return;
    }
    if g.td.is_none() {
        // TDLib not running yet — apply_proxy_config() reads
        // vpn::is_connected() directly when TDLib parameters are sent, so
        // the initial proxy setup already honours the VPN state.
        return;
    }

    // Also heal: VPN is down but the managed SOCKS died under us — without
    // this, a one-shot apply after login leaves Telegram blackholed forever.
    let need_heal = !vpn_connected && !managed_proxy_port_open();
    if !transition && !need_heal {
        return;
    }

    unidtk_api::log(format!(
        "telegram: vpn connected={vpn_connected} transition={transition} heal={need_heal}; re-applying proxy mode 'byedpi'"
    ));
    if let Err(error) = g.apply_proxy_config(&config) {
        if error != "managed proxy restart in cooldown" {
            unidtk_api::log(format!(
                "telegram: proxy re-apply after vpn toggle failed: {error}"
            ));
        }
    }
}

/// Entry point for vpn.rs (and any other module) after the CONNECTED flag changes.
pub fn on_vpn_state_changed() {
    if let Some(inner) = TELEGRAM_FOR_VPN.get() {
        sync_vpn_proxy(inner);
    }
}

pub fn chats(runtime: &TelegramRuntime) -> Vec<TelegramChat> {
    let inner = &runtime.inner;
    if let Err(error) = ensure_started(inner) {
        inner.lock().unwrap().remember_error(error);
    }
    // Chat list is polled while the widget is open — reuse it to heal the
    // managed proxy if ciadpi died under us (status() is no longer polled
    // once authorized).
    sync_vpn_proxy(inner);
    {
        let g = inner.lock().unwrap();
        if !g.authorized {
            return Vec::new();
        }
        if g.chats_exhausted {
            return sorted_chats(g.chats.values().cloned().collect());
        }
    }
    // TDLib only loads a batch per loadChats call and requires the
    // client to keep calling until it returns 404 Not Found, otherwise
    // the local main-list cache stays incomplete.
    // Cap total wait so a slow/stuck TDLib cannot block the UI for ~100s.
    const BATCH: i64 = 100;
    const MAX_ROUNDS: usize = 50; // up to ~5000 chats per chats() call
    let deadline = Instant::now() + CHATS_LOAD_DEADLINE;
    for _ in 0..MAX_ROUNDS {
        if Instant::now() >= deadline {
            unidtk_api::log(format!(
                "telegram: load chats: overall deadline ({}s) reached; returning partial list",
                CHATS_LOAD_DEADLINE.as_secs()
            ));
            break;
        }
        match td_request(
            inner,
            json!({"@type": "loadChats", "chat_list": null, "limit": BATCH}),
            Duration::from_secs(2),
        ) {
            Ok(_) => {}
            Err(error) if error.starts_with("404:") || error.contains("Not Found") => {
                inner.lock().unwrap().chats_exhausted = true;
                break;
            }
            Err(error) => {
                unidtk_api::log(format!("telegram: load chats: {error}"));
                break;
            }
        }
    }
    // Touch the main list so TDLib materializes positions. Do not prune
    // self.chats against the returned ids — getChats is still capped by its
    // limit, and retaining would drop already-loaded chats outside that
    // window. Archive / leave removals are handled in handle_chat_update.
    let _ = td_request(
        inner,
        json!({"@type": "getChats", "chat_list": null, "limit": 1000}),
        Duration::from_secs(2),
    );
    let mut list: Vec<TelegramChat> = inner.lock().unwrap().chats.values().cloned().collect();
    {
        let g = inner.lock().unwrap();
        for chat in list.iter_mut() {
            if let Some(uid_str) = &chat.user_id {
                if let Ok(uid) = uid_str.parse::<i64>() {
                    if let Some(st) = g.user_statuses.get(&uid) {
                        chat.status = Some(st.clone());
                    }
                }
            }
        }
    }
    sorted_chats(list)
}

/// Return chats in Telegram's Archive list. The Main and Archive maps are
/// populated only by TDLib list membership updates, so loading one list cannot
/// accidentally make its chats appear in the other.
pub fn archive_chats(runtime: &TelegramRuntime) -> Vec<TelegramChat> {
    let inner = &runtime.inner;
    if let Err(error) = ensure_started(inner) {
        inner.lock().unwrap().remember_error(error);
    }
    if !inner.lock().unwrap().authorized {
        return Vec::new();
    }
    {
        let guard = inner.lock().unwrap();
        if guard.archive_chats_exhausted {
            return sorted_chats(guard.archived_chats.values().cloned().collect());
        }
    }

    const BATCH: i64 = 100;
    const MAX_ROUNDS: usize = 50;
    let deadline = Instant::now() + CHATS_LOAD_DEADLINE;
    for _ in 0..MAX_ROUNDS {
        if Instant::now() >= deadline {
            unidtk_api::log(format!(
                "telegram: load archive: overall deadline ({}s) reached; returning partial list",
                CHATS_LOAD_DEADLINE.as_secs()
            ));
            break;
        }
        match td_request(
            inner,
            json!({
                "@type": "loadChats",
                "chat_list": { "@type": "chatListArchive" },
                "limit": BATCH
            }),
            Duration::from_secs(2),
        ) {
            Ok(_) => {}
            Err(error) if error.starts_with("404:") || error.contains("Not Found") => {
                inner.lock().unwrap().archive_chats_exhausted = true;
                break;
            }
            Err(error) => {
                unidtk_api::log(format!("telegram: load archive: {error}"));
                break;
            }
        }
    }
    let _ = td_request(
        inner,
        json!({
            "@type": "getChats",
            "chat_list": { "@type": "chatListArchive" },
            "limit": 1000
        }),
        Duration::from_secs(2),
    );
    sorted_chats(
        inner
            .lock()
            .unwrap()
            .archived_chats
            .values()
            .cloned()
            .collect(),
    )
}

pub fn set_chat_archived(
    runtime: &TelegramRuntime,
    request: TelegramArchiveRequest,
) -> Result<(), String> {
    if request.chat_id.trim().is_empty() {
        return Err("chat_id is empty".into());
    }
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    let list_type = if request.archived {
        "chatListArchive"
    } else {
        "chatListMain"
    };
    td_request(
        inner,
        json!({
            "@type": "addChatToList",
            "chat_id": parse_chat_id(&request.chat_id),
            "chat_list": { "@type": list_type }
        }),
        Duration::from_secs(5),
    )?;
    Ok(())
}

/// Cache key for a chat's message history. Forum topic threads are stored
/// under `chat_id::<thread_id>` so the main chat and each topic keep separate
/// history windows.
fn cache_key_for(chat_id: &str, thread_id: Option<i64>) -> String {
    match thread_id.filter(|id| *id != 0) {
        Some(thread_id) => format!("{chat_id}::{thread_id}"),
        None => chat_id.to_string(),
    }
}

fn thread_id_from_message(message: &Value) -> Option<i64> {
    message
        .get("topic_id")
        .filter(|topic| topic.get("@type").and_then(Value::as_str) == Some("messageTopicForum"))
        .and_then(|topic| topic.get("forum_topic_id"))
        .and_then(Value::as_i64)
        .filter(|id| *id != 0)
        .or_else(|| {
            message
                .get("message_thread_id")
                .and_then(Value::as_i64)
                .filter(|id| *id != 0)
        })
}

fn message_cache_keys_for_chat(
    messages: &HashMap<String, Vec<TelegramMessage>>,
    chat_id: &str,
) -> Vec<(String, Option<i64>)> {
    let thread_prefix = format!("{chat_id}::");
    messages
        .keys()
        .filter_map(|key| {
            if key == chat_id {
                Some((key.clone(), None))
            } else {
                key.strip_prefix(&thread_prefix)
                    .and_then(|suffix| suffix.parse::<i64>().ok())
                    .map(|thread_id| (key.clone(), Some(thread_id)))
            }
        })
        .collect()
}

fn split_message_cache_key(cache_key: &str) -> (&str, Option<i64>) {
    if let Some((chat_id, thread_id)) = cache_key.rsplit_once("::") {
        if let Ok(thread_id) = thread_id.parse::<i64>() {
            return (chat_id, Some(thread_id));
        }
    }
    (cache_key, None)
}

fn history_request(
    chat_id: &str,
    thread_id: Option<i64>,
    from_message_id: i64,
    limit: i64,
    only_local: bool,
) -> Value {
    match thread_id.filter(|id| *id != 0) {
        Some(forum_topic_id) => json!({
            "@type": "getForumTopicHistory",
            "chat_id": parse_chat_id(chat_id),
            "forum_topic_id": forum_topic_id,
            "from_message_id": from_message_id,
            "offset": 0,
            "limit": limit
        }),
        None => json!({
            "@type": "getChatHistory",
            "chat_id": parse_chat_id(chat_id),
            "from_message_id": from_message_id,
            "offset": 0,
            "limit": limit,
            "only_local": only_local
        }),
    }
}

pub fn messages(runtime: &TelegramRuntime, chat_id: String) -> Vec<TelegramMessage> {
    messages_for_thread(runtime, chat_id, None)
}

/// Fetch chat history for a chat or forum topic. Topics use TDLib's dedicated
/// getForumTopicHistory request; each thread is cached under its own key.
pub fn messages_for_thread(
    runtime: &TelegramRuntime,
    chat_id: String,
    thread_id: Option<i64>,
) -> Vec<TelegramMessage> {
    let inner = &runtime.inner;
    if let Err(error) = ensure_started(inner) {
        inner.lock().unwrap().remember_error(error);
    }
    if !inner.lock().unwrap().authorized {
        return Vec::new();
    }
    let thread_id = thread_id.filter(|id| *id != 0);
    let cache_key = cache_key_for(chat_id.as_str(), thread_id);
    let local_request = history_request(&chat_id, thread_id, 0, 100, true);
    let mut list: Option<Vec<TelegramMessage>> = None;
    match td_request(inner, local_request.clone(), Duration::from_millis(700)) {
        Ok(response) => {
            let mut g = inner.lock().unwrap();
            if let Some(parsed) = g.parse_messages(&response) {
                if !parsed.is_empty() {
                    list = Some(parsed);
                }
            }
        }
        Err(error) => {
            unidtk_api::log(format!(
                "telegram: history local failed for {chat_id}: {error}"
            ));
        }
    }
    let need_remote = if thread_id.is_some() {
        list.is_none()
    } else {
        list.as_ref().map_or(true, |l| l.len() < 25)
    };
    if need_remote {
        let mut remote_request = local_request;
        if thread_id.is_none() {
            remote_request["only_local"] = json!(false);
        }
        match td_request(inner, remote_request, Duration::from_secs(5)) {
            Ok(response) => {
                let mut g = inner.lock().unwrap();
                if let Some(parsed) = g.parse_messages(&response) {
                    if !parsed.is_empty() {
                        list = Some(parsed);
                    }
                } else {
                    unidtk_api::log(format!(
                        "telegram: history remote empty/unparsed for {chat_id}"
                    ));
                }
            }
            Err(error) => {
                unidtk_api::log(format!(
                    "telegram: history remote failed for {chat_id}: {error}"
                ));
            }
        }
    }
    // If the initial batch returned fewer than 25 messages, fetch the preceding batch from TDLib
    if let Some(ref current_list) = list {
        if current_list.len() < 25 {
            if let Some(oldest) = current_list.first() {
                let oldest_id = parse_chat_id(&oldest.id);
                if oldest_id != 0 {
                    let older_request = history_request(&chat_id, thread_id, oldest_id, 50, false);
                    if let Ok(response) = td_request(inner, older_request, Duration::from_secs(4)) {
                        let mut g = inner.lock().unwrap();
                        if let Some(older_parsed) = g.parse_messages(&response) {
                            if !older_parsed.is_empty() {
                                let mut combined = older_parsed;
                                combined.extend(current_list.clone());
                                list = Some(merge_messages(Vec::new(), combined));
                            }
                        }
                    }
                }
            }
        }
    }
    let mut g = inner.lock().unwrap();
    if let Some(list) = list {
        let existing = g.messages.remove(&cache_key).unwrap_or_default();
        let mut merged = merge_messages(existing, list);
        trim_messages_to_cap(&mut merged, MAX_CACHED_MESSAGES_PER_CHAT);
        g.messages.insert(cache_key.clone(), merged);
    }
    g.messages.get(&cache_key).cloned().unwrap_or_default()
}

pub fn mark_read(runtime: &TelegramRuntime, chat_id: String) {
    let mut inner = runtime.inner.lock().unwrap();
    if !inner.authorized || inner.td.is_none() {
        return;
    }
    if let Some(chat) = inner.chats.get_mut(&chat_id) {
        chat.unread = 0;
    }
    let cached = inner
        .messages
        .get(&chat_id)
        .cloned()
        .unwrap_or_default();
    inner.mark_messages_read(&chat_id, &cached);
}

pub fn get_message(
    runtime: &TelegramRuntime,
    chat_id: String,
    message_id: String,
) -> Result<TelegramMessage, String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    let cid = parse_chat_id(&chat_id);
    let mid = parse_chat_id(&message_id);
    let res = td_request(
        inner,
        json!({
            "@type": "getMessage",
            "chat_id": cid,
            "message_id": mid
        }),
        Duration::from_secs(5),
    )?;
    let msg = {
        let mut guard = inner.lock().unwrap();
        guard.parse_message(&res)
    };
    Ok(msg)
}

pub fn get_chat_pinned_message(
    runtime: &TelegramRuntime,
    chat_id: String,
    thread_id: Option<i64>,
) -> Result<Option<TelegramMessage>, String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    let cid = parse_chat_id(&chat_id);
    let thread = thread_id.filter(|&id| id != 0);

    let req = match thread {
        Some(topic_id) => json!({
            "@type": "searchChatMessages",
            "chat_id": cid,
            "query": "",
            "from_message_id": 0,
            "offset": 0,
            "limit": 1,
            "filter": { "@type": "searchMessagesFilterPinned" },
            "message_thread_id": topic_id
        }),
        None => json!({
            "@type": "getChatPinnedMessage",
            "chat_id": cid
        }),
    };

    match td_request(inner, req, CHAT_PINNED_REQUEST_TIMEOUT) {
        Ok(res) => {
            let res_type = res.get("@type").and_then(Value::as_str).unwrap_or("");
            if res_type == "message" {
                let mut guard = inner.lock().unwrap();
                Ok(Some(guard.parse_message(&res)))
            } else if res_type == "messages" {
                if let Some(first_msg) = res.get("messages").and_then(Value::as_array).and_then(|arr| arr.first()) {
                    let mut guard = inner.lock().unwrap();
                    Ok(Some(guard.parse_message(first_msg)))
                } else {
                    Ok(None)
                }
            } else {
                Ok(None)
            }
        }
        Err(err) if is_no_pinned_message_error(&err) => {
            Ok(None)
        }
        Err(err) => {
            throttle_pin_error_log(inner, &chat_id, &err);
            // A timeout or transport failure says nothing about whether the
            // chat has a pinned message. Surface the error instead of masking
            // it as "no pin": the frontend cools this chat's pin requests
            // down and retries once the transport can answer again.
            Err(format!("pinned message lookup failed: {err}"))
        }
    }
}

/// TDLib reports "this chat has no pinned message" as a 404 error. Match by
/// the leading error code, not by substring — "…1404…" or a 404-like number
/// inside the message text must not pass.
fn is_no_pinned_message_error(err: &str) -> bool {
    if err.split(':').next().map(str::trim) == Some("404") {
        return true;
    }
    err.to_ascii_lowercase().contains("not found")
}

/// Log pin-request failures at most once per
/// [`CHAT_PINNED_ERROR_LOG_INTERVAL`] per chat: the frontend retries the
/// request on every render, so each failure would otherwise flood the
/// journal with identical lines while the transport is down.
fn throttle_pin_error_log(inner: &Arc<Mutex<TelegramInner>>, chat_id: &str, err: &str) {
    let mut guard = inner.lock().unwrap();
    let now = Instant::now();
    let due = guard
        .pinned_error_log_at
        .get(chat_id)
        .map(|t| now.duration_since(*t) >= CHAT_PINNED_ERROR_LOG_INTERVAL)
        .unwrap_or(true);
    if !due {
        return;
    }
    if guard.pinned_error_log_at.len() > 256 {
        guard
            .pinned_error_log_at
            .retain(|_, t| now.duration_since(*t) < CHAT_PINNED_ERROR_LOG_INTERVAL * 2);
    }
    guard.pinned_error_log_at.insert(chat_id.to_string(), now);
    unidtk_api::log(format!(
        "telegram: get_chat_pinned_message error for {chat_id}: {err}"
    ));
}

/// Tell TDLib the user is viewing this chat (openChat). Switches away from any
/// previously open chat. Safe to call repeatedly for the same chat.
pub fn open_chat(runtime: &TelegramRuntime, chat_id: String) {
    let mut inner = runtime.inner.lock().unwrap();
    if !inner.authorized || inner.td.is_none() || chat_id.is_empty() {
        return;
    }
    inner.open_chat(&chat_id);
}

/// Tell TDLib the user left a chat. If `chat_id` is empty, closes whatever is open.
pub fn close_chat(runtime: &TelegramRuntime, chat_id: Option<String>) {
    let mut inner = runtime.inner.lock().unwrap();
    if !inner.authorized || inner.td.is_none() {
        return;
    }
    inner.close_chat(chat_id.as_deref());
}

/// Fetch a window of messages centered around `message_id`.
/// Does NOT overwrite or alter the canonical chat stream in `inner.messages`.
pub fn messages_around(
    runtime: &TelegramRuntime,
    chat_id: String,
    message_id: String,
    thread_id: Option<i64>,
    limit: Option<u32>,
) -> Result<Vec<TelegramMessage>, String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }

    let mid = parse_chat_id(&message_id);
    if mid == 0 {
        return Err("Invalid message_id".into());
    }

    let thread = thread_id.filter(|id| *id != 0);
    let effective_limit = limit.unwrap_or(50).clamp(1, 100) as i64;
    let offset = -((effective_limit as i32) / 2);

    let request = match thread {
        Some(topic_id) => json!({
            "@type": "getForumTopicHistory",
            "chat_id": parse_chat_id(&chat_id),
            "forum_topic_id": topic_id,
            "from_message_id": mid,
            "offset": offset,
            "limit": effective_limit
        }),
        None => json!({
            "@type": "getChatHistory",
            "chat_id": parse_chat_id(&chat_id),
            "from_message_id": mid,
            "offset": offset,
            "limit": effective_limit,
            "only_local": false
        }),
    };

    let response = td_request(inner, request, Duration::from_secs(5))?;
    let mut g = inner.lock().unwrap();
    let messages = g.parse_messages(&response).unwrap_or_default();
    Ok(messages)
}

pub fn older_messages(
    runtime: &TelegramRuntime,
    chat_id: String,
    before_message_id: String,
) -> Vec<TelegramMessage> {
    older_messages_for_thread(runtime, chat_id, before_message_id, None)
}

/// Load older messages for a forum topic thread (see `messages_for_thread`).
pub fn older_messages_for_thread(
    runtime: &TelegramRuntime,
    chat_id: String,
    before_message_id: String,
    thread_id: Option<i64>,
) -> Vec<TelegramMessage> {
    let inner = &runtime.inner;
    if ensure_started(inner).is_err() || !inner.lock().unwrap().authorized {
        return Vec::new();
    }
    let thread_id = thread_id.filter(|id| *id != 0);
    let cache_key = cache_key_for(chat_id.as_str(), thread_id);
    let local_request = history_request(
        &chat_id,
        thread_id,
        parse_chat_id(&before_message_id),
        100,
        true,
    );
    let mut older: Option<Vec<TelegramMessage>> = None;
    match td_request(inner, local_request.clone(), Duration::from_secs(2)) {
        Ok(value) => {
            let mut g = inner.lock().unwrap();
            if let Some(parsed) = g.parse_messages(&value) {
                older = Some(parsed);
            }
        }
        Err(error) => {
            unidtk_api::log(format!(
                "telegram: older history local failed for {chat_id}: {error}"
            ));
        }
    }
    let current = inner
        .lock()
        .unwrap()
        .messages
        .get(&cache_key)
        .cloned()
        .unwrap_or_default();
    let known: HashSet<_> = current.iter().map(|message| message.id.clone()).collect();
    let mut new_older = older.unwrap_or_default();
    new_older.retain(|message| !message.id.is_empty() && !known.contains(&message.id));
    // Local cache boundary: no new messages — try remote once.
    if new_older.is_empty() {
        let mut remote_request = local_request;
        if thread_id.is_none() {
            remote_request["only_local"] = json!(false);
        }
        match td_request(inner, remote_request, Duration::from_secs(5)) {
            Ok(value) => {
                let mut g = inner.lock().unwrap();
                if let Some(mut parsed) = g.parse_messages(&value) {
                    parsed.retain(|message| {
                        !message.id.is_empty() && !known.contains(&message.id)
                    });
                    new_older = parsed;
                } else {
                    unidtk_api::log(format!(
                        "telegram: older history remote empty/unparsed for {chat_id}"
                    ));
                }
            }
            Err(error) => {
                unidtk_api::log(format!(
                    "telegram: older history remote failed for {chat_id}: {error}"
                ));
            }
        }
    }
    if !new_older.is_empty() {
        let mut g = inner.lock().unwrap();
        let current = g.messages.remove(&cache_key).unwrap_or_default();
        let mut merged = merge_messages(current, new_older.clone());
        trim_messages_to_cap(&mut merged, MAX_CACHED_MESSAGES_PER_CHAT);
        g.messages.insert(cache_key, merged);
    }
    new_older
}

pub fn send_message(
    runtime: &TelegramRuntime,
    request: TelegramSendRequest,
) -> Result<TelegramMessage, String> {
    let text = request.text.trim();
    if request.chat_id.trim().is_empty() {
        return Err("chat_id is empty".into());
    }
    if text.is_empty() {
        return Err("message is empty".into());
    }
    // Frontend-built entities (see TelegramSendRequest). Offsets are UTF-16
    // code units — the same unit TDLib and JS string indices both use.
    let entities: Vec<Value> = request
        .entities
        .unwrap_or_default()
        .iter()
        .filter_map(|entity| {
            let offset = entity.get("offset").and_then(Value::as_u64)?;
            let length = entity.get("length").and_then(Value::as_u64)?;
            match entity.get("type").and_then(Value::as_str) {
                Some("mentionName") => {
                    let user_id = entity.get("userId").and_then(|v| {
                        v.as_i64()
                            .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
                    })?;
                    Some(json!({
                        "offset": offset,
                        "length": length,
                        "type": { "@type": "textEntityTypeMentionName", "user_id": user_id }
                    }))
                }
                Some("mention") => Some(json!({
                    "offset": offset,
                    "length": length,
                    "type": { "@type": "textEntityTypeMention" }
                })),
                _ => None,
            }
        })
        .collect();
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    // sendMessage returns the sent message (temp id + sending state). Prefer
    // that as the local echo so the UI gets a real id/timestamp; only fall
    // back to a handmade "now" row when the response is not a message.
    // updateNewMessage may also arrive while we wait — store_sent_message
    // and the update handler dedupe by id for outgoing messages.
    let message_thread_id = request.message_thread_id.unwrap_or(0);
    let mut send_payload = json!({
        "@type": "sendMessage",
        "chat_id": parse_chat_id(&request.chat_id),
        "topic_id": if message_thread_id > 0 {
            json!({ "@type": "messageTopicForum", "forum_topic_id": message_thread_id })
        } else {
            Value::Null
        },
        "input_message_content": {
            "@type": "inputMessageText",
            "text": {
                "@type": "formattedText",
                "text": text,
                "entities": entities
            }
        }
    });
    if let Some(reply_to) = request.reply_to_message_id.as_ref() {
        let rid = parse_chat_id(reply_to);
        if rid != 0 {
            send_payload["reply_to"] = json!({
                "@type": "inputMessageReplyToMessage",
                "message_id": rid
            });
        }
    }
    let response = td_request(inner, send_payload, Duration::from_secs(2))?;
    let mut g = inner.lock().unwrap();
    let echo = if response.get("@type").and_then(Value::as_str) == Some("message") {
        g.parse_message(&response)
    } else {
        msg("me", text, "now", None, None)
    };
    let thread_key = request.message_thread_id.filter(|id| *id != 0);
    Ok(g.store_sent_message(cache_key_for(&request.chat_id, thread_key), echo))
}

pub fn edit_message(
    runtime: &TelegramRuntime,
    request: TelegramEditMessageRequest,
) -> Result<TelegramMessage, String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    let text = request.text.trim();
    if text.is_empty() {
        return Err("message text cannot be empty".into());
    }
    let chat_id = parse_chat_id(&request.chat_id);
    let message_id = parse_chat_id(&request.message_id);

    let response = td_request(
        inner,
        json!({
            "@type": "editMessageText",
            "chat_id": chat_id,
            "message_id": message_id,
            "input_message_content": {
                "@type": "inputMessageText",
                "text": {
                    "@type": "formattedText",
                    "text": text,
                    "entities": []
                }
            }
        }),
        Duration::from_secs(3),
    )?;

    let mut g = inner.lock().unwrap();
    let echo = if response.get("@type").and_then(Value::as_str) == Some("message") {
        g.parse_message(&response)
    } else {
        msg("me", text, "now", None, None)
    };

    let keys = message_cache_keys_for_chat(&g.messages, &request.chat_id);
    for (key, _) in keys {
        if let Some(msgs) = g.messages.get_mut(&key) {
            if let Some(m) = msgs.iter_mut().find(|m| m.id == request.message_id) {
                m.text = text.to_string();
            }
        }
    }

    Ok(echo)
}

pub fn delete_messages(
    runtime: &TelegramRuntime,
    request: TelegramDeleteMessagesRequest,
) -> Result<(), String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    let chat_id = parse_chat_id(&request.chat_id);
    let message_ids: Vec<i64> = request.message_ids.iter().map(|id| parse_chat_id(id)).collect();

    td_request(
        inner,
        json!({
            "@type": "deleteMessages",
            "chat_id": chat_id,
            "message_ids": message_ids,
            "revoke": request.revoke
        }),
        Duration::from_secs(3),
    )?;

    let mut g = inner.lock().unwrap();
    let id_set: HashSet<String> = request.message_ids.into_iter().collect();
    let keys = message_cache_keys_for_chat(&g.messages, &request.chat_id);
    for (key, _) in keys {
        if let Some(msgs) = g.messages.get_mut(&key) {
            msgs.retain(|m| !id_set.contains(&m.id));
        }
    }

    Ok(())
}

pub fn set_reaction(
    runtime: &TelegramRuntime,
    request: TelegramReactionRequest,
) -> Result<(), String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    let chat_id = parse_chat_id(&request.chat_id);
    let message_id = parse_chat_id(&request.message_id);
    if chat_id == 0 || message_id == 0 {
        return Err("chat_id or message_id is invalid".into());
    }

    let is_remove = request.remove
        || (request.emoji.as_deref() == Some("") && request.custom_emoji_id.is_none());

    let is_custom = request.reaction_type.as_deref() == Some("custom_emoji")
        || request.custom_emoji_id.as_deref().is_some_and(|s| !s.is_empty());

    let reaction_obj = if is_custom {
        let cid = request.custom_emoji_id.unwrap_or_default();
        if cid.is_empty() {
            return Err("custom_emoji_id is empty".into());
        }
        json!({
            "@type": "reactionTypeCustomEmoji",
            "custom_emoji_id": cid
        })
    } else {
        let emoji = request.emoji.unwrap_or_else(|| "👍".into());
        json!({
            "@type": "reactionTypeEmoji",
            "emoji": emoji
        })
    };

    if is_remove {
        match td_request(
            inner,
            json!({
                "@type": "removeMessageReaction",
                "chat_id": chat_id,
                "message_id": message_id,
                "reaction_type": reaction_obj
            }),
            Duration::from_secs(3),
        ) {
            Ok(_) => Ok(()),
            Err(e) => {
                if e.contains("REACTION_NOT_FOUND") || e.contains("MESSAGE_NOT_MODIFIED") {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    } else {
        td_request(
            inner,
            json!({
                "@type": "addMessageReaction",
                "chat_id": chat_id,
                "message_id": message_id,
                "reaction_type": reaction_obj,
                "is_big": false,
                "update_recent_reactions": true
            }),
            Duration::from_secs(3),
        )?;
        Ok(())
    }
}

pub fn get_custom_emoji_path(
    runtime: &TelegramRuntime,
    custom_emoji_id: &str,
) -> Result<Option<String>, String> {
    let mut g = runtime.inner.lock().unwrap();
    if let Some(path) = g.custom_emojis.get(custom_emoji_id).cloned() {
        return Ok(Some(path));
    }
    g.request_custom_emojis(&[custom_emoji_id.to_string()]);
    Ok(None)
}

pub fn pin_message(
    runtime: &TelegramRuntime,
    request: TelegramPinMessageRequest,
) -> Result<(), String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    let chat_id = parse_chat_id(&request.chat_id);
    let message_id = parse_chat_id(&request.message_id);

    td_request(
        inner,
        json!({
            "@type": "pinChatMessage",
            "chat_id": chat_id,
            "message_id": message_id,
            "disable_notification": false,
            "only_for_self": false
        }),
        Duration::from_secs(3),
    )?;

    Ok(())
}

pub fn unpin_message(
    runtime: &TelegramRuntime,
    request: TelegramPinMessageRequest,
) -> Result<(), String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    let chat_id = parse_chat_id(&request.chat_id);
    let message_id = parse_chat_id(&request.message_id);

    td_request(
        inner,
        json!({
            "@type": "unpinChatMessage",
            "chat_id": chat_id,
            "message_id": message_id
        }),
        Duration::from_secs(3),
    )?;

    Ok(())
}

pub fn forward_messages(
    runtime: &TelegramRuntime,
    request: TelegramForwardMessagesRequest,
) -> Result<Vec<TelegramMessage>, String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    let to_chat_id = parse_chat_id(&request.to_chat_id);
    let from_chat_id = parse_chat_id(&request.from_chat_id);
    let message_ids: Vec<i64> = request.message_ids.iter().map(|id| parse_chat_id(id)).collect();

    let response = td_request(
        inner,
        json!({
            "@type": "forwardMessages",
            "chat_id": to_chat_id,
            "from_chat_id": from_chat_id,
            "message_ids": message_ids,
            "send_copy": false,
            "remove_caption": false
        }),
        Duration::from_secs(4),
    )?;

    let mut g = inner.lock().unwrap();
    if let Some(messages) = response.get("messages").and_then(Value::as_array) {
        let parsed: Vec<TelegramMessage> = messages.iter().map(|m| g.parse_message(m)).collect();
        return Ok(parsed);
    }

    Ok(Vec::new())
}

pub fn delete_chat_history(
    runtime: &TelegramRuntime,
    request: TelegramDeleteChatHistoryRequest,
) -> Result<(), String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    let chat_id = parse_chat_id(&request.chat_id);

    td_request(
        inner,
        json!({
            "@type": "deleteChatHistory",
            "chat_id": chat_id,
            "remove_from_chat_list": request.remove_from_chat_list,
            "revoke": request.revoke
        }),
        Duration::from_secs(3),
    )?;

    let mut g = inner.lock().unwrap();
    for (key, _) in message_cache_keys_for_chat(&g.messages, &request.chat_id) {
        g.messages.remove(&key);
    }
    if request.remove_from_chat_list {
        g.chats.remove(&request.chat_id);
        g.archived_chats.remove(&request.chat_id);
    }

    Ok(())
}

pub fn send_callback_query(
    runtime: &TelegramRuntime,
    request: TelegramCallbackQueryRequest,
) -> Result<Value, String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    let chat_id = parse_chat_id(&request.chat_id);
    let message_id = parse_chat_id(&request.message_id);

    td_request(
        inner,
        json!({
            "@type": "getCallbackQueryAnswer",
            "chat_id": chat_id,
            "message_id": message_id,
            "payload": {
                "@type": "callbackQueryPayloadData",
                "data": request.data
            }
        }),
        Duration::from_secs(10),
    )
}

pub fn get_forum_topics(
    runtime: &TelegramRuntime,
    request: TelegramForumTopicsRequest,
) -> Result<Value, String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    let chat_id = parse_chat_id(&request.chat_id);
    let query = request.query.unwrap_or_default();

    let td_res = td_request(
        inner,
        json!({
            "@type": "getForumTopics",
            "chat_id": chat_id,
            "query": query,
            "offset_date": 0,
            "offset_message_id": 0,
            "offset_forum_topic_id": 0,
            "limit": 100
        }),
        Duration::from_secs(10),
    );

    if let Ok(ref val) = td_res {
        if val.get("topics").and_then(Value::as_array).map(|a| !a.is_empty()).unwrap_or(false) {
            return td_res;
        }
    }

    // Community / linked subchats fallback
    let guard = inner.lock().unwrap();
    let parent_id = request.chat_id.clone();
    let parent_chat = guard.chats.get(&parent_id);
    let parent_title = parent_chat.map(|c| c.title.as_str()).unwrap_or("");

    let mut subchats: Vec<Value> = guard.chats.values()
        .filter(|c| {
            if c.id == parent_id {
                return false;
            }
            if let Some(ref l_id) = c.linked_community_id {
                if l_id == &parent_id {
                    return true;
                }
            }
            if !parent_title.is_empty() && (
                (parent_title.contains("Элиор") && c.title.contains("Элиор")) ||
                (parent_title.contains('|') && c.title.starts_with(parent_title.split('|').next().unwrap_or("").trim()))
            ) {
                return true;
            }
            false
        })
        .map(|c| {
            json!({
                "info": {
                    "message_thread_id": 0,
                    "name": c.title,
                    "subchat_id": c.id,
                },
                "unread_count": c.unread,
                "last_message": {
                    "content": {
                        "text": { "text": c.preview }
                    }
                }
            })
        })
        .collect();

    if !subchats.is_empty() {
        subchats.sort_by(|a, b| {
            let na = a.pointer("/info/name").and_then(Value::as_str).unwrap_or("");
            let nb = b.pointer("/info/name").and_then(Value::as_str).unwrap_or("");
            na.cmp(nb)
        });
        return Ok(json!({
            "@type": "forumTopics",
            "total_count": subchats.len(),
            "topics": subchats
        }));
    }

    td_res
}

/// Group/supergroup members for the composer `@` autocomplete. Each entry
/// carries the display name and the public username (when set) so the
/// frontend can insert either a plain `@username` text mention or a
/// `mentionName` entity for users without one.
pub fn chat_members(
    runtime: &TelegramRuntime,
    chat_id: String,
    query: String,
) -> Result<Vec<TelegramMember>, String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    if chat_id.trim().is_empty() {
        return Err("chat_id is empty".into());
    }
    let response = td_request(
        inner,
        json!({
            "@type": "searchChatMembers",
            "chat_id": parse_chat_id(&chat_id),
            "query": query.trim(),
            "limit": 8
        }),
        Duration::from_secs(3),
    )?;
    let user_ids: Vec<i64> = response
        .get("members")
        .and_then(Value::as_array)
        .map(|members| {
            members
                .iter()
                .filter(|member| {
                    member
                        .get("member_id")
                        .and_then(|id| id.get("@type"))
                        .and_then(Value::as_str)
                        == Some("messageSenderUser")
                })
                .filter_map(|member| {
                    member
                        .get("member_id")
                        .and_then(|id| id.get("user_id"))
                        .and_then(Value::as_i64)
                })
                .filter(|id| *id != 0)
                .collect()
        })
        .unwrap_or_default();
    let mut members = Vec::new();
    for user_id in user_ids.into_iter().take(8) {
        let user = td_request(
            inner,
            json!({ "@type": "getUser", "user_id": user_id }),
            Duration::from_secs(2),
        )
        .ok();
        let mut name = String::new();
        let mut username = String::new();
        if let Some(user) = user.as_ref() {
            let first = user
                .get("first_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let last = user
                .get("last_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            name = format!("{first} {last}").trim().to_string();
            // TDLib ≥1.8 nests usernames; older builds expose a flat field.
            username = user
                .get("usernames")
                .and_then(|u| u.get("active_usernames"))
                .and_then(Value::as_array)
                .and_then(|list| list.first())
                .and_then(Value::as_str)
                .or_else(|| user.get("username").and_then(Value::as_str))
                .unwrap_or("")
                .to_string();
        }
        if name.is_empty() {
            name = inner
                .lock()
                .unwrap()
                .users
                .get(&user_id)
                .cloned()
                .unwrap_or_default();
        }
        if name.is_empty() {
            name = format!("User {user_id}");
        }
        members.push(TelegramMember {
            user_id: user_id.to_string(),
            name,
            username,
        });
    }
    Ok(members)
}

pub fn send_attachment(
    runtime: &TelegramRuntime,
    request: TelegramSendAttachmentRequest,
) -> Result<TelegramMessage, String> {
    let path = request.path.trim();
    if request.chat_id.trim().is_empty() {
        return Err("chat_id is empty".into());
    }
    if path.is_empty() {
        return Err("attachment path is empty".into());
    }
    if !std::path::Path::new(path).exists() {
        return Err("attachment file does not exist".into());
    }
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }

    let caption = request.caption.unwrap_or_default();
    let caption_obj = json!({
        "@type": "formattedText",
        "text": caption,
        "entities": []
    });
    let file_local = json!({
        "@type": "inputFileLocal",
        "path": path
    });
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let input_message_content = match ext.as_str() {
        "jpg" | "jpeg" | "png" | "webp" => json!({
            "@type": "inputMessagePhoto",
            "photo": file_local,
            "caption": caption_obj
        }),
        "gif" => json!({
            "@type": "inputMessageAnimation",
            "animation": file_local,
            "caption": caption_obj
        }),
        _ => json!({
            "@type": "inputMessageDocument",
            "document": file_local,
            "caption": caption_obj
        }),
    };
    let message_thread_id = request.message_thread_id.unwrap_or(0);
    let mut send_payload = json!({
        "@type": "sendMessage",
        "chat_id": parse_chat_id(&request.chat_id),
        "topic_id": if message_thread_id > 0 {
            json!({ "@type": "messageTopicForum", "forum_topic_id": message_thread_id })
        } else {
            Value::Null
        },
        "input_message_content": input_message_content
    });
    if let Some(reply_to) = request.reply_to_message_id.as_ref() {
        let rid = parse_chat_id(reply_to);
        if rid != 0 {
            send_payload["reply_to"] = json!({
                "@type": "inputMessageReplyToMessage",
                "message_id": rid
            });
        }
    }
    let response = td_request(
        inner,
        send_payload,
        Duration::from_secs(5),
    )?;
    // Same as send_message: prefer TDLib's message; fall back only if the
    // response is not a parseable message object.
    let mut g = inner.lock().unwrap();
    let echo = if response.get("@type").and_then(Value::as_str) == Some("message") {
        g.parse_message(&response)
    } else {
        let file_name = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");
        let kind = match ext.as_str() {
            "jpg" | "jpeg" | "png" | "webp" => "photo",
            "gif" => "animation",
            _ => "document",
        };
        msg("me", &caption, "now", Some(file_name), Some(kind))
    };
    let thread_key = request.message_thread_id.filter(|id| *id != 0);
    Ok(g.store_sent_message(cache_key_for(&request.chat_id, thread_key), echo))
}

pub fn set_phone(
    runtime: &TelegramRuntime,
    phone: TelegramPhone,
) -> Result<TelegramStatus, String> {
    let inner = &runtime.inner;
    ensure_started(inner)?;
    {
        let mut g = inner.lock().unwrap();
        if g.auth_state != "WaitPhoneNumber" {
            // Recoverable sequencing mistake: keep the real TDLib state so the
            // UI does not treat this as a fatal "error" / API-setup screen.
            g.auth_hint = format!(
                "Telegram is waiting for {}, not a phone number",
                g.auth_state
            );
            return Ok(g.status());
        }
        let phone_number = phone.phone_number.trim();
        if phone_number.is_empty() {
            g.auth_hint = "Phone number is empty.".into();
            return Ok(g.status());
        }
    }
    let previous = inner.lock().unwrap().auth_state.clone();
    let phone_number = phone.phone_number.trim().to_string();
    match td_request(
        inner,
        json!({
            "@type": "setAuthenticationPhoneNumber",
            "phone_number": phone_number,
            "settings": null
        }),
        Duration::from_secs(5),
    ) {
        Ok(_) => {
            // Prefer the post-request auth update (WaitCode) before returning.
            await_auth_progress(inner, &previous, Duration::from_secs(3));
            Ok(inner.lock().unwrap().status())
        }
        Err(error) => {
            // Wrong/invalid phone and similar TDLib errors must stay on
            // WaitPhoneNumber with a hint — bubbling Err makes the client
            // rewrite authState to "error" and drop the phone form.
            let mut g = inner.lock().unwrap();
            g.auth_hint = error;
            Ok(g.status())
        }
    }
}

pub fn check_code(
    runtime: &TelegramRuntime,
    code: TelegramAuthCode,
) -> Result<TelegramStatus, String> {
    let inner = &runtime.inner;
    ensure_started(inner)?;
    {
        let mut g = inner.lock().unwrap();
        if g.auth_state != "WaitCode" {
            g.auth_hint = format!(
                "Telegram is waiting for {}, not a login code",
                g.auth_state
            );
            return Ok(g.status());
        }
        let code = code.code.trim();
        if code.is_empty() {
            g.auth_hint = "Login code is empty.".into();
            return Ok(g.status());
        }
    }
    let previous = inner.lock().unwrap().auth_state.clone();
    let code = code.code.trim().to_string();
    match td_request(
        inner,
        json!({"@type": "checkAuthenticationCode", "code": code}),
        Duration::from_secs(5),
    ) {
        Ok(_) => {
            // May advance to WaitPassword (2FA) or Ready; wait so status()
            // reflects the next step instead of still saying WaitCode.
            await_auth_progress(inner, &previous, Duration::from_secs(3));
            Ok(inner.lock().unwrap().status())
        }
        Err(error) => {
            // PHONE_CODE_INVALID / PHONE_CODE_EXPIRED / etc. leave state on
            // WaitCode so the user can retry; do not return Err.
            let mut g = inner.lock().unwrap();
            g.auth_hint = error;
            Ok(g.status())
        }
    }
}

pub fn check_password(
    runtime: &TelegramRuntime,
    password: TelegramPassword,
) -> Result<TelegramStatus, String> {
    let inner = &runtime.inner;
    ensure_started(inner)?;
    {
        let mut g = inner.lock().unwrap();
        if g.auth_state != "WaitPassword" {
            g.auth_hint = format!(
                "Telegram is waiting for {}, not a 2FA password",
                g.auth_state
            );
            return Ok(g.status());
        }
        // TDLib rejects empty passwords; surface that without leaving WaitPassword.
        if password.password.is_empty() {
            g.auth_hint = "Password is empty.".into();
            return Ok(g.status());
        }
    }
    let previous = inner.lock().unwrap().auth_state.clone();
    match td_request(
        inner,
        json!({"@type": "checkAuthenticationPassword", "password": password.password}),
        Duration::from_secs(5),
    ) {
        Ok(_) => {
            await_auth_progress(inner, &previous, Duration::from_secs(3));
            Ok(inner.lock().unwrap().status())
        }
        Err(error) => {
            // PASSWORD_HASH_INVALID and flood waits stay on WaitPassword.
            let mut g = inner.lock().unwrap();
            g.auth_hint = error;
            Ok(g.status())
        }
    }
}

pub fn set_email(
    runtime: &TelegramRuntime,
    email: TelegramEmail,
) -> Result<TelegramStatus, String> {
    let inner = &runtime.inner;
    ensure_started(inner)?;
    {
        let mut g = inner.lock().unwrap();
        if g.auth_state != "WaitEmailAddress" {
            g.auth_hint = format!(
                "Telegram is waiting for {}, not an email address",
                g.auth_state
            );
            return Ok(g.status());
        }
        let email_address = email.email_address.trim();
        if email_address.is_empty() {
            g.auth_hint = "Email address is empty.".into();
            return Ok(g.status());
        }
    }
    let previous = inner.lock().unwrap().auth_state.clone();
    let email_address = email.email_address.trim().to_string();
    match td_request(
        inner,
        json!({
            "@type": "setAuthenticationEmailAddress",
            "email_address": email_address
        }),
        Duration::from_secs(5),
    ) {
        Ok(_) => {
            await_auth_progress(inner, &previous, Duration::from_secs(3));
            Ok(inner.lock().unwrap().status())
        }
        Err(error) => {
            let mut g = inner.lock().unwrap();
            g.auth_hint = error;
            Ok(g.status())
        }
    }
}

pub fn check_email_code(
    runtime: &TelegramRuntime,
    code: TelegramEmailCode,
) -> Result<TelegramStatus, String> {
    let inner = &runtime.inner;
    ensure_started(inner)?;
    {
        let mut g = inner.lock().unwrap();
        if g.auth_state != "WaitEmailCode" {
            g.auth_hint = format!(
                "Telegram is waiting for {}, not an email code",
                g.auth_state
            );
            return Ok(g.status());
        }
        let code = code.code.trim();
        if code.is_empty() {
            g.auth_hint = "Email code is empty.".into();
            return Ok(g.status());
        }
    }
    let previous = inner.lock().unwrap().auth_state.clone();
    let code = code.code.trim().to_string();
    match td_request(
        inner,
        json!({
            "@type": "checkAuthenticationEmailCode",
            "code": {
                "@type": "emailAddressAuthenticationCode",
                "code": code
            }
        }),
        Duration::from_secs(5),
    ) {
        Ok(_) => {
            await_auth_progress(inner, &previous, Duration::from_secs(3));
            Ok(inner.lock().unwrap().status())
        }
        Err(error) => {
            let mut g = inner.lock().unwrap();
            g.auth_hint = error;
            Ok(g.status())
        }
    }
}

pub fn register_user(
    runtime: &TelegramRuntime,
    registration: TelegramRegistration,
) -> Result<TelegramStatus, String> {
    let inner = &runtime.inner;
    ensure_started(inner)?;
    {
        let mut g = inner.lock().unwrap();
        if g.auth_state != "WaitRegistration" {
            g.auth_hint = format!(
                "Telegram is waiting for {}, not registration",
                g.auth_state
            );
            return Ok(g.status());
        }
        let first_name = registration.first_name.trim();
        if first_name.is_empty() {
            g.auth_hint = "First name is empty.".into();
            return Ok(g.status());
        }
    }
    let previous = inner.lock().unwrap().auth_state.clone();
    let first_name = registration.first_name.trim().to_string();
    let last_name = registration
        .last_name
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    match td_request(
        inner,
        json!({
            "@type": "registerUser",
            "first_name": first_name,
            "last_name": last_name,
            "disable_notification": false
        }),
        Duration::from_secs(5),
    ) {
        Ok(_) => {
            await_auth_progress(inner, &previous, Duration::from_secs(3));
            Ok(inner.lock().unwrap().status())
        }
        Err(error) => {
            let mut g = inner.lock().unwrap();
            g.auth_hint = error;
            Ok(g.status())
        }
    }
}

pub fn logout(runtime: &TelegramRuntime) -> Result<TelegramStatus, String> {
    runtime.inner.lock().unwrap().logout()
}

/// Kick off a non-blocking TDLib download. Does **not** hold the global
/// mutex across a 30s wait: returns immediately with the local path if
/// already known, or `Ok("")` once an async `downloadFile` has been sent.
/// Completion is delivered via `updateFile` → `handle_file_update` (chat
/// photo → `telegram-chat-update`; message attachment →
/// `telegram-attachment-ready`).
pub fn download_file(runtime: &TelegramRuntime, file_id: i32) -> Result<String, String> {
    let inner = &runtime.inner;
    if !ensure_started(inner)?.authorized {
        return Err("Telegram is not authorized".into());
    }
    // Short getFile probe: if TDLib already has the file on disk, return it.
    if let Ok(response) = td_request(
        inner,
        json!({"@type": "getFile", "file_id": file_id}),
        Duration::from_secs(1),
    ) {
        let local = response.get("local");
        let completed = local
            .and_then(|l| l.get("is_downloading_completed"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if completed {
            if let Some(path) = local
                .and_then(|l| l.get("path"))
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())
            {
                unidtk_api::log(format!(
                    "telegram avatar: file={file_id} frontend request hit local cache"
                ));
                return Ok(path.to_string());
            }
        }
    }
    let mut g = inner.lock().unwrap();
    // Deduplicate with chat-parse fire-and-forget and concurrent UI calls.
    // Stale (>30s) entries do not block a fresh request.
    if g.download_in_flight(file_id) {
        unidtk_api::log(format!(
            "telegram avatar: file={file_id} frontend request deduplicated (already in flight)"
        ));
        return Ok(String::new());
    }
    // Record which message owns this file_id (if any) so completion can emit
    // telegram-attachment-ready with chatId/messageId.
    if let Some((chat_id, message_id, thread_id)) = g.messages.iter().find_map(|(key, msgs)| {
        msgs.iter()
            .find(|m| m.attachment_file_id == Some(file_id))
            .map(|m| {
                let (chat_id, thread_id) = split_message_cache_key(key);
                (chat_id.to_string(), m.id.clone(), thread_id)
            })
    }) {
        g.attachment_file_owners
            .insert(file_id, (chat_id, message_id, thread_id));
    }
    // Fire-and-forget async download; completion via updateFile.
    unidtk_api::log(format!("telegram avatar: file={file_id} frontend request queued"));
    if let Err(error) = g.send(json!({
        "@type": "downloadFile",
        "file_id": file_id,
        "priority": 1,
        "offset": 0,
        "limit": 0,
        "synchronous": false
    })) {
        unidtk_api::log(format!("telegram avatar: file={file_id} frontend queue failed: {error}"));
        g.attachment_file_owners.remove(&file_id);
        return Err(error);
    }
    g.downloading_files.insert(file_id, Instant::now());
    Ok(String::new())
}

pub fn get_config() -> TelegramConfig {
    let mut config = with_config_defaults(load_config());
    config.api_hash.clear();
    config.proxy_password = None;
    config
}

pub fn debug_info(runtime: &TelegramRuntime) -> TelegramDebugInfo {
    let inner = runtime.inner.lock().unwrap();
    let config = with_config_defaults(load_config());
    let config_path = telegram_config_path();
    let tdjson_path = config.tdjson_path.trim().to_string();
    let proxy_kind = normalize_proxy_type(config.proxy_type.as_deref());
    // "byedpi" is fully managed (own ciadpi on a fixed local port), so it
    // needs no host/port from the config; manual socks5/http modes do.
    let proxy_configured = if proxy_kind == "byedpi" {
        true
    } else {
        !proxy_kind.is_empty()
            && proxy_kind != "none"
            && config
                .proxy_host
                .as_deref()
                .map(|host| !host.trim().is_empty())
                .unwrap_or(false)
            && config.proxy_port.unwrap_or(0) > 0
    };
    TelegramDebugInfo {
        mode: inner.mode.clone(),
        authorized: inner.authorized,
        auth_state: inner.auth_state.clone(),
        auth_hint: inner.auth_hint.clone(),
        config_exists: config_path.exists(),
        config_path: config_path.to_string_lossy().to_string(),
        tdjson_exists: !tdjson_path.is_empty() && std::path::Path::new(&tdjson_path).exists(),
        tdjson_path,
        api_id_configured: !config.api_id.trim().is_empty(),
        api_hash_configured: !config.api_hash.trim().is_empty(),
        proxy_configured,
        tdlib_data_dir: tdlib_account_dir(&inner.active_account_id)
            .to_string_lossy()
            .to_string(),
    }
}

pub fn restart(runtime: &TelegramRuntime) -> TelegramStatus {
    shutdown_client(&runtime.inner);
    let mut inner = runtime.inner.lock().unwrap();
    inner.last_td_load_failure = None;
    inner.mode = "mock".into();
    inner.authorized = false;
    inner.auth_state = "saved_config".into();
    inner.auth_hint = "TDLib runtime restarted. Request status to start it again.".into();
    unidtk_api::log("telegram: runtime restarted");
    inner.status()
}

pub fn save_config(
    runtime: &TelegramRuntime,
    mut config: TelegramConfig,
) -> Result<TelegramStatus, String> {
    let path = telegram_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let existing = load_config();
    if config.api_id.trim().is_empty() {
        config.api_id = existing.api_id;
    }
    if config.api_hash.trim().is_empty() {
        config.api_hash = existing.api_hash;
    }
    if config.proxy_password.as_deref().unwrap_or("").is_empty() {
        config.proxy_password = existing.proxy_password;
    }
    if config.accounts.is_empty() {
        config.accounts = existing.accounts;
    }
    if config.active_account_id.is_none() {
        config.active_account_id = existing.active_account_id;
    }
    config = with_config_defaults(config);
    let text = serde_json::to_string_pretty(&config).map_err(|error| error.to_string())?;
    std::fs::write(path, text).map_err(|error| error.to_string())?;

    shutdown_client(&runtime.inner);
    let mut inner = runtime.inner.lock().unwrap();
    // Allow ensure_started to retry TdJson::load immediately after a path/API change.
    inner.last_td_load_failure = None;
    inner.mode = "mock".into();
    inner.authorized = false;
    inner.auth_state = "saved_config".into();
    inner.auth_hint = "Telegram settings saved. Restarting TDLib on next request.".into();
    Ok(inner.status())
}

pub fn list_accounts(runtime: &TelegramRuntime) -> Vec<TelegramAccount> {
    let mut config = load_config();
    let active_id = runtime.inner.lock().unwrap().active_account_id.clone();
    for acc in &mut config.accounts {
        acc.is_active = acc.id == active_id;
    }
    config.accounts
}

pub fn switch_account(
    runtime: &TelegramRuntime,
    account_id: String,
) -> Result<TelegramStatus, String> {
    shutdown_client(&runtime.inner);
    let mut config = load_config();
    if !config.accounts.iter().any(|a| a.id == account_id) {
        return Err(format!("Account '{account_id}' not found"));
    }
    config.active_account_id = Some(account_id.clone());
    for acc in &mut config.accounts {
        acc.is_active = acc.id == account_id;
    }
    save_raw_config(&config)?;

    let app_handle = {
        let mut inner = runtime.inner.lock().unwrap();
        inner.active_account_id = account_id;
        inner.my_user_id = None;
        inner.last_td_load_failure = None;
        inner.chats.clear();
        inner.archived_chats.clear();
        inner.messages.clear();
        inner.users.clear();
        inner.user_statuses.clear();
        inner.chat_titles_by_id.clear();
        inner.downloading_files.clear();
        inner.attachment_file_owners.clear();
        inner.chats_exhausted = false;
        inner.archive_chats_exhausted = false;
        inner.open_chat_id = None;
        inner.mode = "mock".into();
        inner.authorized = false;
        inner.auth_state = "starting".into();
        inner.auth_hint = "Switching account...".into();
        inner.app_handle.clone()
    };

    unidtk_api::log("telegram: switched account");
    let _ = ensure_started(&runtime.inner);
    let status = runtime.inner.lock().unwrap().status();
    if let Some(app) = app_handle {
        let _ = tauri::Emitter::emit(&app, "telegram-status-update", &status);
    }
    Ok(status)
}

pub fn add_account(runtime: &TelegramRuntime) -> Result<TelegramStatus, String> {
    shutdown_client(&runtime.inner);
    let mut config = load_config();
    let count = config.accounts.len() + 1;
    let new_id = format!(
        "acc_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let new_account = TelegramAccount {
        id: new_id.clone(),
        name: format!("Account {count}"),
        phone: None,
        username: None,
        avatar_path: None,
        is_active: true,
    };
    for acc in &mut config.accounts {
        acc.is_active = false;
    }
    config.accounts.push(new_account);
    config.active_account_id = Some(new_id.clone());
    save_raw_config(&config)?;

    let app_handle = {
        let mut inner = runtime.inner.lock().unwrap();
        inner.active_account_id = new_id;
        inner.my_user_id = None;
        inner.last_td_load_failure = None;
        inner.chats.clear();
        inner.archived_chats.clear();
        inner.messages.clear();
        inner.users.clear();
        inner.user_statuses.clear();
        inner.chat_titles_by_id.clear();
        inner.downloading_files.clear();
        inner.attachment_file_owners.clear();
        inner.chats_exhausted = false;
        inner.archive_chats_exhausted = false;
        inner.open_chat_id = None;
        inner.mode = "mock".into();
        inner.authorized = false;
        inner.auth_state = "starting".into();
        inner.auth_hint = "Setting up new account...".into();
        inner.app_handle.clone()
    };

    unidtk_api::log("telegram: added new account");
    let _ = ensure_started(&runtime.inner);
    let status = runtime.inner.lock().unwrap().status();
    if let Some(app) = app_handle {
        let _ = tauri::Emitter::emit(&app, "telegram-status-update", &status);
    }
    Ok(status)
}

pub fn remove_account(
    runtime: &TelegramRuntime,
    account_id: String,
) -> Result<Vec<TelegramAccount>, String> {
    let mut config = load_config();
    if config.accounts.len() <= 1 {
        return Err("Cannot remove the only remaining account".into());
    }
    let was_active = config.active_account_id.as_deref() == Some(&account_id);
    config.accounts.retain(|a| a.id != account_id);

    let data_dir = tdlib_account_dir(&account_id);
    if account_id != "default" && data_dir.exists() {
        let _ = std::fs::remove_dir_all(&data_dir);
    }

    if was_active {
        let next_id = config
            .accounts
            .first()
            .map(|a| a.id.clone())
            .unwrap_or_else(|| "default".into());
        config.active_account_id = Some(next_id.clone());
        save_raw_config(&config)?;
        let _ = switch_account(runtime, next_id);
    } else {
        save_raw_config(&config)?;
    }

    Ok(list_accounts(runtime))
}

#[cfg(windows)]
type TdCreate = unsafe extern "C" fn() -> *mut c_void;
#[cfg(windows)]
type TdSend = unsafe extern "C" fn(*mut c_void, *const i8);
#[cfg(windows)]
type TdReceive = unsafe extern "C" fn(*mut c_void, f64) -> *const i8;
#[cfg(windows)]
type TdDestroy = unsafe extern "C" fn(*mut c_void);

#[cfg(windows)]
struct TdJson {
    client: *mut c_void,
    send_fn: TdSend,
    receive_fn: TdReceive,
    destroy_fn: TdDestroy,
    _library: windows::Win32::Foundation::HMODULE,
}

#[cfg(not(windows))]
struct TdJson;

#[cfg(windows)]
unsafe impl Send for TdJson {}
unsafe impl Sync for TdJson {}

#[cfg(windows)]
impl TdJson {
    fn load(path: Option<String>) -> Result<Self, String> {
        let path_str = path.unwrap_or_else(|| "tdjson.dll".into());
        let mut path_w: Vec<u16> = path_str.encode_utf16().collect();
        path_w.push(0);
        // Tauri may alter the DLL search path.  TDLib can have native
        // dependencies, so load it with its own directory as the dependency
        // search root instead of relying on the application's current folder.
        let library = unsafe {
            LoadLibraryExW(
                PCWSTR(path_w.as_ptr()),
                None,
                LOAD_WITH_ALTERED_SEARCH_PATH,
            )
        }
        .map_err(|error| format!("failed to load tdjson.dll from {path_str}: {error}"))?;

        let create_fn: TdCreate = load_symbol(library, b"td_json_client_create\0")?;
        let send_fn: TdSend = load_symbol(library, b"td_json_client_send\0")?;
        let receive_fn: TdReceive = load_symbol(library, b"td_json_client_receive\0")?;
        let destroy_fn: TdDestroy = load_symbol(library, b"td_json_client_destroy\0")?;
        let client = unsafe { create_fn() };
        if client.is_null() {
            return Err("td_json_client_create returned null".into());
        }

        Ok(Self {
            client,
            send_fn,
            receive_fn,
            destroy_fn,
            _library: library,
        })
    }

    fn send(&self, request: &str) -> Result<(), String> {
        let request =
            CString::new(request).map_err(|_| "TDLib request contains NUL".to_string())?;
        unsafe { (self.send_fn)(self.client, request.as_ptr()) };
        Ok(())
    }

    fn receive(&self, timeout: f64) -> Result<Option<String>, String> {
        let ptr = unsafe { (self.receive_fn)(self.client, timeout) };
        if ptr.is_null() {
            return Ok(None);
        }
        let text = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .map_err(|error| format!("TDLib returned invalid UTF-8: {error}"))?
            .to_string();
        Ok(Some(text))
    }
}

#[cfg(windows)]
impl Drop for TdJson {
    fn drop(&mut self) {
        unsafe {
            (self.destroy_fn)(self.client);
            let _ = FreeLibrary(self._library);
        }
    }
}

#[cfg(windows)]
fn load_symbol<T>(library: windows::Win32::Foundation::HMODULE, name: &[u8]) -> Result<T, String>
where
    T: Copy,
{
    let proc = unsafe { GetProcAddress(library, PCSTR(name.as_ptr())) }
        .ok_or_else(|| format!("tdjson symbol not found: {}", String::from_utf8_lossy(name)))?;
    Ok(unsafe { std::mem::transmute_copy(&proc) })
}

#[cfg(not(windows))]
impl TdJson {
    fn load(_path: Option<String>) -> Result<Self, String> {
        Err("TDLib dynamic loading is implemented for Windows builds only".into())
    }
}

fn parse_chat(chat: &Value) -> TelegramChat {
    let id = chat.get("id").map(td_id_to_string).unwrap_or_default();
    let title = chat
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Telegram")
        .to_string();
    let unread = chat
        .get("unread_count")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let chat_type = chat.get("type");
    let kind = parse_chat_kind(chat_type);
    let favorite = chat
        .get("positions")
        .and_then(Value::as_array)
        .map(|positions| {
            positions.iter().any(|pos| {
                is_main_position(pos)
                    && pos
                        .get("is_pinned")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    let order = chat_order(chat);
    let last = chat.get("last_message");
    let muted = chat
        .get("notification_settings")
        .map(is_muted)
        .unwrap_or(false);
    let (photo_file_id, photo_path) = chat
        .get("photo")
        .and_then(|photo| photo.get("small"))
        .map(file_info)
        .unwrap_or((None, None));
    let last_message_id = last
        .and_then(|message| message.get("id"))
        .map(td_id_to_string)
        .filter(|value| !value.is_empty());
    let pinned_message_id = chat
        .get("pinned_message_id")
        .map(td_id_to_string)
        .filter(|id| !id.is_empty() && id != "0");

    let (user_id, is_forum, member_count) = match chat_type.and_then(|t| t.get("@type")).and_then(Value::as_str) {
        Some("chatTypePrivate") | Some("chatTypeSecret") => {
            let uid = chat_type.and_then(|t| t.get("user_id")).map(td_id_to_string);
            (uid, Some(false), None)
        }
        Some("chatTypeSupergroup") => {
            let is_forum = chat
                .get("view_as_topics")
                .and_then(Value::as_bool)
                .or_else(|| chat_type.and_then(|t| t.get("is_forum")).and_then(Value::as_bool))
                .or_else(|| chat.get("is_forum").and_then(Value::as_bool))
                .unwrap_or(false);
            let member_count = chat.get("member_count").and_then(Value::as_i64).map(|n| n as i32);
            (None, Some(is_forum), member_count)
        }
        Some("chatTypeBasicGroup") => {
            let member_count = chat.get("member_count").and_then(Value::as_i64).map(|n| n as i32);
            (None, Some(false), member_count)
        }
        _ => (None, Some(false), None),
    };

    let linked_community_id = chat
        .get("linked_community_id")
        .map(td_id_to_string)
        .or_else(|| chat_type.and_then(|t| t.get("linked_community_id")).map(td_id_to_string));
    let has_subchats = if title.contains("Элиор") && kind == "channel" {
        Some(true)
    } else {
        None
    };

    TelegramChat {
        id,
        title: title.clone(),
        initials: initials(&title),
        favorite,
        order,
        unread,
        kind,
        preview: last.map(preview_from_message).unwrap_or_default(),
        time: last.map(time_from_message).unwrap_or_default(),
        muted,
        photo_file_id,
        photo_path,
        last_message_id,
        user_id,
        status: None,
        is_forum,
        member_count,
        linked_community_id,
        has_subchats,
        pinned_message_id,
    }
}

fn parse_user_status(status: &Value) -> String {
    match status.get("@type").and_then(Value::as_str) {
        Some("userStatusOnline") => "online".into(),
        Some("userStatusRecently") => "recently".into(),
        Some("userStatusLastWeek") => "last_week".into(),
        Some("userStatusLastMonth") => "last_month".into(),
        Some("userStatusOffline") => {
            let was_online = status.get("was_online").and_then(Value::as_i64).unwrap_or(0);
            format!("was_online:{was_online}")
        }
        _ => "recently".into(),
    }
}

fn chat_order(chat: &Value) -> i64 {
    chat.get("positions")
        .and_then(Value::as_array)
        .and_then(|positions| {
            positions
                .iter()
                .filter(|position| is_main_position(position))
                .filter_map(|position| json_i64(position.get("order")))
                .max()
        })
        .unwrap_or(0)
}

fn is_main_position(position: &Value) -> bool {
    position
        .get("list")
        .and_then(|list| list.get("@type"))
        .and_then(Value::as_str)
        == Some("chatListMain")
        && json_i64(position.get("order")).unwrap_or(0) != 0
}

fn is_archive_position(position: &Value) -> bool {
    position
        .get("list")
        .and_then(|list| list.get("@type"))
        .and_then(Value::as_str)
        == Some("chatListArchive")
        && json_i64(position.get("order")).unwrap_or(0) != 0
}

fn is_removed_from_main(position: &Value) -> bool {
    position
        .get("list")
        .and_then(|list| list.get("@type"))
        .and_then(Value::as_str)
        == Some("chatListMain")
        && json_i64(position.get("order")).unwrap_or(0) == 0
}

fn json_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
    })
}

fn parse_chat_kind(chat_type: Option<&Value>) -> String {
    let Some(chat_type) = chat_type else {
        return "private".into();
    };
    match chat_type.get("@type").and_then(Value::as_str).unwrap_or("") {
        "chatTypePrivate" => "private".into(),
        "chatTypeBasicGroup" => "group".into(),
        "chatTypeSupergroup" => {
            let is_channel = chat_type
                .get("is_channel")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if is_channel {
                "channel".into()
            } else {
                "group".into()
            }
        }
        "chatTypeSecret" => "secret".into(),
        other => other.trim_start_matches("chatType").to_lowercase(),
    }
}

fn is_muted(settings: &Value) -> bool {
    settings
        .get("mute_for")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        > 0
}

// Message body shown inside the thread. Unlike `preview_from_message` (used
// for one-line chat previews), media messages must NOT fall back to a type
// label ("Photo", "Video") — the attachment itself is rendered separately, so
// the text is the caption or nothing.
fn message_text(message: &Value) -> String {
    let Some(content) = message.get("content") else {
        return String::new();
    };
    match content.get("@type").and_then(Value::as_str).unwrap_or("") {
        "messagePhoto" | "messageVideo" | "messageAnimation" | "messageDocument"
        | "messageAudio" | "messageVoiceNote" | "messageVideoNote" | "messageSticker" => content
            .get("caption")
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => preview_from_message(message),
    }
}

/// Union two message lists by id (later wins for the same id). Order is
/// oldest→newest by numeric TDLib message id. Empty-id local echoes are kept
/// at the end. Matches the Vec layout used by `parse_messages` (reversed TDLib
/// newest-first batches) and `updateNewMessage` / `store_sent_message` pushes.
fn merge_messages(
    existing: Vec<TelegramMessage>,
    incoming: Vec<TelegramMessage>,
) -> Vec<TelegramMessage> {
    let mut by_id: HashMap<String, TelegramMessage> = HashMap::new();
    let mut empty_id: Vec<TelegramMessage> = Vec::new();
    for message in existing.into_iter().chain(incoming) {
        if message.id.is_empty() {
            empty_id.push(message);
            continue;
        }
        by_id.insert(message.id.clone(), message);
    }
    let mut ids: Vec<String> = by_id.keys().cloned().collect();
    ids.sort_by_key(|id| id.parse::<i64>().unwrap_or(0));
    let mut out: Vec<TelegramMessage> = ids
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect();
    out.extend(empty_id);
    out
}

/// Drop the oldest entries so the Vec keeps at most `max` chronologically
/// newest messages (layout is oldest→newest).
fn trim_messages_to_cap(list: &mut Vec<TelegramMessage>, max: usize) {
    if list.len() > max {
        let drop_count = list.len() - max;
        list.drain(0..drop_count);
    }
}

fn parse_message_with(
    message: &Value,
    from: String,
    known_custom_emojis: &HashMap<String, String>,
) -> (TelegramMessage, Vec<String>) {
    let attachment_info = parse_attachment(message);
    let reply_to = message.get("reply_to");
    let reply_to_message_id = reply_to
        .and_then(|r| r.get("message_id"))
        .or_else(|| message.get("reply_to_message_id"))
        .map(td_id_to_string)
        .filter(|id| !id.is_empty() && id != "0");
    let reply_to_text = reply_to
        .and_then(|r| r.get("quote"))
        .and_then(|q| q.get("text"))
        .and_then(|t| t.get("text"))
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .or_else(|| {
            reply_to
                .and_then(|r| r.get("content"))
                .map(message_text)
                .filter(|s| !s.is_empty())
        });
    let reply_to_sender = reply_to
        .and_then(|r| r.get("origin"))
        .and_then(|o| match o.get("@type").and_then(Value::as_str)? {
            "messageOriginHiddenUser" => o.get("sender_user_name").and_then(Value::as_str).map(|s| s.to_string()),
            _ => None,
        });
    let reply_markup = message.get("reply_markup").cloned();
    let message_thread_id = thread_id_from_message(message);
    let author_signature = message
        .get("author_signature")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|signature| !signature.is_empty())
        .map(str::to_string);
    let media_album_id = message
        .get("media_album_id")
        .map(td_id_to_string)
        .filter(|id| !id.is_empty() && id != "0");
    let is_pinned = message.get("is_pinned").and_then(Value::as_bool).filter(|&p| p);

    let (reactions, reaction_items, missing_custom_ids) = parse_reactions_full(
        message.get("interaction_info"),
        known_custom_emojis,
    );

    let msg = TelegramMessage {
        id: message.get("id").map(td_id_to_string).unwrap_or_default(),
        from,
        author_signature,
        text: message_text(message),
        entities: parse_entities(message),
        time: time_from_message(message),
        reactions,
        reaction_items,
        attachment: attachment_info.as_ref().map(|info| info.label.clone()),
        attachment_kind: attachment_info.as_ref().map(|info| info.kind.clone()),
        attachment_file_id: attachment_info.as_ref().and_then(|info| info.file_id),
        attachment_local_path: attachment_info.as_ref().and_then(|info| info.local_path.clone()),
        attachment_width: attachment_info.as_ref().and_then(|info| info.width),
        attachment_height: attachment_info.as_ref().and_then(|info| info.height),
        attachment_duration: attachment_info.as_ref().and_then(|info| info.duration),
        attachment_size: attachment_info.as_ref().and_then(|info| info.size),
        attachment_performer: attachment_info.as_ref().and_then(|info| info.performer.clone()),
        attachment_title: attachment_info.as_ref().and_then(|info| info.title.clone()),
        attachment_waveform: attachment_info.as_ref().and_then(|info| info.waveform.clone()),
        thumbnail_file_id: attachment_info.as_ref().and_then(|info| info.thumbnail_file_id),
        thumbnail_local_path: attachment_info.as_ref().and_then(|info| info.thumbnail_local_path.clone()),
        sticker: parse_sticker(message),
        reply_to_message_id,
        reply_to_sender,
        reply_to_text,
        reply_markup,
        message_thread_id,
        media_album_id,
        is_pinned,
    };

    (msg, missing_custom_ids)
}

/// Handmade local echo when sendMessage does not return a parseable message.
fn msg(
    from: &str,
    text: &str,
    time: &str,
    attachment: Option<&str>,
    attachment_kind: Option<&str>,
) -> TelegramMessage {
    TelegramMessage {
        id: String::new(),
        from: from.into(),
        author_signature: None,
        text: text.into(),
        entities: None,
        time: time.into(),
        reactions: None,
        reaction_items: None,
        attachment: attachment.map(str::to_string),
        attachment_kind: attachment_kind.map(str::to_string),
        attachment_file_id: None,
        attachment_local_path: None,
        attachment_width: None,
        attachment_height: None,
        attachment_duration: None,
        attachment_size: None,
        attachment_performer: None,
        attachment_title: None,
        attachment_waveform: None,
        thumbnail_file_id: None,
        thumbnail_local_path: None,
        sticker: None,
        reply_to_message_id: None,
        reply_to_sender: None,
        reply_to_text: None,
        reply_markup: None,
        message_thread_id: None,
        media_album_id: None,
        is_pinned: None,
    }
}

fn parse_entities(message: &Value) -> Option<Vec<TelegramEntity>> {
    let content = message.get("content")?;
    let text_obj = match content.get("@type").and_then(Value::as_str)? {
        "messageText" => content.get("text"),
        "messagePhoto" | "messageVideo" | "messageAnimation" | "messageDocument" | "messageAudio" => {
            content.get("caption")
        }
        _ => None,
    }?;
    let entities = text_obj.get("entities")?.as_array()?;
    let parsed: Vec<TelegramEntity> = entities.iter().filter_map(parse_entity).collect();
    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

fn parse_entity(entity: &Value) -> Option<TelegramEntity> {
    let offset = entity.get("offset").and_then(Value::as_i64)? as i32;
    let length = entity.get("length").and_then(Value::as_i64)? as i32;
    let entity_type = entity.get("type")?;
    let raw_kind = entity_type
        .get("@type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_start_matches("textEntityType");
    let mut chars = raw_kind.chars();
    let kind = match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => return None,
    };
    let url = entity_type.get("url").and_then(Value::as_str).map(str::to_string);
    let language = entity_type
        .get("language")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(TelegramEntity {
        kind,
        offset,
        length,
        url,
        language,
    })
}

struct AttachmentInfo {
    label: String,
    kind: String,
    file_id: Option<i32>,
    local_path: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
    duration: Option<i32>,
    size: Option<i64>,
    performer: Option<String>,
    title: Option<String>,
    waveform: Option<String>,
    thumbnail_file_id: Option<i32>,
    thumbnail_local_path: Option<String>,
}

impl AttachmentInfo {
    fn plain(label: &str, kind: &str) -> Self {
        Self {
            label: label.into(),
            kind: kind.into(),
            file_id: None,
            local_path: None,
            width: None,
            height: None,
            duration: None,
            size: None,
            performer: None,
            title: None,
            waveform: None,
            thumbnail_file_id: None,
            thumbnail_local_path: None,
        }
    }
}

fn value_dims(value: Option<&Value>) -> (Option<i32>, Option<i32>) {
    let Some(value) = value else {
        return (None, None);
    };
    (
        value.get("width").and_then(Value::as_i64).map(|v| v as i32),
        value.get("height").and_then(Value::as_i64).map(|v| v as i32),
    )
}

fn file_info(file: &Value) -> (Option<i32>, Option<String>) {
    let id = file.get("id").and_then(Value::as_i64).map(|v| v as i32);
    let local_path = file.get("local").and_then(|local| {
        let completed = local
            .get("is_downloading_completed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !completed {
            return None;
        }
        local
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|path| !path.is_empty())
    });
    (id, local_path)
}

fn parse_sender(message: &Value) -> String {
    if message
        .get("is_outgoing")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return "me".into();
    }
    message
        .get("sender_id")
        .and_then(|sender| sender.get("@type"))
        .and_then(Value::as_str)
        .unwrap_or("sender")
        .trim_start_matches("messageSender")
        .to_lowercase()
}

fn preview_from_message(message: &Value) -> String {
    let Some(content) = message.get("content") else {
        return String::new();
    };
    match content.get("@type").and_then(Value::as_str).unwrap_or("") {
        "messageText" => content
            .get("text")
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "messagePhoto" => content
            .get("caption")
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .unwrap_or("Photo")
            .to_string(),
        "messageDocument" => content
            .get("document")
            .and_then(|value| value.get("file_name"))
            .and_then(Value::as_str)
            .unwrap_or("Document")
            .to_string(),
        "messageVideo" => caption_or_label(content, "Video"),
        "messageAnimation" => caption_or_label(content, "GIF"),
        "messageAudio" => content
            .get("audio")
            .and_then(|value| value.get("file_name"))
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or("Audio")
            .to_string(),
        "messageVoiceNote" => "Voice message".into(),
        "messageVideoNote" => "Video message".into(),
        "messageContact" => "Contact".into(),
        "messageLocation" => "Location".into(),
        "messagePoll" => content
            .get("poll")
            .and_then(|value| value.get("question"))
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .unwrap_or("Poll")
            .to_string(),
        "messageSticker" => "Sticker".into(),
        other => other.trim_start_matches("message").to_string(),
    }
}

fn caption_or_label(content: &Value, label: &str) -> String {
    content
        .get("caption")
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .unwrap_or(label)
        .to_string()
}

fn parse_attachment(message: &Value) -> Option<AttachmentInfo> {
    let content = message.get("content")?;
    match content.get("@type").and_then(Value::as_str)? {
        "messagePhoto" => {
            // Prefer smallest size with width >= 480 for preview; fall back to largest.
            // TDLib does not guarantee sizes are ordered by width — sort first.
            let best = content
                .get("photo")
                .and_then(|photo| photo.get("sizes"))
                .and_then(Value::as_array)
                .and_then(|sizes| {
                    let mut ordered: Vec<&Value> = sizes.iter().collect();
                    ordered.sort_by_key(|size| {
                        size.get("width").and_then(Value::as_i64).unwrap_or(0)
                    });
                    ordered
                        .iter()
                        .find(|size| {
                            size.get("width")
                                .and_then(Value::as_i64)
                                .map(|w| w >= 480)
                                .unwrap_or(false)
                        })
                        .copied()
                        .or_else(|| ordered.last().copied())
                });
            let (file_id, local_path) = best
                .and_then(|size| size.get("photo"))
                .map(file_info)
                .unwrap_or((None, None));
            let (width, height) = value_dims(best);
            let size = best.and_then(|size| size.get("photo")).and_then(|f| f.get("size").and_then(Value::as_i64));
            Some(AttachmentInfo {
                label: "Photo".into(),
                kind: "photo".into(),
                file_id,
                local_path,
                width,
                height,
                duration: None,
                size,
                performer: None,
                title: None,
                waveform: None,
                thumbnail_file_id: None,
                thumbnail_local_path: None,
            })
        }
        "messageAnimation" => {
            let animation = content.get("animation");
            let file = animation.and_then(|a| a.get("animation"));
            let (file_id, local_path) = file.map(file_info).unwrap_or((None, None));
            let (width, height) = value_dims(animation);
            let duration = animation.and_then(|a| a.get("duration")).and_then(Value::as_i64).map(|v| v as i32);
            let size = file.and_then(|f| f.get("size").and_then(Value::as_i64));
            let (thumbnail_file_id, thumbnail_local_path) = animation
                .and_then(|a| a.get("thumbnail"))
                .and_then(|t| t.get("file"))
                .map(file_info)
                .unwrap_or((None, None));
            Some(AttachmentInfo {
                label: "GIF".into(),
                kind: "animation".into(),
                file_id,
                local_path,
                width,
                height,
                duration,
                size,
                performer: None,
                title: None,
                waveform: None,
                thumbnail_file_id,
                thumbnail_local_path,
            })
        }
        "messageDocument" => {
            let doc = content.get("document");
            let file = doc.and_then(|d| d.get("document"));
            let (file_id, local_path) = file.map(file_info).unwrap_or((None, None));
            let label = doc
                .and_then(|d| d.get("file_name"))
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or("Document")
                .to_string();
            let size = file.and_then(|f| f.get("size").and_then(Value::as_i64));
            let (thumbnail_file_id, thumbnail_local_path) = doc
                .and_then(|d| d.get("thumbnail"))
                .and_then(|t| t.get("file"))
                .map(file_info)
                .unwrap_or((None, None));
            Some(AttachmentInfo {
                label,
                kind: "document".into(),
                file_id,
                local_path,
                width: None,
                height: None,
                duration: None,
                size,
                performer: None,
                title: None,
                waveform: None,
                thumbnail_file_id,
                thumbnail_local_path,
            })
        }
        "messageVideo" => {
            let video = content.get("video");
            let file = video.and_then(|v| v.get("video"));
            let (file_id, local_path) = file.map(file_info).unwrap_or((None, None));
            let (width, height) = value_dims(video);
            let duration = video.and_then(|v| v.get("duration")).and_then(Value::as_i64).map(|v| v as i32);
            let size = file.and_then(|f| f.get("size").and_then(Value::as_i64));
            let (thumbnail_file_id, thumbnail_local_path) = video
                .and_then(|v| v.get("thumbnail"))
                .and_then(|t| t.get("file"))
                .map(file_info)
                .unwrap_or((None, None));
            let label = video
                .and_then(|v| v.get("file_name"))
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or("Video")
                .to_string();
            Some(AttachmentInfo {
                label,
                kind: "video".into(),
                file_id,
                local_path,
                width,
                height,
                duration,
                size,
                performer: None,
                title: None,
                waveform: None,
                thumbnail_file_id,
                thumbnail_local_path,
            })
        }
        "messageAudio" => {
            let audio = content.get("audio");
            let file = audio.and_then(|a| a.get("audio"));
            let (file_id, local_path) = file.map(file_info).unwrap_or((None, None));
            let duration = audio.and_then(|a| a.get("duration")).and_then(Value::as_i64).map(|v| v as i32);
            let performer = audio.and_then(|a| a.get("performer")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
            let title = audio.and_then(|a| a.get("title")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
            let size = file.and_then(|f| f.get("size").and_then(Value::as_i64));
            let (thumbnail_file_id, thumbnail_local_path) = audio
                .and_then(|a| a.get("album_cover_thumbnail"))
                .and_then(|t| t.get("file"))
                .map(file_info)
                .unwrap_or((None, None));
            let label = title.clone().or_else(|| audio.and_then(|a| a.get("file_name")).and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| "Audio".into());
            Some(AttachmentInfo {
                label,
                kind: "audio".into(),
                file_id,
                local_path,
                width: None,
                height: None,
                duration,
                size,
                performer,
                title,
                waveform: None,
                thumbnail_file_id,
                thumbnail_local_path,
            })
        }
        "messageVoiceNote" => {
            let voice = content.get("voice_note");
            let file = voice.and_then(|v| v.get("voice"));
            let (file_id, local_path) = file.map(file_info).unwrap_or((None, None));
            let duration = voice.and_then(|v| v.get("duration")).and_then(Value::as_i64).map(|v| v as i32);
            let waveform = voice.and_then(|v| v.get("waveform")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
            let size = file.and_then(|f| f.get("size").and_then(Value::as_i64));
            Some(AttachmentInfo {
                label: "Voice message".into(),
                kind: "voice".into(),
                file_id,
                local_path,
                width: None,
                height: None,
                duration,
                size,
                performer: None,
                title: None,
                waveform,
                thumbnail_file_id: None,
                thumbnail_local_path: None,
            })
        }
        "messageVideoNote" => {
            let note = content.get("video_note");
            let file = note.and_then(|v| v.get("video"));
            let (file_id, local_path) = file.map(file_info).unwrap_or((None, None));
            let length = note.and_then(|v| v.get("length")).and_then(Value::as_i64).map(|v| v as i32);
            let duration = note.and_then(|v| v.get("duration")).and_then(Value::as_i64).map(|v| v as i32);
            let size = file.and_then(|f| f.get("size").and_then(Value::as_i64));
            let (thumbnail_file_id, thumbnail_local_path) = note
                .and_then(|v| v.get("thumbnail"))
                .and_then(|t| t.get("file"))
                .map(file_info)
                .unwrap_or((None, None));
            Some(AttachmentInfo {
                label: "Video message".into(),
                kind: "videoNote".into(),
                file_id,
                local_path,
                width: length,
                height: length,
                duration,
                size,
                performer: None,
                title: None,
                waveform: None,
                thumbnail_file_id,
                thumbnail_local_path,
            })
        }
        "messageContact" => Some(AttachmentInfo::plain("Contact", "contact")),
        "messageLocation" => Some(AttachmentInfo::plain("Location", "location")),
        "messagePoll" => Some(AttachmentInfo::plain("Poll", "poll")),
        _ => None,
    }
}

fn parse_sticker(message: &Value) -> Option<String> {
    let content = message.get("content")?;
    if content.get("@type").and_then(Value::as_str) != Some("messageSticker") {
        return None;
    }
    content
        .get("sticker")
        .and_then(|value| value.get("emoji"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn parse_reactions_full(
    interaction_info: Option<&Value>,
    known_custom_emojis: &HashMap<String, String>,
) -> (Option<String>, Option<Vec<TelegramReactionItem>>, Vec<String>) {
    let Some(interaction_info) = interaction_info else {
        return (None, None, Vec::new());
    };
    let Some(reactions_arr) = interaction_info
        .get("reactions")
        .and_then(|r| r.get("reactions"))
        .and_then(Value::as_array)
    else {
        return (None, None, Vec::new());
    };

    let mut legacy_parts = Vec::new();
    let mut items = Vec::new();
    let mut missing_ids = Vec::new();

    for reaction in reactions_arr {
        let count = reaction
            .get("total_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let is_chosen = reaction
            .get("is_chosen")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let Some(type_val) = reaction.get("type") else {
            continue;
        };
        let type_name = type_val.get("@type").and_then(Value::as_str).unwrap_or("");

        match type_name {
            "reactionTypeEmoji" => {
                if let Some(emoji_str) = type_val.get("emoji").and_then(Value::as_str) {
                    if count == 0 {
                        legacy_parts.push(emoji_str.to_string());
                    } else {
                        legacy_parts.push(format!("{emoji_str} {count}"));
                    }
                    items.push(TelegramReactionItem {
                        reaction_type: "emoji".into(),
                        emoji: Some(emoji_str.to_string()),
                        custom_emoji_id: None,
                        custom_emoji_path: None,
                        count,
                        is_chosen,
                    });
                }
            }
            "reactionTypeCustomEmoji" => {
                let custom_emoji_id = type_val.get("custom_emoji_id").map(td_id_to_string).unwrap_or_default();
                if !custom_emoji_id.is_empty() && custom_emoji_id != "0" {
                    if count == 0 {
                        legacy_parts.push("custom".to_string());
                    } else {
                        legacy_parts.push(format!("custom {count}"));
                    }
                    let path = known_custom_emojis.get(&custom_emoji_id).cloned();
                    if path.is_none() {
                        missing_ids.push(custom_emoji_id.clone());
                    }
                    items.push(TelegramReactionItem {
                        reaction_type: "custom_emoji".into(),
                        emoji: None,
                        custom_emoji_id: Some(custom_emoji_id),
                        custom_emoji_path: path,
                        count,
                        is_chosen,
                    });
                }
            }
            "reactionTypePaid" => {
                if count == 0 {
                    legacy_parts.push("⭐️".to_string());
                } else {
                    legacy_parts.push(format!("⭐️ {count}"));
                }
                items.push(TelegramReactionItem {
                    reaction_type: "paid".into(),
                    emoji: Some("⭐️".into()),
                    custom_emoji_id: None,
                    custom_emoji_path: None,
                    count,
                    is_chosen,
                });
            }
            _ => {}
        }
    }

    let legacy_str = if legacy_parts.is_empty() {
        None
    } else {
        Some(legacy_parts.join("  "))
    };
    let items_vec = if items.is_empty() {
        None
    } else {
        Some(items)
    };

    (legacy_str, items_vec, missing_ids)
}

fn time_from_message(message: &Value) -> String {
    let Some(date) = message.get("date").and_then(Value::as_i64) else {
        return String::new();
    };
    date.to_string()
}

fn sorted_chats(mut chats: Vec<TelegramChat>) -> Vec<TelegramChat> {
    chats.sort_by(|a, b| {
        b.favorite
            .cmp(&a.favorite)
            .then_with(|| b.order.cmp(&a.order))
            .then_with(|| b.unread.cmp(&a.unread))
            .then_with(|| a.title.cmp(&b.title))
    });
    chats
}

fn initials(title: &str) -> String {
    let mut result = title
        .split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>();
    if result.is_empty() {
        result = "?".into();
    }
    result.to_uppercase()
}

fn parse_chat_id(chat_id: &str) -> i64 {
    chat_id.parse::<i64>().unwrap_or_default()
}

fn td_id_to_string(value: &Value) -> String {
    value
        .as_i64()
        .map(|value| value.to_string())
        .or_else(|| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn tdlib_account_dir(account_id: &str) -> std::path::PathBuf {
    let base = unidtk_api::paths::data_dir().join("tdlib");
    if account_id == "default" || account_id.is_empty() {
        base
    } else {
        base.join("accounts").join(account_id)
    }
}

fn telegram_config_path() -> std::path::PathBuf {
    unidtk_api::paths::data_dir().join("telegram_config.json")
}

fn save_raw_config(config: &TelegramConfig) -> Result<(), String> {
    if let Some(db) = TELEGRAM_DB.get() {
        let bytes = serde_json::to_vec(config).map_err(|e| e.to_string())?;
        db.kv_set("config", &bytes)?;
        return Ok(());
    }
    let path = telegram_config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

fn load_config() -> TelegramConfig {
    if let Some(db) = TELEGRAM_DB.get() {
        if let Ok(Some(bytes)) = db.kv_get("config") {
            if let Ok(config) = serde_json::from_slice::<TelegramConfig>(&bytes) {
                return with_config_defaults(config);
            }
        }
    }

    let path = telegram_config_path();
    if let Some(config) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
    {
        let res = with_config_defaults(config);
        let _ = std::fs::remove_file(&path);
        let _ = save_raw_config(&res);
        return res;
    }

    let config = with_config_defaults(TelegramConfig::default());
    let _ = save_raw_config(&config);
    config
}

fn load_config_from_db(db: &dyn unidtk_api::ModuleDb) -> TelegramConfig {
    if let Ok(Some(bytes)) = db.kv_get("config") {
        if let Ok(config) = serde_json::from_slice::<TelegramConfig>(&bytes) {
            return with_config_defaults(config);
        }
    }
    with_config_defaults(TelegramConfig::default())
}

fn with_config_defaults(mut config: TelegramConfig) -> TelegramConfig {
    if config.api_id.trim().is_empty() {
        config.api_id = DEFAULT_TELEGRAM_API_ID.to_string();
    }
    if config.api_hash.trim().is_empty() {
        config.api_hash = DEFAULT_TELEGRAM_API_HASH.to_string();
    }
    // Old builds saved a path under `backend\vendor`, while TDLib now lives
    // in `backend\opt` and is bundled next to the executable. A saved path
    // must not prevent the working bundled library from being used.
    let configured_path_is_valid = !config.tdjson_path.trim().is_empty()
        && std::path::Path::new(config.tdjson_path.trim()).is_file();
    if !configured_path_is_valid {
        if let Some(path) = bundled_tdjson_path() {
            config.tdjson_path = path.to_string_lossy().to_string();
        }
    }
    // Default proxy is the managed ByeDPI desync, which auto-yields to the
    // wallpaper VPN when that is connected. An explicit "none"/socks/http
    // choice is preserved; only missing/blank values get the default.
    // Legacy "vless" is migrated forward to "byedpi".
    let proxy = normalize_proxy_type(config.proxy_type.as_deref());
    if proxy == "byedpi" && config.proxy_type.as_deref().map(|p| p.trim().to_lowercase()) != Some("byedpi".into()) {
        config.proxy_type = Some("byedpi".into());
    }
    let active_id = config
        .active_account_id
        .clone()
        .unwrap_or_else(|| "default".to_string());
    if config.accounts.is_empty() {
        config.accounts.push(TelegramAccount {
            id: active_id.clone(),
            name: "Main Account".to_string(),
            phone: None,
            username: None,
            avatar_path: None,
            is_active: true,
        });
        config.active_account_id = Some(active_id);
    }
    config
}

fn bundled_tdjson_path() -> Option<std::path::PathBuf> {
    let current_dir = std::env::current_dir().ok();
    let current_exe = std::env::current_exe().ok();
    let mut candidates = Vec::new();
    if let Some(dir) = current_dir {
        candidates.push(
            dir.join("app-tauri")
                .join("backend")
                .join("opt")
                .join("tdlib-native")
                .join("win32-x64")
                .join("tdjson.dll"),
        );
        candidates.push(
            dir.join("backend")
                .join("opt")
                .join("tdlib-native")
                .join("win32-x64")
                .join("tdjson.dll"),
        );
        candidates.push(
            dir.join("opt")
                .join("tdlib-native")
                .join("win32-x64")
                .join("tdjson.dll"),
        );
    }
    if let Some(exe) = current_exe.and_then(|path| path.parent().map(std::path::Path::to_path_buf))
    {
        candidates.push(exe.join("tdjson.dll"));
        candidates.push(
            exe.join("app-tauri")
                .join("backend")
                .join("opt")
                .join("tdlib-native")
                .join("win32-x64")
                .join("tdjson.dll"),
        );
    }
    candidates.push(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("opt")
            .join("tdlib-native")
            .join("win32-x64")
            .join("tdjson.dll"),
    );
    candidates.into_iter().find(|path| path.exists())
}

/// Модуль Telegram: рантайм создаётся в `init` (там же прокидывается AppHandle
/// и подписка на VPN), в managed-state ядра лежит клон для обёрток команд.
pub struct TelegramModule {
    runtime: std::sync::Mutex<Option<TelegramRuntime>>,
}

impl TelegramModule {
    pub fn new() -> Self {
        Self {
            runtime: std::sync::Mutex::new(None),
        }
    }

    pub fn runtime(&self) -> Option<TelegramRuntime> {
        self.runtime.lock().expect("telegram module lock").clone()
    }
}

pub static TELEGRAM_DEPENDENCIES: &[unidtk_api::manifest::ModuleDependency] = &[
    unidtk_api::manifest::ModuleDependency {
        id: "telegram:tdjson",
        description: "TDLib native JSON FFI library with crypto companions",
        severity: unidtk_api::manifest::DependencySeverity::Mandatory,
        kind: unidtk_api::manifest::DependencyKind::DynamicLibrary {
            file_name: "tdjson.dll",
            companions: &["libcrypto-3-x64.dll", "libssl-3-x64.dll", "z.dll"],
            min_bytes: 20_000_000,
        },
    },
    unidtk_api::manifest::ModuleDependency {
        id: "telegram:byedpi",
        description: "ByeDPI local DPI-desync SOCKS proxy (managed)",
        severity: unidtk_api::manifest::DependencySeverity::Optional,
        kind: unidtk_api::manifest::DependencyKind::Executable {
            file_name: "ciadpi.exe",
            probe_args: Some(&["--help"]),
        },
    },
];

pub fn create_module() -> Box<dyn unidtk_api::Module> {
    Box::new(TelegramModule::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_mode_defaults_to_byedpi_and_migrates_vless() {
        assert_eq!(normalize_proxy_type(None), "byedpi");
        assert_eq!(normalize_proxy_type(Some("")), "byedpi");
        assert_eq!(normalize_proxy_type(Some("vless")), "byedpi");
        assert_eq!(normalize_proxy_type(Some("VLESS")), "byedpi");
        assert_eq!(normalize_proxy_type(Some("none")), "none");
        assert_eq!(normalize_proxy_type(Some("socks5")), "socks5");
        assert_eq!(normalize_proxy_type(Some("http")), "http");

        let legacy = with_config_defaults(TelegramConfig {
            proxy_type: Some("vless".into()),
            ..TelegramConfig::default()
        });
        assert_eq!(legacy.proxy_type.as_deref(), Some("byedpi"));

        let manual = with_config_defaults(TelegramConfig {
            proxy_type: Some("none".into()),
            ..TelegramConfig::default()
        });
        assert_eq!(manual.proxy_type.as_deref(), Some("none"));
    }

    #[test]
    fn add_proxy_request_uses_modern_nested_schema() {
        // Regression guard: the legacy flat shape (`server`/`port`/`type` at the
        // top level) is rejected by TDLib with `400 Proxy must be non-empty`,
        // which silently kept the managed ByeDPI proxy disabled.
        let request = add_proxy_request("127.0.0.1", 12909, socks5_proxy_type("", ""), true);
        assert_eq!(request["@type"], "addProxy");
        assert_eq!(request["enable"], true);
        assert_eq!(request["comment"], "");
        assert_eq!(request["proxy"]["@type"], "proxy");
        assert_eq!(request["proxy"]["server"], "127.0.0.1");
        assert_eq!(request["proxy"]["port"], 12909);
        assert_eq!(request["proxy"]["type"]["@type"], "proxyTypeSocks5");
        assert_eq!(request["proxy"]["type"]["username"], "");
        assert!(request.get("server").is_none(), "flat legacy shape must not return");
        assert!(request.get("port").is_none(), "flat legacy shape must not return");
        assert!(request.get("type").is_none(), "flat legacy shape must not return");

        let http = add_proxy_request("10.0.0.5", 8080, http_proxy_type("u", "p"), false);
        assert_eq!(http["proxy"]["type"]["@type"], "proxyTypeHttp");
        assert_eq!(http["proxy"]["type"]["http_only"], false);
        assert_eq!(http["proxy"]["type"]["username"], "u");
        assert_eq!(http["enable"], false);
    }

    #[test]
    fn connection_states_are_normalized() {
        // TDLib reports PascalCase names after the `connectionState` prefix;
        // comparing them against `online`/`connecting`/`waitingForNetwork` kept
        // the stuck-connection heal dead.
        assert_eq!(normalize_connection_state("Ready"), "ready");
        assert_eq!(
            normalize_connection_state("connectionStateConnecting"),
            "connecting"
        );
        assert_eq!(
            normalize_connection_state("ConnectingToProxy"),
            "connecting_to_proxy"
        );
        assert_eq!(normalize_connection_state("Updating"), "updating");
        assert_eq!(
            normalize_connection_state("WaitingForNetwork"),
            "waiting_for_network"
        );
        assert_eq!(normalize_connection_state("online"), "ready");
        assert_eq!(normalize_connection_state("???"), "unknown");
    }

    #[test]
    fn added_proxy_reply_yields_id_and_endpoint_key() {
        let reply = json!({
            "@type": "addedProxy",
            "id": 2,
            "is_enabled": true,
            "comment": "",
            "proxy": {
                "@type": "proxy",
                "server": "127.0.0.1",
                "port": 12909,
                "type": {"@type": "proxyTypeSocks5", "username": "", "password": ""}
            }
        });
        let (id, key) = parse_added_proxy(&reply).expect("addedProxy must parse");
        assert_eq!(id, 2);
        // Must equal the key built by apply_proxy_config for the byedpi mode,
        // otherwise `enableProxy(id)` would never be chosen over `addProxy`.
        assert_eq!(key, proxy_endpoint_key("socks5", "", "127.0.0.1", MANAGED_PROXY_PORT));
        assert_eq!(key, "socks5://@127.0.0.1:12909");
    }
}

impl unidtk_api::Module for TelegramModule {
    fn id(&self) -> &'static str {
        "telegram"
    }

    fn dependencies(&self) -> &'static [unidtk_api::manifest::ModuleDependency] {
        TELEGRAM_DEPENDENCIES
    }

    fn init(&self, ctx: unidtk_api::ModuleContext) -> Result<(), String> {
        if ctx.is_diagnostic() {
            // В режиме диагностики проверяем конфиг из базы без запуска C++ TDLib
            let _cfg = load_config_from_db(&*ctx.db());
            return Ok(());
        }

        let _ = TELEGRAM_DB.set(ctx.db());
        let handle = ctx
            .host::<tauri::AppHandle>()
            .cloned()
            .ok_or("telegram: AppHandle missing in ModuleContext")?;
        let mut runtime = TelegramRuntime::default();
        runtime.set_app_handle(handle);
        if !ctx.enabled() {
            unidtk_api::log("telegram: disabled — TDLib runtime kept inert, vpn events not subscribed");
        } else {
            subscribe_vpn_events();
        }
        *self.runtime.lock().expect("telegram module lock") = Some(runtime);
        Ok(())
    }

    fn shutdown(&self, _budget: std::time::Duration) -> Result<(), String> {
        begin_shutdown();
        Ok(())
    }
}

