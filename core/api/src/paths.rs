//! UniDTK paths: where the **program** lives and where the **user data** lives.
//!
//! The split follows the Windows convention exactly:
//!
//! * **program**: `C:\Program Files\UniDTK`, holding the executables and native
//!   dependencies (`unidtk.exe`, `udtk.exe`, `vpn-helper.exe`, `sing-box.exe`,
//!   DLLs, AI DJ models). The directory is read-only: the application has no right
//!   to write **anything** next to itself;
//! * **data**: `%LOCALAPPDATA%\UniDTK`: databases, logs, cache, the Telegram session,
//!   the music library. Everything mutable lives only here.
//!
//! Why this is a contract and not core knowledge: these are pure functions of the environment,
//! so the core and every module get the same answer. A module does not need to know
//! the layout, and moving the directories affects no module.

use std::path::PathBuf;

/// The program directory: next to `unidtk.exe`.
///
/// It is only **read** from here (DLLs, sing-box, the helper, models). Under Program Files
/// the directory is read-only, so any write here is a design error.
pub fn install_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The user data directory: `%LOCALAPPDATA%\UniDTK`, or the current directory if the
/// known Windows folders are unavailable.
///
/// Resolved via the Windows known-folder API (`dirs`), not via the `%LOCALAPPDATA%`
/// variable: scheduler tasks launch processes in which this variable is not guaranteed.
pub fn data_dir() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("UniDTK");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Derived data that can be deleted as a whole:
/// `%LOCALAPPDATA%\UniDTK\cache`. Module stores declared as
/// [`unidtk_api::StoreKind::OwnedCache`] are placed here.
pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

/// Logs: `%LOCALAPPDATA%\UniDTK\logs`.
pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

/// The default music library: `%LOCALAPPDATA%\UniDTK\music`.
///
/// The directory can be overridden in the player config (`library_dir`): this is user
/// content, and keeping it strictly inside the application data is the owner's decision,
/// not a Windows requirement.
pub fn music_dir() -> PathBuf {
    data_dir().join("music")
}

/// Models (ONNX for AI DJ), in case the application did not receive them from the installer.
pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// The sing-box binary: **only** next to the program.
///
/// There are no fallbacks any more: previously a third candidate was `sing-box.exe` from
/// a third-party program's bundle (`C:\Program Files\FlyFrogLLC\Happ\...`), so our VPN
/// subsystem could silently start on someone else's binary.
/// `sing-box.exe` must arrive with the installer, together with the application.
pub fn singbox_exe() -> PathBuf {
    install_dir().join("sing-box.exe")
}

/// ByeDPI (`ciadpi.exe`): local DPI bypass for the telegram module. Only
/// next to the program, with no PATH fallback: the application must ship its own
/// copy.
pub fn byedpi_exe() -> PathBuf {
    install_dir().join("ciadpi.exe")
}

/// The native VPN helper: next to the program, like the other binaries.
/// Scheduler tasks launch it from the same place (elevated).
pub fn vpn_helper_exe() -> PathBuf {
    install_dir().join("vpn-helper.exe")
}

/// Passive view of the data directory path WITHOUT calling create_dir_all or migration.
/// Used by diagnostics to honor the Zero Disk Pollution rule.
pub fn data_dir_view() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("UniDTK")
}

/// The canonical lookup for a binary or DLL: checks install_dir, target/{release,debug}
/// and the source-library folders in the checkout (opt/).
pub fn resolve_binary(name: &str) -> Option<PathBuf> {
    let install = install_dir();
    let direct = install.join(name);
    if direct.is_file() {
        return Some(direct);
    }

    let mut curr = install;
    for _ in 0..5 {
        for sub in ["target/release", "target/debug"] {
            let p = curr.join(sub).join(name);
            if p.is_file() {
                return Some(p);
            }
        }
        for opt in [
            "opt",
            "opt/singbox",
            "opt/byedpi",
            "opt/libmpv",
            "opt/tdlib-native/win32-x64",
        ] {
            let p = curr.join(opt).join(name);
            if p.is_file() {
                return Some(p);
            }
        }
        if !curr.pop() {
            break;
        }
    }
    None
}

/// The canonical lookup for a model or asset file: checks install/models,
/// data/models and opt/models.
pub fn resolve_asset(relative_path: &str) -> Option<PathBuf> {
    let install = install_dir();
    let direct = install.join("models").join(relative_path);
    if direct.is_file() {
        return Some(direct);
    }

    let data = data_dir_view().join("models").join(relative_path);
    if data.is_file() {
        return Some(data);
    }

    let mut curr = install;
    for _ in 0..5 {
        let p = curr.join("opt/models").join(relative_path);
        if p.is_file() {
            return Some(p);
        }
        if !curr.pop() {
            break;
        }
    }
    None
}
