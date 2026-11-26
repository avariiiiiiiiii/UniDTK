//! The settings a module shows in the menu (`r[modules.settings-contract]`), the third
//! contract beside the log ([`crate::log`]) and the store ([`crate::ModuleDb`]). A module
//! declares what it has ([`crate::Module::settings`]); the kernel keeps the values and
//! their versions in `unidtk.redb`, checks every write against the declaration and the
//! module's own [`crate::Module::check_setting`], and the menu draws the fields from the
//! declaration. The module reads and writes through [`ModuleSettings`], which
//! [`crate::ModuleContext::settings`] hands out, as `db()` hands out the store.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;

/// What kind of value a setting holds, and how the menu draws it.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingKind {
    /// A switch.
    Switch { default: bool },
    /// One line of text, trimmed.
    Text { placeholder: &'static str, max_chars: usize },
    /// A whole number in a range.
    Number { min: i64, max: i64, default: i64 },
    /// One of a few values: `(value, label)`.
    Choice { options: &'static [(&'static str, &'static str)], default: &'static str },
    /// Rows of text with add and remove (a list of addresses); rows are trimmed and
    /// empty rows dropped.
    TextList { placeholder: &'static str, max_items: usize, max_chars: usize },
    /// A JSON object the module checks and its own pane section draws.
    Record,
}

/// One setting a module declares.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct SettingSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub summary: &'static str,
    #[serde(flatten)]
    pub kind: SettingKind,
    /// The value is a credential (an address with a token, a key): it is never logged,
    /// never served on the socket, and never repeated in a refusal.
    pub secret: bool,
}

/// A stored value and its version. Version 0: never stored, the declaration's default.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SettingValue {
    pub version: u64,
    pub value: Value,
}

impl SettingSpec {
    /// The value before anything was stored.
    pub fn default_value(&self) -> Value {
        match self.kind {
            SettingKind::Switch { default } => Value::Bool(default),
            SettingKind::Text { .. } => Value::String(String::new()),
            SettingKind::Number { default, .. } => Value::from(default),
            SettingKind::Choice { default, .. } => Value::String(default.to_string()),
            SettingKind::TextList { .. } => Value::Array(Vec::new()),
            SettingKind::Record => Value::Object(Default::default()),
        }
    }

    /// The value as it will be stored, or why it does not fit this setting. The reason
    /// names the setting, never the value.
    pub fn check(&self, value: Value) -> Result<Value, String> {
        let label = self.label;
        match self.kind {
            SettingKind::Switch { .. } => value
                .as_bool()
                .map(Value::Bool)
                .ok_or_else(|| format!("«{label}»: нужно да или нет")),
            SettingKind::Text { max_chars, .. } => {
                let text = value.as_str().ok_or_else(|| format!("«{label}»: нужен текст"))?.trim();
                if text.chars().count() > max_chars {
                    return Err(format!("«{label}»: не длиннее {max_chars} знаков"));
                }
                Ok(Value::String(text.to_string()))
            }
            SettingKind::Number { min, max, .. } => match value.as_i64() {
                Some(number) if (min..=max).contains(&number) => Ok(Value::from(number)),
                _ => Err(format!("«{label}»: число от {min} до {max}")),
            },
            SettingKind::Choice { options, .. } => match value.as_str() {
                Some(choice) if options.iter().any(|(option, _)| *option == choice) => Ok(Value::String(choice.to_string())),
                _ => Err(format!("«{label}»: нет такого варианта")),
            },
            SettingKind::TextList { max_items, max_chars, .. } => {
                let rows = value.as_array().ok_or_else(|| format!("«{label}»: нужен список"))?;
                let mut kept = Vec::new();
                for row in rows {
                    let text = row.as_str().ok_or_else(|| format!("«{label}»: строки должны быть текстом"))?.trim();
                    if text.is_empty() {
                        continue;
                    }
                    if text.chars().count() > max_chars {
                        return Err(format!("«{label}»: строка не длиннее {max_chars} знаков"));
                    }
                    kept.push(Value::String(text.to_string()));
                }
                if kept.len() > max_items {
                    return Err(format!("«{label}»: не больше {max_items} строк"));
                }
                Ok(Value::Array(kept))
            }
            SettingKind::Record => {
                if value.is_object() {
                    Ok(value)
                } else {
                    Err(format!("«{label}»: нужна запись"))
                }
            }
        }
    }
}

/// A module's settings, as the kernel keeps them.
pub trait ModuleSettings: Send + Sync {
    /// The stored value with its version, or the default with version 0.
    fn get(&self, key: &str) -> Result<SettingValue, String>;

    /// Stores `value` after the declaration's check and the module's
    /// [`crate::Module::check_setting`]. `expected`: the version the caller read; any other
    /// stored version refuses the write, so an edit made from an old read cannot undo a
    /// newer one. An unchanged value is not written and keeps its version.
    fn set(&self, key: &str, value: Value, expected: Option<u64>) -> Result<SettingValue, String>;
}

/// Settings kept in memory: for tests and for `udtk doctor -modules`, which runs a module's
/// lifecycle without touching the user's data. Declaration checks apply; a module's own
/// check does not (there is no module here).
pub struct InMemorySettings {
    specs: &'static [SettingSpec],
    values: Mutex<HashMap<&'static str, SettingValue>>,
}

impl InMemorySettings {
    pub fn new(specs: &'static [SettingSpec]) -> Self {
        Self { specs, values: Mutex::new(HashMap::new()) }
    }

    fn spec(&self, key: &str) -> Result<&'static SettingSpec, String> {
        self.specs.iter().find(|spec| spec.key == key).ok_or_else(|| format!("unknown setting '{key}'"))
    }
}

impl ModuleSettings for InMemorySettings {
    fn get(&self, key: &str) -> Result<SettingValue, String> {
        let spec = self.spec(key)?;
        let values = self.values.lock().map_err(|_| "settings lock poisoned".to_string())?;
        Ok(values.get(spec.key).cloned().unwrap_or_else(|| SettingValue { version: 0, value: spec.default_value() }))
    }

    fn set(&self, key: &str, value: Value, expected: Option<u64>) -> Result<SettingValue, String> {
        let spec = self.spec(key)?;
        let value = spec.check(value)?;
        let mut values = self.values.lock().map_err(|_| "settings lock poisoned".to_string())?;
        let current = values.get(spec.key).cloned().unwrap_or_else(|| SettingValue { version: 0, value: spec.default_value() });
        if expected.is_some_and(|expected| expected != current.version) {
            return Err(STALE.into());
        }
        if current.version > 0 && current.value == value {
            return Ok(current);
        }
        let stored = SettingValue { version: current.version + 1, value };
        values.insert(spec.key, stored.clone());
        Ok(stored)
    }
}

/// The refusal for a write made from an old read.
pub const STALE: &str = "Настройку только что изменили в другом месте: откройте её заново";

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
