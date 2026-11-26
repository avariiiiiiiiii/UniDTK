//! Native VPN helper: part of the `vpn` module, not a separate utility.
//!
//! A separate process is needed for exactly one reason: creating the TUN adapter requires
//! administrator rights, while the shell itself must stay unelevated (otherwise the window
//! cannot be re-parented under `WorkerW` — see `toolchain/build.rs`). So this is a second
//! binary of the same module, not a foreign tool: its source lives next to the module
//! (`modules/vpn/helper/`), and it is built by the single manifest `toolchain/Cargo.toml`.
//!
//! This also replaces the previous chain
//! `wscript.exe → vpn_hidden_launch.vbs → powershell.exe → vpn_stop.ps1`,
//! which took several seconds to start (WMI/.NET) and did not fit within the Windows
//! shutdown time budget.
//!
//! It is launched by the `UniDTKSingbox` / `UniDTKVpnStop` scheduler tasks with a single
//! argument:
//! * `start` — launch `sing-box.exe` with `singbox_config.json` as a separate process
//!   (no window, survives this process) and remember its PID;
//! * `stop`  — kill the remembered `sing-box` (verifying the process image and PID so that
//!   nothing else is affected) and write the same nonce to `vpn_stop.done` that the
//!   application-side protocol expects.
//!
//! Layout: binaries (`sing-box.exe`, itself) come from the program directory
//! ([`unidtk_api::paths::install_dir`]); state (PID, config, markers) lives in the
//! data directory ([`unidtk_api::paths::data_dir`]).
#![windows_subsystem = "windows"]

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;

/// VPN state: sing-box config, PID, stop markers.
fn data_dir() -> PathBuf {
    unidtk_api::paths::data_dir()
}

/// The sing-box binary, located next to the program like its other binaries.
fn singbox_exe() -> PathBuf {
    unidtk_api::paths::singbox_exe()
}

/// Where a log may grow before it is moved to `<name>.1` (the previous `.1` goes).
const LOG_LIMIT: u64 = 1024 * 1024;
/// `singbox.log` is moved aside on the next start past this size: it had reached 15 MB,
/// and the kernel reads its tail after every failed connect.
const CORE_LOG_LIMIT: u64 = 5 * 1024 * 1024;

/// Whether this elevated process may write into `dir`: it lives in the user's own
/// profile, so a junction or symlink there would point an administrator's write
/// anywhere on the disk. A missing directory is created first; a link is refused.
fn safe_dir(dir: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let _ = std::fs::create_dir_all(dir);
    let data = data_dir();
    // The data directory and everything between it and `dir` must be plain folders.
    let mut checked = Some(dir);
    while let Some(path) = checked {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 && meta.is_dir() => {}
            _ => return false,
        }
        if path == data.as_path() {
            break;
        }
        checked = path.parent().filter(|parent| parent.starts_with(&data));
    }
    true
}

/// Moves `path` to `<path>.1` once it is larger than `limit`.
fn rotate(path: &Path, limit: u64) {
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > limit) {
        let mut aside = path.as_os_str().to_owned();
        aside.push(".1");
        let _ = std::fs::rename(path, PathBuf::from(aside));
    }
}

/// One line per event, with the local time: without it a stop could not be placed
/// against the application's own log, and one nobody asked for could not be found.
fn helper_log(message: &str) {
    let dir = unidtk_api::paths::logs_dir();
    if !safe_dir(&dir) {
        return;
    }
    let path = dir.join("vpn_helper.log");
    rotate(&path, LOG_LIMIT);
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let _ = writeln!(file, "{now} [{}] {message}", std::process::id());
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
    // The core runs elevated and writes its log into the user's data directory.
    let logs = unidtk_api::paths::logs_dir();
    if !safe_dir(&dir) || !safe_dir(&logs) {
        helper_log("start: the data directory is a link; refusing to start an elevated core");
        return 1;
    }
    rotate(&dir.join("singbox.log"), CORE_LOG_LIMIT);
    let stderr_path = logs.join("singbox_stderr.log");
    rotate(&stderr_path, LOG_LIMIT);
    // A panic of the core goes to stderr, not to its own log: keep it.
    let stderr = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_path)
        .map(Stdio::from)
        .unwrap_or_else(|_| Stdio::null());

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
        .stderr(stderr)
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
    if !safe_dir(&dir) {
        helper_log("stop: the data directory is a link; refusing to write its markers");
        return 1;
    }
    let request = std::fs::read_to_string(dir.join("vpn_stop.cutoff")).unwrap_or_default();
    let mut lines = request.lines().map(str::trim);
    let nonce = lines.next().unwrap_or_default().to_string();
    // The application writes its request (`<its pid>-<n>`, then the time) just before it
    // runs this task. A request older than that was not written for this run: someone
    // ran the task or this helper by hand.
    let asked = lines
        .next()
        .and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
        .map(|time| chrono::Utc::now().signed_duration_since(time).num_seconds());
    match asked {
        Some(age) if age <= 15 => helper_log(&format!("stop: requested by the app, nonce {nonce} ({age} s ago)")),
        Some(age) => helper_log(&format!("stop: NOT requested by the app: the last request ({nonce}) is {age} s old")),
        None => helper_log("stop: NOT requested by the app: no request on file"),
    }

    if let Some(pid) = read_pid(&dir) {
        if process_is_singbox(pid) {
            match terminate(pid) {
                Ok(()) => helper_log(&format!("stop: terminated sing-box pid {pid}")),
                Err(error) => helper_log(&format!("stop: could not terminate sing-box pid {pid}: {error}")),
            }
        } else {
            helper_log(&format!("stop: pid {pid} is not sing-box; not killing"));
        }
        let _ = std::fs::remove_file(dir.join("vpn_core.pid"));
    } else {
        helper_log("stop: no recorded pid (nothing to kill)");
    }
    // A core can outlive its PID file: the helper may die between `spawn` and the
    // write, or the app is killed first. Its TUN then black-holes the network with
    // no record left to kill, so the sweep is the only recovery. Matched by the
    // exact image path, so nothing else is ever touched.
    let swept = sweep_orphaned_cores();
    if swept > 0 {
        helper_log(&format!("stop: swept {swept} orphaned sing-box process(es)"));
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

/// Every running process whose image is our bundled `sing-box.exe`, terminated.
/// The PID file covers the normal case; this covers a core that outlived it.
#[cfg(windows)]
fn sweep_orphaned_cores() -> usize {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let exe = singbox_exe();
    let wanted = exe.to_string_lossy().to_ascii_lowercase();
    let mut swept = 0;
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return 0;
        };
        let mut entry = PROCESSENTRY32W::default();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = Process32FirstW(snapshot, &mut entry).is_ok();
        while found {
            let pid = entry.th32ProcessID;
            if pid != 0 && pid != std::process::id() && process_image_is(pid, &wanted) {
                match terminate(pid) {
                    Ok(()) => swept += 1,
                    Err(error) => helper_log(&format!("stop: could not sweep sing-box pid {pid}: {error}")),
                }
            }
            found = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    swept
}

#[cfg(not(windows))]
fn sweep_orphaned_cores() -> usize {
    0
}

/// Whether `pid`'s image is exactly the path `wanted` names.
#[cfg(windows)]
fn process_image_is(pid: u32, wanted: &str) -> bool {
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
        result.is_ok() && String::from_utf16_lossy(&buffer[..size as usize]).to_ascii_lowercase() == wanted
    }
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

/// Kills `pid`; an error says why it is still running (or never was).
#[cfg(windows)]
fn terminate(pid: u32) -> Result<(), String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_TERMINATE,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .map_err(|error| format!("OpenProcess: {error}"))?;
        let killed = TerminateProcess(handle, 0).map_err(|error| format!("TerminateProcess: {error}"));
        // Bounded wait: the caller (elevated task) must not linger, and the
        // app side re-verifies through the port/TUN checks anyway.
        let _ = WaitForSingleObject(handle, 2000);
        let _ = CloseHandle(handle);
        killed
    }
}
