#![windows_subsystem = "windows"]

mod wallpaper;
mod weather_location;
mod network;

// Every module lives behind the `unidtk-api` contract as a real crate: the
// kernel depends on them as Cargo packages instead of including their sources.
// No `#[path]` source includes remain.
use unidtk_mod_explorer as explorer;
use unidtk_mod_micrographics as micrographics;

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
mod start_menu;

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
        // The log lives in the data directory, not next to the program: under Program
        // Files, writing next to the exe is impossible, and all startup logging was silently lost.
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
static SHUTDOWN: ShutdownLatch = ShutdownLatch::new();

/// How long a second exit route waits for the first one: the module stops, the
/// 2 s VPN stop, and the log flush, with room to spare.
const SECOND_SHUTDOWN_BUDGET: Duration = Duration::from_secs(10);

/// How long an exit off the main thread waits for the elevated helper to
/// confirm that sing-box is gone.
const VPN_STOP_BUDGET: Duration = Duration::from_secs(2);

/// Who runs the shutdown, and when it is finished. A second exit route must not
/// return while the first is still stopping the VPN: a session end that returns
/// from `WM_ENDSESSION` early lets Windows end the process with the TUN up.
struct ShutdownLatch {
    started: std::sync::atomic::AtomicBool,
    done: Mutex<bool>,
    finished: std::sync::Condvar,
}

impl ShutdownLatch {
    const fn new() -> Self {
        Self { started: std::sync::atomic::AtomicBool::new(false), done: Mutex::new(false), finished: std::sync::Condvar::new() }
    }

    /// True for the first caller only; that caller must call `finish`.
    fn begin(&self) -> bool {
        !self.started.swap(true, std::sync::atomic::Ordering::SeqCst)
    }

    fn is_started(&self) -> bool {
        self.started.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn finish(&self) {
        let mut done = self.done.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *done = true;
        self.finished.notify_all();
    }

    /// Waits until the first caller finished; false when the budget ran out.
    fn wait(&self, budget: Duration) -> bool {
        let done = self.done.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (done, _) = self.finished.wait_timeout_while(done, budget, |done| !*done)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *done
    }
}

/// Stop every subsystem exactly once. `wait_for_vpn` blocks (bounded) until the
/// elevated helper has actually killed sing-box — pass `true` only from threads
/// where sleeping is acceptable (async command, session-end listener), never
/// from the main event loop. A second call with `wait_for_vpn` waits for the
/// first to finish; without it, it returns at once.
pub(crate) fn app_shutdown(app: &tauri::AppHandle, wait_for_vpn: bool) {
    if !SHUTDOWN.begin() {
        if wait_for_vpn && !SHUTDOWN.wait(SECOND_SHUTDOWN_BUDGET) {
            log_line("shutdown: the first exit route did not finish within the budget");
        }
        return;
    }
    log_line("shutdown: begin");
    // A switch that is still running finishes first; any later one sees the exit
    // and refuses (`apply_module_enabled`). The main thread does not wait for it,
    // because a switch may need the event loop.
    drop(if wait_for_vpn { MODULE_SWITCH.lock().ok() } else { MODULE_SWITCH.try_lock().ok() });
    // Every exit latch first, then each module in reverse start order: Telegram
    // stops before the VPN, whose stop fires the state event that would start
    // Telegram's proxy again. Off the main thread the VPN waits, bounded, for its
    // elevated helper; on it, nothing waits.
    let budget = if wait_for_vpn { VPN_STOP_BUDGET } else { Duration::ZERO };
    if let Some(registry) = app.try_state::<registry::Registry>() {
        for (id, error) in registry.shutdown(budget) {
            log_line(format!("shutdown: module {id}: {error}"));
        }
    }
    hotkeys::shutdown();
    let _ = wallpaper::reset();
    // Drain queued log lines into unidtk_logs.redb / unidtk.log.
    crate::logging::flush();
    log_line("shutdown: done");
    SHUTDOWN.finish();
}

/// True once the exit path has started (see `app_shutdown`).
pub(crate) fn is_shutting_down() -> bool {
    SHUTDOWN.is_started()
}

/// Module context: its own database slice + AppHandle as host state.
/// If redb failed to open, the module receives an empty stub; modules that do not
/// need the database must still start in degraded mode.
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

/// Modules the user has disabled (from redb `disabled_modules`). Default `[]`.
/// If redb failed to open, all modules are treated as enabled (the previous behavior, no panic).
fn disabled_module_set(app: &tauri::AppHandle) -> std::collections::HashSet<String> {
    match app.try_state::<Arc<UniDtkDatabase>>() {
        Some(state) => state
            .inner()
            .get_disabled_modules_or_reset(|why| {
                log_line(format!("setup: disabled_modules is damaged ({why}); every module starts enabled"));
            })
            .unwrap_or_else(|error| {
                log_line(format!("setup: disabled_modules cannot be read ({error}); every module starts enabled"));
                Vec::new()
            })
            .into_iter()
            .collect(),
        None => std::collections::HashSet::new(),
    }
}

/// File-store factory: one per process, because redb locks the file
/// exclusively and each path must have exactly one open handle.
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

/// Stub database slice for startup without redb: there is nothing to read, and writing is not allowed.
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
    explorer::clipboard::copy_text_to_clipboard(&text)
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

// ─── Desktop folder shortcuts (start_menu.rs) ───────────────────────────────

#[tauri::command(async)]
fn start_menu_list() -> Result<Vec<start_menu::ShortcutDraft>, String> {
    start_menu::start_menu()
}

#[tauri::command(async)]
fn shortcut_icon(target: String, size: Option<u32>) -> Result<Option<start_menu::IconPixels>, String> {
    start_menu::icon(target, size.unwrap_or(48))
}

#[tauri::command(async)]
fn describe_shortcut_target(target: String) -> Result<start_menu::ShortcutDraft, String> {
    start_menu::describe(target)
}

#[tauri::command(async)]
fn missing_targets(targets: Vec<String>) -> Vec<bool> {
    start_menu::missing(&targets)
}

#[tauri::command(async)]
fn launch_shortcut(target: String, args: Option<String>) -> Result<(), String> {
    start_menu::launch(&target, args.as_deref().unwrap_or(""))
}

/// The system file/folder dialog. No parent window: ours is a child of the
/// desktop and would pull the dialog under it.
#[tauri::command(async)]
fn pick_shortcut_target(app: tauri::AppHandle, folder: bool) -> Result<Option<start_menu::ShortcutDraft>, String> {
    use tauri_plugin_dialog::DialogExt;
    let dialog = app.dialog().file();
    let picked = if folder {
        dialog.set_title("Выберите папку").blocking_pick_folder()
    } else {
        dialog.set_title("Выберите файл или программу").blocking_pick_file()
    };
    let Some(picked) = picked else { return Ok(None) };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    start_menu::describe(path.display().to_string()).map(Some)
}

#[derive(serde::Serialize)]
struct ClipboardShortcuts {
    files: Vec<String>,
    text: Option<String>,
}

#[tauri::command(async)]
fn clipboard_shortcut_targets() -> Result<ClipboardShortcuts, String> {
    let files: Vec<String> = explorer::clipboard::paste_files_from_clipboard()?
        .into_iter()
        .map(|p| p.display().to_string())
        .collect();
    let text = if files.is_empty() { explorer::clipboard::paste_text_from_clipboard()? } else { None };
    Ok(ClipboardShortcuts { files, text })
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

/// Serves a module's command that arrived through Tauri, or hands the invoke back
/// when no module declares the name. The module runs on the blocking pool, never
/// on the main thread (r[kernel.no-blocking-main-thread]), and the door is checked
/// as for the other doors.
fn invoke_module_command(invoke: tauri::ipc::Invoke) -> Result<bool, tauri::ipc::Invoke> {
    let app = invoke.message.webview().app_handle().clone();
    let cmd = invoke.message.command().to_string();
    let declared = app
        .try_state::<registry::Registry>()
        .is_some_and(|registry| registry.command(&cmd).is_some());
    if !declared {
        return Err(invoke);
    }
    let args = match invoke.message.payload() {
        tauri::ipc::InvokeBody::Json(value) => value.clone(),
        tauri::ipc::InvokeBody::Raw(_) => serde_json::Value::Null,
    };
    invoke.resolver.respond_async(async move {
        tauri::async_runtime::spawn_blocking(move || {
            ipc::serve_module(&app, &cmd, &args, unidtk_api::commands::Door::Ui)
                .unwrap_or_else(|| Err(format!("unknown cmd: {cmd}")))
        })
        .await
        .map_err(|error| tauri::ipc::InvokeError::from(error.to_string()))?
        .map_err(tauri::ipc::InvokeError::from)
    });
    Ok(true)
}

fn hides_on_close(label: &str) -> bool {
    label == "wallpaper"
}

#[tauri::command(async)]
fn weather_location_request(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    weather_location::request(app)
}

#[tauri::command(async)]
fn frontend_log(message: String, level: Option<String>) {
    crate::logging::log_frontend(&message, level.as_deref());
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

/// Switches are one read-modify-write of the stored list plus a module restart.
/// The exit takes the same lock, so a switch never runs beside it.
static MODULE_SWITCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Turn a module on or off in the running app and remember the choice. The
/// stored list changes only after the module did, so a failure leaves both as
/// they were.
pub(crate) fn apply_module_enabled(app: &tauri::AppHandle, db: &UniDtkDatabase, id: &str, enabled: bool) -> Result<(), String> {
    let _switch = MODULE_SWITCH.lock().map_err(|_| "module switch lock poisoned".to_string())?;
    if is_shutting_down() {
        return Err("the application is shutting down".to_string());
    }
    let registry = app.try_state::<registry::Registry>().ok_or("modules are not started yet")?;
    let module = registry.get(id).ok_or_else(|| format!("unknown module: {id}"))?;
    // Read the stored list before the module changes: a read that fails afterwards
    // would leave the module switched and the choice not saved.
    let mut disabled = db.get_disabled_modules_or_reset(|why| {
        log_line(format!("module {id}: the stored disabled list is damaged ({why}); starting a new one"));
    })?;
    if enabled {
        module.resume(module_context(app, id, true))?;
    } else {
        module.suspend()?;
    }
    disabled.retain(|m| m != id);
    if !enabled {
        disabled.push(id.to_string());
    }
    db.set_disabled_modules(&disabled)?;
    log_line(format!("module {id}: {}", if enabled { "on" } else { "off" }));
    Ok(())
}

#[tauri::command(async)]
fn set_module_enabled(
    app: tauri::AppHandle,
    state: tauri::State<Arc<UniDtkDatabase>>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    apply_module_enabled(&app, &state, &id, enabled)
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
        // Only the attach that raised the flag may lower it: a forced attach never
        // raised it, and clearing it would let a second loop start beside a running one.
        struct ResetFlag(bool);
        impl Drop for ResetFlag {
            fn drop(&mut self) {
                if self.0 {
                    ATTACH_IN_PROGRESS.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }
        let _guard = ResetFlag(!force);

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
mod shutdown_latch_tests {
    use super::ShutdownLatch;
    use std::time::{Duration, Instant};

    #[test]
    fn a_second_exit_route_waits_for_the_first() {
        static LATCH: ShutdownLatch = ShutdownLatch::new();
        assert!(LATCH.begin(), "the first caller runs the shutdown");
        assert!(!LATCH.begin(), "only once");
        let first = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(200));
            LATCH.finish();
        });
        let started = Instant::now();
        assert!(LATCH.wait(Duration::from_secs(5)), "the second caller saw the end");
        assert!(started.elapsed() >= Duration::from_millis(150), "returned before the first finished");
        first.join().unwrap();
    }

    #[test]
    fn the_wait_is_bounded() {
        static LATCH: ShutdownLatch = ShutdownLatch::new();
        assert!(LATCH.begin());
        assert!(!LATCH.wait(Duration::from_millis(50)), "a stuck first caller does not hold the second forever");
    }
}

#[cfg(test)]
mod frontend_contract_tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    /// Every `invoke("…")` in the interface must be served: by the kernel's
    /// `generate_handler!` or by a module's command table.
    ///
    /// Guards the failure mode where a command is renamed or deleted while the frontend
    /// keeps calling it — that surfaces at runtime as an empty widget, not at build
    /// time. It is deliberately a *test*: `generate_handler!` is a macro, so `build.rs`
    /// cannot read its contents structurally (see `docs/history/rfcs/udtk-migration.md`, H2).
    #[test]
    fn frontend_invoke_names_are_registered() {
        let mut registered = registered_commands();
        assert!(
            registered.len() > 10,
            "could not read the command list from generate_handler! (found {} entries)",
            registered.len()
        );
        let module_commands = crate::registry::all_module_commands();
        assert!(module_commands.len() > 100, "the modules declare their commands");
        registered.extend(module_commands.into_iter().map(|(_, spec)| spec.name.to_string()));

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

    /// Every kernel command the inventory promises on the UI door must exist as a Tauri
    /// command. A module's command is served by construction: its table generates the
    /// spec and the handler together.
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
                // Tauri's own events (`tauri://drag-drop`) are emitted by Tauri itself.
                if name.starts_with("tauri://") || known.contains(&name) {
                    continue;
                }
                orphans.push(format!("{}: listen(\"{name}\")", file.display()));
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
                Ok(Some(report)) => {
                    log_line(format!(
                        "library: migrated {} tracks / {} playlists / {} links into mods/mp3_player",
                        report.tracks, report.playlists, report.links
                    ));
                    if report.skipped_links > 0 {
                        log_line(format!(
                            "library: {} playlist link row(s) have keys of an unknown shape and were not migrated; the old tables stay in the file",
                            report.skipped_links
                        ));
                    }
                }
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
        // The Tauri door: a command a module declares goes to the module, every
        // other name to the kernel's own handlers.
        .invoke_handler({
            // Names the runtime the generated handler is for; alone it cannot be inferred.
            fn for_this_runtime<F: Fn(tauri::ipc::Invoke) -> bool>(handler: F) -> F {
                handler
            }
            let kernel_commands = for_this_runtime(tauri::generate_handler![
            session_info,
            trim_process_memory,
            // wallpaper-only
            attach_wallpaper,
            detach_wallpaper,
            set_clipboard_text,
            attach_main_window,
            detach_main_window,
            quit_app,
            start_menu_list,
            shortcut_icon,
            describe_shortcut_target,
            missing_targets,
            launch_shortcut,
            pick_shortcut_target,
            clipboard_shortcut_targets,
            frontend_log,
            log_list,
            log_clear,
            log_stats,
            explorer_tabs_get,
            explorer_tabs_set,
            explorer_sort_get,
            explorer_sort_set,
            get_widget_config,
            save_widget_config,
            get_disabled_modules,
            set_module_enabled,
            refresh_display_layout,
            get_folders,
            save_folder,
            delete_folder,
            weather_location_request,
            open_explorer,
            // core hotkeys & focus
            hotkeys_list,
            hotkey_toggle,
            set_desktop_input_focus,
            ]);
            move |invoke: tauri::ipc::Invoke| invoke_module_command(invoke).unwrap_or_else(|invoke| kernel_commands(invoke))
        })
        .setup(move |app| {
            log_line("setup: start");
            log_line(format!("startup: Tauri entered setup at {} ms", startup_started.elapsed().as_millis()));

            // The registry exists before anything can publish, exit or ask a module
            // for a command: building a module starts nothing, `init` below does.
            let registry = registry::Registry::new(registry::all_modules().into_iter().map(Arc::from).collect())?;
            app.manage(registry);

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
                        let folders_to_migrate = unidtk_api::paths::data_dir().join("folders");
                        if let Ok(migrated) = arc.auto_migrate_disk_folders(&folders_to_migrate) {
                            if migrated > 0 {
                                log_line(format!("setup: auto-migrated {migrated} folders from disk to redb"));
                            }
                        }

                        // Off the UI thread: resolving `.lnk` copies is COM work.
                        let shortcuts_db = arc.clone();
                        thread::spawn(move || match start_menu::migrate_copied_shortcuts(&shortcuts_db) {
                            Ok(0) => {}
                            Ok(n) => log_line(format!("setup: {n} folder shortcuts now point at their real targets")),
                            Err(e) => log_line(format!("setup: shortcut copy migration failed: {e}")),
                        });

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

            let disabled_mods = disabled_module_set(app.handle());

            let show = MenuItemBuilder::with_id("show", "Показать виджеты").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Выход").build(app)?;

            let menu = MenuBuilder::new(app)
                .item(&show)
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
                    "quit" => {
                        // The same path as the menu's Exit: it waits, bounded, for the
                        // elevated helper to stop the VPN. The wait sleeps, so it runs
                        // off the event loop.
                        let app = app.clone();
                        thread::spawn(move || {
                            let _ = quit_app(app);
                        });
                    }
                    _ => {}
                })
                .build(app)?;

            // Windows session-end listener (WM_QUERYENDSESSION / WM_ENDSESSION):
            // the wallpaper window is a WS_CHILD of WorkerW and never receives
            // those broadcasts, so a separate hidden top-level window listens
            // and stops the VPN synchronously inside the shutdown budget.
            shutdown::install(app.handle().clone());
            shutdown::guard_event_loop();

            // Modules start through the `unidtk-api` contract, in the registry's
            // order: the core gives each one its own database slice and AppHandle.
            // mp3-player loads its library on a thread of its own, so the slow load
            // stays off the wallpaper attach; until then its commands answer "not ready".
            for module in app.state::<registry::Registry>().in_start_order() {
                let id = module.id();
                module.init(module_context(app.handle(), id, !disabled_mods.contains(id)))?;
                log_line(format!("setup: module {id} started"));
            }

            // Localhost IPC server for CLI/agent debugging (r[ipc.socket-read-only]):
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
