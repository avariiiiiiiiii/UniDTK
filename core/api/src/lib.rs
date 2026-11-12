//! `unidtk-api`: the stable contract between the core and the modules.
//!
//! This crate holds **only** the contract. No `tauri` details of the core, no
//! database access: a module must be able to build without depending on the **core crate**,
//! otherwise an update of the core would start breaking modules again
//! (r[modules.contract-only]). Tauri host types (`AppHandle`, `Emitter`,
//! `async_runtime`) are allowed: a module owns its runtime and receives the
//! host via [`ModuleContext::host`].
//!
//! Three things a module gets from here:
//!
//! 1. **Core services**: [`log`], [`emit`], [`subscribe`], [`state`]. These are
//!    global functions, not an object that has to be passed through every
//!    call: the core installs its implementations once at startup, and the module just
//!    calls them. So a module writes logs and publishes events without knowing what
//!    sits behind them (Tauri and redb).
//! 2. **Module data**: [`ModuleDb`]. The core itself allocates the module a key namespace
//!    from its id, so "its own part of the database" exists automatically, and the module
//!    cannot take someone else's name.
//! 3. **Lifecycle and commands**: [`Module`]: id, initialization, shutdown, and the
//!    commands the module serves, declared with [`module_commands!`].
//!
//! ## Why services are global rather than context-based
//!
//! Passing `ctx` around would mean threading it through hundreds of places (one `vpn.rs`
//! alone has over a hundred log calls). A global service locator gives the same isolation:
//! the module sees exactly the surface declared here and nothing more. But it does not force
//! rewriting every function. Access to data, where a context is genuinely needed
//! (which database, which id), still goes through [`ModuleContext`].

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use serde_json::Value;

pub mod commands;
pub mod manifest;
pub mod paths;

// For `module_commands!`, so a module needs no `serde_json` of its own to use it.
#[doc(hidden)]
pub use serde_json as __serde_json;

// ═══════════════════════════════════════════════════════════════════════════════
// Module identification
// ═══════════════════════════════════════════════════════════════════════════════

/// Unique module identifier (`"mp3-player"`, `"telegram"`, `"vpn"`, ...).
/// Wrapped in a type so that an id and a slug cannot be confused by accident.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModuleId(String);

impl ModuleId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert hyphens to underscores: `"mp3-player"` -> `"mp3_player"`.
    /// Useful for table prefixes in redb and for store keys.
    pub fn storage_slug(&self) -> String {
        self.0.replace('-', "_")
    }
}

impl<T: Into<String>> From<T> for ModuleId {
    fn from(s: T) -> Self {
        Self::new(s)
    }
}

impl std::fmt::Display for ModuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::ops::Deref for ModuleId {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Core service: logs
// ═══════════════════════════════════════════════════════════════════════════════

type LogFn = dyn Fn(&str) + Send + Sync;

static LOGGER: OnceLock<Box<LogFn>> = OnceLock::new();

thread_local! {
    static THREAD_CAPTURE: std::cell::RefCell<Option<Arc<Mutex<Vec<String>>>>> = const { std::cell::RefCell::new(None) };
}

/// Install the log sink. Called only by the core, once at startup.
pub fn set_logger(f: impl Fn(&str) + Send + Sync + 'static) {
    let _ = LOGGER.set(Box::new(f));
}

/// Run function `f`, capturing all calls to [`log`] on the current thread.
/// Safe under panics: logs captured before a panic are NOT lost; they are returned
/// together with the result `std::thread::Result<R>`.
pub fn capture_logs_unwind<F, R>(f: F) -> (std::thread::Result<R>, Vec<String>)
where
    F: FnOnce() -> R,
{
    let buffer = Arc::new(Mutex::new(Vec::new()));
    THREAD_CAPTURE.with(|tc| {
        *tc.borrow_mut() = Some(Arc::clone(&buffer));
    });

    struct ResetGuard;
    impl Drop for ResetGuard {
        fn drop(&mut self) {
            THREAD_CAPTURE.with(|tc| {
                *tc.borrow_mut() = None;
            });
        }
    }
    let guard = ResetGuard;

    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    drop(guard);

    let logs = Arc::try_unwrap(buffer)
        .map(|m| m.into_inner().unwrap_or_default())
        .unwrap_or_else(|m| {
            m.lock()
                .map(|g| g.clone())
                .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
        });

    (res, logs)
}

/// Run function `f`, capturing all calls to [`log`] on the current thread.
/// Thread-safe, does not interfere with other threads, and restores state even on panic.
pub fn capture_logs<F, R>(f: F) -> (R, Vec<String>)
where
    F: FnOnce() -> R,
{
    let (res, logs) = capture_logs_unwind(std::panic::AssertUnwindSafe(f));
    match res {
        Ok(val) => (val, logs),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Write a line to the shared UniDTK log (formerly `crate::log_line`).
///
/// Safe to call before initialization: the call then silently does nothing.
pub fn log(message: impl AsRef<str>) {
    let msg = message.as_ref();
    let _ = THREAD_CAPTURE.with(|tc| {
        if let Some(buf) = tc.borrow().as_ref() {
            if let Ok(mut list) = buf.lock() {
                list.push(msg.to_string());
            }
        }
    });
    if let Some(logger) = LOGGER.get() {
        logger(msg);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Core service: events
// ═══════════════════════════════════════════════════════════════════════════════

type EmitFn = dyn Fn(&str, Value) + Send + Sync;
type Handler = Arc<dyn Fn(Value) + Send + Sync>;

static EMITTER: OnceLock<Box<EmitFn>> = OnceLock::new();
static BUS: OnceLock<Mutex<Bus>> = OnceLock::new();

#[derive(Default)]
struct Bus {
    /// Subscribers keyed by event name.
    handlers: HashMap<String, Vec<Handler>>,
    /// The last published value of each event.
    ///
    /// Needed so a module can learn the state without calling back into
    /// another module: `telegram` asks `state("vpn")` instead of
    /// calling `vpn::is_connected()`. This is exactly what breaks the vpn↔telegram cycle.
    last: HashMap<String, Value>,
}

fn bus() -> &'static Mutex<Bus> {
    BUS.get_or_init(|| Mutex::new(Bus::default()))
}

static EMIT_ORDER: Mutex<()> = Mutex::new(());

/// Install the event transport (Tauri emit). Called only by the core.
pub fn set_emitter(f: impl Fn(&str, Value) + Send + Sync + 'static) {
    let _ = EMITTER.set(Box::new(f));
}

/// Publish an event: remember the last value, deliver it to subscribers, and
/// send it to the frontend.
pub fn emit(event: &str, payload: Value) {
    // The stored value and the transports must agree on which emit came last, so
    // concurrent emits pass through both in one order. Handlers run outside: they
    // may emit themselves.
    let handlers = {
        let _order = EMIT_ORDER.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let handlers = match bus().lock() {
            Ok(mut bus) => {
                bus.last.insert(event.to_string(), payload.clone());
                bus.handlers.get(event).cloned().unwrap_or_default()
            }
            Err(_) => Vec::new(),
        };
        if let Some(emitter) = EMITTER.get() {
            emitter(event, payload.clone());
        }
        handlers
    };
    for handler in handlers {
        handler(payload.clone());
    }
}

/// Subscribe to an event. Called by a module during initialization.
pub fn subscribe(event: &str, handler: impl Fn(Value) + Send + Sync + 'static) {
    if let Ok(mut bus) = bus().lock() {
        bus.handlers
            .entry(event.to_string())
            .or_default()
            .push(Arc::new(handler));
    }
}

/// The last known value of an event (without waiting for the next one).
pub fn state(event: &str) -> Option<Value> {
    bus().lock().ok().and_then(|bus| bus.last.get(event).cloned())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Module data: the automatically allocated part of the database
// ═══════════════════════════════════════════════════════════════════════════════

/// A store issued to a module by the core.
///
/// The core allocates the namespace from the module's id, so the module declares
/// nothing in advance and cannot take someone else's name: physically the key looks like
/// `mods/<id>/<key>`, while the module sees only `<key>`.
///
/// Values are opaque bytes: the serialization format is chosen by the module itself
/// (mp3-player stores the library as bincode on top of `kv_batch`/`kv_list`);
/// the core knows neither the format nor the set of keys.
pub trait ModuleDb: Send + Sync {
    fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>, String>;
    fn kv_set(&self, key: &str, value: &[u8]) -> Result<(), String>;
    fn kv_remove(&self, key: &str) -> Result<(), String>;
    /// All pairs with the given prefix, in lexicographic key order.
    ///
    /// Keys are returned **with** the requested prefix and without the module's internal
    /// prefix (`mods/<id>/`): the received key can be passed straight to
    /// [`ModuleDb::kv_remove`] or [`DbOp::Remove`]. This is how playlist links
    /// are cleaned up, without rebuilding the key.
    fn kv_list(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, String>;
    /// Apply a batch of changes atomically (the redb implementation uses one transaction).
    /// Needed where one logical operation of the module touches several keys:
    /// without it, the write would split into independent commits.
    fn kv_batch(&self, ops: Vec<DbOp>) -> Result<(), String> {
        for op in ops {
            match op {
                DbOp::Set { key, value } => self.kv_set(&key, &value)?,
                DbOp::Remove { key } => self.kv_remove(&key)?,
            }
        }
        Ok(())
    }

    /// How many keys with the given prefix are stored.
    ///
    /// A separate method rather than `kv_list(..).len()`: for a module's file store,
    /// counting the whole file (empty prefix) is read from the B-tree in O(1), while
    /// the embeddings list can run to hundreds of elements.
    fn kv_count(&self, prefix: &str) -> Result<u64, String> {
        Ok(self.kv_list(prefix)?.len() as u64)
    }
}

/// One mutation in [`ModuleDb::kv_batch`].
pub enum DbOp {
    Set { key: String, value: Vec<u8> },
    Remove { key: String },
}

/// In-memory module store (for tests, isolation and `udtk doctor -modules`).
///
/// Implements the full [`ModuleDb`] contract without creating files on disk and without
/// touching redb or sqlite.
#[derive(Default)]
pub struct InMemoryModuleDb {
    map: RwLock<BTreeMap<String, Vec<u8>>>,
}

impl InMemoryModuleDb {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ModuleDb for InMemoryModuleDb {
    fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let guard = self.map.read().map_err(|e| e.to_string())?;
        Ok(guard.get(key).cloned())
    }

    fn kv_set(&self, key: &str, value: &[u8]) -> Result<(), String> {
        let mut guard = self.map.write().map_err(|e| e.to_string())?;
        guard.insert(key.to_string(), value.to_vec());
        Ok(())
    }

    fn kv_remove(&self, key: &str) -> Result<(), String> {
        let mut guard = self.map.write().map_err(|e| e.to_string())?;
        guard.remove(key);
        Ok(())
    }

    fn kv_list(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
        let guard = self.map.read().map_err(|e| e.to_string())?;
        let res = guard
            .range(prefix.to_string()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Ok(res)
    }

    fn kv_count(&self, prefix: &str) -> Result<u64, String> {
        let guard = self.map.read().map_err(|e| e.to_string())?;
        if prefix.is_empty() {
            Ok(guard.len() as u64)
        } else {
            let count = guard
                .range(prefix.to_string()..)
                .take_while(|(k, _)| k.starts_with(prefix))
                .count();
            Ok(count as u64)
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Module store: a slice of the shared database or a separate file
// ═══════════════════════════════════════════════════════════════════════════════

/// The kind of store a module needs.
///
/// The module declares **what** it needs; the core decides **where** it lives and with what
/// durability. No module may build a path itself: the data location is core knowledge
/// (otherwise moving the directories would affect all modules at once).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    /// A slice of the core's shared database (`mods/<id>/<key>`). Cheap, but shares the file with everyone:
    /// redb keeps one writer, so a long series of writes here would
    /// queue up UI writes. For small state and configs.
    Shared,
    /// A separate file. User data: ACID, included in backups, not removed by a
    /// "cache cleanup".
    OwnedDurable,
    /// A separate file. Derived and rebuildable data: writes without fsync on
    /// every commit; the file can be deleted as a whole without losing meaning.
    OwnedCache,
}

/// A store declaration made by a module.
pub struct StoreSpec {
    /// A short stable name: the core builds the file name from it. Path separators
    /// are forbidden; see [`StoreSpec::validate`].
    pub name: String,
    pub kind: StoreKind,
    /// Names of this store's previous files: the core moves them itself to the new
    /// location and new name on first open.
    pub legacy_files: Vec<String>,
    /// Previous module tables → key prefix in the new flat namespace.
    ///
    /// While the module kept its data in its own redb tables, the schema was its
    /// private business. The contract provides one key namespace, so the old
    /// layout must be rewritten once with prefixes, declaratively (see
    /// [`StoreSpec::legacy_tables`]).
    pub legacy_tables: Vec<(&'static str, String)>,
}

impl StoreSpec {
    pub fn new(name: impl Into<String>, kind: StoreKind) -> Self {
        Self {
            name: name.into(),
            kind,
            legacy_files: Vec::new(),
            legacy_tables: Vec::new(),
        }
    }

    /// A separate file for derived or rebuildable data.
    pub fn cache(name: impl Into<String>) -> Self {
        Self::new(name, StoreKind::OwnedCache)
    }

    /// A separate file for user data.
    pub fn durable(name: impl Into<String>) -> Self {
        Self::new(name, StoreKind::OwnedDurable)
    }

    /// Declare the previous file names; the core moves files from those names itself.
    pub fn legacy(mut self, files: &[&str]) -> Self {
        self.legacy_files = files.iter().map(|f| (*f).to_string()).collect();
        self
    }

    /// Declare previous tables: `("embeddings", "emb:")` — the rows of table
    /// `embeddings` move into keys `emb:<previous key>`.
    ///
    /// Performed by the core (see [`StoreFactory::open`]): only it knows redb, and
    /// the module has no access to it. The operation is idempotent: the old table no longer exists afterwards.
    pub fn legacy_tables(mut self, tables: &[(&'static str, &str)]) -> Self {
        self.legacy_tables = tables
            .iter()
            .map(|(table, prefix)| (*table, (*prefix).to_string()))
            .collect();
        self
    }

    /// The name must be safe for the file system: no separators, no upward
    /// traversal, and no paths; otherwise a module declaration would become a hole.
    pub fn validate(&self) -> Result<(), String> {
        let ok = !self.name.is_empty()
            && self.name.len() <= 64
            && self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if ok {
            Ok(())
        } else {
            Err(format!(
                "store name must be [A-Za-z0-9_-]{{1,64}}, got {:?}",
                self.name
            ))
        }
    }
}

/// What the core knows about a module's store, for diagnostics and cleanup.
pub struct StoreInfo {
    pub name: String,
    pub kind: StoreKind,
    /// `None`: the store lives inside the core's shared database.
    pub path: Option<std::path::PathBuf>,
}

/// Store factory: implemented by the core, declared in the contract (like [`ModuleDb`],
/// without a single dependency on redb, tauri or Win32).
pub trait StoreFactory: Send + Sync {
    /// Open the module's store, creating it if necessary.
    fn open(&self, id: &ModuleId, spec: &StoreSpec) -> Result<Arc<dyn ModuleDb>, String>;

    /// What this module has already declared.
    fn list(&self, id: &ModuleId) -> Vec<StoreInfo>;
}

/// Module run mode: production (in the core) or diagnostic (in the CLI doctor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleRunMode {
    /// Production run inside the application. AppHandle is available, and background threads are started.
    Production,
    /// Diagnostic run (udtk doctor). Network tunnels and endless background threads are forbidden.
    Diagnostic,
}

struct InMemoryStoreFactory;

impl StoreFactory for InMemoryStoreFactory {
    fn open(&self, _id: &ModuleId, _spec: &StoreSpec) -> Result<Arc<dyn ModuleDb>, String> {
        Ok(Arc::new(InMemoryModuleDb::new()))
    }
    fn list(&self, _id: &ModuleId) -> Vec<StoreInfo> {
        Vec::new()
    }
}

/// The context the core hands to a module on initialization.
pub struct ModuleContext {
    id: ModuleId,
    db: Arc<dyn ModuleDb>,
    /// The module's owner in the core: needed if the module requires something
    /// specific (a command handler, a state type). Downcast on the core side.
    host: Option<Arc<dyn Any + Send + Sync>>,
    /// Issues separate file stores. The core installs its own implementation;
    /// in unit tests it may be absent, in which case [`ModuleContext::store`] returns an error.
    stores: Option<Arc<dyn StoreFactory>>,
    run_mode: ModuleRunMode,
    /// Whether the core started the module. `false` means the user has disabled the module: the core
    /// still keeps the managed shell (so Tauri commands do not panic), but the module must NOT
    /// start heavy background work (TDLib, audio capture, sing-box,
    /// background poll). The module saves the resources itself, based on the gate at startup.
    enabled: bool,
}

impl ModuleContext {
    pub fn new(id: impl Into<ModuleId>, db: Arc<dyn ModuleDb>) -> Self {
        Self {
            id: id.into(),
            db,
            host: None,
            stores: None,
            run_mode: ModuleRunMode::Production,
            enabled: true,
        }
    }

    /// Create a diagnostic context for dry-run lifecycle testing (`udtk doctor -modules`).
    /// Creates no files on disk; it is equipped with an in-memory store factory.
    pub fn new_diagnostic(id: impl Into<ModuleId>, db: Arc<dyn ModuleDb>) -> Self {
        Self {
            id: id.into(),
            db,
            host: None,
            stores: Some(Arc::new(InMemoryStoreFactory)),
            run_mode: ModuleRunMode::Diagnostic,
            enabled: true,
        }
    }

    pub fn with_host(mut self, host: Arc<dyn Any + Send + Sync>) -> Self {
        self.host = Some(host);
        self
    }

    /// Install a store factory (only the core does this).
    pub fn with_store_factory(mut self, stores: Arc<dyn StoreFactory>) -> Self {
        self.stores = Some(stores);
        self
    }

    /// Mark the module as disabled by the user. The core still manages
    /// the shell, but the module does not start heavy background work.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn id(&self) -> &ModuleId {
        &self.id
    }

    pub fn run_mode(&self) -> ModuleRunMode {
        self.run_mode
    }

    pub fn is_diagnostic(&self) -> bool {
        self.run_mode == ModuleRunMode::Diagnostic
    }

    pub fn is_production(&self) -> bool {
        self.run_mode == ModuleRunMode::Production
    }

    /// Reference to the module's store (a key slice `mods/<slug>/...`).
    pub fn db(&self) -> Arc<dyn ModuleDb> {
        self.db.clone()
    }

    /// Take the whole store: the module owns it for its entire lifetime.
    pub fn into_db(self) -> Arc<dyn ModuleDb> {
        self.db
    }

    /// This module's own store: either a slice of the shared database or a separate file,
    /// depending on the declared [`StoreKind`]. The core chooses the location and durability.
    ///
    /// For [`StoreKind::Shared`] the same slice as [`Self::db`] is returned,
    /// so declaring the shared store explicitly has no point: it is the default.
    pub fn store(&self, spec: StoreSpec) -> Result<Arc<dyn ModuleDb>, String> {
        spec.validate()?;
        match spec.kind {
            StoreKind::Shared => Ok(self.db.clone()),
            _ => match self.stores.as_ref() {
                Some(factory) => factory.open(&self.id, &spec),
                None => Err(format!(
                    "store {:?}: kernel did not provide a store factory",
                    spec.name
                )),
            },
        }
    }

    /// What this module has already declared (for diagnostics and cache cleanup).
    pub fn stores(&self) -> Vec<StoreInfo> {
        match self.stores.as_ref() {
            Some(factory) => factory.list(&self.id),
            None => Vec::new(),
        }
    }

    /// Access to state the core put in place (for example `AppHandle`).
    pub fn host<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.host.as_ref().and_then(|host| host.downcast_ref::<T>())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Module lifecycle
// ═══════════════════════════════════════════════════════════════════════════════

/// Execution status of a diagnostic check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

/// The result of one diagnostic check of a module or a core subsystem.
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    pub logs: Vec<String>,
}

impl CheckResult {
    pub fn pass(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Pass,
            detail: detail.into(),
            logs: Vec::new(),
        }
    }

    pub fn warn(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Warn,
            detail: detail.into(),
            logs: Vec::new(),
        }
    }

    pub fn fail(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Fail,
            detail: detail.into(),
            logs: Vec::new(),
        }
    }

    pub fn with_logs(mut self, logs: Vec<String>) -> Self {
        self.logs = logs;
        self
    }
}

/// Context for dry-run functional checks (`udtk doctor -modules`).
/// Isolated from the production databases on disk.
pub struct DiagnosticContext {
    pub db: Arc<dyn ModuleDb>,
    pub dry_run: bool,
}

impl DiagnosticContext {
    pub fn new_in_memory() -> Self {
        Self {
            db: Arc::new(InMemoryModuleDb::new()),
            dry_run: true,
        }
    }
}

/// A key combination a module asks the kernel to listen for.
///
/// The kernel's low-level keyboard hook matches it. On a match it brings the desktop
/// forward when `shows_desktop` is set and publishes `event` with an empty string as
/// the payload, so the module's page reacts without the kernel knowing what it means.
#[derive(Clone, Copy, Debug)]
pub struct HotkeySpec {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// The combination as people write it, such as `Win+E`.
    pub key: &'static str,
    /// The Windows virtual-key code of the non-modifier key.
    pub vk_code: u32,
    pub win: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The key is consumed, so Windows does not act on it too.
    pub swallow: bool,
    /// Only while the desktop or a shell window is in front; anywhere else the key
    /// stays with Windows.
    pub desktop_only: bool,
    pub event: &'static str,
    pub shows_desktop: bool,
}

/// A UniDTK module. The core holds a set of these and calls them on startup and shutdown.
pub trait Module: Send + Sync {
    /// Stable identifier: matches the folder `modules/<id>/`.
    fn id(&self) -> &'static str;

    /// Declarative manifest of the module's system dependencies (binaries, DLLs, assets, tasks).
    fn dependencies(&self) -> &'static [manifest::ModuleDependency] {
        &[]
    }

    /// Ids of the modules this one must start after (and so stop before). The kernel
    /// orders the start by these; an unknown id or a cycle is a startup error.
    /// Not to be confused with [`Self::dependencies`], which lists files and tasks.
    fn starts_after(&self) -> &'static [&'static str] {
        &[]
    }

    /// Initialization: event subscriptions, opening its own tables.
    fn init(&self, ctx: ModuleContext) -> Result<(), String>;

    /// The first pass of the exit: raise the module's exit latches, so that no
    /// other module's stop can start its work again. Cheap, never waits, and
    /// idempotent; called on every module before any [`Self::shutdown`].
    fn begin_shutdown(&self) {}

    /// Shutdown with a time budget: stop background threads, close the module's processes.
    /// A zero budget means "do not wait at all": the caller is the main thread.
    fn shutdown(&self, _budget: std::time::Duration) -> Result<(), String> {
        Ok(())
    }

    /// Disabling by the user on the fly: the application keeps running, so
    /// global exit latches must not be set here.
    fn suspend(&self) -> Result<(), String> {
        self.shutdown(std::time::Duration::from_millis(1500))
    }

    /// Enabling on the fly, including a module that was disabled since the last launch.
    fn resume(&self, ctx: ModuleContext) -> Result<(), String> {
        self.init(ctx)
    }

    /// The commands this module serves, usually the `COMMANDS` its
    /// [`module_commands!`] table generates.
    fn commands(&self) -> &'static [commands::CommandSpec] {
        &[]
    }

    /// Runs one of [`Self::commands`], usually through the table's `dispatch_command`.
    /// `None` means the name is not this module's. The kernel calls it on a blocking
    /// thread, never on the main thread, and only after the door was checked.
    fn dispatch(&self, _cmd: &str, _args: &Value) -> Option<Result<Value, String>> {
        None
    }

    /// The events this module publishes whose newest value is all that matters: a
    /// state, not a stream. The kernel's event broker keeps one frame of each and
    /// coalesces a burst into the newest, so a subscriber gets the current value at
    /// once instead of a replay of samples.
    fn state_events(&self) -> &'static [&'static str] {
        &[]
    }

    /// The hotkeys this module contributes to the kernel's keyboard hook.
    fn hotkeys(&self) -> &'static [HotkeySpec] {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_module_db_operations() {
        let db = InMemoryModuleDb::new();
        assert_eq!(db.kv_get("test").unwrap(), None);
        db.kv_set("alpha", b"1").unwrap();
        db.kv_set("beta", b"2").unwrap();
        db.kv_set("albatross", b"3").unwrap();
        assert_eq!(db.kv_get("alpha").unwrap(), Some(b"1".to_vec()));
        assert_eq!(db.kv_count("al").unwrap(), 2);
        assert_eq!(db.kv_count("").unwrap(), 3);
        let list = db.kv_list("al").unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].0, "albatross");
        assert_eq!(list[1].0, "alpha");
        db.kv_remove("alpha").unwrap();
        assert_eq!(db.kv_get("alpha").unwrap(), None);
    }

    #[test]
    fn capture_logs_intercepts_thread_local() {
        let (_, logs) = capture_logs(|| {
            log("capture test 1");
            log("capture test 2");
        });
        assert_eq!(logs, vec!["capture test 1", "capture test 2"]);
    }

    #[test]
    fn module_id_slug_is_safe_for_keys() {
        assert_eq!(ModuleId::new("mp3-player").storage_slug(), "mp3_player");
        assert_eq!(ModuleId::new("vpn").storage_slug(), "vpn");
    }

    #[test]
    fn events_remember_last_state() {
        // A fresh bus cache in this test may already have been populated by another test in the
        // same process, so we check specifically the last value that was written.
        emit("vpn", serde_json::json!({ "connected": true }));
        assert_eq!(
            state("vpn").and_then(|v| v["connected"].as_bool()),
            Some(true)
        );
        emit("vpn", serde_json::json!({ "connected": false }));
        assert_eq!(
            state("vpn").and_then(|v| v["connected"].as_bool()),
            Some(false)
        );
    }

    #[test]
    fn subscribers_receive_emitted_payload() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        subscribe("test-event", move |payload| {
            if let Ok(mut list) = sink.lock() {
                list.push(payload);
            }
        });
        emit("test-event", serde_json::json!({ "n": 1 }));
        let list = seen.lock().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["n"], 1);
    }

    #[test]
    fn logger_is_optional_and_silent_before_init() {
        // Nothing should panic, even if the core did not install a logger.
        log("no logger is fine");
    }

    #[test]
    fn capture_logs_unwind_preserves_logs_on_panic() {
        let (res, logs) = capture_logs_unwind(std::panic::AssertUnwindSafe(|| {
            log("before panic 1");
            log("before panic 2");
            panic!("test simulated panic");
        }));
        assert!(res.is_err());
        assert_eq!(logs, vec!["before panic 1", "before panic 2"]);
    }

    #[test]
    fn module_context_run_mode_diagnostic() {
        let ctx = ModuleContext::new_diagnostic("test-mod", Arc::new(InMemoryModuleDb::new()));
        assert!(ctx.is_diagnostic());
        assert!(!ctx.is_production());
        assert_eq!(ctx.run_mode(), ModuleRunMode::Diagnostic);
        // InMemoryStoreFactory returns an InMemoryModuleDb on store()
        let store = ctx.store(StoreSpec::cache("scratch")).unwrap();
        store.kv_set("foo", b"bar").unwrap();
        assert_eq!(store.kv_get("foo").unwrap(), Some(b"bar".to_vec()));
    }

    #[test]
    fn manifest_declaration() {
        let dep = manifest::ModuleDependency {
            id: "test:bin",
            description: "A test binary",
            severity: manifest::DependencySeverity::Mandatory,
            kind: manifest::DependencyKind::Executable {
                file_name: "test.exe",
                probe_args: Some(&["--version"]),
            },
        };
        assert_eq!(dep.id, "test:bin");
        assert_eq!(dep.severity, manifest::DependencySeverity::Mandatory);
    }
}
