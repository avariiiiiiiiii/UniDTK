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
                settings.auto_connect = true;
                entered.wait();
                release.wait();
            })
        });
        entered.wait();
        let lock_is_held = matches!(store.revision.try_lock(), Err(TryLockError::WouldBlock));
        let second =
            scope.spawn(|| store.update(|settings| settings.last_tag = Some("chosen".into())));
        release.wait();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
        assert!(lock_is_held);
    });
    let saved = store.read().unwrap();
    assert!(saved.auto_connect);
    assert_eq!(saved.last_tag.as_deref(), Some("chosen"));
}

#[test]
fn failed_writes_preserve_settings_and_the_subscription_revision() {
    let dir = LegacyDir::new();
    let db = Arc::new(InMemoryModuleDb::new());
    let original = br#"{"subscription_url":"https://old.example/","auto_connect":true}"#;
    db.kv_set(SETTINGS, original).unwrap();
    db.kv_set(CACHE, b"previous cache").unwrap();
    let store = SettingsStore::new(Arc::new(RejectWrites(db.clone())), dir.0.clone());

    assert!(store
        .update(|settings| settings.auto_connect = false)
        .is_err());
    assert!(store
        .update(|settings| settings.subscription_url = "https://new.example/".into())
        .is_err());
    assert_eq!(db.kv_get(SETTINGS).unwrap().as_deref(), Some(&original[..]));
    assert_eq!(
        db.kv_get(CACHE).unwrap().as_deref(),
        Some(&b"previous cache"[..])
    );
    assert_eq!(store.snapshot(None).unwrap().revision, 0);
}

#[test]
fn changing_the_subscription_invalidates_even_an_earlier_fetch_of_the_same_url() {
    let dir = LegacyDir::new();
    let db = Arc::new(InMemoryModuleDb::new());
    let store = SettingsStore::new(db.clone(), dir.0.clone());
    let first_url = "https://first.example/";
    store
        .update(|settings| settings.subscription_url = first_url.into())
        .unwrap();
    let first = store.snapshot(None).unwrap();
    db.kv_set(CACHE, b"existing cache").unwrap();
    store
        .update(|settings| settings.subscription_url = first_url.into())
        .unwrap();
    assert_eq!(store.snapshot(Some(first.revision)).unwrap().revision, first.revision);
    assert_eq!(
        db.kv_get(CACHE).unwrap().as_deref(),
        Some(&b"existing cache"[..])
    );

    store
        .update(|settings| settings.subscription_url = "https://second.example/".into())
        .unwrap();
    store
        .update(|settings| settings.subscription_url = first_url.into())
        .unwrap();
    store
        .remember_connected(first.revision, "stale tag".into())
        .unwrap();
    let last = store.snapshot(None).unwrap();
    assert_eq!(last.settings.subscription_url, first_url);
    assert!(last.settings.last_tag.is_none());
    assert_ne!(last.revision, first.revision);
    assert_eq!(
        store.snapshot(Some(first.revision)).err().as_deref(),
        Some(SUBSCRIPTION_CHANGED)
    );
    assert_eq!(db.kv_get(CACHE).unwrap().as_deref(), Some(&b"[]"[..]));
    let result: Result<(), String> = store.with_current_subscription(first.revision, || {
        panic!("a stale fetch must never publish");
    });
    assert!(result.is_err());
}

#[test]
fn connection_reservation_checks_the_subscription_even_for_a_warm_list() {
    let dir = LegacyDir::new();
    let store = SettingsStore::new(Arc::new(InMemoryModuleDb::new()), dir.0.clone());
    store.update(|settings| settings.subscription_url = "https://old.example/".into()).unwrap();
    let old = store.snapshot(None).unwrap();
    let mut inner = RuntimeInner {
        servers: vec![parse_vless_line("vless://test@old.example:443#same").unwrap()],
        servers_revision: old.revision,
        connected_revision: old.revision,
        connected: true,
        selected_tag: Some("same".into()),
        ..RuntimeInner::default()
    };
    assert!(inner.reserve_connect(&store, &old, Some("same")).unwrap().is_none());
    assert!(!inner.connecting);

    store.update(|settings| settings.subscription_url = "https://new.example/".into()).unwrap();
    assert_eq!(
        inner.reserve_connect(&store, &old, Some("same")).err().as_deref(),
        Some(SUBSCRIPTION_CHANGED)
    );
    assert!(!inner.connecting);
    assert!(inner.connected);

    let current = store.snapshot(None).unwrap();
    inner.use_subscription(current.revision);
    inner.servers.push(parse_vless_line("vless://test@new.example:443#same").unwrap());
    let (server, _) = inner.reserve_connect(&store, &current, Some("same")).unwrap().unwrap();
    assert!(inner.connecting);
    assert_eq!(server.host, "new.example");
}

#[test]
fn legacy_settings_are_removed_only_after_a_successful_import() {
    let dir = LegacyDir::new();
    let db = Arc::new(InMemoryModuleDb::new());
    let store = SettingsStore::new(db.clone(), dir.0.clone());
    assert!(!store.read().unwrap().auto_connect);
    assert!(db.kv_get(SETTINGS).unwrap().is_none());

    let legacy = "\u{feff}{\"auto_connect\":true,\"home_server\":{}}";
    fs::write(dir.settings_path(), legacy).unwrap();
    let read_only = SettingsStore::new(Arc::new(RejectWrites(db.clone())), dir.0.clone());
    assert!(read_only.read().is_err());
    assert_eq!(fs::read_to_string(dir.settings_path()).unwrap(), legacy);

    let imported = store.read().unwrap();
    assert!(imported.auto_connect);
    assert_eq!(imported.home_server.port, 443);
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
        .update(|settings| settings.auto_connect = true)
        .is_err());
    assert_eq!(fs::read_to_string(dir.settings_path()).unwrap(), "not JSON");
    assert!(db.kv_get(SETTINGS).unwrap().is_none());

    db.kv_set(SETTINGS, b"bad record").unwrap();
    fs::write(dir.settings_path(), "{}").unwrap();
    assert!(store.read().is_err());
    assert!(store
        .update(|settings| settings.auto_connect = true)
        .is_err());
    assert_eq!(
        db.kv_get(SETTINGS).unwrap().as_deref(),
        Some(&b"bad record"[..])
    );
    assert_eq!(fs::read_to_string(dir.settings_path()).unwrap(), "{}");
}
