//! The kernel's side of the settings contract (`r[modules.settings-contract]`): the values
//! a module declared in [`unidtk_api::Module::settings`], kept in one table of
//! `unidtk.redb` with a version each. A write is checked against the declaration and the
//! module's own `check_setting`, refused when the caller read an older version, and
//! announced after it committed: the module hears `setting_changed` and the state event
//! `settings-changed { module, key, version }` goes out, never with the value.

use std::sync::{Arc, Weak};

use redb::{Database, ReadableTable, TableDefinition, TableError};
use serde_json::{json, Value};
use unidtk_api::settings::{ModuleSettings, SettingSpec, SettingValue, STALE};
use unidtk_api::Module;

/// `<module>/<key>` → the value as JSON; `<module>/<key>#v` → its version (u64, little
/// endian), a row of its own so that a damaged value never takes its version with it.
pub(crate) const MODULE_SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("module_settings");

/// The state event a stored setting publishes.
pub const EVENT: &str = "settings-changed";

/// One module's settings.
pub struct KernelSettings {
    db: Arc<Database>,
    module_id: &'static str,
    specs: &'static [SettingSpec],
    /// The module's object, for its check and its notice. Weak: the module keeps this
    /// handle in its context.
    module: Weak<dyn Module>,
}

impl KernelSettings {
    pub fn new(db: Arc<Database>, module: &Arc<dyn Module>) -> Self {
        Self { db, module_id: module.id(), specs: module.settings(), module: Arc::downgrade(module) }
    }

    pub fn specs(&self) -> &'static [SettingSpec] {
        self.specs
    }

    fn spec(&self, key: &str) -> Result<&'static SettingSpec, String> {
        self.specs
            .iter()
            .find(|spec| spec.key == key)
            .ok_or_else(|| format!("{}: unknown setting '{key}'", self.module_id))
    }

    fn value_key(&self, key: &str) -> String {
        format!("{}/{key}", self.module_id)
    }

    fn version_key(&self, key: &str) -> String {
        format!("{}/{key}#v", self.module_id)
    }

    /// The stored bytes and version, in one read.
    fn read_raw(&self, key: &str) -> Result<(Option<Vec<u8>>, u64), String> {
        let txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = match txn.open_table(MODULE_SETTINGS) {
            Ok(table) => table,
            Err(TableError::TableDoesNotExist(_)) => return Ok((None, 0)),
            Err(error) => return Err(error.to_string()),
        };
        let value = table.get(self.value_key(key).as_str()).map_err(|e| e.to_string())?.map(|v| v.value().to_vec());
        let version = table
            .get(self.version_key(key).as_str())
            .map_err(|e| e.to_string())?
            .and_then(|v| v.value().try_into().ok().map(u64::from_le_bytes))
            .unwrap_or(0);
        Ok((value, version))
    }

    /// A value that no longer reads as its declaration is moved to `<key>.damaged`, only
    /// if it is still the same bytes (a write in between is kept).
    fn set_aside(&self, key: &str, damaged: &[u8]) {
        let result = (|| -> Result<bool, String> {
            let txn = self.db.begin_write().map_err(|e| e.to_string())?;
            let moved = {
                let mut table = txn.open_table(MODULE_SETTINGS).map_err(|e| e.to_string())?;
                let still = table.get(self.value_key(key).as_str()).map_err(|e| e.to_string())?.is_some_and(|v| v.value() == damaged);
                if still {
                    table.insert(format!("{}.damaged", self.value_key(key)).as_str(), damaged).map_err(|e| e.to_string())?;
                    table.remove(self.value_key(key).as_str()).map_err(|e| e.to_string())?;
                }
                still
            };
            txn.commit().map_err(|e| e.to_string())?;
            Ok(moved)
        })();
        match result {
            Ok(true) => crate::log_line(format!("settings: {}/{key} did not read as declared; set aside, default used", self.module_id)),
            Ok(false) => {}
            Err(error) => crate::log_line(format!("settings: could not set aside {}/{key}: {error}", self.module_id)),
        }
    }

    fn announce(&self, key: &str, version: u64) {
        let Some(module) = self.module.upgrade() else { return };
        let (module_id, key) = (self.module_id, key.to_string());
        let spawned = std::thread::Builder::new().name(format!("settings-{module_id}")).spawn(move || {
            module.setting_changed(&key, version);
            crate::events::publish(EVENT, json!({ "module": module_id, "key": key, "version": version }));
        });
        if let Err(error) = spawned {
            crate::log_line(format!("settings: could not announce {module_id}: {error}"));
        }
    }
}

impl ModuleSettings for KernelSettings {
    fn get(&self, key: &str) -> Result<SettingValue, String> {
        let spec = self.spec(key)?;
        let (raw, version) = self.read_raw(key)?;
        let Some(bytes) = raw else {
            return Ok(SettingValue { version, value: spec.default_value() });
        };
        match serde_json::from_slice::<Value>(&bytes).ok().and_then(|value| spec.check(value).ok()) {
            Some(value) => Ok(SettingValue { version, value }),
            None => {
                self.set_aside(key, &bytes);
                Ok(SettingValue { version, value: spec.default_value() })
            }
        }
    }

    fn set(&self, key: &str, value: Value, expected: Option<u64>) -> Result<SettingValue, String> {
        let spec = self.spec(key)?;
        let value = spec.check(value)?;
        let current = self.get(key)?;
        if expected.is_some_and(|expected| expected != current.version) {
            return Err(STALE.into());
        }
        let module = self.module.upgrade().ok_or_else(|| format!("{}: the module is gone", self.module_id))?;
        // The module's check runs with nothing held: no transaction, no lock.
        let value = spec.check(module.check_setting(key, &current.value, value)?)?;
        if current.version > 0 && current.value == value {
            return Ok(current);
        }
        let bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        let version = current.version + 1;
        let txn = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut table = txn.open_table(MODULE_SETTINGS).map_err(|e| e.to_string())?;
            let stored = table
                .get(self.version_key(key).as_str())
                .map_err(|e| e.to_string())?
                .and_then(|v| v.value().try_into().ok().map(u64::from_le_bytes))
                .unwrap_or(0);
            // Compare and set: another write landed between this one's read and now.
            if stored != current.version {
                return Err(STALE.into());
            }
            table.insert(self.value_key(key).as_str(), bytes.as_slice()).map_err(|e| e.to_string())?;
            table.insert(self.version_key(key).as_str(), version.to_le_bytes().as_slice()).map_err(|e| e.to_string())?;
        }
        txn.commit().map_err(|e| e.to_string())?;
        // Never the value: a secret one is a credential, and the log is readable.
        crate::log_line(format!("settings: {}/{key} stored (version {version})", self.module_id));
        self.announce(key, version);
        Ok(SettingValue { version, value })
    }
}

// ── The commands (`settings_*`), one body for the socket, the WebSocket and Tauri ──

fn handle(app: &tauri::AppHandle, db: Arc<Database>, id: &str) -> Result<KernelSettings, String> {
    use tauri::Manager;
    let registry = app.try_state::<crate::registry::Registry>().ok_or("modules are not started yet")?;
    let module = registry.get(id).ok_or_else(|| format!("unknown module: {id}"))?;
    Ok(KernelSettings::new(db, module))
}

/// Every module's declarations, for the menu to know where "Настроить…" goes.
pub(crate) fn specs_of_all(app: &tauri::AppHandle) -> Result<Value, String> {
    use tauri::Manager;
    let registry = app.try_state::<crate::registry::Registry>().ok_or("modules are not started yet")?;
    Ok(Value::Object(
        registry
            .in_start_order()
            .iter()
            .filter(|module| !module.settings().is_empty())
            .map(|module| (module.id().to_string(), json!(module.settings())))
            .collect(),
    ))
}

/// One module's declarations, values and versions (the interface door only).
pub(crate) fn get_all(app: &tauri::AppHandle, db: Arc<Database>, module: &str) -> Result<Value, String> {
    let settings = handle(app, db, module)?;
    let mut values = serde_json::Map::new();
    for spec in settings.specs() {
        values.insert(spec.key.to_string(), json!(settings.get(spec.key)?));
    }
    Ok(json!({ "specs": settings.specs(), "values": values }))
}

pub(crate) fn set_one(app: &tauri::AppHandle, db: Arc<Database>, module: &str, key: &str, value: Value, expected: Option<u64>) -> Result<Value, String> {
    Ok(json!(handle(app, db, module)?.set(key, value, expected)?))
}

/// On the socket: what is declared and whether it is stored, never a value.
pub(crate) fn keys_of(app: &tauri::AppHandle, db: Arc<Database>, module: &str) -> Result<Value, String> {
    let settings = handle(app, db, module)?;
    let mut stored = serde_json::Map::new();
    for spec in settings.specs() {
        let version = settings.get(spec.key)?.version;
        stored.insert(spec.key.to_string(), json!({ "version": version, "stored": version > 0 }));
    }
    Ok(json!({ "specs": settings.specs(), "stored": stored }))
}

#[cfg(test)]
#[path = "module_settings_tests.rs"]
mod tests;
