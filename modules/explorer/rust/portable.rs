use std::path::PathBuf;

#[allow(dead_code)]
pub const PORTABLE_PREFIX: &str = "portable://";

pub fn is_portable_path(_path: &PathBuf) -> bool {
    false
}

pub fn get_portable_devices() -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

pub fn get_device_contents(_device_id: &str) -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

#[allow(dead_code)]
pub fn list_portable_devices_with_ids() -> Vec<(String, String)> {
    Vec::new()
}

#[allow(dead_code)]
pub fn make_portable_path(device_id: &str, object_id: &str) -> PathBuf {
    PathBuf::from(format!("portable://{}/{}", device_id, object_id))
}
