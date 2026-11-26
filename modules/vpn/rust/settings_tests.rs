use super::*;
use super::super::{parse_vless_line, RuntimeInner};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Barrier, TryLockError};
use unidtk_api::InMemoryModuleDb;

struct LegacyDir(PathBuf);

impl LegacyDir {
    fn new() -> Self {
        static ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "unidtk-vpn-settings-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn settings_path(&self) -> PathBuf {
        self.0.join("vpn_settings.json")
    }
}

impl Drop for LegacyDir {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.settings_path());
        let _ = fs::remove_dir(&self.0);
    }
}

struct RejectWrites(Arc<InMemoryModuleDb>);

impl ModuleDb for RejectWrites {
    fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        self.0.kv_get(key)
    }

    fn kv_set(&self, _: &str, _: &[u8]) -> Result<(), String> {
        Err("test storage failure".into())
    }

    fn kv_remove(&self, _: &str) -> Result<(), String> {
        Err("test storage failure".into())
    }

    fn kv_list(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
        self.0.kv_list(prefix)
    }
}

#[test]
fn concurrent_writes_merge_on_the_latest_settings() {
    let dir = LegacyDir::new();
    let store = SettingsStore::new(Arc::new(InMemoryModuleDb::new()), dir.0.clone());
    let entered = Barrier::new(2);
    let release = Barrier::new(2);
    std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            store.update(|settings| {
                settings.default_tag = "picked".into();
                entered.wait();
                release.wait();
            })
        });
        entered.wait();
        let lock_is_held = matches!(store.lock.try_lock(), Err(TryLockError::WouldBlock));
        let second =
            scope.spawn(|| store.update(|settings| settings.last_tag = Some("chosen".into())));
        release.wait();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
        assert!(lock_is_held);
    });
    let saved = store.read().unwrap();
    assert_eq!(saved.default_tag, "picked");
    assert_eq!(saved.last_tag.as_deref(), Some("chosen"));
}

fn declared() -> Arc<unidtk_api::settings::InMemorySettings> {
    Arc::new(unidtk_api::settings::InMemorySettings::new(crate::DECLARED_SETTINGS))
}

#[test]
fn failed_writes_preserve_settings() {
    let dir = LegacyDir::new();
    let db = Arc::new(InMemoryModuleDb::new());
    let original = br#"{"default_tag":"kept"}"#;
    db.kv_set(SETTINGS, original).unwrap();
    let store = SettingsStore::new(Arc::new(RejectWrites(db.clone())), dir.0.clone());
    assert!(store.update(|settings| settings.default_tag = "changed".into()).is_err());
    assert_eq!(db.kv_get(SETTINGS).unwrap().as_deref(), Some(&original[..]));
    assert_eq!(store.snapshot(None).unwrap().revision, 0);
}

// The revision is the stored version of the subscriptions (r[modules.settings-contract]):
// it survives a kernel restart, so the panel never sees it go back.
#[test]
fn the_revision_is_the_stored_version_of_the_subscriptions() {
    let dir = LegacyDir::new();
    let store = SettingsStore::new(Arc::new(InMemoryModuleDb::new()), dir.0.clone());
    let declared = declared();
    store.declare(declared.clone());
    declared.set("subscriptions", serde_json::json!(["https://first.example/"]), None).unwrap();
    let first = store.snapshot(None).unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(first.subscriptions.len(), 1);
    assert_eq!(first.subscriptions[0].url, "https://first.example/");
    assert_eq!(first.subscriptions[0].id.len(), 8);

    declared.set("subscriptions", serde_json::json!(["https://first.example/"]), None).unwrap();
    assert_eq!(store.snapshot(Some(first.revision)).unwrap().revision, 1, "an unchanged list keeps its revision");

    declared.set("subscriptions", serde_json::json!(["https://first.example/", "https://second.example/"]), None).unwrap();
    store.remember_connected(first.revision, "stale/key".into()).unwrap();
    assert!(store.read().unwrap().last_tag.is_none(), "a connect from the old list is not remembered");
    assert_eq!(store.snapshot(Some(first.revision)).err().as_deref(), Some(SUBSCRIPTION_CHANGED));
    let result: Result<(), String> = store.with_current_subscription(first.revision, || {
        panic!("a stale fetch must never publish");
    });
    assert!(result.is_err());
    let second = store.snapshot(None).unwrap();
    assert_eq!(second.subscriptions[0].id, first.subscriptions[0].id, "an address keeps its id");
}

#[test]
fn every_subscription_keeps_its_own_cache_and_a_removed_one_loses_it() {
    let dir = LegacyDir::new();
    let db = Arc::new(InMemoryModuleDb::new());
    let store = SettingsStore::new(db.clone(), dir.0.clone());
    let server = parse_vless_line("vless://id@a.example:443#a").unwrap();
    store.save_servers_cache("aaaa1111", std::slice::from_ref(&server)).unwrap();
    store.save_servers_cache("bbbb2222", std::slice::from_ref(&server)).unwrap();
    db.kv_set("servers_cache", b"[]").unwrap();
    assert_eq!(store.load_servers_cache("aaaa1111").len(), 1);
    store.prune_caches(&["aaaa1111".to_string()]);
    assert_eq!(store.load_servers_cache("aaaa1111").len(), 1);
    assert!(store.load_servers_cache("bbbb2222").is_empty());
    assert!(db.kv_get("servers_cache").unwrap().is_none(), "the single cache of old is gone");
}

#[test]
fn connection_reservation_checks_the_subscription_even_for_a_warm_list() {
    let dir = LegacyDir::new();
    let store = SettingsStore::new(Arc::new(InMemoryModuleDb::new()), dir.0.clone());
    let declared = declared();
    store.declare(declared.clone());
    declared.set("subscriptions", serde_json::json!(["https://old.example/"]), None).unwrap();
    let old = store.snapshot(None).unwrap();
    let mut server = parse_vless_line("vless://test@old.example:443#same").unwrap();
    server.key = "old0id00/same".into();
    let mut inner = RuntimeInner {
        servers: vec![server],
        servers_revision: old.revision,
        connected_revision: old.revision,
        connected: true,
        selected_tag: Some("old0id00/same".into()),
        ..RuntimeInner::default()
    };
    assert!(inner.reserve_connect(&store, &old, Some("old0id00/same")).unwrap().is_none());
    assert!(!inner.connecting);

    declared.set("subscriptions", serde_json::json!(["https://new.example/"]), None).unwrap();
    assert_eq!(
        inner.reserve_connect(&store, &old, Some("old0id00/same")).err().as_deref(),
        Some(SUBSCRIPTION_CHANGED)
    );
    assert!(!inner.connecting);
    assert!(inner.connected);

    let current = store.snapshot(None).unwrap();
    inner.use_subscription(current.revision);
    let mut fresh = parse_vless_line("vless://test@new.example:443#same").unwrap();
    fresh.key = "new0id00/same".into();
    inner.servers.push(fresh);
    let (server, _) = inner.reserve_connect(&store, &current, Some("new0id00/same")).unwrap().unwrap();
    assert!(inner.connecting);
    assert_eq!(server.host, "new.example");
}

#[test]
fn legacy_settings_are_removed_only_after_a_successful_import() {
    let dir = LegacyDir::new();
    let db = Arc::new(InMemoryModuleDb::new());
    let store = SettingsStore::new(db.clone(), dir.0.clone());
    assert!(store.read().unwrap().default_tag.is_empty());
    assert!(db.kv_get(SETTINGS).unwrap().is_none());

    let legacy = "\u{feff}{\"default_tag\":\"legacy\",\"home_server\":{}}";
    fs::write(dir.settings_path(), legacy).unwrap();
    let read_only = SettingsStore::new(Arc::new(RejectWrites(db.clone())), dir.0.clone());
    assert!(read_only.read().is_err());
    assert_eq!(fs::read_to_string(dir.settings_path()).unwrap(), legacy);

    let imported = store.read().unwrap();
    assert_eq!(imported.default_tag, "legacy");
    assert!(db.kv_get(SETTINGS).unwrap().is_some());
    assert!(!dir.settings_path().exists());
}

#[test]
fn malformed_storage_cannot_be_overwritten_with_defaults_or_legacy_data() {
    let dir = LegacyDir::new();
    let db = Arc::new(InMemoryModuleDb::new());
    let store = SettingsStore::new(db.clone(), dir.0.clone());
    fs::write(dir.settings_path(), "not JSON").unwrap();
    assert!(store
        .update(|settings| settings.default_tag = "x".into())
        .is_err());
    assert_eq!(fs::read_to_string(dir.settings_path()).unwrap(), "not JSON");
    assert!(db.kv_get(SETTINGS).unwrap().is_none());

    db.kv_set(SETTINGS, b"bad record").unwrap();
    fs::write(dir.settings_path(), "{}").unwrap();
    assert!(store.read().is_err());
    assert!(store
        .update(|settings| settings.default_tag = "x".into())
        .is_err());
    assert_eq!(
        db.kv_get(SETTINGS).unwrap().as_deref(),
        Some(&b"bad record"[..])
    );
    assert_eq!(fs::read_to_string(dir.settings_path()).unwrap(), "{}");
}
