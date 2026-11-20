//! The VPN's settings and subscription cache in its ModuleDb slice (r[storage.no-raw-db]).

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use unidtk_api::{DbOp, ModuleDb};

use super::{HomeServerConfig, VpnServer};

const SETTINGS: &str = "settings";
const CACHE: &str = "servers_cache";

static STORE: OnceLock<SettingsStore> = OnceLock::new();

pub(super) fn bind(db: Arc<dyn ModuleDb>) {
    STORE.get_or_init(|| SettingsStore::new(db, unidtk_api::paths::data_dir()));
}

pub(super) fn store() -> Result<&'static SettingsStore, String> {
    STORE
        .get()
        .ok_or_else(|| "VPN settings store is unavailable".into())
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct VpnSettings {
    pub subscription_url: String,
    pub default_tag: String,
    pub last_tag: Option<String>,
    pub auto_connect: bool,
    pub home_server: HomeServerConfig,
}

/// One subscription revision and the settings used to choose and start a server.
pub(super) struct Snapshot {
    pub revision: u64,
    pub settings: VpnSettings,
}

pub(super) struct SettingsStore {
    db: Arc<dyn ModuleDb>,
    legacy_dir: PathBuf,
    // Serializes all read-modify-write operations and cache publication within this process.
    // Guarded code does only storage work and never acquires the runtime lock.
    revision: Mutex<u64>,
}

impl SettingsStore {
    fn new(db: Arc<dyn ModuleDb>, legacy_dir: PathBuf) -> Self {
        Self {
            db,
            legacy_dir,
            revision: Mutex::new(0),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, u64>, String> {
        self.revision
            .lock()
            .map_err(|_| "VPN settings lock is poisoned".into())
    }

    pub(super) fn read(&self) -> Result<VpnSettings, String> {
        if let Some(settings) = read_record(self.db.as_ref())? {
            return Ok(settings);
        }
        let _guard = self.lock()?;
        self.read_or_import()
    }

    pub(super) fn snapshot(&self) -> Result<Snapshot, String> {
        let revision = self.lock()?;
        let settings = self.read_or_import()?;
        Ok(Snapshot {
            revision: *revision,
            settings,
        })
    }

    /// The caller holds the settings lock. Only an absent key can trigger a legacy import.
    fn read_or_import(&self) -> Result<VpnSettings, String> {
        if let Some(settings) = read_record(self.db.as_ref())? {
            return Ok(settings);
        }
        let path = self.legacy_dir.join("vpn_settings.json");
        let Some(settings) = read_legacy::<VpnSettings>(&path)? else {
            return Ok(VpnSettings::default());
        };
        self.write_record(&settings)?;
        remove_legacy(&path);
        Ok(settings)
    }

    fn write_record(&self, settings: &VpnSettings) -> Result<(), String> {
        let bytes = serde_json::to_vec(settings)
            .map_err(|error| format!("encode VPN settings: {error}"))?;
        self.db
            .kv_set(SETTINGS, &bytes)
            .map_err(|error| format!("save VPN settings: {error}"))
    }

    pub(super) fn update(
        &self,
        edit: impl FnOnce(&mut VpnSettings),
    ) -> Result<VpnSettings, String> {
        let mut revision = self.lock()?;
        self.update_locked(&mut revision, edit)
    }

    pub(super) fn remember_connected(&self, expected: u64, tag: String) -> Result<(), String> {
        let mut revision = self.lock()?;
        if *revision == expected {
            self.update_locked(&mut revision, |settings| settings.last_tag = Some(tag))?;
        }
        Ok(())
    }

    fn update_locked(
        &self,
        revision: &mut u64,
        edit: impl FnOnce(&mut VpnSettings),
    ) -> Result<VpnSettings, String> {
        let mut settings = self.read_or_import()?;
        let old_url = settings.subscription_url.clone();
        edit(&mut settings);
        if settings.subscription_url == old_url {
            self.write_record(&settings)?;
        } else {
            let value = serde_json::to_vec(&settings)
                .map_err(|error| format!("encode VPN settings: {error}"))?;
            self.db
                .kv_batch(vec![
                    DbOp::Set {
                        key: SETTINGS.into(),
                        value,
                    },
                    // An explicit empty list suppresses the fallback to a legacy cache file.
                    DbOp::Set {
                        key: CACHE.into(),
                        value: b"[]".to_vec(),
                    },
                ])
                .map_err(|error| format!("save VPN subscription: {error}"))?;
            *revision = revision.wrapping_add(1);
        }
        Ok(settings)
    }

    /// Accept a refresh result only while it still belongs to the current subscription.
    /// The caller completes its cache work in the closure, after the network request.
    pub(super) fn with_current_subscription<T>(
        &self,
        expected: u64,
        finish: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let revision = self.lock()?;
        if *revision != expected {
            return Err("VPN subscription changed during the request".into());
        }
        finish()
    }

    /// Used inside `with_current_subscription` to keep the disk cache and revision together.
    pub(super) fn save_servers_cache(&self, servers: &[VpnServer]) -> Result<(), String> {
        let bytes = serde_json::to_vec(servers).map_err(|error| error.to_string())?;
        self.db.kv_set(CACHE, &bytes)
    }

    /// Used inside `with_current_subscription` after a failed refresh of the same URL.
    pub(super) fn load_servers_cache(&self) -> Vec<VpnServer> {
        if let Ok(Some(bytes)) = self.db.kv_get(CACHE) {
            if let Ok(servers) = serde_json::from_slice(&bytes) {
                return servers;
            }
        }
        let path = self.legacy_dir.join("vpn_servers.json");
        let Ok(Some(servers)) = read_legacy::<Vec<VpnServer>>(&path) else {
            return Vec::new();
        };
        if self.save_servers_cache(&servers).is_ok() {
            remove_legacy(&path);
        }
        servers
    }
}

/// The diagnostic path calls this directly on its in-memory store.
pub(super) fn read_record(db: &dyn ModuleDb) -> Result<Option<VpnSettings>, String> {
    db.kv_get(SETTINGS)
        .map_err(|error| format!("read VPN settings: {error}"))?
        .map(|bytes| {
            serde_json::from_slice(&bytes).map_err(|_| "Stored VPN settings are invalid".into())
        })
        .transpose()
}

fn read_legacy<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read legacy {}: {error}", path.display())),
    };
    serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map(Some)
        .map_err(|_| format!("Legacy VPN data is invalid: {}", path.display()))
}

fn remove_legacy(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        unidtk_api::log(format!(
            "vpn: cannot remove imported legacy file {}: {error}",
            path.display()
        ));
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
