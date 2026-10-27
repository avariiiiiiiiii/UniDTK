use std::os::windows::ffi::OsStrExt;

pub fn get_all_drives() -> Result<Vec<DriveInfo>, String> {
    let mut drives = Vec::new();
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };

    for i in 0..26u32 {
        if (mask >> i) & 1 == 1 {
            let letter = (b'A' + i as u8) as char;
            let drive_path = format!("{}:\\", letter);

            let wide: Vec<u16> = std::ffi::OsString::from(&drive_path)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let drive_type = unsafe {
                windows::Win32::Storage::FileSystem::GetDriveTypeW(windows::core::PCWSTR(wide.as_ptr()))
            };

            let is_removable = drive_type == 2; // DRIVE_REMOVABLE

            let (total_space, free_space) = get_drive_space_info(&drive_path);

            let display = get_volume_label(&drive_path)
                .map(|label| format!("{} ({})", label, drive_path))
                .or_else(|| {
                    if drive_type == 4 {
                        // DRIVE_REMOTE (Network drive / mapped SMB share)
                        get_network_remote_path(letter)
                            .map(|unc| format!("{} ({})", unc, drive_path))
                            .or_else(|| Some(format!("Network Drive ({})", drive_path)))
                    } else if drive_type == 2 {
                        Some(format!("Removable Disk ({})", drive_path))
                    } else if drive_type == 3 {
                        Some(format!("Local Disk ({})", drive_path))
                    } else if drive_type == 5 {
                        Some(format!("CD/DVD Drive ({})", drive_path))
                    } else {
                        Some(format!("Drive ({})", drive_path))
                    }
                })
                .unwrap_or_else(|| drive_path.clone());

            drives.push(DriveInfo {
                display,
                path: drive_path,
                is_removable,
                total_space,
                free_space,
            });
        }
    }

    unidtk_api::log(format!("get_all_drives: {} drives", drives.len()));
    Ok(drives)
}

pub fn get_raw_drive_letters() -> Result<Vec<String>, String> {
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    let mut drives = Vec::new();

    for i in 0..26u32 {
        if (mask >> i) & 1 == 1 {
            let letter = (b'A' + i as u8) as char;
            drives.push(format!("{}:\\", letter));
        }
    }

    Ok(drives)
}

pub fn get_unmounted_volumes() -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

pub fn is_removable_drive(drive: &str) -> bool {
    let normalized = if drive.ends_with('\\') || drive.ends_with('/') {
        drive.to_string()
    } else {
        format!("{}\\", drive)
    };

    let wide: Vec<u16> = std::ffi::OsString::from(&normalized)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let drive_type = windows::Win32::Storage::FileSystem::GetDriveTypeW(windows::core::PCWSTR(wide.as_ptr()));
        drive_type == 2 // DRIVE_REMOVABLE
    }
}

fn get_drive_space_info(drive_path: &str) -> (Option<u64>, Option<u64>) {
    let wide: Vec<u16> = std::ffi::OsString::from(drive_path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut free_bytes_available: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut _total_free_bytes: u64 = 0;

        let result = windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            windows::core::PCWSTR(wide.as_ptr()),
            Some(&mut free_bytes_available),
            Some(&mut total_bytes),
            Some(&mut _total_free_bytes),
        );

        if result.is_ok() && total_bytes > 0 {
            (Some(total_bytes), Some(free_bytes_available))
        } else {
            (None, None)
        }
    }
}

fn get_network_remote_path(drive_letter: char) -> Option<String> {
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    let local_name = format!("{}:", drive_letter);
    let wide_local: Vec<u16> = std::ffi::OsString::from(&local_name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mpr = LoadLibraryW(w!("mpr.dll")).ok()?;
        type WNetGetConnectionWFn = unsafe extern "system" fn(
            lp_local_name: PCWSTR,
            lp_remote_name: *mut u16,
            lpn_length: *mut u32,
        ) -> u32;

        let proc_name = b"WNetGetConnectionW\0";
        let func_ptr = GetProcAddress(mpr, windows::core::PCSTR(proc_name.as_ptr()))?;
        let wnet_get_connection: WNetGetConnectionWFn = std::mem::transmute(func_ptr);

        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let res = wnet_get_connection(
            PCWSTR(wide_local.as_ptr()),
            buffer.as_mut_ptr(),
            &mut len,
        );

        if res == 0 {
            let zero_pos = buffer.iter().position(|&c| c == 0).unwrap_or(0);
            if zero_pos > 0 {
                let remote_path = String::from_utf16_lossy(&buffer[..zero_pos]).trim().to_string();
                if !remote_path.is_empty() {
                    return Some(remote_path);
                }
            }
        }
    }
    None
}

fn get_volume_label(drive_path: &str) -> Option<String> {
    let wide: Vec<u16> = std::ffi::OsString::from(drive_path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut volume_name = [0u16; 256];
        let result = windows::Win32::Storage::FileSystem::GetVolumeInformationW(
            windows::core::PCWSTR(wide.as_ptr()),
            Some(&mut volume_name),
            None,
            None,
            None,
            None,
        );

        if result.is_ok() {
            let len = volume_name.iter().position(|&c| c == 0).unwrap_or(0);
            if len > 0 {
                let label = String::from_utf16_lossy(&volume_name[..len]).trim().to_string();
                if !label.is_empty() {
                    return Some(label);
                }
            }
        }
    }

    None
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DriveInfo {
    pub display: String,
    pub path: String,
    pub is_removable: bool,
    pub total_space: Option<u64>,
    pub free_space: Option<u64>,
}
