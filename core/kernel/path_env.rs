//! Adds UniDTK CLI (`udtk.exe`) to the current user's PATH in Windows Registry.

use std::path::{Path, PathBuf};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};

/// Ensure that the directory holding the app (and therefore `udtk.exe`) is
/// present in the User PATH environment variable (`HKCU\Environment`).
///
/// Ничего не копируется: CLI лежит рядом с программой, и это **единственная**
/// его копия. Раньше `udtk.exe` дублировался в `%LOCALAPPDATA%\UniDTK\bin`, а две
/// копии одного бинарника неизбежно расходятся по версиям — в PATH мог оказаться
/// вчерашний CLI при сегодняшнем приложении.
pub fn ensure_cli_in_path() -> Result<PathBuf, String> {
    let dir = unidtk_api::paths::install_dir();
    if !dir.join("udtk.exe").is_file() {
        return Err(format!(
            "udtk.exe is not next to the application ({})",
            dir.display()
        ));
    }
    add_directory_to_user_path(&dir)?;
    Ok(dir)
}

fn add_directory_to_user_path(target_dir: &Path) -> Result<bool, String> {
    let target_str = target_dir.to_str().ok_or("Invalid unicode in target directory path")?;
    let subkey_wide: Vec<u16> = "Environment\0".encode_utf16().collect();
    let value_name_wide: Vec<u16> = "Path\0".encode_utf16().collect();

    unsafe {
        let mut hkey = HKEY::default();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_wide.as_ptr()),
            0,
            KEY_READ | KEY_WRITE,
            &mut hkey,
        );
        if status.is_err() {
            return Err(format!("Failed to open HKCU\\Environment: {status:?}"));
        }

        // Query current Path value
        let mut val_type = REG_VALUE_TYPE::default();
        let mut data_size: u32 = 0;
        let query_size_status = RegQueryValueExW(
            hkey,
            PCWSTR(value_name_wide.as_ptr()),
            None,
            Some(&mut val_type),
            None,
            Some(&mut data_size),
        );

        let mut current_path = String::new();
        let mut is_expand_sz = true;

        if query_size_status.is_ok() && data_size > 0 {
            let mut buffer = vec![0u8; data_size as usize + 2];
            let query_status = RegQueryValueExW(
                hkey,
                PCWSTR(value_name_wide.as_ptr()),
                None,
                Some(&mut val_type),
                Some(buffer.as_mut_ptr()),
                Some(&mut data_size),
            );

            if query_status.is_ok() {
                let u16_slice = std::slice::from_raw_parts(
                    buffer.as_ptr() as *const u16,
                    (data_size as usize) / 2,
                );
                current_path = String::from_utf16_lossy(u16_slice)
                    .trim_matches('\0')
                    .to_string();
                if val_type == REG_SZ {
                    is_expand_sz = false;
                }
            }
        }

        // Check if target directory is already part of PATH
        let parts: Vec<&str> = current_path
            .split(';')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        for part in &parts {
            if part.eq_ignore_ascii_case(target_str) {
                let _ = RegCloseKey(hkey);
                return Ok(false); // Already present
            }
        }

        // Append target directory to PATH
        let new_path = if current_path.is_empty() {
            target_str.to_string()
        } else if current_path.ends_with(';') {
            format!("{current_path}{target_str}")
        } else {
            format!("{current_path};{target_str}")
        };

        let wide_data: Vec<u16> = new_path.encode_utf16().chain(Some(0)).collect();
        let set_type = if is_expand_sz { REG_EXPAND_SZ } else { REG_SZ };
        let set_status = RegSetValueExW(
            hkey,
            PCWSTR(value_name_wide.as_ptr()),
            0,
            set_type,
            Some(std::slice::from_raw_parts(
                wide_data.as_ptr() as *const u8,
                wide_data.len() * 2,
            )),
        );
        let _ = RegCloseKey(hkey);

        if set_status.is_err() {
            return Err(format!("Failed to set HKCU\\Environment\\Path: {set_status:?}"));
        }

        // Also update PATH in the current process
        if let Ok(proc_path) = std::env::var("PATH") {
            let combined = format!("{proc_path};{target_str}");
            std::env::set_var("PATH", combined);
        }

        // Broadcast WM_SETTINGCHANGE so other apps & new terminals get the updated PATH
        let env_param: Vec<u16> = "Environment\0".encode_utf16().collect();
        let mut result_dummy: usize = 0;
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(env_param.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            2000,
            Some(&mut result_dummy),
        );

        Ok(true)
    }
}
