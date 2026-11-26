//! The device headers a provider's device limit counts (Remnawave, docs.rw "HWID device
//! limit": `x-hwid` is the one it requires; `x-device-os`, `x-ver-os` and
//! `x-device-model` describe the device in its list). The id is made per provider host,
//! so two services cannot tell they serve one machine, and from the machine's own id, so
//! it stays the same across reinstalls and a reset of the module's data.

use std::sync::OnceLock;

use sha2::{Digest, Sha256};

const SALT: &str = "unidtk-vpn-hwid";

/// The id for one provider: 32 hex characters, inside Remnawave's `^[a-zA-Z0-9=-]{10,64}$`
/// (a value outside it is ignored as if absent).
pub fn hwid(secret: &str, host: &str) -> String {
    let digest = Sha256::digest(format!("{secret}|{SALT}|{}", host.to_ascii_lowercase()));
    digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect()
}

/// The model as a header may carry it: printable ASCII, at most 64 characters, and "PC"
/// for the names firmware leaves when the maker filled in nothing.
pub fn sanitize_model(raw: &str) -> String {
    let clean: String = raw.chars().filter(|c| (' '..='~').contains(c)).collect();
    let clean = clean.trim();
    const PLACEHOLDERS: [&str; 5] = ["system product name", "to be filled by o.e.m.", "default string", "not applicable", "none"];
    if clean.is_empty() || PLACEHOLDERS.contains(&clean.to_ascii_lowercase().as_str()) {
        return "PC".into();
    }
    clean.chars().take(64).collect()
}

struct Device {
    secret: String,
    os_version: String,
    model: String,
}

static DEVICE: OnceLock<Device> = OnceLock::new();

fn device() -> &'static Device {
    DEVICE.get_or_init(|| {
        // Never a hash of nothing: without the machine's id every such machine would share
        // one device slot. The fallback is a random id the module keeps.
        let secret = registry::string(r"SOFTWARE\Microsoft\Cryptography", "MachineGuid")
            .or_else(|| super::settings::store().ok().and_then(|store| store.device_secret().ok()))
            .unwrap_or_else(random_secret);
        let version = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        let os_version = match (
            registry::dword(version, "CurrentMajorVersionNumber"),
            registry::dword(version, "CurrentMinorVersionNumber"),
            registry::string(version, "CurrentBuildNumber"),
        ) {
            (Some(major), Some(minor), Some(build)) => format!("{major}.{minor}.{build}"),
            _ => "10.0".into(),
        };
        let model = sanitize_model(
            &registry::string(r"HARDWARE\DESCRIPTION\System\BIOS", "SystemProductName").unwrap_or_default(),
        );
        Device { secret, os_version, model }
    })
}

/// 128 bits from the OS-seeded hasher, for a machine whose id cannot be read.
pub fn random_secret() -> String {
    use std::hash::{BuildHasher, Hasher};
    let half = || {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        hasher.finish()
    };
    format!("{:016x}{:016x}", half(), half())
}

/// The id this machine sends to `host`, for the settings pane to show.
pub fn id_for(host: &str) -> String {
    hwid(&device().secret, host)
}

/// The headers for a request to `host`.
pub fn headers(host: &str) -> Vec<(&'static str, String)> {
    let device = device();
    vec![
        ("x-hwid", hwid(&device.secret, host)),
        ("x-device-os", "Windows".into()),
        ("x-ver-os", device.os_version.clone()),
        ("x-device-model", device.model.clone()),
    ]
}

/// Values under `HKEY_LOCAL_MACHINE`, read from the 64-bit view whatever the process is.
mod registry {
    #[cfg(windows)]
    fn read(subkey: &str, value: &str, kind: windows::Win32::System::Registry::REG_ROUTINE_FLAGS) -> Option<Vec<u8>> {
        use windows::core::HSTRING;
        use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_SUBKEY_WOW6464KEY};
        let mut data = vec![0u8; 1024];
        let mut size = data.len() as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                &HSTRING::from(subkey),
                &HSTRING::from(value),
                kind | RRF_SUBKEY_WOW6464KEY,
                None,
                Some(data.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status.is_err() {
            return None;
        }
        data.truncate(size as usize);
        Some(data)
    }

    #[cfg(windows)]
    pub fn string(subkey: &str, value: &str) -> Option<String> {
        let bytes = read(subkey, value, windows::Win32::System::Registry::RRF_RT_REG_SZ)?;
        let wide: Vec<u16> = bytes.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
        let text = String::from_utf16_lossy(&wide);
        let text = text.trim_end_matches('\0').trim();
        (!text.is_empty()).then(|| text.to_string())
    }

    #[cfg(windows)]
    pub fn dword(subkey: &str, value: &str) -> Option<u32> {
        let bytes = read(subkey, value, windows::Win32::System::Registry::RRF_RT_REG_DWORD)?;
        bytes.get(..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    #[cfg(not(windows))]
    pub fn string(_: &str, _: &str) -> Option<String> {
        None
    }

    #[cfg(not(windows))]
    pub fn dword(_: &str, _: &str) -> Option<u32> {
        None
    }
}

#[cfg(test)]
#[path = "hwid_tests.rs"]
mod tests;
