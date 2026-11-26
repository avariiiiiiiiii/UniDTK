//! The VPN's own state in its ModuleDb slice (r[storage.no-raw-db]): the server it
//! remembers, each subscription's cached servers and the device secret. What the menu
//! shows (the subscriptions, auto-connect, the home server) is in the settings contract
//! (r[modules.settings-contract]); this store reads the subscriptions from there, and the
//! stored version of that list is the subscription revision every decision checks.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use unidtk_api::settings::ModuleSettings;
use unidtk_api::ModuleDb;

use super::{HomeServerConfig, VpnServer};

pub(super) const SETTINGS: &str = "settings";
/// `servers_cache/<subscription id>`: one cache per subscription.
const CACHE_PREFIX: &str = "servers_cache/";
/// The one cache of the single-subscription days, dropped by [`SettingsStore::prune_caches`].
const OLD_CACHE: &str = "servers_cache";
const DEVICE_SECRET: &str = "device_secret";
const SUBSCRIPTION_CHANGED: &str = "VPN subscription changed during the request";

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
    /// The key (`<subscription id>/<tag>`) of the server to connect to by default.
    pub default_tag: String,
    /// The key of the server the last connect reached.
    pub last_tag: Option<String>,
}

/// A subscription as the module uses it: its address and an id that stays while the
/// address does (`hwid::subscription_id`), which keys its cache and its servers.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Subscription {
    pub id: String,
    pub url: String,
}

/// One subscription revision and the settings used to choose and start a server.
pub(super) struct Snapshot {
    pub revision: u64,
    pub settings: VpnSettings,
    pub subscriptions: Vec<Subscription>,
}

pub(super) struct SettingsStore {
    db: Arc<dyn ModuleDb>,
    legacy_dir: PathBuf,
    /// Serializes the module's writes and its decisions about a subscription within this
    /// process. Guarded code may use an already locked runtime, but never acquires its
    /// lock. A write of the subscriptions through the kernel does not take it: the caches
    /// and the remembered server are keyed by subscription id, so a decision made just
    /// before such a write never mixes two subscriptions.
    lock: Mutex<()>,
    declared: OnceLock<Arc<dyn ModuleSettings>>,
}

impl SettingsStore {
    pub(super) fn new(db: Arc<dyn ModuleDb>, legacy_dir: PathBuf) -> Self {
        Self { db, legacy_dir, lock: Mutex::new(()), declared: OnceLock::new() }
    }

    /// The kernel's keeper of the declared settings, from the module's context.
    pub(super) fn declare(&self, declared: Arc<dyn ModuleSettings>) {
        let _ = self.declared.set(declared);
    }

    fn lock(&self) -> Result<MutexGuard<'_, ()>, String> {
        self.lock.lock().map_err(|_| "VPN settings lock is poisoned".into())
    }

    /// The stored subscriptions and their version (0 with none stored or no contract).
    pub(super) fn subscriptions(&self) -> Result<(u64, Vec<Subscription>), String> {
        let Some(declared) = self.declared.get() else { return Ok((0, Vec::new())) };
        let stored = declared.get("subscriptions")?;
        let list = stored
            .value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|url| Subscription { id: super::hwid::subscription_id(url), url: url.to_string() })
            .collect();
        Ok((stored.version, list))
    }

    /// The home server, as stored through the contract; the defaults when none is.
    pub(super) fn home_server(&self) -> HomeServerConfig {
        self.declared
            .get()
            .and_then(|declared| declared.get("home_server").ok())
            .and_then(|stored| serde_json::from_value(stored.value).ok())
            .unwrap_or_default()
    }

    pub(super) fn read(&self) -> Result<VpnSettings, String> {
        if let Some(settings) = read_record(self.db.as_ref())? {
            return Ok(settings);
        }
        let _guard = self.lock()?;
        self.read_or_import()
    }

    pub(super) fn snapshot(&self, expected: Option<u64>) -> Result<Snapshot, String> {
        let _guard = self.lock()?;
        let (revision, subscriptions) = self.subscriptions()?;
        if expected.is_some_and(|expected| expected != revision) {
            return Err(SUBSCRIPTION_CHANGED.into());
        }
        let settings = self.read_or_import()?;
        Ok(Snapshot { revision, settings, subscriptions })
    }

    /// The record as stored, before it is read as [`VpnSettings`]: the migration into the
    /// settings contract reads the fields this struct no longer has. The legacy file
    /// counts when the store has no record yet.
    pub(super) fn raw_record(&self) -> Option<Value> {
        if let Ok(Some(bytes)) = self.db.kv_get(SETTINGS) {
            return serde_json::from_slice(&bytes).ok();
        }
        read_legacy::<Value>(&self.legacy_dir.join("vpn_settings.json")).ok().flatten()
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

    pub(super) fn update(&self, edit: impl FnOnce(&mut VpnSettings)) -> Result<VpnSettings, String> {
        let _guard = self.lock()?;
        let mut settings = self.read_or_import()?;
        edit(&mut settings);
        self.write_record(&settings)?;
        Ok(settings)
    }

    pub(super) fn remember_connected(&self, expected: u64, key: String) -> Result<(), String> {
        let _guard = self.lock()?;
        if self.subscriptions()?.0 == expected {
            let mut settings = self.read_or_import()?;
            settings.last_tag = Some(key);
            self.write_record(&settings)?;
        }
        Ok(())
    }

    /// Make a state decision only while the subscriptions are still the ones read.
    /// The closure runs under the settings lock and must finish without network I/O.
    pub(super) fn with_current_subscription<T>(
        &self,
        expected: u64,
        finish: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = self.lock()?;
        if self.subscriptions()?.0 != expected {
            return Err(SUBSCRIPTION_CHANGED.into());
        }
        finish()
    }

    /// A random id made once and kept, for a machine whose own id cannot be read
    /// (`hwid.rs`): the device headers then stay the same from one start to the next.
    pub(super) fn device_secret(&self) -> Result<String, String> {
        let _guard = self.lock()?;
        if let Some(bytes) = self.db.kv_get(DEVICE_SECRET)? {
            if let Ok(secret) = String::from_utf8(bytes) {
                if !secret.is_empty() {
                    return Ok(secret);
                }
            }
        }
        let secret = super::hwid::random_secret();
        self.db.kv_set(DEVICE_SECRET, secret.as_bytes())?;
        Ok(secret)
    }

    pub(super) fn save_servers_cache(&self, id: &str, servers: &[VpnServer]) -> Result<(), String> {
        let bytes = serde_json::to_vec(servers).map_err(|error| error.to_string())?;
        self.db.kv_set(&format!("{CACHE_PREFIX}{id}"), &bytes)
    }

    /// One subscription's servers as last fetched; for a transport failure only.
    pub(super) fn load_servers_cache(&self, id: &str) -> Vec<VpnServer> {
        self.db
            .kv_get(&format!("{CACHE_PREFIX}{id}"))
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Removes the caches of subscriptions that are gone, and the single cache of the
    /// one-subscription days (and its legacy file).
    pub(super) fn prune_caches(&self, keep: &[String]) {
        if let Ok(caches) = self.db.kv_list(CACHE_PREFIX) {
            for (key, _) in caches {
                let id = key.trim_start_matches(CACHE_PREFIX);
                if !keep.iter().any(|kept| kept == id) {
                    let _ = self.db.kv_remove(&key);
                }
            }
        }
        let _ = self.db.kv_remove(OLD_CACHE);
        let legacy = self.legacy_dir.join("vpn_servers.json");
        if legacy.exists() {
            remove_legacy(&legacy);
        }
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
