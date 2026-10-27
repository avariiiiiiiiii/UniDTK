//! Нативный VPN-хелпер — часть модуля `vpn`, а не отдельная утилита.
//!
//! Отдельный процесс нужен ровно за одним: создание TUN-адаптера требует прав
//! администратора, а сама оболочка обязана оставаться unelevated (иначе окно
//! нельзя переподчинить `WorkerW` — см. `toolchain/build.rs`). Поэтому это
//! второй бинарник того же модуля, а не чужой инструмент: его источник лежит
//! рядом с модулем (`modules/vpn/helper/`), а собирает его единый манифест
//! `toolchain/Cargo.toml`.
//!
//! Это ещё и замена прежней цепочки
//! `wscript.exe → vpn_hidden_launch.vbs → powershell.exe → vpn_stop.ps1`,
//! которая стартовала секунды (WMI/.NET) и не укладывалась в бюджет выключения
//! Windows.
//!
//! Запускается задачами планировщика `UniDTKSingbox` / `UniDTKVpnStop` с одним
//! аргументом:
//! * `start` — поднять `sing-box.exe` с `singbox_config.json` отдельным
//!   процессом (без окна, переживает этот процесс) и запомнить его PID;
//! * `stop`  — снять запомненный `sing-box` (сверяя образ процесса и PID, чтобы
//!   не задеть чужое) и записать в `vpn_stop.done` тот же nonce, который
//!   протокол ожидает на стороне приложения.
//!
//! Раскладка: бинарники (`sing-box.exe`, он сам) — из каталога программы
//! ([`unidtk_api::paths::install_dir`]), состояние (PID, конфиг, маркеры) — в
//! каталоге данных ([`unidtk_api::paths::data_dir`]).
#![windows_subsystem = "windows"]

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;

/// Состояние VPN: конфиг sing-box, PID, маркеры остановки.
fn data_dir() -> PathBuf {
    unidtk_api::paths::data_dir()
}

/// Бинарник sing-box — рядом с программой, как и остальные её бинарники.
fn singbox_exe() -> PathBuf {
    unidtk_api::paths::singbox_exe()
}

fn helper_log(message: &str) {
    let dir = unidtk_api::paths::logs_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("vpn_helper.log"))
    {
        let _ = writeln!(file, "{message}");
    }
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    let code = match arg.as_str() {
        "start" => start(),
        "stop" => stop(),
        "probe" | "--probe" | "check" => 0,
        other => {
            helper_log(&format!("unknown mode: {other}"));
            2
        }
    };
    std::process::exit(code);
}

/// Spawn sing-box detached and remember its PID. Exits immediately: the core
/// must outlive this helper (exactly like the old VBS launch), and a resident
/// helper would keep the `UniDTKSingbox` task "running" forever.
fn start() -> i32 {
    let dir = data_dir();
    let exe = singbox_exe();
    let config = dir.join("singbox_config.json");
    if !exe.exists() {
        helper_log(&format!("start: sing-box not found at {}", exe.display()));
        return 1;
    }
    if !config.exists() {
        helper_log("start: singbox_config.json missing");
        return 1;
    }

    if let Some(pid) = read_pid(&dir) {
        if process_is_singbox(pid) {
            helper_log(&format!("start: sing-box pid {pid} already running"));
            return 0;
        }
    }

    match Command::new(&exe)
        .args(["run", "-c"])
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
    {
        Ok(child) => {
            let _ = std::fs::write(dir.join("vpn_core.pid"), child.id().to_string());
            helper_log(&format!("start: spawned sing-box pid {}", child.id()));
            0
        }
        Err(error) => {
            helper_log(&format!("start: spawn failed: {error}"));
            1
        }
    }
}

/// Kill the recorded sing-box and echo the stop nonce. Always writes the
/// done-marker: the app's waiter treats it as "the stop has run", and a hung
/// kill would otherwise stall every connect/disconnect for its full timeout.
fn stop() -> i32 {
    let dir = data_dir();
    let request = std::fs::read_to_string(dir.join("vpn_stop.cutoff")).unwrap_or_default();
    let nonce = request
        .lines()
        .next()
        .map(str::trim)
        .unwrap_or_default()
        .to_string();

    if let Some(pid) = read_pid(&dir) {
        if process_is_singbox(pid) {
            terminate(pid);
            helper_log(&format!("stop: terminated sing-box pid {pid}"));
        } else {
            helper_log(&format!("stop: pid {pid} is not sing-box; not killing"));
        }
        let _ = std::fs::remove_file(dir.join("vpn_core.pid"));
    } else {
        helper_log("stop: no recorded pid (nothing to kill)");
    }

    // Plain UTF-8, no BOM: the app compares the trimmed contents to the nonce.
    match std::fs::write(dir.join("vpn_stop.done"), &nonce) {
        Ok(()) => 0,
        Err(error) => {
            helper_log(&format!("stop: write done-marker failed: {error}"));
            1
        }
    }
}

fn read_pid(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(dir.join("vpn_core.pid"))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
}

#[cfg(windows)]
fn process_is_singbox(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        if result.is_err() {
            return false;
        }
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        path.to_ascii_lowercase().ends_with("sing-box.exe")
    }
}

#[cfg(windows)]
fn terminate(pid: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_TERMINATE,
    };
    unsafe {
        let Ok(handle) = OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        ) else {
            return;
        };
        let _ = TerminateProcess(handle, 0);
        // Bounded wait: the caller (elevated task) must not linger, and the
        // app side re-verifies through the port/TUN checks anyway.
        let _ = WaitForSingleObject(handle, 2000);
        let _ = CloseHandle(handle);
    }
}
