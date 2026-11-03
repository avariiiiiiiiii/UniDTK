//! Glyph Bar — system master volume (WASAPI IAudioEndpointVolume) for the LED strip.
//!
//! Rust half of the `glyph-bar` vertical slice: logs via `unidtk_api::log` and
//! publishes `system-volume` via `unidtk_api::emit`. No dependency on the kernel
//! crate, none on other modules.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use serde::Serialize;

const POLL: Duration = Duration::from_millis(120);
/// Polls between looking up the default device again: about two seconds.
const DEVICE_RECHECK_POLLS: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemVolume {
    /// 0–100 master level (scalar × 100), independent of mute flag.
    pub level: u8,
    pub muted: bool,
}

pub struct VolumeState {
    stop: Arc<AtomicBool>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl VolumeState {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut threads) = self.threads.lock() {
            for thread in threads.drain(..) {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for VolumeState {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start() -> VolumeState {
    let stop = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();

    if let Ok(thread) = thread::Builder::new()
        .name("system-volume".to_string())
        .spawn({
            let stop = stop.clone();
            move || poll_loop(stop)
        })
    {
        threads.push(thread);
    }

    VolumeState {
        stop,
        threads: Mutex::new(threads),
    }
}

/// One COM session and one endpoint serve every poll; only the default-device
/// lookup repeats, so a change of output device is picked up within seconds.
#[cfg(windows)]
fn poll_loop(stop: Arc<AtomicBool>) {
    let _com = match ComSession::enter() {
        Ok(com) => com,
        Err(error) => {
            unidtk_api::log(format!("glyph-bar: {error}"));
            return;
        }
    };
    let mut endpoint = None;
    let mut polls = 0u32;
    let mut last: Option<SystemVolume> = None;
    while !stop.load(Ordering::Relaxed) {
        if endpoint.is_none() || polls % DEVICE_RECHECK_POLLS == 0 {
            endpoint = match default_endpoint() {
                Ok(current) => Some(current),
                Err(error) => {
                    unidtk_api::log(format!("glyph-bar: volume poll error: {error}"));
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }
            };
        }
        polls = polls.wrapping_add(1);
        let reading = endpoint.as_ref().map(read_endpoint).expect("endpoint opened above");
        match reading {
            Ok(current) => {
                if last != Some(current) {
                    last = Some(current);
                    unidtk_api::emit("system-volume", serde_json::json!(current));
                }
            }
            Err(error) => {
                unidtk_api::log(format!("glyph-bar: volume poll error: {error}"));
                endpoint = None;
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        }
        thread::sleep(POLL);
    }
}

#[cfg(not(windows))]
fn poll_loop(_stop: Arc<AtomicBool>) {}

// IPC commands are declared by the kernel, not here: `#[tauri::command]` places
// a `__cmd__<name>` macro at the crate root and must live in the same crate as
// `generate_handler!` (marking it `pub` in a library crate makes the macro
// collide with itself — E0255). The module exposes plain functions; the kernel
// wraps them. That also keeps this crate free of any tauri dependency.

pub fn get_system_volume() -> Result<SystemVolume, String> {
    read_volume()
}

pub fn set_system_volume(level: u8) -> Result<SystemVolume, String> {
    let clamped = level.min(100);
    with_endpoint(|endpoint| unsafe {
        // Setting a non-zero level while muted should unmute (OS may keep mute
        // on pure scalar set — clear mute explicitly when raising above 0).
        endpoint
            .SetMasterVolumeLevelScalar(clamped as f32 / 100.0, std::ptr::null())
            .map_err(|e| format!("SetMasterVolumeLevelScalar: {e}"))?;
        if clamped > 0 {
            endpoint
                .SetMute(false, std::ptr::null())
                .map_err(|e| format!("SetMute(false): {e}"))?;
        }
        Ok(())
    })?;
    read_volume()
}

pub fn set_system_mute(muted: bool) -> Result<SystemVolume, String> {
    with_endpoint(|endpoint| unsafe {
        endpoint
            .SetMute(muted, std::ptr::null())
            .map_err(|e| format!("SetMute: {e}"))
    })?;
    read_volume()
}

fn read_volume() -> Result<SystemVolume, String> {
    with_endpoint(read_endpoint)
}

#[cfg(windows)]
fn read_endpoint(endpoint: &windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume) -> Result<SystemVolume, String> {
    unsafe {
        let scalar = endpoint
            .GetMasterVolumeLevelScalar()
            .map_err(|e| format!("GetMasterVolumeLevelScalar: {e}"))?;
        let muted = endpoint
            .GetMute()
            .map_err(|e| format!("GetMute: {e}"))?
            .as_bool();
        let level = (scalar * 100.0).round().clamp(0.0, 100.0) as u8;
        Ok(SystemVolume { level, muted })
    }
}

#[cfg(not(windows))]
fn read_endpoint(_endpoint: &()) -> Result<SystemVolume, String> {
    Err("system volume is only supported on Windows".into())
}

/// COM for the current thread, released on drop if this session started it.
#[cfg(windows)]
struct ComSession {
    initialized: bool,
}

#[cfg(windows)]
impl ComSession {
    fn enter() -> Result<Self, String> {
        use windows::Win32::{
            Foundation::RPC_E_CHANGED_MODE,
            System::Com::{CoInitializeEx, COINIT_MULTITHREADED},
        };
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(format!("CoInitializeEx failed: {hr:?}"));
        }
        Ok(Self { initialized: hr.is_ok() })
    }
}

#[cfg(windows)]
impl Drop for ComSession {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

#[cfg(windows)]
fn default_endpoint() -> Result<windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume, String> {
    use windows::Win32::{
        Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator},
        System::Com::{CoCreateInstance, CLSCTX_ALL},
    };
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|e| format!("CoCreateInstance MMDeviceEnumerator: {e}"))?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("GetDefaultAudioEndpoint: {e}"))?;
        device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("Activate IAudioEndpointVolume: {e}"))
    }
}

#[cfg(windows)]
fn with_endpoint<T>(f: impl FnOnce(&windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume) -> Result<T, String>) -> Result<T, String> {
    let _com = ComSession::enter()?;
    let endpoint = default_endpoint()?;
    f(&endpoint)
}

#[cfg(not(windows))]
fn with_endpoint<T>(_f: impl FnOnce(&()) -> Result<T, String>) -> Result<T, String> {
    Err("system volume is only supported on Windows".into())
}

/// Glyph-bar module: the kernel creates and stops it through the
/// `unidtk-api` contract; its state lives inside the module.
pub struct GlyphBarModule {
    state: std::sync::Mutex<Option<VolumeState>>,
}

impl GlyphBarModule {
    pub fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(None),
        }
    }
}

pub fn create_module() -> Box<dyn unidtk_api::Module> {
    Box::new(GlyphBarModule::new())
}

/// Answers the socket commands this module owns.
///
/// The kernel registers this function under the module id and calls it instead of
/// naming this crate's types itself. `host` is unused here: the volume endpoint needs
/// no application state, which is exactly why this module can stay Tauri-free.
pub fn ipc_call(
    cmd: &str,
    args: &serde_json::Value,
    _host: Option<unidtk_api::commands::HostHandle<'_>>,
) -> Option<Result<serde_json::Value, String>> {
    match cmd {
        // The name must equal the manifest's, because the kernel addresses a module by
        // the name it read from the inventory: while these read different things
        // (`volume_get` here, `get_system_volume` in the kernel), the command failed as
        // "declared in the inventory but has no handler".
        "get_system_volume" => Some(
            get_system_volume()
                .and_then(|volume| serde_json::to_value(volume).map_err(|e| e.to_string())),
        ),
        // The writes answer with the same shape as the Tauri commands of the same name:
        // the caller gets the resulting state, so the effect is readable, not assumed.
        "set_system_volume" => Some(json_arg(args, "level").and_then(|level| {
            set_system_volume(volume_level(level))
                .and_then(|volume| serde_json::to_value(volume).map_err(|e| e.to_string()))
        })),
        "set_system_mute" => Some(
            args.get("muted")
                .and_then(|value| value.as_bool())
                .ok_or_else(|| "set_system_mute requires {muted}".to_string())
                .and_then(|muted| {
                    set_system_mute(muted)
                        .and_then(|volume| serde_json::to_value(volume).map_err(|e| e.to_string()))
                }),
        ),
        _ => None,
    }
}

/// Reads one numeric argument, or says which one is missing.
/// Clamps before narrowing: `300 as u8` would wrap to 44, not stop at 100.
fn volume_level(level: u64) -> u8 {
    level.min(100) as u8
}

fn json_arg(args: &serde_json::Value, name: &str) -> Result<u64, String> {
    args.get(name)
        .and_then(|value| value.as_u64())
        .ok_or_else(|| format!("this command requires the argument {{{name}}}"))
}

impl unidtk_api::Module for GlyphBarModule {
    fn id(&self) -> &'static str {
        "glyph-bar"
    }

    fn dependencies(&self) -> &'static [unidtk_api::manifest::ModuleDependency] {
        &[]
    }

    fn init(&self, ctx: unidtk_api::ModuleContext) -> Result<(), String> {
        if ctx.is_diagnostic() {
            // Diagnostic mode: check that WASAPI is available without starting a background thread
            let _ = get_system_volume();
            return Ok(());
        }
        if !ctx.enabled() {
            unidtk_api::log("glyph-bar: disabled — volume poll not started");
            return Ok(());
        }
        *self.state.lock().expect("glyph-bar module lock") = Some(start());
        Ok(())
    }

    fn shutdown(&self, _budget: std::time::Duration) -> Result<(), String> {
        if let Some(state) = self.state.lock().expect("glyph-bar module lock").as_ref() {
            state.stop();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::volume_level;

    #[test]
    fn volume_clamps_before_it_narrows() {
        assert_eq!(volume_level(0), 0);
        assert_eq!(volume_level(55), 55);
        assert_eq!(volume_level(100), 100);
        assert_eq!(volume_level(256), 100);
        assert_eq!(volume_level(300), 100);
        assert_eq!(volume_level(u64::MAX), 100);
    }
}
