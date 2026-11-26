use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use unidtk_api::settings::{ModuleSettings, SettingKind, SettingSpec};
use unidtk_api::{Module, ModuleContext};

use super::{KernelSettings, MODULE_SETTINGS};

const SPECS: &[SettingSpec] = &[
    SettingSpec { key: "auto", label: "Авто", summary: "", kind: SettingKind::Switch { default: false }, secret: false },
    SettingSpec { key: "links", label: "Ссылки", summary: "", kind: SettingKind::TextList { placeholder: "", max_items: 4, max_chars: 100 }, secret: true },
];

/// A module that upper-cases its links and refuses "bad", and records what it heard.
#[derive(Default)]
struct Probe {
    changed: Mutex<Vec<(String, u64)>>,
}

impl Module for Probe {
    fn id(&self) -> &'static str {
        "probe"
    }
    fn init(&self, _ctx: ModuleContext) -> Result<(), String> {
        Ok(())
    }
    fn settings(&self) -> &'static [SettingSpec] {
        SPECS
    }
    fn check_setting(&self, key: &str, _old: &Value, new: Value) -> Result<Value, String> {
        if key != "links" {
            return Ok(new);
        }
        let rows: Vec<Value> = new.as_array().unwrap().iter().map(|row| json!(row.as_str().unwrap().to_uppercase())).collect();
        if rows.iter().any(|row| row == "BAD") {
            return Err("refused".into());
        }
        Ok(Value::Array(rows))
    }
    fn setting_changed(&self, key: &str, version: u64) {
        self.changed.lock().unwrap().push((key.to_string(), version));
    }
}

fn fixture(name: &str) -> (Arc<redb::Database>, Arc<Probe>, KernelSettings, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("unidtk-settings-{name}-{}.redb", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let db = Arc::new(redb::Database::create(&path).unwrap());
    let probe = Arc::new(Probe::default());
    let module: Arc<dyn Module> = probe.clone();
    let settings = KernelSettings::new(db.clone(), &module);
    (db, probe, settings, path)
}

fn wait_for(probe: &Probe, count: usize) {
    for _ in 0..200 {
        if probe.changed.lock().unwrap().len() >= count {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_value_is_checked_stored_versioned_and_announced() {
    let (_db, probe, settings, path) = fixture("store");
    assert_eq!(settings.get("auto").unwrap().version, 0);
    let stored = settings.set("links", json!([" a ", "", "b"]), Some(0)).unwrap();
    assert_eq!((stored.version, stored.value.clone()), (1, json!(["A", "B"])), "declaration check, then the module's");
    assert_eq!(settings.get("links").unwrap(), stored);
    assert!(settings.set("links", json!(["bad"]), None).is_err(), "the module refuses");
    assert!(settings.set("links", json!(["c"]), Some(0)).is_err(), "a write from an old read");
    assert_eq!(settings.set("links", json!(["a", "b"]), Some(1)).unwrap().version, 1, "unchanged: no new version");
    assert!(settings.set("nope", json!(1), None).is_err());
    wait_for(&probe, 1);
    assert_eq!(probe.changed.lock().unwrap().as_slice(), [("links".to_string(), 1)]);
    let _ = std::fs::remove_file(path);
}

// A damaged value must not reset the version: the VPN's subscription revision is this
// version, and the panel ignores a revision that goes back.
#[test]
fn a_damaged_value_is_set_aside_and_the_version_goes_on() {
    let (db, _probe, settings, path) = fixture("damaged");
    settings.set("auto", json!(true), None).unwrap();
    settings.set("auto", json!(false), None).unwrap();
    {
        let txn = db.begin_write().unwrap();
        {
            let mut table = txn.open_table(MODULE_SETTINGS).unwrap();
            table.insert("probe/auto", b"{not json".as_slice()).unwrap();
        }
        txn.commit().unwrap();
    }
    let read = settings.get("auto").unwrap();
    assert_eq!((read.version, read.value), (2, json!(false)), "the default, at the old version");
    {
        let txn = db.begin_read().unwrap();
        let table = txn.open_table(MODULE_SETTINGS).unwrap();
        assert!(table.get("probe/auto.damaged").unwrap().is_some(), "kept aside for a look");
        assert!(table.get("probe/auto").unwrap().is_none());
    }
    assert_eq!(settings.set("auto", json!(true), Some(2)).unwrap().version, 3);
    let _ = std::fs::remove_file(path);
}
