//! Explicit UI permission request. The wallpaper never leaves WorkerW.
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use tauri::Manager;

static IN_FLIGHT: AtomicBool = AtomicBool::new(false);
const LABEL: &str = "weather-location-permission";

#[cfg(windows)]
fn foreground_is_ours() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid)); }
    pid == std::process::id()
}

#[cfg(not(windows))]
fn foreground_is_ours() -> bool { false }

fn complete(app: tauri::AppHandle, status: &'static str) {
    if let Some(runtime) = app.try_state::<crate::micrographics::WeatherRuntime>() {
        if let Err(error) = runtime.set_location_access(status) {
            crate::log_line(format!("weather: location permission update failed: {error}"));
        }
    }
    let cleanup = app.clone();
    if app.run_on_main_thread(move || {
        if let Some(window) = cleanup.get_webview_window(LABEL) { let _ = window.close(); }
        IN_FLIGHT.store(false, Ordering::Release);
    }).is_err() { IN_FLIGHT.store(false, Ordering::Release); }
}

fn dispatch_begin(app: tauri::AppHandle, started: Arc<AtomicBool>, cancel: Arc<AtomicBool>) {
    let ui_app = app.clone();
    let dispatch_started = started.clone();
    if app.run_on_main_thread(move || begin(ui_app, started, cancel)).is_err() {
        dispatch_started.store(true, Ordering::Release);
        complete(app, "unavailable");
    }
}

fn begin(app: tauri::AppHandle, started: Arc<AtomicBool>, cancel: Arc<AtomicBool>) {
    if !foreground_is_ours() || cancel.load(Ordering::Acquire) ||
        started.swap(true, Ordering::AcqRel) { return; }
    let done = app.clone();
    if let Err(error) = crate::micrographics::request_location_access_on_ui(cancel, move |status| complete(done, status)) {
        crate::log_line(format!("weather: location permission request failed: {error}"));
        complete(app, "unavailable");
    }
}

pub fn request(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    if IN_FLIGHT.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return Err("Запрос местоположения уже выполняется".into());
    }
    let status = app.try_state::<crate::micrographics::WeatherRuntime>()
        .ok_or_else(|| "micrographics is not ready".to_string())
        .and_then(|runtime| runtime.set_location_access("requesting"));
    if let Err(error) = status { IN_FLIGHT.store(false, Ordering::Release); return Err(error); }
    let ui_app = app.clone();
    // WebviewWindowBuilder can deadlock on Windows inside event/UI callbacks.
    // Build on this worker; only the WinRT permission initiation uses the UI thread.
    if let Err(error) = std::thread::Builder::new().name("weather-permission-host".into()).spawn(move || {
        let started = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let timeout_app = ui_app.clone(); let timeout_started = started.clone(); let timeout_cancel = cancel.clone();
        // Failure to obtain foreground must also terminate a request.
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(15));
            if !timeout_started.swap(true, Ordering::AcqRel) {
                timeout_cancel.store(true, Ordering::Release); complete(timeout_app, "unavailable");
            }
        });
        if foreground_is_ours() { dispatch_begin(ui_app, started, cancel); return; }
        // A readable foreground host is needed for WinRT's consent surface.
        let window = match tauri::WebviewWindowBuilder::new(&ui_app, LABEL,
            tauri::WebviewUrl::App("modules/micrographics/location-permission.html".into()))
            .title("UniDTK — местоположение для погоды").inner_size(440.0, 250.0)
            .resizable(false).center().visible(false).build() {
            Ok(window) => window,
            Err(error) => { crate::log_line(format!("weather: permission window failed: {error}"));
                if !started.swap(true, Ordering::AcqRel) { complete(ui_app, "unavailable"); } return; }
        };
        if cancel.load(Ordering::Acquire) { let _ = window.close(); return; }
        let event_app = ui_app.clone(); let event_started = started.clone(); let event_cancel = cancel.clone();
        window.on_window_event(move |event| match event {
            tauri::WindowEvent::Focused(true) => dispatch_begin(event_app.clone(), event_started.clone(), event_cancel.clone()),
            tauri::WindowEvent::CloseRequested { .. } => {
                event_cancel.store(true, Ordering::Release);
                if !event_started.swap(true, Ordering::AcqRel) { complete(event_app.clone(), "unavailable"); }
            }
            _ => {}
        });
        let _ = window.show(); let _ = window.set_focus();
        dispatch_begin(ui_app.clone(), started.clone(), cancel.clone());
    }) {
        complete(app, "unavailable"); return Err(error.to_string());
    }
    Ok(serde_json::json!({ "accepted": true }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_the_wallpaper_survives_a_close_request() {
        assert!(crate::hides_on_close("wallpaper"));
        assert!(!crate::hides_on_close(super::LABEL));
    }
}
