#![windows_subsystem = "windows"]

mod wallpaper;
mod weather_location;
mod network;

// Every module lives behind the `unidtk-api` contract as a real crate: the
// kernel depends on them as Cargo packages instead of including their sources.
// No `#[path]` source includes remain.
use unidtk_mod_explorer as explorer;
use unidtk_mod_glyph_bar as glyph_bar;
use unidtk_mod_mp3_player as music;
use unidtk_mod_micrographics as micrographics;
use unidtk_mod_telegram as telegram;
use unidtk_mod_vpn as vpn;

mod hotkeys;
mod input;
mod touchpad;
mod ipc;
mod library_migration;
mod log_store;
mod logging;
mod module_db;
mod module_store;
mod panic_hook;
pub mod registry;
mod command_inventory;
mod events;
mod ws;
mod unidtk_db;
mod shell_db;
mod shutdown;

use unidtk_mod_mp3_player::commands;
use crate::unidtk_db::UniDtkDatabase;
use crate::log_store::LogStore;

#[global_allocator]
static GLOBAL_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    sync::{mpsc, Arc, Mutex, OnceLock},
    thread,
    time::Duration,
};

use tauri::Emitter;
use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, RunEvent, WindowEvent};

use wallpaper::AttachOptions;

const ATTACH_OPTS: AttachOptions = AttachOptions {
    transparent: false,
    forward_mouse: true,
    // Keyboard is forwarded too (raw-input sink → focused WebView2 child), so
    // text fields in widgets (telegram search/composer, auth forms) accept
    // typing while the desktop is foreground.
    forward_keyboard: true,
};

// Set once in setup; used by input.rs to deliver synthesized wheel events to
// the webview (the reparented WorkerW child never receives real wheel input,
// and Chromium ignores a posted WM_MOUSEWHEEL, so scrolling goes through a
// Tauri event handled in JS instead).
pub(crate) static APP_HANDLE: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

/// Single shared append handle so concurrent `log_line` callers cannot splice
/// partial writes mid-line (receiver thread + command threads).
static LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();

fn append_log_file(message: &str) {
    let slot = LOG_FILE.get_or_init(|| {
        // Журнал живёт в каталоге данных, а не рядом с программой: под Program
        // Files запись рядом с exe невозможна, и весь старт-логинг молча терялся.
        let dir = unidtk_api::paths::logs_dir();
        let opened = if std::fs::create_dir_all(&dir).is_ok() {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("unidtk.log"))
                .ok()
        } else {
            None
        };
        Mutex::new(opened)
    });

    if let Ok(mut guard) = slot.lock() {
        if let Some(file) = guard.as_mut() {
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            let _ = writeln!(file, "[{timestamp}] {message}");
            let _ = file.flush();
        }
    }
}

pub(crate) fn log_line(message: impl AsRef<str>) {
    let message = message.as_ref();
    if !crate::logging::is_ready() {
        // Early startup window (before setup): the file sink is all we have.
        append_log_file(message);
    }
    // After init the flusher owns both sinks (redb batch + unidtk.log).
    crate::logging::log_backend("unidtk", message);
}

/// Single idempotent shutdown path shared by every exit route (tray quit,
/// `quit_app` command, `RunEvent::ExitRequested`, Windows session end).
/// Without the guard a tray quit raced ExitRequested into two VPN stops with
/// two different nonces, so the first waiter timed out on a marker that the
/// second stop had already replaced.
static SHUTDOWN_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Stop every subsystem exactly once. `wait_for_vpn` blocks (bounded) until the
/// elevated helper has actually killed sing-box — pass `true` only from threads
/// where sleeping is acceptable (async command, session-end listener), never
/// from the main event loop.
pub(crate) fn app_shutdown(app: &tauri::AppHandle, wait_for_vpn: bool) {
    use std::sync::atomic::Ordering;
    if SHUTDOWN_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    log_line("shutdown: begin");
    // mp3-player: глобальный латч закрывается безусловно (на случай, если
    // состояние модуля не успело появиться), затем — штатная остановка через
    // контракт: анализ, захват спектра и пауза, без join'ов.
    music::commands::begin_shutdown();
    if let Some(module) = app.try_state::<commands::Mp3PlayerModule>() {
        let _ = unidtk_api::Module::shutdown(&*module, Duration::from_millis(1500));
    }
    if let Some(module) = app.try_state::<glyph_bar::GlyphBarModule>() {
        let _ = unidtk_api::Module::shutdown(&*module, Duration::from_millis(1500));
    }
    // Telegram first: vpn::shutdown() fires on_vpn_state_changed() mid-exit,
    // which used to race the managed proxy back to life; begin_shutdown()
    // latches that off permanently and kills the child process. The latch is a
    // process-global static, so it runs unconditionally even if the module
    // state was never managed.
    telegram::begin_shutdown();
    if let Some(module) = app.try_state::<telegram::TelegramModule>() {
        let _ = unidtk_api::Module::shutdown(&*module, Duration::from_millis(1500));
    }
    if let Some(module) = app.try_state::<vpn::VpnModule>() {
        if wait_for_vpn {
            let _ = module.stop_with_budget(true);
        } else {
            let _ = unidtk_api::Module::shutdown(&*module, Duration::from_millis(1500));
        }
    }
    // Last, and cheap by construction: the module only raises a flag and hands
    // its join to a reaper, so it cannot spend the budget waiting on the network.
    if let Some(module) = app.try_state::<micrographics::MicrographicsModule>() {
        let _ = unidtk_api::Module::shutdown(&*module, Duration::from_millis(1500));
    }
    let _ = wallpaper::reset();
    // Drain queued log lines into unidtk_logs.redb / unidtk.log.
    crate::logging::flush();
    log_line("shutdown: done");
}

/// True once the exit path has started (see `app_shutdown`).
pub(crate) fn is_shutting_down() -> bool {
    SHUTDOWN_DONE.load(std::sync::atomic::Ordering::SeqCst)
}

/// Контекст модуля: собственный срез базы + AppHandle как host-состояние.
/// Если redb не открылась, модуль получает пустую заглушку — модули, которым
/// база не нужна, обязаны подниматься и в деградированном режиме.
fn module_context(app: &tauri::AppHandle, id: &str, enabled: bool) -> unidtk_api::ModuleContext {
    let module_id = unidtk_api::ModuleId::new(id);
    let db: Arc<dyn unidtk_api::ModuleDb> = match shared_db(app) {
        Some(shared) => Arc::new(crate::module_db::RedbModuleDb::new(shared.raw_db(), &module_id)),
        None => Arc::new(NoopModuleDb),
    };
    unidtk_api::ModuleContext::new(module_id, db)
        .with_host(Arc::new(app.clone()))
        .with_store_factory(store_factory())
        .with_enabled(enabled)
}

/// Модули, которые пользователь выключил (из redb `disabled_modules`). Default `[]`.
/// Если redb не открылась — считаем все включёнными (бывшее поведение, без паники).
fn disabled_module_set(app: &tauri::AppHandle) -> std::collections::HashSet<String> {
    match app.try_state::<Arc<UniDtkDatabase>>() {
        Some(state) => state.inner().get_disabled_modules().unwrap_or_default().into_iter().collect(),
        None => std::collections::HashSet::new(),
    }
}

/// Фабрика файловых хранилищ: одна на процесс, потому что redb лочит файл
/// эксклюзивно и на каждый путь должен быть ровно один открытый хендл.
fn store_factory() -> Arc<dyn unidtk_api::StoreFactory> {
    static FACTORY: std::sync::OnceLock<Arc<dyn unidtk_api::StoreFactory>> =
        std::sync::OnceLock::new();
    FACTORY
        .get_or_init(|| Arc::new(crate::module_store::RedbStoreFactory::new()))
        .clone()
}

fn shared_db(app: &tauri::AppHandle) -> Option<Arc<UniDtkDatabase>> {
    app.try_state::<Arc<UniDtkDatabase>>()
        .map(|state| state.inner().clone())
}

/// Срез-заглушка для старта без redb: читать нечего, писать нельзя.
struct NoopModuleDb;

impl unidtk_api::ModuleDb for NoopModuleDb {
    fn kv_get(&self, _key: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }

    fn kv_set(&self, _key: &str, _value: &[u8]) -> Result<(), String> {
        Err("module db unavailable (no redb)".to_string())
    }

    fn kv_remove(&self, _key: &str) -> Result<(), String> {
        Err("module db unavailable (no redb)".to_string())
    }

    fn kv_list(&self, _prefix: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
        Ok(Vec::new())
    }
}

#[tauri::command(async)]
fn reset_wallpaper(app: tauri::AppHandle) -> Result<(), String> {
    run_on_main_thread(&app, |_| wallpaper::reset())
}

#[tauri::command(async)]
fn attach_wallpaper(app: tauri::AppHandle, hwnd: isize) -> Result<(), String> {
    let host = wallpaper::resolve_desktop_host(true)?;
    run_on_main_thread(&app, move |_| wallpaper::attach_to_host(hwnd, ATTACH_OPTS, host))
}

#[tauri::command(async)]
fn detach_wallpaper(app: tauri::AppHandle, hwnd: isize) -> Result<(), String> {
    run_on_main_thread(&app, move |_| wallpaper::detach(hwnd, ATTACH_OPTS))
}

/// Copy text to the system clipboard. The wallpaper window never receives OS
/// focus (WorkerW child), so the webview's JS clipboard API and
/// execCommand("copy") are unreliable; widget copy actions (telegram context
/// menu) go through this command instead.
#[tauri::command(async)]
fn set_clipboard_text(text: String) -> Result<(), String> {
    use windows::Win32::{
        Foundation::{HANDLE, HWND},
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
        },
    };
    unsafe {
        OpenClipboard(HWND::default()).map_err(|e| e.to_string())?;
        let result = (|| {
            EmptyClipboard().map_err(|e| e.to_string())?;
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let hmem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).map_err(|e| e.to_string())?;
            let ptr = GlobalLock(hmem);
            if ptr.is_null() {
                return Err("GlobalLock failed".to_string());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr as *mut u16, wide.len());
            let _ = GlobalUnlock(hmem);
            // On success the clipboard owns `hmem` (no GlobalFree).
            // 13 = CF_UNICODETEXT.
            SetClipboardData(13, HANDLE(hmem.0)).map_err(|e| e.to_string())?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

#[tauri::command(async)]
fn attach_main_window(app: tauri::AppHandle) -> Result<(), String> {
    let host = wallpaper::resolve_desktop_host(true)?;
    run_on_main_thread(&app, move |app| {
        let hwnd = wallpaper_hwnd(&app)?;
        wallpaper::attach_to_host(hwnd, ATTACH_OPTS, host)
    })
}

#[tauri::command(async)]
fn detach_main_window(app: tauri::AppHandle) -> Result<(), String> {
    run_on_main_thread(&app, |app| {
        let hwnd = wallpaper_hwnd(&app)?;
        wallpaper::detach(hwnd, ATTACH_OPTS)
    })
}

#[tauri::command(async)]
fn quit_app(app: tauri::AppHandle) -> Result<(), String> {
    let _ = run_on_main_thread(&app, |app| {
        if let Ok(hwnd) = wallpaper_hwnd(&app) {
            let _ = wallpaper::detach(hwnd, ATTACH_OPTS);
        }
        wallpaper::reset()
    });
    // This command runs on the async runtime, so the bounded VPN wait inside
    // app_shutdown() cannot freeze the UI/event loop.
    app_shutdown(&app, true);
    app.exit(0);
    Ok(())
}

// ─── Module IPC surface ──────────────────────────────────────────────────────
// Tauri commands must be declared where `generate_handler!` lives, so the kernel
// owns the thin wrappers around each module's plain functions.

#[tauri::command(async)]
fn get_system_volume() -> Result<glyph_bar::SystemVolume, String> {
    glyph_bar::get_system_volume()
}

#[tauri::command(async)]
fn set_system_volume(level: u8) -> Result<glyph_bar::SystemVolume, String> {
    glyph_bar::set_system_volume(level)
}

#[tauri::command(async)]
fn set_system_mute(muted: bool) -> Result<glyph_bar::SystemVolume, String> {
    glyph_bar::set_system_mute(muted)
}

/// The UI door's half of `weather_current`.
///
/// Deliberately thin and deliberately *not* the place the weather is fetched:
/// the module's background thread owns that. This only hands the interface the
/// snapshot the module already has, so a repaint can never wait on the network.
#[tauri::command(async)]
fn weather_current(
    state: tauri::State<micrographics::WeatherRuntime>,
) -> Result<micrographics::WeatherSnapshot, String> {
    state
        .snapshot()
        .ok_or_else(|| "weather: no observation yet".to_string())
}

// ═══════════════════════════════════════════════════════════════════════════════

fn hides_on_close(label: &str) -> bool {
    label == "wallpaper"
}

#[tauri::command(async)]
fn weather_location_request(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    weather_location::request(app)
}

#[tauri::command(async)]
fn vpn_list_servers(state: tauri::State<vpn::VpnRuntime>) -> Result<Vec<vpn::VpnServer>, String> {
    vpn::list_servers(&state)
}

#[tauri::command(async)]
fn vpn_refresh_servers(
    state: tauri::State<vpn::VpnRuntime>,
) -> Result<Vec<vpn::VpnServer>, String> {
    vpn::refresh_servers(&state)
}

#[tauri::command(async)]
fn vpn_status(state: tauri::State<vpn::VpnRuntime>) -> vpn::VpnStatus {
    vpn::status(&state)
}

#[tauri::command(async)]
fn vpn_settings() -> vpn::VpnSettingsStatus {
    vpn::settings()
}

#[tauri::command(async)]
fn vpn_set_auto_connect(enabled: bool) -> vpn::VpnSettingsStatus {
    vpn::set_auto_connect(enabled)
}

#[tauri::command(async)]
fn vpn_set_subscription_url(url: String) -> Result<vpn::VpnSettingsStatus, String> {
    vpn::set_subscription_url(url)
}

#[tauri::command(async)]
fn vpn_set_home_server_config(config: vpn::HomeServerConfig) -> Result<vpn::VpnSettingsStatus, String> {
    vpn::set_home_server_config(config)
}

#[tauri::command(async)]
fn frontend_log(message: String) {
    crate::logging::log_frontend(&message);
}

#[tauri::command(async)]
fn log_list(
    limit: Option<usize>,
    level: Option<String>,
    source: Option<String>,
) -> Result<Vec<crate::log_store::LogEntry>, String> {
    crate::logging::list_entries(
        limit.unwrap_or(200),
        level.as_deref(),
        source.as_deref(),
    )
}

#[tauri::command(async)]
fn log_clear() -> Result<u64, String> {
    crate::logging::clear_logs()
}

#[tauri::command(async)]
fn log_stats() -> Result<serde_json::Value, String> {
    crate::logging::stats()
}

/// Explorer tabs persisted in the unified redb (table EXPLORER_TABS) —
/// survives app restarts and is queryable via CLI (`shell invoke explorer_tabs_get`).
#[tauri::command(async)]
fn explorer_tabs_get(state: tauri::State<Arc<UniDtkDatabase>>) -> Result<Vec<(String, String)>, String> {
    state.get_explorer_tabs()
}

#[tauri::command(async)]
fn explorer_tabs_set(
    state: tauri::State<Arc<UniDtkDatabase>>,
    tabs: Vec<(String, String)>,
) -> Result<(), String> {
    state.set_explorer_tabs(&tabs)
}

#[tauri::command(async)]
fn explorer_sort_get(state: tauri::State<Arc<UniDtkDatabase>>) -> Result<(String, bool), String> {
    state.get_explorer_sort()
}

#[tauri::command(async)]
fn explorer_sort_set(
    state: tauri::State<Arc<UniDtkDatabase>>,
    column: String,
    ascending: bool,
) -> Result<(), String> {
    state.set_explorer_sort(&column, ascending)
}

/// Layout seeded into redb on first start; "Сброс позиций" restores the
/// geometry from it. The frontend asks for it instead of keeping its own copy.
const DEFAULT_WIDGET_CONFIG: &str = r#"{
  "order": [],
  "align": "left",
  "positions": {
    "clock": { "x": 510, "y": -10, "w": 210, "h": 190, "version": "rings" },
    "mp3-player": { "x": 500, "y": 260, "w": 210, "h": 220, "version": "circular", "authUser": 1, "autoShuffle": false, "aiDj": true },
    "telegram": { "x": 1090, "y": 30, "w": 70, "h": 590, "expanded": false, "showSettings": false },
    "glyph-bar": { "x": 1170, "y": 30, "w": 56, "h": 587 }
  },
  "instances": [],
  "hidden": ["tools-shortcut", "scripts"]
}"#;

fn default_widget_config() -> serde_json::Value {
    serde_json::from_str(DEFAULT_WIDGET_CONFIG).expect("DEFAULT_WIDGET_CONFIG is valid JSON")
}

#[tauri::command]
fn get_default_widget_config() -> serde_json::Value {
    default_widget_config()
}

#[cfg(test)]
mod default_widget_config_tests {
    #[test]
    fn default_layout_parses_with_core_widgets() {
        let config = super::default_widget_config();
        for id in ["clock", "mp3-player", "telegram", "glyph-bar"] {
            let pos = &config["positions"][id];
            assert!(pos["x"].is_number() && pos["y"].is_number(), "{id} has no position");
        }
    }
}

#[tauri::command(async)]
fn get_widget_config(
    state: tauri::State<Arc<UniDtkDatabase>>,
) -> Result<Option<serde_json::Value>, String> {
    state.get_widget_config()
}

#[tauri::command(async)]
fn save_widget_config(
    state: tauri::State<Arc<UniDtkDatabase>>,
    config: serde_json::Value,
) -> Result<(), String> {
    state.set_widget_config(&config)
}

#[tauri::command(async)]
fn get_disabled_modules(state: tauri::State<Arc<UniDtkDatabase>>) -> Result<Vec<String>, String> {
    state.get_disabled_modules()
}

#[tauri::command(async)]
fn set_disabled_modules(
    state: tauri::State<Arc<UniDtkDatabase>>,
    ids: Vec<String>,
) -> Result<(), String> {
    state.set_disabled_modules(&ids)
}

#[tauri::command(async)]
fn get_folders(
    state: tauri::State<Arc<UniDtkDatabase>>,
) -> Result<Vec<crate::unidtk_db::FolderRecord>, String> {
    state.list_folders()
}

#[tauri::command(async)]
fn save_folder(
    state: tauri::State<Arc<UniDtkDatabase>>,
    folder: crate::unidtk_db::FolderRecord,
) -> Result<(), String> {
    state.save_folder(&folder)
}

#[tauri::command(async)]
fn delete_folder(
    state: tauri::State<Arc<UniDtkDatabase>>,
    id: String,
) -> Result<bool, String> {
    state.delete_folder(&id)
}

#[tauri::command(async)]
fn vpn_connect(state: tauri::State<vpn::VpnRuntime>, tag: Option<String>) -> Result<(), String> {
    log_line(format!("vpn: command connect tag={tag:?}"));
    vpn::connect(&state, tag)
}

#[tauri::command(async)]
fn vpn_disconnect(state: tauri::State<vpn::VpnRuntime>) -> Result<(), String> {
    vpn::disconnect(&state);
    Ok(())
}

#[tauri::command(async)]
fn vpn_select_server(state: tauri::State<vpn::VpnRuntime>, tag: String) -> Result<(), String> {
    vpn::select_server(&state, tag)
}

#[tauri::command(async)]
fn vpn_ping_all(state: tauri::State<vpn::VpnRuntime>) -> Result<Vec<vpn::VpnPingResult>, String> {
    vpn::ping_all(&state)
}

#[tauri::command(async)]
fn telegram_status(state: tauri::State<telegram::TelegramRuntime>) -> telegram::TelegramStatus {
    telegram::status(&state)
}

#[tauri::command(async)]
fn telegram_chats(state: tauri::State<telegram::TelegramRuntime>) -> Vec<telegram::TelegramChat> {
    telegram::chats(&state)
}

#[tauri::command(async)]
fn telegram_chats_archive(
    state: tauri::State<telegram::TelegramRuntime>,
) -> Vec<telegram::TelegramChat> {
    telegram::archive_chats(&state)
}

#[tauri::command(async)]
fn telegram_set_archived(
    state: tauri::State<telegram::TelegramRuntime>,
    request: telegram::TelegramArchiveRequest,
) -> Result<(), String> {
    telegram::set_chat_archived(&state, request)
}

#[tauri::command(async)]
fn telegram_messages(
    state: tauri::State<telegram::TelegramRuntime>,
    chat_id: String,
    thread_id: Option<i64>,
) -> Vec<telegram::TelegramMessage> {
    match thread_id {
        Some(thread_id) if thread_id != 0 => telegram::messages_for_thread(&state, chat_id, Some(thread_id)),
        _ => telegram::messages(&state, chat_id),
    }
}

#[tauri::command(async)]
fn telegram_get_message(
    state: tauri::State<telegram::TelegramRuntime>,
    chat_id: String,
    message_id: String,
) -> Result<telegram::TelegramMessage, String> {
    telegram::get_message(&state, chat_id, message_id)
}

#[tauri::command(async)]
fn telegram_mark_read(state: tauri::State<telegram::TelegramRuntime>, chat_id: String) {
    telegram::mark_read(&state, chat_id)
}

#[tauri::command(async)]
fn telegram_open_chat(state: tauri::State<telegram::TelegramRuntime>, chat_id: String) {
    telegram::open_chat(&state, chat_id)
}

#[tauri::command(async)]
fn telegram_close_chat(
    state: tauri::State<telegram::TelegramRuntime>,
    chat_id: Option<String>,
) {
    telegram::close_chat(&state, chat_id)
}

#[tauri::command(async)]
fn telegram_older_messages(
    state: tauri::State<telegram::TelegramRuntime>,
    chat_id: String,
    before_message_id: String,
    thread_id: Option<i64>,
) -> Vec<telegram::TelegramMessage> {
    match thread_id {
        Some(thread_id) if thread_id != 0 => telegram::older_messages_for_thread(&state, chat_id, before_message_id, Some(thread_id)),
        _ => telegram::older_messages(&state, chat_id, before_message_id),
    }
}

#[tauri::command(async)]
fn telegram_messages_around(
    state: tauri::State<telegram::TelegramRuntime>,
    chat_id: String,
    message_id: String,
    thread_id: Option<i64>,
    limit: Option<u32>,
) -> Result<Vec<telegram::TelegramMessage>, String> {
    telegram::messages_around(&state, chat_id, message_id, thread_id, limit)
}

#[tauri::command(async)]
fn telegram_send_message(
    state: tauri::State<telegram::TelegramRuntime>,
    request: telegram::TelegramSendRequest,
) -> Result<telegram::TelegramMessage, String> {
    telegram::send_message(&state, request)
}

#[tauri::command(async)]
fn telegram_chat_members(
    state: tauri::State<telegram::TelegramRuntime>,
    chat_id: String,
    query: Option<String>,
) -> Result<Vec<telegram::TelegramMember>, String> {
    telegram::chat_members(&state, chat_id, query.unwrap_or_default())
}

#[tauri::command(async)]
fn telegram_send_attachment(
    state: tauri::State<telegram::TelegramRuntime>,
    request: telegram::TelegramSendAttachmentRequest,
) -> Result<telegram::TelegramMessage, String> {
    telegram::send_attachment(&state, request)
}

#[tauri::command(async)]
fn telegram_set_phone(
    state: tauri::State<telegram::TelegramRuntime>,
    phone: telegram::TelegramPhone,
) -> Result<telegram::TelegramStatus, String> {
    telegram::set_phone(&state, phone)
}

#[tauri::command(async)]
fn telegram_check_code(
    state: tauri::State<telegram::TelegramRuntime>,
    code: telegram::TelegramAuthCode,
) -> Result<telegram::TelegramStatus, String> {
    telegram::check_code(&state, code)
}

#[tauri::command(async)]
fn telegram_check_password(
    state: tauri::State<telegram::TelegramRuntime>,
    password: telegram::TelegramPassword,
) -> Result<telegram::TelegramStatus, String> {
    telegram::check_password(&state, password)
}

#[tauri::command(async)]
fn telegram_set_email(
    state: tauri::State<telegram::TelegramRuntime>,
    email: telegram::TelegramEmail,
) -> Result<telegram::TelegramStatus, String> {
    telegram::set_email(&state, email)
}

#[tauri::command(async)]
fn telegram_check_email_code(
    state: tauri::State<telegram::TelegramRuntime>,
    code: telegram::TelegramEmailCode,
) -> Result<telegram::TelegramStatus, String> {
    telegram::check_email_code(&state, code)
}

#[tauri::command(async)]
fn telegram_register_user(
    state: tauri::State<telegram::TelegramRuntime>,
    registration: telegram::TelegramRegistration,
) -> Result<telegram::TelegramStatus, String> {
    telegram::register_user(&state, registration)
}

#[tauri::command(async)]
fn telegram_get_config() -> telegram::TelegramConfig {
    telegram::get_config()
}

#[tauri::command(async)]
fn telegram_debug(state: tauri::State<telegram::TelegramRuntime>) -> telegram::TelegramDebugInfo {
    telegram::debug_info(&state)
}

#[tauri::command(async)]
fn telegram_restart(state: tauri::State<telegram::TelegramRuntime>) -> telegram::TelegramStatus {
    telegram::restart(&state)
}

#[tauri::command(async)]
fn telegram_logout(
    state: tauri::State<telegram::TelegramRuntime>,
) -> Result<telegram::TelegramStatus, String> {
    telegram::logout(&state)
}

/// Non-blocking: starts an async TDLib download and returns immediately.
/// Empty string means the download was kicked off (or is already in flight);
/// a non-empty path means the file was already available. Completion is
/// delivered via `updateFile` / `telegram-chat-update` (avatars) or
/// `telegram-attachment-ready` (message attachments), not this call.
#[tauri::command(async)]
fn telegram_download_file(
    state: tauri::State<telegram::TelegramRuntime>,
    file_id: i32,
) -> Result<String, String> {
    telegram::download_file(&state, file_id)
}

#[tauri::command(async)]
fn telegram_save_config(
    state: tauri::State<telegram::TelegramRuntime>,
    config: telegram::TelegramConfig,
) -> Result<telegram::TelegramStatus, String> {
    telegram::save_config(&state, config)
}

#[tauri::command(async)]
fn telegram_edit_message(
    state: tauri::State<telegram::TelegramRuntime>,
    request: telegram::TelegramEditMessageRequest,
) -> Result<telegram::TelegramMessage, String> {
    telegram::edit_message(&state, request)
}

#[tauri::command(async)]
fn telegram_delete_messages(
    state: tauri::State<telegram::TelegramRuntime>,
    request: telegram::TelegramDeleteMessagesRequest,
) -> Result<(), String> {
    telegram::delete_messages(&state, request)
}

#[tauri::command(async)]
fn telegram_set_reaction(
    state: tauri::State<telegram::TelegramRuntime>,
    request: telegram::TelegramReactionRequest,
) -> Result<(), String> {
    telegram::set_reaction(&state, request)
}

#[tauri::command(async)]
fn telegram_get_custom_emoji_path(
    state: tauri::State<telegram::TelegramRuntime>,
    custom_emoji_id: String,
) -> Result<Option<String>, String> {
    telegram::get_custom_emoji_path(&state, &custom_emoji_id)
}

#[tauri::command(async)]
fn telegram_pin_message(
    state: tauri::State<telegram::TelegramRuntime>,
    request: telegram::TelegramPinMessageRequest,
) -> Result<(), String> {
    telegram::pin_message(&state, request)
}

#[tauri::command(async)]
fn telegram_unpin_message(
    state: tauri::State<telegram::TelegramRuntime>,
    request: telegram::TelegramPinMessageRequest,
) -> Result<(), String> {
    telegram::unpin_message(&state, request)
}

#[tauri::command(async)]
fn telegram_get_chat_pinned_message(
    state: tauri::State<telegram::TelegramRuntime>,
    chat_id: String,
    thread_id: Option<i64>,
) -> Result<Option<telegram::TelegramMessage>, String> {
    telegram::get_chat_pinned_message(&state, chat_id, thread_id)
}

#[tauri::command(async)]
fn telegram_forward_messages(
    state: tauri::State<telegram::TelegramRuntime>,
    request: telegram::TelegramForwardMessagesRequest,
) -> Result<Vec<telegram::TelegramMessage>, String> {
    telegram::forward_messages(&state, request)
}

#[tauri::command(async)]
fn telegram_delete_chat_history(
    state: tauri::State<telegram::TelegramRuntime>,
    request: telegram::TelegramDeleteChatHistoryRequest,
) -> Result<(), String> {
    telegram::delete_chat_history(&state, request)
}

#[tauri::command(async)]
fn telegram_send_callback_query(
    state: tauri::State<telegram::TelegramRuntime>,
    request: telegram::TelegramCallbackQueryRequest,
) -> Result<serde_json::Value, String> {
    telegram::send_callback_query(&state, request)
}

#[tauri::command(async)]
fn telegram_get_forum_topics(
    state: tauri::State<telegram::TelegramRuntime>,
    request: telegram::TelegramForumTopicsRequest,
) -> Result<serde_json::Value, String> {
    telegram::get_forum_topics(&state, request)
}

#[tauri::command(async)]
fn telegram_list_accounts(
    state: tauri::State<telegram::TelegramRuntime>,
) -> Vec<telegram::TelegramAccount> {
    telegram::list_accounts(&state)
}

#[tauri::command(async)]
fn telegram_switch_account(
    state: tauri::State<telegram::TelegramRuntime>,
    account_id: String,
) -> Result<telegram::TelegramStatus, String> {
    telegram::switch_account(&state, account_id)
}

#[tauri::command(async)]
fn telegram_add_account(
    state: tauri::State<telegram::TelegramRuntime>,
) -> Result<telegram::TelegramStatus, String> {
    telegram::add_account(&state)
}

#[tauri::command(async)]
fn telegram_remove_account(
    state: tauri::State<telegram::TelegramRuntime>,
    account_id: String,
) -> Result<Vec<telegram::TelegramAccount>, String> {
    telegram::remove_account(&state, account_id)
}

// ═══════════════════════════════════════════════════════════════════════════════

#[tauri::command]
async fn scan_directory(path: String) -> Result<Vec<explorer::fs::FileItem>, String> {
    explorer::commands::scan_directory(path).await
}

#[tauri::command]
async fn scan_directory_full(path: String) -> Result<Vec<explorer::fs::FileItem>, String> {
    explorer::commands::scan_directory_full(path).await
}

#[tauri::command]
async fn get_drives() -> Result<Vec<explorer::drives::DriveInfo>, String> {
    explorer::commands::get_drives().await
}

#[tauri::command(async)]
fn get_user_places() -> Result<Vec<explorer::commands::UserPlace>, String> {
    explorer::commands::get_user_places()
}

#[tauri::command(async)]
fn path_is_directory(path: String) -> Result<bool, String> {
    explorer::commands::path_is_directory(path)
}

#[tauri::command(async)]
fn get_raw_drives() -> Result<Vec<String>, String> {
    explorer::commands::get_raw_drives()
}

#[tauri::command(async)]
fn get_unmounted_volumes() -> Result<Vec<String>, String> {
    explorer::commands::get_unmounted_volumes()
}

#[tauri::command(async)]
fn is_drive_removable(drive: String) -> Result<bool, String> {
    explorer::commands::is_drive_removable(drive)
}

#[tauri::command(async)]
fn get_portable_devices() -> Result<Vec<String>, String> {
    explorer::commands::get_portable_devices()
}

#[tauri::command(async)]
fn get_portable_contents(device_id: String) -> Result<Vec<String>, String> {
    explorer::commands::get_portable_contents(device_id)
}

#[tauri::command(async)]
fn copy_to_clipboard(paths: Vec<String>, cut: bool) -> Result<(), String> {
    explorer::commands::copy_to_clipboard(paths, cut)
}

#[tauri::command(async)]
fn paste_from_clipboard() -> Result<Vec<String>, String> {
    explorer::commands::paste_from_clipboard()
}

#[tauri::command(async)]
fn clear_clipboard() -> Result<(), String> {
    explorer::commands::clear_clipboard()
}

#[tauri::command(async)]
fn file_exists(path: String) -> Result<bool, String> {
    explorer::commands::file_exists(path)
}

#[tauri::command(async)]
fn get_file_info(path: String) -> Result<explorer::files::FileInfo, String> {
    explorer::commands::get_file_info(path)
}

#[tauri::command(async)]
fn format_file_size(bytes: u64) -> String {
    explorer::commands::format_file_size(bytes)
}

#[tauri::command(async)]
fn get_file_type_name(extension: String) -> String {
    explorer::commands::get_file_type_name(extension)
}

#[tauri::command(async)]
fn is_valid_filename(name: String) -> Result<(), String> {
    explorer::commands::is_valid_filename(name)
}

#[tauri::command(async)]
fn delete_to_recycle_bin(path: String) -> Result<(), String> {
    explorer::commands::delete_to_recycle_bin(path)
}

#[tauri::command(async)]
fn get_folder_size(path: String) -> Result<u64, String> {
    explorer::commands::get_folder_size(path)
}

#[tauri::command(async)]
fn sort_files(
    files: Vec<explorer::fs::FileItem>,
    column: String,
    ascending: bool,
) -> Result<Vec<explorer::fs::FileItem>, String> {
    explorer::commands::sort_files(files, column, ascending)
}

#[tauri::command(async)]
fn search_files(
    state: tauri::State<'_, explorer::commands::SharedSearch>,
    query: String,
) -> Vec<explorer::search::SearchResult> {
    explorer::commands::search_files(&state, query)
}

#[tauri::command(async)]
fn add_to_search_index(
    state: tauri::State<'_, explorer::commands::SharedSearch>,
    path: String,
    is_dir: bool,
) {
    explorer::commands::add_to_search_index(&state, path, is_dir)
}

#[tauri::command(async)]
fn remove_from_search_index(state: tauri::State<'_, explorer::commands::SharedSearch>, path: String) {
    explorer::commands::remove_from_search_index(&state, path)
}

#[tauri::command(async)]
fn clear_search_index(state: tauri::State<'_, explorer::commands::SharedSearch>) {
    explorer::commands::clear_search_index(&state)
}

#[tauri::command(async)]
fn get_search_suggestions(
    state: tauri::State<'_, explorer::commands::SharedSearch>,
    prefix: String,
    limit: usize,
) -> Vec<String> {
    explorer::commands::get_search_suggestions(&state, prefix, limit)
}

#[tauri::command(async)]
fn hash_file(path: String) -> Result<explorer::hashing::FileHash, String> {
    explorer::commands::hash_file(path)
}

#[tauri::command(async)]
fn hash_file_md5(path: String) -> Result<String, String> {
    explorer::commands::hash_file_md5(path)
}

#[tauri::command(async)]
fn hash_file_sha256(path: String) -> Result<String, String> {
    explorer::commands::hash_file_sha256(path)
}

#[tauri::command(async)]
fn get_archive_info(path: String) -> Result<explorer::archive::ArchiveInfo, String> {
    explorer::commands::get_archive_info(path)
}

#[tauri::command(async)]
fn extract_archive(archive_path: String, dest_dir: String) -> Result<Vec<String>, String> {
    explorer::commands::extract_archive(archive_path, dest_dir)
}

#[tauri::command(async)]
fn extract_archive_entry(
    archive_path: String,
    entry_name: String,
    dest_dir: String,
) -> Result<String, String> {
    explorer::commands::extract_archive_entry(archive_path, entry_name, dest_dir)
}

#[tauri::command(async)]
fn create_zip_archive(files: Vec<String>, archive_path: String) -> Result<(), String> {
    explorer::commands::create_zip_archive(files, archive_path)
}

#[tauri::command(async)]
fn is_zip_file(path: String) -> bool {
    explorer::commands::is_zip_file(path)
}

#[tauri::command(async)]
fn get_file_preview(path: String) -> explorer::preview::FilePreview {
    explorer::commands::get_file_preview(path)
}

#[tauri::command(async)]
fn get_text_preview(path: String, max_bytes: usize) -> Option<String> {
    explorer::commands::get_text_preview(path, max_bytes)
}

#[tauri::command(async)]
fn get_file_icon(extension: String) -> String {
    explorer::commands::get_file_icon(extension)
}

#[tauri::command(async)]
fn open_path_default(path: String) -> Result<(), String> {
    explorer::commands::open_path_default(path)
}

#[tauri::command(async)]
fn create_directory(path: String) -> Result<(), String> {
    explorer::commands::create_directory(path)
}

#[tauri::command(async)]
fn create_empty_file(path: String) -> Result<(), String> {
    explorer::commands::create_empty_file(path)
}

#[tauri::command(async)]
fn rename_path(from: String, to: String) -> Result<(), String> {
    explorer::commands::rename_path(from, to)
}

#[tauri::command(async)]
fn copy_path(from: String, to: String) -> Result<(), String> {
    explorer::commands::copy_path(from, to)
}

#[tauri::command(async)]
fn open_terminal(path: String) -> Result<(), String> {
    explorer::commands::open_terminal(path)
}

// ═══════════════════════════════════════════════════════════════════════════════
// mp3-player module (music core) — the logic lives in `shell_mod_mp3_player::commands`;
// these thin wrappers exist only because `generate_handler!` must live in the kernel.

#[tauri::command(async)]
fn get_config(
    state: tauri::State<'_, music::commands::AppState>,
) -> music::library::config::AppConfig {
    music::commands::get_config(&state)
}

#[tauri::command(async)]
fn get_config_path(state: tauri::State<'_, music::commands::AppState>) -> String {
    music::commands::get_config_path(&state)
}

#[tauri::command(async)]
fn save_config(
    state: tauri::State<'_, music::commands::AppState>,
    config: music::library::config::AppConfig,
) -> Result<music::library::config::AppConfig, String> {
    music::commands::save_config(&state, config)
}

#[tauri::command]
async fn list_tracks(
    state: tauri::State<'_, music::commands::AppState>,
    search: Option<String>,
    sort_key: Option<String>,
    sort_dir: Option<String>,
) -> Result<Vec<music::library::db::Track>, String> {
    music::commands::list_tracks(&state, search, sort_key, sort_dir).await
}

#[tauri::command]
async fn get_track(
    state: tauri::State<'_, music::commands::AppState>,
    track_id: String,
) -> Result<Option<music::library::db::Track>, String> {
    music::commands::get_track(&state, track_id).await
}

#[tauri::command]
async fn list_playlists(
    state: tauri::State<'_, music::commands::AppState>,
) -> Result<Vec<music::library::db::Playlist>, String> {
    music::commands::list_playlists(&state).await
}

#[tauri::command]
async fn list_playlist_tracks(
    state: tauri::State<'_, music::commands::AppState>,
    playlist_id: String,
    sort_key: Option<String>,
    sort_dir: Option<String>,
) -> Result<Vec<music::library::db::Track>, String> {
    music::commands::list_playlist_tracks(&state, playlist_id, sort_key, sort_dir).await
}

#[tauri::command]
async fn sync_url(
    state: tauri::State<'_, music::commands::AppState>,
    url: String,
    name: Option<String>,
) -> Result<String, String> {
    music::commands::sync_url(&state, url, name).await
}

#[tauri::command]
async fn sync_playlist(
    state: tauri::State<'_, music::commands::AppState>,
    playlist_id: String,
) -> Result<String, String> {
    music::commands::sync_playlist(&state, playlist_id).await
}

#[tauri::command]
async fn sync_all(
    state: tauri::State<'_, music::commands::AppState>,
) -> Result<Vec<String>, String> {
    music::commands::sync_all(&state).await
}

#[tauri::command]
async fn sync_tasks(
    state: tauri::State<'_, music::commands::AppState>,
) -> Result<Vec<music::sync::sync::SyncTask>, String> {
    music::commands::sync_tasks(&state).await
}

#[tauri::command]
async fn update_ytdlp(
    state: tauri::State<'_, music::commands::AppState>,
) -> Result<String, String> {
    music::commands::update_ytdlp(&state).await
}

#[tauri::command]
async fn player_load_queue(
    state: tauri::State<'_, music::commands::AppState>,
    track_ids: Vec<String>,
    start_index: usize,
) -> Result<music::playback::player::PlayerState, String> {
    music::commands::player_load_queue(&state, track_ids, start_index).await
}

#[tauri::command]
async fn player_load_view_queue(
    state: tauri::State<'_, music::commands::AppState>,
    playlist_id: Option<String>,
    search: Option<String>,
    sort_key: Option<String>,
    sort_dir: Option<String>,
    start_index: usize,
) -> Result<music::playback::player::PlayerState, String> {
    music::commands::player_load_view_queue(
        &state,
        playlist_id,
        search,
        sort_key,
        sort_dir,
        start_index,
    )
    .await
}

#[tauri::command]
async fn player_toggle_pause(
    state: tauri::State<'_, music::commands::AppState>,
) -> Result<music::playback::player::PlayerState, String> {
    music::commands::player_toggle_pause(&state).await
}

#[tauri::command]
async fn player_set_pause(
    state: tauri::State<'_, music::commands::AppState>,
    paused: bool,
) -> Result<music::playback::player::PlayerState, String> {
    music::commands::player_set_pause(&state, paused).await
}

#[tauri::command]
async fn player_seek(
    state: tauri::State<'_, music::commands::AppState>,
    seconds: f64,
) -> Result<music::playback::player::PlayerState, String> {
    music::commands::player_seek(&state, seconds).await
}

#[tauri::command]
async fn player_set_volume(
    state: tauri::State<'_, music::commands::AppState>,
    volume: f64,
) -> Result<music::playback::player::PlayerState, String> {
    music::commands::player_set_volume(&state, volume).await
}

#[tauri::command]
async fn player_next(
    state: tauri::State<'_, music::commands::AppState>,
) -> Result<music::playback::player::PlayerState, String> {
    music::commands::player_next(&state).await
}

#[tauri::command]
async fn player_previous(
    state: tauri::State<'_, music::commands::AppState>,
) -> Result<music::playback::player::PlayerState, String> {
    music::commands::player_previous(&state).await
}

#[tauri::command(async)]
fn player_status(
    state: tauri::State<'_, music::commands::AppState>,
) -> music::playback::player::PlayerState {
    music::commands::player_status(&state)
}

#[tauri::command]
async fn toggle_ai_dj(
    state: tauri::State<'_, music::commands::AppState>,
    enabled: bool,
) -> Result<bool, String> {
    music::commands::toggle_ai_dj(&state, enabled).await
}

#[tauri::command(async)]
fn ai_dj_status(
    state: tauri::State<'_, music::commands::AppState>,
) -> music::commands::AiDjStatus {
    music::commands::ai_dj_status(&state)
}

/// Докачка AI DJ модели: немедленный ответ, ход идёт в нити лоадера.
#[tauri::command(async)]
fn ai_dj_model_download() -> Result<serde_json::Value, String> {
    music::commands::ai_dj_model_download()
}

#[tauri::command]
async fn ai_dj_build_queue(
    state: tauri::State<'_, music::commands::AppState>,
    track_ids: Vec<String>,
) -> Result<Vec<String>, String> {
    music::commands::ai_dj_build_queue(&state, track_ids).await
}

#[tauri::command(async)]
fn get_crossfade_duration(state: tauri::State<'_, music::commands::AppState>) -> f64 {
    music::commands::get_crossfade_duration(&state)
}

#[tauri::command(async)]
fn set_crossfade_duration(
    state: tauri::State<'_, music::commands::AppState>,
    seconds: f64,
) -> Result<f64, String> {
    music::commands::set_crossfade_duration(&state, seconds)
}

#[tauri::command]
async fn check_track_finished(
    state: tauri::State<'_, music::commands::AppState>,
) -> Result<bool, String> {
    music::commands::check_track_finished(&state).await
}

#[tauri::command]
async fn analyze_tracks(
    state: tauri::State<'_, music::commands::AppState>,
) -> Result<usize, String> {
    music::commands::analyze_tracks(&state).await
}

#[tauri::command]
async fn reanalyze_tracks(
    state: tauri::State<'_, music::commands::AppState>,
) -> Result<usize, String> {
    music::commands::reanalyze_tracks(&state).await
}

#[tauri::command(async)]
fn ai_dj_history(
    state: tauri::State<'_, music::commands::AppState>,
) -> Vec<serde_json::Value> {
    music::commands::ai_dj_history(&state)
}

#[tauri::command]
async fn ai_dj_graph(
    state: tauri::State<'_, music::commands::AppState>,
) -> Result<music::ai::ai_dj::SimilarityGraph, String> {
    music::commands::ai_dj_graph(&state).await
}

#[tauri::command]
async fn ai_dj_undo(
    state: tauri::State<'_, music::commands::AppState>,
) -> Result<Option<music::playback::player::PlayerState>, String> {
    music::commands::ai_dj_undo(&state).await
}

#[tauri::command]
async fn ai_dj_set_mode(
    state: tauri::State<'_, music::commands::AppState>,
    mode: String,
) -> Result<(), String> {
    music::commands::ai_dj_set_mode(&state, mode).await
}

#[tauri::command(async)]
fn ai_dj_get_mode(state: tauri::State<'_, music::commands::AppState>) -> String {
    music::commands::ai_dj_get_mode(&state)
}

#[tauri::command(async)]
fn get_submodules(state: tauri::State<'_, music::commands::AppState>) -> serde_json::Value {
    music::commands::get_submodules(&state)
}

#[tauri::command]
async fn rename_playlist(
    state: tauri::State<'_, music::commands::AppState>,
    playlist_id: String,
    name: String,
) -> Result<(), String> {
    music::commands::rename_playlist(&state, playlist_id, name).await
}

#[tauri::command]
async fn delete_playlist(
    state: tauri::State<'_, music::commands::AppState>,
    playlist_id: String,
) -> Result<(), String> {
    music::commands::delete_playlist(&state, playlist_id).await
}

#[tauri::command]
async fn set_submodule_setting(
    state: tauri::State<'_, music::commands::AppState>,
    module: String,
    key: String,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    music::commands::set_submodule_setting(&state, module, key, value).await
}

#[tauri::command(async)]
fn open_explorer(app: tauri::AppHandle, path: Option<String>) -> Result<(), String> {
    let target_path = path.unwrap_or_default();
    log_line(format!("open_explorer: path={target_path}"));
    request_wallpaper_visible(&app);
    // Through the contract, so the event broker (and therefore the WebSocket door) sees
    // it too — the kernel's emitter keeps the Tauri behaviour unchanged.
    unidtk_api::emit("explorer-open", serde_json::json!(target_path));
    Ok(())
}

#[tauri::command(async)]
fn hotkeys_list() -> Result<Vec<hotkeys::HotkeyDefinition>, String> {
    Ok(hotkeys::list_hotkeys())
}

#[tauri::command(async)]
fn hotkey_toggle(id: String, enabled: bool) -> Result<(), String> {
    hotkeys::toggle_hotkey(&id, enabled)
}

#[tauri::command(async)]
fn set_desktop_input_focus(app: tauri::AppHandle, focused: bool) -> Result<(), String> {
    hotkeys::set_input_focus(&app, focused)
}

#[tauri::command(async)]
fn setup_registry_override() -> Result<(), String> {
    explorer::registry::install_as_default_file_manager()
}

#[allow(dead_code)]
fn _placeholder() {}

fn show_and_attach_wallpaper(app: tauri::AppHandle, host: (isize, u32)) -> Result<(), String> {
    log_line("show_and_attach: start");
    let window = app
        .get_webview_window("wallpaper")
        .ok_or_else(|| "wallpaper window not found".to_string())?;
    log_line("show_and_attach: window found");
    let _ = window.set_decorations(false);
    let _ = window.set_shadow(false);
    let _ = window.set_skip_taskbar(true);
    let _ = window.hide();

    let hwnd = wallpaper_hwnd(&app)?;
    log_line(format!("show_and_attach: hwnd={hwnd}"));
    if let Err(error) = wallpaper::attach_to_host(hwnd, ATTACH_OPTS, host) {
        log_line(format!("show_and_attach: attach failed; window stays hidden: {error}"));
        return Err(error);
    }

    log_line("show_and_attach: attached to desktop host");
    window.show().map_err(|e| format!("show wallpaper window: {e}"))?;
    log_line("show_and_attach: shown");
    Ok(())
}

fn run_on_main_thread<F>(app: &tauri::AppHandle, task: F) -> Result<(), String>
where
    F: FnOnce(tauri::AppHandle) -> Result<(), String> + Send + 'static,
{
    // Raw input windows need the Win32 message loop owned by Tauri's UI thread.
    let app_for_task = app.clone();
    let (tx, rx) = mpsc::channel();

    app.run_on_main_thread(move || {
        let _ = tx.send(task(app_for_task));
    })
    .map_err(|e| e.to_string())?;

    rx.recv_timeout(Duration::from_millis(3000)).map_err(|e| e.to_string())?
}

pub(crate) fn request_wallpaper_visible(app: &tauri::AppHandle) {
    request_wallpaper_attach(app, false);
}

static ATTACH_IN_PROGRESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn request_wallpaper_attach(app: &tauri::AppHandle, force: bool) {
    if !force && ATTACH_IN_PROGRESS.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_err() {
        return;
    }
    let handle = app.clone();
    thread::spawn(move || {
        struct ResetFlag;
        impl Drop for ResetFlag {
            fn drop(&mut self) {
                ATTACH_IN_PROGRESS.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let _guard = ResetFlag;

        // Phase 1 (up to ~15 s): strictly wait for WorkerW, disallowing Progman fallback
        // to avoid black screen covering wallpaper during logon initialization.
        // First 4 attempts wait 500 ms to give Explorer breathing room.
        for attempt in 0..40 {
            if is_shutting_down() { return; }
            let allow_progman = attempt >= 25; // Only after ~15 s allow fallback
            if let Ok(host) = wallpaper::resolve_desktop_host(allow_progman) {
                if run_on_main_thread(&handle, move |app| {
                    if !force && app.get_webview_window("wallpaper")
                        .is_some_and(|window| window.is_visible().unwrap_or(false)) {
                        return Ok(());
                    }
                    show_and_attach_wallpaper(app, host)
                }).is_ok() {
                    log_line(format!("request_wallpaper_attach: attached at attempt {attempt}"));
                    return;
                }
            }
            let sleep_ms = if attempt < 4 { 500 } else if attempt < 25 { 500 } else { 1000 };
            thread::sleep(Duration::from_millis(sleep_ms));
        }
        log_line("request_wallpaper_visible: desktop host unavailable after 40 attempts");
    });
}

fn wallpaper_hwnd(app: &tauri::AppHandle) -> Result<isize, String> {
    let window = app
        .get_webview_window("wallpaper")
        .ok_or_else(|| "wallpaper window not found".to_string())?;
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    Ok(hwnd.0 as isize)
}

static DISPLAY_CHANGE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn trigger_display_change() {
    if let Some(app) = APP_HANDLE.get() {
        let app_clone = app.clone();
        let seq = DISPLAY_CHANGE_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            if DISPLAY_CHANGE_SEQ.load(std::sync::atomic::Ordering::SeqCst) != seq {
                return;
            }
            if let Ok(hwnd) = wallpaper_hwnd(&app_clone) {
                let _ = wallpaper::update_geometry(hwnd);
            }
            unidtk_api::emit("display-changed", serde_json::Value::Null);
        });
    }
}

#[tauri::command(async)]
fn refresh_display_layout(app: tauri::AppHandle) -> Result<(), String> {
    if let Ok(hwnd) = wallpaper_hwnd(&app) {
        let _ = wallpaper::update_geometry(hwnd);
    }
    unidtk_api::emit("display-changed", serde_json::Value::Null);
    Ok(())
}

fn build_tray_icon_rgba() -> Image<'static> {
    let size = 16usize;
    let mut buf = vec![0u8; size * size * 4];

    for y in 0..size {
        for x in 0..size {
            let dx = x as f64 - 7.5;
            let dy = y as f64 - 7.5;
            let dist = (dx * dx + dy * dy).sqrt();
            let idx = (y * size + x) * 4;

            if (4.8..=6.8).contains(&dist) {
                let alpha = (1.0 - (dist - 5.8).abs()).max(0.0);
                buf[idx] = 255;
                buf[idx + 1] = 255;
                buf[idx + 2] = 255;
                buf[idx + 3] = (alpha * 255.0).round() as u8;
            }

            if dist <= 2.2 {
                let alpha = (1.0 - dist / 2.2).max(0.0);
                buf[idx] = 255;
                buf[idx + 1] = 255;
                buf[idx + 2] = 255;
                buf[idx + 3] = buf[idx + 3].max((alpha * 255.0).round() as u8);
            }
        }
    }

    Image::new_owned(buf, 16, 16)
}

#[tauri::command(async)]
fn trim_process_memory() -> Result<(), String> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
    Ok(())
}

fn load_env_into_process() {
    let mut dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    for _ in 0..6 {
        let candidate = dir.join(".env");
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    let k = k.trim();
                    let v = v.trim().trim_matches('"').to_string();
                    if !k.is_empty() && std::env::var(k).is_err() {
                        std::env::set_var(k, v);
                    }
                }
            }
            break;
        }
        if !dir.pop() {
            break;
        }
    }
    // Fallback to the values baked in by toolchain/build.rs (rustc-env): the
    // installed copy has no .env file anywhere above it, and the tdlib keys are
    // the same for every user. Dev checkout (.env on disk) still wins above.
    for (key, baked) in [
        ("TELEGRAM_API_ID", option_env!("TELEGRAM_API_ID")),
        ("TELEGRAM_API_HASH", option_env!("TELEGRAM_API_HASH")),
    ] {
        if let Some(value) = baked {
            if !value.is_empty() && std::env::var(key).is_err() {
                std::env::set_var(key, value);
            }
        }
    }
}

/// CDP port of this run, published to the CLI through the IPC `ping` response.
static CDP_PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();

pub(crate) fn cdp_port() -> Option<u16> {
    CDP_PORT.get().copied()
}

/// Prefer the conventional 9222, but never take a port that is already in use: a
/// stale instance would otherwise keep it and `udtk` would drive the wrong WebView.
fn pick_cdp_port() -> u16 {
    const PREFERRED: u16 = 9222;
    if std::net::TcpListener::bind(("127.0.0.1", PREFERRED)).is_ok() {
        return PREFERRED;
    }
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .ok()
        .and_then(|listener| listener.local_addr().ok())
        .map(|addr| addr.port())
        .unwrap_or(PREFERRED)
}

/// Merge our WebView2 arguments with whatever the caller already set (for example
/// `udtk run --debug`) instead of overwriting it — the previous code silently threw
/// the caller's value away. An inherited `--remote-debugging-port` /
/// `--remote-allow-origins` is dropped so the discovered port wins.
fn merge_browser_args(own: &[&str], cdp_port: u16) -> String {
    let own_line = format!("{} --remote-debugging-port={cdp_port}", own.join(" "));
    let inherited = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
    let kept: Vec<&str> = inherited
        .split_whitespace()
        .filter(|arg| {
            !arg.starts_with("--remote-debugging-port=")
                && !arg.starts_with("--remote-allow-origins=")
        })
        .collect();
    if kept.is_empty() {
        own_line
    } else {
        format!("{} {own_line}", kept.join(" "))
    }
}

/// Bootstrap for the page's own door (S4).
///
/// The port is public knowledge; the token is handed out **here and nowhere else** —
/// not over the unauthenticated tool door, not in a file, not on the command line.
/// Only code running inside this application can ask for it.
#[tauri::command]
fn session_info() -> serde_json::Value {
    serde_json::json!({
        "v": 1,
        "wsPort": ws::port(),
        "token": ws::token(),
    })
}

#[cfg(test)]
mod frontend_contract_tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    /// Every `invoke("…")` in the interface must be registered in `generate_handler!`.
    ///
    /// Guards the failure mode where a command is renamed or deleted in the kernel while
    /// the frontend keeps calling it — that surfaces at runtime as an empty widget, not
    /// at build time. It is deliberately a *test*: `generate_handler!` is a macro, so
    /// `build.rs` cannot read its contents structurally (see `docs/cliudtk.md` §10.8.2).
    #[test]
    fn frontend_invoke_names_are_registered() {
        let registered = registered_commands();
        assert!(
            registered.len() > 10,
            "could not read the command list from generate_handler! (found {} entries)",
            registered.len()
        );

        let mut missing = Vec::new();
        for file in ui_scripts() {
            let Ok(source) = std::fs::read_to_string(&file) else {
                continue;
            };
            for name in invoke_names(&source) {
                // Plugin commands (`plugin:fs|read_file`) are registered by Tauri itself.
                if name.contains(':') || registered.contains(&name) {
                    continue;
                }
                missing.push(format!("{}: invoke(\"{name}\")", file.display()));
            }
        }
        assert!(
            missing.is_empty(),
            "the interface calls commands the kernel does not register:\n{}",
            missing.join("\n")
        );
    }

    /// Every command the manifest promises on the UI door must exist as a Tauri command.
    ///
    /// The manifest is what the CLI, the acceptance checks and the migration are read
    /// against, so a name no door serves is worse than a missing entry — it reads as a
    /// promise. Two had drifted: the write was declared as `log` while the kernel
    /// registers `frontend_log`, and the volume read as `volume_get` while it registers
    /// `get_system_volume`. Nothing caught it, because the other contract test compares
    /// the *interface* with the handler list and never the manifest with either.
    ///
    /// The converse is deliberately not required: the kernel registers far more commands
    /// than the manifest describes, and the manifest is a contract, not an inventory of
    /// every internal command.
    #[test]
    fn every_manifest_ui_command_is_registered() {
        let registered = registered_commands();
        assert!(
            registered.len() > 10,
            "could not read the command list from generate_handler! (found {} entries)",
            registered.len()
        );
        let mut orphans = Vec::new();
        for spec in crate::command_inventory::ALL.iter().copied() {
            if !spec.doors.contains(&unidtk_api::commands::Door::Ui) {
                continue;
            }
            if !registered.contains(spec.name) {
                orphans.push(spec.name);
            }
        }
        assert!(
            orphans.is_empty(),
            "the manifest promises UI commands the kernel does not register: {}",
            orphans.join(", ")
        );
    }

    /// Names listed inside the handler macro.
    fn registered_commands() -> HashSet<String> {
        let source = include_str!("main.rs");
        // Assembled at runtime: written as one literal, this test's own source would be
        // the first match in the file it inspects.
        let marker = ["generate_handler!", "["].concat();
        let list = source
            .split(&marker)
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .unwrap_or("");
        list.lines()
            .map(|line| line.trim().trim_end_matches(','))
            .filter(|line| !line.is_empty() && !line.starts_with("//"))
            .map(str::to_string)
            .collect()
    }

    /// Every event the interface listens for must still be named somewhere in Rust.
    ///
    /// Strength, stated honestly: this proves the *name exists* on the Rust side — it
    /// does not prove it is emitted on the path you expect. A stricter check is not
    /// possible by text alone, because event names are often passed as a separate
    /// argument (`emit(app, "telegram-message-new", payload)` spans lines) and because
    /// some names are built dynamically. What it does catch is the real failure mode:
    /// a renamed or deleted event leaves the listener permanently silent.
    #[test]
    fn frontend_listen_names_exist_in_rust() {
        let mut known: HashSet<String> = HashSet::new();
        for file in rust_sources() {
            let Ok(source) = std::fs::read_to_string(&file) else {
                continue;
            };
            known.extend(quoted_literals(&source));
        }

        let listen_marker = ["li", "sten("].concat();
        let mut orphans = Vec::new();
        for file in ui_scripts() {
            let Ok(source) = std::fs::read_to_string(&file) else {
                continue;
            };
            for name in literals_after(&source, &listen_marker) {
                if !known.contains(&name) {
                    orphans.push(format!("{}: listen(\"{name}\")", file.display()));
                }
            }
        }
        assert!(
            orphans.is_empty(),
            "the interface listens for events no longer named in Rust:\n{}",
            orphans.join("\n")
        );
    }

    /// Every double-quoted literal in a source file.
    fn quoted_literals(source: &str) -> Vec<String> {
        let mut literals = Vec::new();
        let mut chars = source.chars();
        while let Some(c) = chars.next() {
            if c != '"' {
                continue;
            }
            let value: String = chars.by_ref().take_while(|c| *c != '"').collect();
            if !value.is_empty() {
                literals.push(value);
            }
        }
        literals
    }

    /// Literal first arguments of `marker("…")` / `marker('…')`.
    fn literals_after(source: &str, marker: &str) -> Vec<String> {
        let mut names = Vec::new();
        for (index, _) in source.match_indices(marker) {
            let rest = &source[index + marker.len()..];
            let mut chars = rest.chars();
            let Some(quote) = chars.next() else { continue };
            if quote != '"' && quote != '\'' {
                continue;
            }
            let name: String = chars.take_while(|c| *c != quote).collect();
            if !name.is_empty() && !name.contains(' ') {
                names.push(name);
            }
        }
        names
    }

    /// Every `.rs` file in the checkout.
    fn rust_sources() -> Vec<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        for dir in ["core", "modules", "opt"] {
            collect_by_extension(&root.join(dir), "rs", &mut files);
        }
        files
    }

    fn collect_by_extension(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_by_extension(&path, extension, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some(extension) {
                out.push(path);
            }
        }
    }

    /// Every UI script in the checkout: `core/ui` plus each module's `ui/`.
    fn ui_scripts() -> Vec<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        collect_js(&root.join("core/ui"), &mut files);
        if let Ok(entries) = std::fs::read_dir(root.join("modules")) {
            for entry in entries.flatten() {
                collect_js(&entry.path().join("ui"), &mut files);
            }
        }
        files
    }

    fn collect_js(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_js(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("js") {
                out.push(path);
            }
        }
    }

    /// `invoke("name")` / `invoke('name')` — literal command names only.
    fn invoke_names(source: &str) -> Vec<String> {
        let mut names = Vec::new();
        for (index, _) in source.match_indices("invoke(") {
            let rest = &source[index + "invoke(".len()..];
            let mut chars = rest.chars();
            let Some(quote) = chars.next() else { continue };
            if quote != '"' && quote != '\'' {
                continue;
            }
            let name: String = chars.take_while(|c| *c != quote).collect();
            if !name.is_empty() && !name.contains(' ') {
                names.push(name);
            }
        }
        names
    }
}

fn main() {
    panic_hook::install_panic_hook();
    // Modules write through the contract, not through this crate: give them the
    // kernel's log sink before anything else can run.
    unidtk_api::set_logger(|message| log_line(message));
    load_env_into_process();
    log_line("main: start");
    let startup_started = std::time::Instant::now();
    // Open files while Tauri creates WebView2. Setup still waits for the result
    // before registering state, so commands never observe temporary defaults.
    let data_bootstrap = thread::spawn(|| {
        let started = std::time::Instant::now();
        let path = UniDtkDatabase::default_path();
        match library_migration::backup_before_migration(&path) {
            Ok(Some(backup)) => log_line(format!("startup: library backup {}", backup.display())),
            Ok(None) => {},
            Err(error) => log_line(format!("startup: library backup failed: {error}")),
        }
        let result = UniDtkDatabase::open(&path).map(Arc::new);
        if let Ok(db) = &result {
            match library_migration::migrate_if_needed(&db.raw_db()) {
                Ok(Some(report)) => log_line(format!(
                    "library: migrated {} tracks / {} playlists / {} links into mods/mp3_player",
                    report.tracks, report.playlists, report.links
                )),
                Ok(None) => {},
                Err(error) => log_line(format!("library: module-kv migration failed: {error}")),
            }
        }
        log_line(format!("startup: data open {} ms", started.elapsed().as_millis()));
        result
    });
    let logs_bootstrap = thread::spawn(|| {
        let started = std::time::Instant::now();
        let result = LogStore::open(LogStore::default_path()).map(Arc::new);
        if let Ok(store) = &result {
            crate::logging::init(store.clone(), Some(unidtk_api::paths::logs_dir().join("unidtk.log")));
        }
        log_line(format!("startup: logs open {} ms", started.elapsed().as_millis()));
        result
    });

    // Configure WebView2 to minimize working set memory footprint:
    // - single renderer process limit
    // - V8 max old space size capped at 128MB to prevent heap hoarding
    // - disable unused Chromium features, disk caches, and DWM background occlusion throttling
    // NOTE: GPU DirectComposition and shader disk cache are preserved for fast cold boot.
    let browser_args = vec![
        "--renderer-process-limit=1",
        "--js-flags=--max-old-space-size=128",
        "--disable-features=CalculateNativeWinOcclusion,TranslateUI,MediaEngagementBypassAutoplayPolicies",
        "--disable-backgrounding-occluded-windows",
        "--disable-renderer-backgrounding",
    ];

    // CDP is always on: the CLI diagnostic tooling (`udtk gui`/`shot`/`dom`) depends
    // on it. The port is *discovered*, not assumed — if 9222 is held by a stale or
    // foreign process we take a free one and publish it over IPC, so `udtk` can never
    // drive somebody else's WebView. `--remote-allow-origins` is deliberately NOT set:
    // our CLI sends no Origin header, and a client-chosen Origin is not a control.
    let cdp_port = pick_cdp_port();
    let _ = CDP_PORT.set(cdp_port);
    log_line(format!("main: CDP port {cdp_port} enabled for CLI diagnostic tooling"));

    let merged = merge_browser_args(&browser_args, cdp_port);
    std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", merged);

    // Periodic memory trimmer: periodically returns unused/dormant working set pages to Windows.
    #[cfg(windows)]
    let _ = std::thread::Builder::new()
        .name("mem-trimmer".to_string())
        .spawn(|| {
            use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
            loop {
                std::thread::sleep(std::time::Duration::from_secs(60));
                unsafe {
                    let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
                }
            }
        });

    tauri::Builder::default()
        .on_page_load(|_, _| log_line("startup: WebView page load callback"))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(path) = explorer::window::parse_open_arg(&argv) {
                log_line(format!("single-instance: explorer open {path:?}"));
                unidtk_api::emit("explorer-open", serde_json::json!(path));
                return;
            }
            // Second wallpaper launch: re-attach under WorkerW instead of
            // focusing a top-level window (set_focus would flash it over the desktop).
            if app.get_webview_window("wallpaper").is_some() {
                request_wallpaper_attach(app, true);
            }
        }))
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            session_info,
            trim_process_memory,
            // wallpaper-only
            reset_wallpaper,
            attach_wallpaper,
            detach_wallpaper,
            set_clipboard_text,
            attach_main_window,
            detach_main_window,
            quit_app,
            frontend_log,
            log_list,
            log_clear,
            log_stats,
            explorer_tabs_get,
            explorer_tabs_set,
            explorer_sort_get,
            explorer_sort_set,
            get_widget_config,
            get_default_widget_config,
            save_widget_config,
            get_disabled_modules,
            set_disabled_modules,
            refresh_display_layout,
            vpn_list_servers,
            vpn_refresh_servers,
            vpn_status,
            vpn_settings,
            vpn_set_auto_connect,
            vpn_set_subscription_url,
            vpn_set_home_server_config,
            vpn_connect,
            vpn_disconnect,
            vpn_select_server,
            vpn_ping_all,
            get_folders,
            save_folder,
            delete_folder,
            telegram_status,
            telegram_chats,
            telegram_chats_archive,
            telegram_set_archived,
            telegram_messages,
            telegram_get_message,
            telegram_mark_read,
            telegram_open_chat,
            telegram_close_chat,
            telegram_older_messages,
            telegram_messages_around,
            telegram_send_message,
            telegram_chat_members,
            telegram_send_attachment,
            telegram_set_phone,
            telegram_check_code,
            telegram_check_password,
            telegram_set_email,
            telegram_check_email_code,
            telegram_register_user,
            telegram_get_config,
            telegram_debug,
            telegram_restart,
            telegram_save_config,
            telegram_logout,
            telegram_download_file,
            telegram_edit_message,
            telegram_delete_messages,
            telegram_set_reaction,
            telegram_get_custom_emoji_path,
            telegram_pin_message,
            telegram_unpin_message,
            telegram_get_chat_pinned_message,
            telegram_forward_messages,
            telegram_delete_chat_history,
            telegram_send_callback_query,
            telegram_get_forum_topics,
            telegram_list_accounts,
            telegram_switch_account,
            telegram_add_account,
            telegram_remove_account,
            get_system_volume,
            set_system_volume,
            set_system_mute,
            // micrographics
            weather_current,
            weather_location_request,
            // music-core shared
            get_config,
            get_config_path,
            save_config,
            list_tracks,
            get_track,
            list_playlists,
            list_playlist_tracks,
            sync_url,
            sync_playlist,
            sync_all,
            sync_tasks,
            update_ytdlp,
            player_load_queue,
            player_load_view_queue,
            player_toggle_pause,
            player_set_pause,
            player_seek,
            player_set_volume,
            player_next,
            player_previous,
            player_status,
            toggle_ai_dj,
            ai_dj_status,
            ai_dj_model_download,
            ai_dj_build_queue,
            get_crossfade_duration,
            set_crossfade_duration,
            check_track_finished,
            analyze_tracks,
            reanalyze_tracks,
            ai_dj_history,
            ai_dj_graph,
            ai_dj_undo,
            ai_dj_set_mode,
            ai_dj_get_mode,
            get_submodules,
            set_submodule_setting,
            rename_playlist,
            delete_playlist,
            // explorer commands
            scan_directory,
            scan_directory_full,
            get_drives,
            get_user_places,
            path_is_directory,
            get_raw_drives,
            get_unmounted_volumes,
            is_drive_removable,
            get_portable_devices,
            get_portable_contents,
            copy_to_clipboard,
            paste_from_clipboard,
            clear_clipboard,
            file_exists,
            get_file_info,
            format_file_size,
            get_file_type_name,
            is_valid_filename,
            delete_to_recycle_bin,
            get_folder_size,
            sort_files,
            search_files,
            add_to_search_index,
            remove_from_search_index,
            clear_search_index,
            get_search_suggestions,
            hash_file,
            hash_file_md5,
            hash_file_sha256,
            get_archive_info,
            extract_archive,
            extract_archive_entry,
            create_zip_archive,
            is_zip_file,
            get_file_preview,
            get_text_preview,
            get_file_icon,
            open_path_default,
            create_directory,
            create_empty_file,
            rename_path,
            copy_path,
            open_terminal,
            // explorer navigation
            open_explorer,
            setup_registry_override,
            // core hotkeys & focus
            hotkeys_list,
            hotkey_toggle,
            set_desktop_input_focus,
        ])
        .setup(move |app| {
            log_line("setup: start");
            log_line(format!("startup: Tauri entered setup at {} ms", startup_started.elapsed().as_millis()));

            // Modules publish through the contract; the kernel owns the transport
            // (Tauri events) and the "last known state" cache that lets modules
            // PATH registration belongs to the installer, never the UI startup.

            // Modules publish through the contract; the kernel owns the transport
            // (Tauri events) and the "last known state" cache that lets modules
            // read each other's state without depending on each other.
            //
            // The same call also feeds the event broker, so the WebSocket door receives
            // exactly the events the page already gets, with the same numbering — one
            // place to keep in sync instead of a second emit path.
            {
                let handle = app.handle().clone();
                unidtk_api::set_emitter(move |event, payload| {
                    let _ = handle.emit(event, payload.clone());
                    events::publish(event, payload);
                });
            }
            network::install();

            // Await completed background opens before publishing managed state.
            // No commands can mistake a pending database for an empty one.
            {
                let db_path = UniDtkDatabase::default_path();
                match data_bootstrap.join().unwrap_or_else(|_| Err("data bootstrap panicked".into())) {
                    Ok(arc) => {
                        if let Ok(None) = arc.get_widget_config() {
                            let _ = arc.set_widget_config(&default_widget_config());
                            log_line("setup: seeded default widget config in redb");
                        }

                        let local_folders_dir = dirs::data_local_dir()
                            .unwrap_or_else(|| std::path::PathBuf::from("."))
                            .join("UniDTK")
                            .join("folders");
                        let folders_to_migrate = if local_folders_dir.is_dir() {
                            local_folders_dir
                        } else {
                            dirs::data_local_dir()
                                .unwrap_or_else(|| std::path::PathBuf::from("."))
                                .join("Shell")
                                .join("folders")
                        };
                        if let Ok(migrated) = arc.auto_migrate_disk_folders(&folders_to_migrate) {
                            if migrated > 0 {
                                log_line(format!("setup: auto-migrated {migrated} folders from disk to redb"));
                            }
                        }
                        if let Some(local) = dirs::data_local_dir() {
                            match arc.rebase_legacy_shortcut_targets(
                                &local.join("Shell").join("folders"),
                                &local.join("UniDTK").join("folders"),
                            ) {
                                Ok(0) => {}
                                Ok(n) => log_line(format!("setup: moved {n} shortcut targets off the legacy Shell data dir")),
                                Err(e) => log_line(format!("setup: shortcut target migration failed: {e}")),
                            }
                        }

                        app.manage(arc);
                        log_line(format!("setup: redb data store ready ({})", db_path.display()));
                    }
                    Err(error) => {
                        log_line(format!("setup: redb open failed (file log only): {error}"));
                    }
                }

                // Logs live in their own file; a failure here degrades to the
                // plain-text unidtk.log instead of aborting the app.
                match logs_bootstrap.join().unwrap_or_else(|_| Err("logs bootstrap panicked".into())) {
                    Ok(log_store) => {
                        app.manage(log_store);
                        log_line("setup: log store ready (unidtk_logs.redb)");
                    }
                    Err(error) => {
                        log_line(format!("setup: log redb open failed (file log only): {error}"));
                    }
                }
            }

            // Music-core bootstrap (library + mpv pipe) is synchronous and was
            // blocking setup from returning — which delayed the event loop and the
            // wallpaper attach that rides on it (run_on_main_thread only fires once
            // the loop is pumping). Defer it to a background thread and manage the
            // state when ready; music commands are only reachable after the user
            // opens that widget, well after this finishes.
            //
            // The library lives behind the module contract: the kernel hands
            // mp3-player its own key slice of unidtk.redb and never exposes the raw
            // engine (library_migration.rs moved the old tables there once).
            let handle_for_music = app.handle().clone();
            let shared_db_for_music = shared_db(app.handle());
            let disabled_mods = disabled_module_set(app.handle());
            let music_disabled = disabled_mods.contains("mp3-player");
            thread::spawn(move || {
                if shared_db_for_music.is_none() {
                    log_line("setup: qbc music core skipped (no redb)");
                    return;
                }
                if music_disabled {
                    log_line("setup: qbc music core skipped (module disabled)");
                    return;
                }
                let ctx = module_context(&handle_for_music, "mp3-player", true);
                let module = commands::Mp3PlayerModule::new();
                match unidtk_api::Module::init(&module, ctx) {
                    Ok(()) => {
                        if is_shutting_down() {
                            // Вышли, пока модуль поднимался: останавливаем то,
                            // что уже успело запуститься, и не управляем им.
                            let _ = unidtk_api::Module::shutdown(&module, Duration::from_millis(1500));
                            log_line("setup: music core finished after shutdown; stopped");
                            return;
                        }
                        match module.state() {
                            Some(state) => {
                                handle_for_music.manage(state);
                                handle_for_music.manage(module);
                                log_line("setup: qbc music core ready (deferred, auto-sync armed)");
                            }
                            None => log_line("setup: qbc music core produced no state"),
                        }
                    }
                    Err(error) => {
                        log_line(format!("setup: qbc music core bootstrap failed: {error}"))
                    }
                }
            });

            let show = MenuItemBuilder::with_id("show", "Показать виджеты").build(app)?;
            let reset = MenuItemBuilder::with_id("reset", "Сбросить обои").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Выход").build(app)?;

            let menu = MenuBuilder::new(app)
                .item(&show)
                .separator()
                .item(&reset)
                .separator()
                .item(&quit)
                .build()?;

            TrayIconBuilder::new()
                .icon(build_tray_icon_rgba())
                .menu(&menu)
                .tooltip("UniDTK")
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => {
                        request_wallpaper_attach(app, true);
                    }
                    "reset" => {
                        // Forget the original so the next launch re-snapshots
                        // whatever the user set after this reset.
                        wallpaper::forget_original_wallpaper();
                        let _ = wallpaper::reset();
                    }
                    "quit" => {
                        let _ = wallpaper::reset();
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            // Модули поднимаются через контракт `unidtk-api`: ядро даёт каждому
            // свой срез базы и AppHandle, а дальше только управляет состоянием.
            // Захват спектра (визуализатор) стартует внутри mp3-player.
            let glyph_bar_module = glyph_bar::GlyphBarModule::new();
            unidtk_api::Module::init(&glyph_bar_module, module_context(app.handle(), "glyph-bar", !disabled_mods.contains("glyph-bar")))?;
            app.manage(glyph_bar_module);
            log_line("setup: glyph-bar module started (system volume poll)");

            let vpn_module = vpn::VpnModule::new();
            unidtk_api::Module::init(&vpn_module, module_context(app.handle(), "vpn", !disabled_mods.contains("vpn")))?;
            app.manage(vpn_module.runtime());
            app.manage(vpn_module);
            log_line("setup: vpn auto-connect kicked off");

            // Windows session-end listener (WM_QUERYENDSESSION / WM_ENDSESSION):
            // the wallpaper window is a WS_CHILD of WorkerW and never receives
            // those broadcasts, so a separate hidden top-level window listens
            // and stops the VPN synchronously inside the shutdown budget.
            shutdown::install(app.handle().clone());

            let telegram_module = telegram::TelegramModule::new();
            unidtk_api::Module::init(&telegram_module, module_context(app.handle(), "telegram", !disabled_mods.contains("telegram")))?;
            if let Some(runtime) = telegram_module.runtime() {
                app.manage(runtime);
            }
            app.manage(telegram_module);
            log_line("setup: telegram runtime ready");

            let explorer_module = explorer::commands::ExplorerModule::new();
            unidtk_api::Module::init(&explorer_module, module_context(app.handle(), "explorer", !disabled_mods.contains("explorer")))?;
            app.manage(explorer_module.search());
            app.manage(explorer_module);
            log_line("setup: explorer search engine ready");

            // Init first, then register the state: a command arriving between the
            // two would otherwise find "not ready" forever, because the managed
            // clone is taken after `init` filled the shared cell.
            let micrographics_module = micrographics::MicrographicsModule::new();
            unidtk_api::Module::init(&micrographics_module, module_context(app.handle(), "micrographics", !disabled_mods.contains("micrographics")))?;
            app.manage(micrographics_module.runtime());
            app.manage(micrographics_module);
            log_line("setup: micrographics weather feed ready");

            // Localhost IPC server for CLI/agent debugging (architecture §5.3):
            // read-only module snapshots + core log/config commands.
            {
                let db_for_ipc = app.try_state::<Arc<UniDtkDatabase>>().map(|s| s.inner().clone());
                let logs_for_ipc = app.try_state::<Arc<LogStore>>().map(|s| s.inner().clone());
                if let Some(db_arc) = db_for_ipc {
                    let server = ipc::IpcServer::new(db_arc, logs_for_ipc, app.handle().clone());
                    // The page's own door (S4): same dispatcher, guarded by a token
                    // that is minted per run and handed to the page once at boot.
                    // Started before `start()` below, which consumes the server.
                    match ws::start(server.clone(), ws::new_token()) {
                        Some(port) => log_line(format!("setup: ui ws door on 127.0.0.1:{port}")),
                        None => log_line("setup: ui ws door unavailable (app keeps running)"),
                    }
                    server.start();
                } else {
                    log_line("setup: ipc server skipped (no redb)");
                }
            }

            let args: Vec<String> = std::env::args().collect();
            if let Some(path) = explorer::window::parse_open_arg(&args) {
                log_line(format!("setup: explorer launch arg={path:?}"));
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(800));
                    unidtk_api::emit("explorer-open", serde_json::json!(path));
                });
            }

            // Register as default file manager (HKCU overlay) — PowerShell shortcut is heavy, do in background.
            thread::spawn(|| {
                match explorer::registry::install_as_default_file_manager() {
                    Ok(()) => log_line("setup: registry overlay installed"),
                    Err(e) => log_line(format!("setup: registry overlay failed: {e}")),
                }
            });

            // Core hotkeys & system hook dispatcher (Win+E, Escape, Focus, etc.)
            hotkeys::init(app.handle().clone());

            let _ = APP_HANDLE.set(app.handle().clone());

            request_wallpaper_visible(app.handle());

            log_line("setup: done");
            log_line(format!("startup: setup complete at {} ms", startup_started.elapsed().as_millis()));
            Ok(())
        })
        .on_window_event(|window, event| {
            // Only the wallpaper outlives a close request; transient windows
            // (the location-permission host) must really close, or their label
            // stays taken and their page stays a CDP target.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if !hides_on_close(window.label()) {
                    return;
                }
                log_line("window: close requested -> hide");
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("tauri build")
        .run(|app, event| {
            if let RunEvent::ExitRequested { .. } = event {
                log_line("run: exit requested");
                // Never block the main event loop here; the awaited variant is
                // used by quit_app and the session-end handler instead.
                app_shutdown(app, false);
            }
        });
}
