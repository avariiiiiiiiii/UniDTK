//! Register as the current-user default file manager.
//!
//! This follows what Files, File Pilot, OneCommander and Total Commander
//! actually do — **HKCU\Software\Classes**, never HKCR:
//!   * `Directory` / `Drive` `shell\open\command` for double-click
//!   * empty `DelegateExecute` so Explorer's COM handler is skipped
//!   * CLSID `{52205fd8-…}` `OpenNewWindow` for Win+E / taskbar File Explorer
//!   * `Directory\Background` for "open this folder" from empty space
//!
//! HKCR writes need admin and can brick the machine-wide shell; HKCU overlays
//! them for this user only and is what those apps use in production.

use windows::core::PCWSTR;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, REG_SZ,
};

const DIR_OPEN: &str = r"HKCU\Software\Classes\Directory\shell\open";
const DIR_OPEN_CMD: &str = r"HKCU\Software\Classes\Directory\shell\open\command";
const DRIVE_OPEN: &str = r"HKCU\Software\Classes\Drive\shell\open";
const DRIVE_OPEN_CMD: &str = r"HKCU\Software\Classes\Drive\shell\open\command";
const FOLDER_OPEN: &str = r"HKCU\Software\Classes\Folder\shell\open";
const FOLDER_OPEN_CMD: &str = r"HKCU\Software\Classes\Folder\shell\open\command";
const FOLDER_EXPLORE: &str = r"HKCU\Software\Classes\Folder\shell\explore";
const FOLDER_EXPLORE_CMD: &str = r"HKCU\Software\Classes\Folder\shell\explore\command";
const DIR_EXPLORE: &str = r"HKCU\Software\Classes\Directory\shell\explore";
const DIR_EXPLORE_CMD: &str = r"HKCU\Software\Classes\Directory\shell\explore\command";
const DRIVE_EXPLORE: &str = r"HKCU\Software\Classes\Drive\shell\explore";
const DRIVE_EXPLORE_CMD: &str = r"HKCU\Software\Classes\Drive\shell\explore\command";
const DIR_BG: &str = r"HKCU\Software\Classes\Directory\Background\shell\open";
const DIR_BG_CMD: &str = r"HKCU\Software\Classes\Directory\Background\shell\open\command";
const WIN_E: &str =
    r"HKCU\Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\OpenNewWindow";
const WIN_E_CMD: &str = r"HKCU\Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\OpenNewWindow\command";
const RECYCLE_OPEN: &str =
    r"HKCU\Software\Classes\CLSID\{645FF040-5081-101B-9F08-00AA002F954E}\shell\open";
const RECYCLE_OPEN_CMD: &str =
    r"HKCU\Software\Classes\CLSID\{645FF040-5081-101B-9F08-00AA002F954E}\shell\open\command";
const RECYCLE_EXPLORE: &str =
    r"HKCU\Software\Classes\CLSID\{645FF040-5081-101B-9F08-00AA002F954E}\shell\explore";
const RECYCLE_EXPLORE_CMD: &str =
    r"HKCU\Software\Classes\CLSID\{645FF040-5081-101B-9F08-00AA002F954E}\shell\explore\command";

fn exe_path() -> Result<String, String> {
    std::env::current_exe()
        .map_err(|e| format!("current_exe: {e}"))
        .map(|p| p.to_string_lossy().into_owned())
}

fn reg_set(key: &str, value_name: Option<&str>, data: &str) -> Result<(), String> {
    let rel_key = key.strip_prefix(r"HKCU\").unwrap_or(key);
    let wide_key: Vec<u16> = rel_key.encode_utf16().chain(Some(0)).collect();
    let wide_data: Vec<u16> = data.encode_utf16().chain(Some(0)).collect();

    unsafe {
        let mut hkey = HKEY::default();
        let status = RegCreateKeyW(
            HKEY_CURRENT_USER,
            PCWSTR(wide_key.as_ptr()),
            &mut hkey,
        );
        if status.is_err() {
            return Err(format!("RegCreateKeyW error on {rel_key}: {status:?}"));
        }

        let data_bytes = std::slice::from_raw_parts(
            wide_data.as_ptr() as *const u8,
            wide_data.len() * 2,
        );

        let set_res = if let Some(name) = value_name {
            let wide_val: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            RegSetValueExW(
                hkey,
                PCWSTR(wide_val.as_ptr()),
                0,
                REG_SZ,
                Some(data_bytes),
            )
        } else {
            RegSetValueExW(
                hkey,
                PCWSTR::null(),
                0,
                REG_SZ,
                Some(data_bytes),
            )
        };

        let _ = RegCloseKey(hkey);

        if set_res.is_err() {
            return Err(format!("RegSetValueExW on {rel_key}: {set_res:?}"));
        }
    }
    Ok(())
}

/// Create a Windows .lnk shortcut via native COM (IShellLinkW + IPersistFile).
/// Completely in-process, zero child processes spawned, zero console window flashing.
fn create_shortcut_native(
    shortcut_path: &std::path::Path,
    target: &str,
    arguments: &str,
    working_dir: &str,
    icon_path: &str,
    description: &str,
) -> Result<(), String> {
    use windows::core::{Interface, HSTRING, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, IPersistFile,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link_res: windows::core::Result<IShellLinkW> =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER);
        let link = match link_res {
            Ok(l) => l,
            Err(e) => {
                CoUninitialize();
                return Err(format!("CoCreateInstance ShellLink: {e}"));
            }
        };

        let h_target = HSTRING::from(target);
        let _ = link.SetPath(PCWSTR(h_target.as_ptr()));

        let h_args = HSTRING::from(arguments);
        let _ = link.SetArguments(PCWSTR(h_args.as_ptr()));

        let h_dir = HSTRING::from(working_dir);
        let _ = link.SetWorkingDirectory(PCWSTR(h_dir.as_ptr()));

        let h_icon = HSTRING::from(icon_path);
        let _ = link.SetIconLocation(PCWSTR(h_icon.as_ptr()), 0);

        let h_desc = HSTRING::from(description);
        let _ = link.SetDescription(PCWSTR(h_desc.as_ptr()));

        let persist: IPersistFile = match link.cast() {
            Ok(p) => p,
            Err(e) => {
                CoUninitialize();
                return Err(format!("cast IPersistFile: {e}"));
            }
        };

        let h_save = HSTRING::from(shortcut_path.to_string_lossy().as_ref());
        let save_res = persist.Save(PCWSTR(h_save.as_ptr()), true);
        CoUninitialize();

        save_res.map_err(|e| format!("IPersistFile::Save: {e}"))?;
    }
    unidtk_api::log(format!(
        "explorer: Start menu shortcut installed at {}",
        shortcut_path.display()
    ));
    Ok(())
}

/// Add a per-user Start menu entry. Windows indexes `.lnk` files placed in
/// FOLDERID_Programs (`%APPDATA%\Microsoft\Windows\Start Menu\Programs`).
fn install_start_menu_shortcut(exe: &str) -> Result<(), String> {
    let programs = dirs::data_dir()
        .ok_or_else(|| "cannot resolve roaming AppData".to_string())?
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs");
    std::fs::create_dir_all(&programs)
        .map_err(|e| format!("create Start menu Programs directory: {e}"))?;

    let shortcut = programs.join("UniDTK Explorer.lnk");
    let exe_path = std::path::Path::new(exe);
    let exe_dir = exe_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let working_dir = exe_dir.to_string_lossy().into_owned();

    create_shortcut_native(
        &shortcut,
        exe,
        "open",
        &working_dir,
        exe,
        "Open UniDTK Explorer inside the desktop shell",
    )
}

/// Overlay HKCU so folder/drive/Win+E open this app. Idempotent, no admin.
pub fn install_as_default_file_manager() -> Result<(), String> {
    let exe = exe_path()?;
    let open_path = format!("\"{exe}\" open \"%1\"");
    let open_here = format!("\"{exe}\" open \"%V\"");
    let open_this_pc = format!("\"{exe}\" open");
    let open_recycle = format!("\"{exe}\" open \"::{{645FF040-5081-101B-9F08-00AA002F954E}}\"");

    let mut errors = Vec::new();
    // Empty DelegateExecute on both the verb and the command key disables
    // Explorer's IExecuteCommand so our command string is used (Files #16111).
    let mut set_open = |verb_key: &str, cmd_key: &str, command: &str| {
        if let Err(e) = reg_set(verb_key, Some("DelegateExecute"), "") {
            errors.push(e);
        }
        if let Err(e) = reg_set(cmd_key, None, command) {
            errors.push(e);
        }
        if let Err(e) = reg_set(cmd_key, Some("DelegateExecute"), "") {
            errors.push(e);
        }
    };

    set_open(DIR_OPEN, DIR_OPEN_CMD, &open_path);
    set_open(DRIVE_OPEN, DRIVE_OPEN_CMD, &open_path);
    // Folder is the generic Shell folder ProgID (including many virtual-folder
    // launches), while the explore verbs cover callers that explicitly request
    // `explore` instead of the default `open` verb.
    set_open(FOLDER_OPEN, FOLDER_OPEN_CMD, &open_path);
    set_open(FOLDER_EXPLORE, FOLDER_EXPLORE_CMD, &open_path);
    set_open(DIR_EXPLORE, DIR_EXPLORE_CMD, &open_path);
    set_open(DRIVE_EXPLORE, DRIVE_EXPLORE_CMD, &open_path);
    set_open(DIR_BG, DIR_BG_CMD, &open_here);
    set_open(WIN_E, WIN_E_CMD, &open_this_pc);
    // Recycle Bin is a virtual shell folder and its command is sometimes
    // invoked without a useful `%1`, so pass the CLSID explicitly.
    set_open(RECYCLE_OPEN, RECYCLE_OPEN_CMD, &open_recycle);
    set_open(RECYCLE_EXPLORE, RECYCLE_EXPLORE_CMD, &open_recycle);

    if let Err(e) = install_start_menu_shortcut(&exe) {
        errors.push(e);
    }

    if errors.is_empty() {
        unidtk_api::log("explorer: HKCU default file manager registered");
        Ok(())
    } else {
        let joined = errors.join("; ");
        unidtk_api::log(format!("explorer: registry install partial: {joined}"));
        Err(joined)
    }
}

/// Remove HKCU shell overrides so native Explorer handles drives and folders.
#[allow(dead_code)]
pub fn uninstall_as_default_file_manager() -> Result<(), String> {
    let keys_to_delete = [
        r"Software\Classes\Directory\shell\open",
        r"Software\Classes\Directory\shell\explore",
        r"Software\Classes\Drive\shell\open",
        r"Software\Classes\Drive\shell\explore",
        r"Software\Classes\Folder\shell\open",
        r"Software\Classes\Folder\shell\explore",
        r"Software\Classes\Directory\Background\shell\open",
        r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}",
        r"Software\Classes\CLSID\{645FF040-5081-101B-9F08-00AA002F954E}",
    ];

    for key in keys_to_delete {
        let wide: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr()));
        }
    }
    unidtk_api::log("explorer: HKCU default file manager unregistered");
    Ok(())
}
