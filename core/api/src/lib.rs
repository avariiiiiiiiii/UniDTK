//! `unidtk-api` — стабильный контракт между ядром и модулями.
//!
//! Здесь живёт **только** контракт. Никаких `tauri`-деталей ядра, никакого
//! доступа к базе: модуль должен уметь собираться, не завися от **крейта ядра**,
//! иначе обновление ядра снова начнёт ломать модули
//! (`docs/architecture.md` §1.3). Типы Tauri-хоста (`AppHandle`, `Emitter`,
//! `async_runtime`) при этом допустимы: модуль владеет своим рантаймом и получает
//! хост через [`ModuleContext::host`].
//!
//! Три вещи, которые модуль получает отсюда:
//!
//! 1. **Сервисы ядра** — [`log`], [`emit`], [`subscribe`], [`state`]. Это
//!    глобальные функции, а не объект, который надо протаскивать через каждый
//!    вызов: ядро подставляет свои реализации один раз при старте, модуль просто
//!    зовёт их. Так модуль пишет лог и публикует события, не зная, что за этим
//!    стоит Tauri и redb.
//! 2. **Данные модуля** — [`ModuleDb`]. Ядро само выделяет модулю пространство
//!    ключей по его id, поэтому «своя часть базы» появляется автоматически и
//!    модуль не может занять чужое имя.
//! 3. **Жизненный цикл** — [`Module`]: id, инициализация, остановка.
//!
//! ## Почему сервисы глобальные, а не через контекст
//!
//! Через `ctx` пришлось бы протянуть его в сотни мест (в одном `vpn.rs` больше
//! сотни вызовов лога). Глобальный сервис-локатор даёт ту же изоляцию — модуль
//! видит ровно объявленную здесь поверхность и ничего больше — но не заставляет
//! переписывать каждую функцию. Доступ к данным, где действительно нужен
//! контекст (какая база, какой id), по-прежнему идёт через [`ModuleContext`].

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use serde_json::Value;

pub mod commands;
pub mod manifest;
pub mod paths;

// ═══════════════════════════════════════════════════════════════════════════════
// Идентификация модуля
// ═══════════════════════════════════════════════════════════════════════════════

/// Уникальный идентификатор модуля (`"mp3-player"`, `"telegram"`, `"vpn"`, ...).
/// Обернут в тип, чтобы нельзя было случайно перепутать id и slug.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModuleId(String);

impl ModuleId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Превратить дефисы в подчеркивания: `"mp3-player"` -> `"mp3_player"`.
    /// Полезно для префиксов таблиц в redb и ключей хранилищ.
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
// Сервис ядра: логи
// ═══════════════════════════════════════════════════════════════════════════════

type LogFn = dyn Fn(&str) + Send + Sync;

static LOGGER: OnceLock<Box<LogFn>> = OnceLock::new();

thread_local! {
    static THREAD_CAPTURE: std::cell::RefCell<Option<Arc<Mutex<Vec<String>>>>> = const { std::cell::RefCell::new(None) };
}

/// Установить приёмник логов. Вызывает только ядро, один раз при старте.
pub fn set_logger(f: impl Fn(&str) + Send + Sync + 'static) {
    let _ = LOGGER.set(Box::new(f));
}

/// Выполнить функцию `f`, перехватывая все вызовы [`log`] в текущем потоке.
/// Безопасен при паниках: перехваченные до паники логи НЕ теряются, а возвращаются
/// вместе с результатом `std::thread::Result<R>`.
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

/// Выполнить функцию `f`, перехватывая все вызовы [`log`] в текущем потоке.
/// Потокобезопасно, не мешает другим потокам и восстанавливает состояние даже при панике.
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

/// Записать строку в общий журнал Shell (раньше — `crate::log_line`).
///
/// Безопасно вызывать до инициализации: тогда вызов молча ничего не делает.
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
// Сервис ядра: события
// ═══════════════════════════════════════════════════════════════════════════════

type EmitFn = dyn Fn(&str, Value) + Send + Sync;
type Handler = Arc<dyn Fn(Value) + Send + Sync>;

static EMITTER: OnceLock<Box<EmitFn>> = OnceLock::new();
static BUS: OnceLock<Mutex<Bus>> = OnceLock::new();

#[derive(Default)]
struct Bus {
    /// Подписчики по имени события.
    handlers: HashMap<String, Vec<Handler>>,
    /// Последнее опубликованное значение каждого события.
    ///
    /// Нужен, чтобы модуль мог узнать состояние без обратного вызова к
    /// другому модулю: `telegram` спрашивает `state("vpn")`, вместо того чтобы
    /// звать `vpn::is_connected()` — именно это разрывает цикл vpn↔telegram.
    last: HashMap<String, Value>,
}

fn bus() -> &'static Mutex<Bus> {
    BUS.get_or_init(|| Mutex::new(Bus::default()))
}

/// Установить транспорт событий (Tauri emit). Вызывает только ядро.
pub fn set_emitter(f: impl Fn(&str, Value) + Send + Sync + 'static) {
    let _ = EMITTER.set(Box::new(f));
}

/// Опубликовать событие: запомнить последнее значение, раздать подписчикам и
/// отправить во фронтенд.
pub fn emit(event: &str, payload: Value) {
    let handlers = {
        match bus().lock() {
            Ok(mut bus) => {
                bus.last.insert(event.to_string(), payload.clone());
                bus.handlers.get(event).cloned().unwrap_or_default()
            }
            Err(_) => Vec::new(),
        }
    };
    for handler in handlers {
        handler(payload.clone());
    }
    if let Some(emitter) = EMITTER.get() {
        emitter(event, payload);
    }
}

/// Подписаться на событие. Вызывается модулем при инициализации.
pub fn subscribe(event: &str, handler: impl Fn(Value) + Send + Sync + 'static) {
    if let Ok(mut bus) = bus().lock() {
        bus.handlers
            .entry(event.to_string())
            .or_default()
            .push(Arc::new(handler));
    }
}

/// Последнее известное значение события (без ожидания следующего).
pub fn state(event: &str) -> Option<Value> {
    bus().lock().ok().and_then(|bus| bus.last.get(event).cloned())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Данные модуля: автоматически выделяемая часть базы
// ═══════════════════════════════════════════════════════════════════════════════

/// Хранилище, выданное модулю ядром.
///
/// Ядро выделяет пространство имён по id модуля, поэтому модуль не объявляет
/// ничего заранее и не может занять чужое имя: физически ключ выглядит как
/// `mods/<id>/<key>`, а модуль видит только `<key>`.
///
/// Значения — непрозрачные байты: формат сериализации выбирает сам модуль
/// (mp3-player хранит библиотеку как bincode поверх `kv_batch`/`kv_list`),
/// ядро не знает ни формата, ни состава ключей.
pub trait ModuleDb: Send + Sync {
    fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>, String>;
    fn kv_set(&self, key: &str, value: &[u8]) -> Result<(), String>;
    fn kv_remove(&self, key: &str) -> Result<(), String>;
    /// Все пары с данным префиксом, в лексикографическом порядке ключей.
    ///
    /// Ключи возвращаются **с** запрошенным префиксом и без служебного префикса
    /// модуля (`mods/<id>/`): полученный ключ можно сразу передать в
    /// [`ModuleDb::kv_remove`] или [`DbOp::Remove`] — так чистятся связи
    /// плейлистов, не собирая ключ заново.
    fn kv_list(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, String>;
    /// Применить пачку изменений атомарно (redb-реализация — одной транзакцией).
    /// Нужна там, где одна логическая операция модуля трогает несколько ключей:
    /// без неё запись рвётся на независимые коммиты.
    fn kv_batch(&self, ops: Vec<DbOp>) -> Result<(), String> {
        for op in ops {
            match op {
                DbOp::Set { key, value } => self.kv_set(&key, &value)?,
                DbOp::Remove { key } => self.kv_remove(&key)?,
            }
        }
        Ok(())
    }

    /// Сколько ключей с данным префиксом лежит в хранилище.
    ///
    /// Отдельный метод, а не `kv_list(..).len()`, потому что у файлового
    /// хранилища модуля подсчёт всего файла (пустой префикс) берётся из
    /// B-дерева за O(1), а список эмбеддингов бывает в сотни элементов.
    fn kv_count(&self, prefix: &str) -> Result<u64, String> {
        Ok(self.kv_list(prefix)?.len() as u64)
    }
}

/// Одна мутация в [`ModuleDb::kv_batch`].
pub enum DbOp {
    Set { key: String, value: Vec<u8> },
    Remove { key: String },
}

/// Хранилище модуля в оперативной памяти (для тестов, изоляции и `udtk doctor -modules`).
///
/// Реализует полный контракт [`ModuleDb`] без создания файлов на диске и без
/// обращения к redb/sqlite.
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
// Хранилище модуля: срез общей базы или отдельный файл
// ═══════════════════════════════════════════════════════════════════════════════

/// Какого рода хранилище нужен модулю.
///
/// Модуль объявляет **что** ему нужно, ядро решает **где** это лежит и с какой
/// надёжностью. Ни один модуль не имеет права собирать путь сам: расположение
/// данных — знание ядра (иначе переезд каталогов задевает все модули сразу).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    /// Срез общей базы ядра (`mods/<id>/<key>`). Дёшево, но делит файл со всеми:
    /// redb держит одного писателя, поэтому длинная серия записей здесь
    /// выстраивает в очередь запись UI. Для мелкого состояния и конфигов.
    Shared,
    /// Отдельный файл. Данные пользователя: ACID, попадают в бэкап, «очисткой
    /// кэша» не сносятся.
    OwnedDurable,
    /// Отдельный файл. Производные и пересобираемые данные: запись без fsync на
    /// каждый коммит, файл можно удалить целиком без потери смысла.
    OwnedCache,
}

/// Объявление хранилища модулем.
pub struct StoreSpec {
    /// Короткое стабильное имя: из него ядро строит имя файла. Разделители пути
    /// запрещены — см. [`StoreSpec::validate`].
    pub name: String,
    pub kind: StoreKind,
    /// Имена прежних файлов этого хранилища: ядро само переносит их на новое
    /// место и новое имя при первом открытии.
    pub legacy_files: Vec<String>,
    /// Прежние таблицы модуля → префикс ключей в новом плоском пространстве.
    ///
    /// Пока модуль держал данные в собственных redb-таблицах, схема была его
    /// личным делом. Контракт даёт одно пространство ключей, поэтому старую
    /// раскладку нужно один раз переписать с префиксами — декларативно, иначе
    /// (см. [`StoreSpec::legacy_tables`]).
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

    /// Отдельный файл для производных/пересобираемых данных.
    pub fn cache(name: impl Into<String>) -> Self {
        Self::new(name, StoreKind::OwnedCache)
    }

    /// Отдельный файл для данных пользователя.
    pub fn durable(name: impl Into<String>) -> Self {
        Self::new(name, StoreKind::OwnedDurable)
    }

    /// Объявить прежние имена файла — ядро переедет с них само.
    pub fn legacy(mut self, files: &[&str]) -> Self {
        self.legacy_files = files.iter().map(|f| (*f).to_string()).collect();
        self
    }

    /// Объявить прежние таблицы: `("embeddings", "emb:")` — строки таблицы
    /// `embeddings` переезжают в ключи `emb:<прежний ключ>`.
    ///
    /// Выполняет это ядро (см. [`StoreFactory::open`]): redb знает только оно,
    /// модулю он недоступен. Операция идемпотентна — старой таблицы после неё нет.
    pub fn legacy_tables(mut self, tables: &[(&'static str, &str)]) -> Self {
        self.legacy_tables = tables
            .iter()
            .map(|(table, prefix)| (*table, (*prefix).to_string()))
            .collect();
        self
    }

    /// Имя должно быть безопасным для файловой системы: никаких разделителей,
    /// переходов вверх и путей — иначе объявление модуля стало бы дыркой.
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

/// Что ядро знает про хранилище модуля — для диагностики и очистки.
pub struct StoreInfo {
    pub name: String,
    pub kind: StoreKind,
    /// `None` — хранилище внутри общей базы ядра.
    pub path: Option<std::path::PathBuf>,
}

/// Фабрика хранилищ: реализуется ядром, объявлена в контракте (как и [`ModuleDb`],
/// без единой зависимости на redb/tauri/Win32).
pub trait StoreFactory: Send + Sync {
    /// Открыть (при необходимости — создать) хранилище модуля.
    fn open(&self, id: &ModuleId, spec: &StoreSpec) -> Result<Arc<dyn ModuleDb>, String>;

    /// Что уже объявлено этим модулем.
    fn list(&self, id: &ModuleId) -> Vec<StoreInfo>;
}

/// Режим запуска модуля: боевой (в ядре) или диагностический (в CLI doctor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleRunMode {
    /// Боевой запуск внутри приложения. Доступен AppHandle, запускаются фоновые потоки.
    Production,
    /// Диагностический прогон (udtk doctor). Запрещены сетевые туннели, фоновые бесконечные потоки.
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

/// Контекст, который ядро выдаёт модулю при инициализации.
pub struct ModuleContext {
    id: ModuleId,
    db: Arc<dyn ModuleDb>,
    /// Владелец модуля в ядре — нужен, если модулю понадобится что-то
    /// специфичное (обработчик команд, тип состояния). Downcast на стороне ядра.
    host: Option<Arc<dyn Any + Send + Sync>>,
    /// Выдаёт отдельные файловые хранилища. Ядро подставляет свою реализацию;
    /// в юнит-тестах её может не быть — тогда [`ModuleContext::store`] вернёт ошибку.
    stores: Option<Arc<dyn StoreFactory>>,
    run_mode: ModuleRunMode,
    /// Запущен ли модуль ядром. `false` — модуль "выключен" пользователем: ядро
    /// по-прежнему держит managed-оболочку (чтобы Tauri-команды не паниковали),
    /// но модуль обязан НЕ поднимать тяжёлый фон (TDLib, аудиозахват, sing-box,
    /// fоновый poll). Ресурсы экономит сам модуль, от гейта на старте.
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

    /// Создать диагностический контекст для сухого тестирования жизненного цикла (`udtk doctor -modules`).
    /// Не создает файлов на диске, снабжен in-memory фабрикой хранилищ.
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

    /// Подставить фабрику хранилищ (делает только ядро).
    pub fn with_store_factory(mut self, stores: Arc<dyn StoreFactory>) -> Self {
        self.stores = Some(stores);
        self
    }

    /// Пометить модуль выключенным пользователем. Ядро всё равно управляет
    /// оболочкой, но модуль не поднимает тяжёлый фон.
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

    /// Ссылка на хранилище модуля (срез ключей `mods/<slug>/...`).
    pub fn db(&self) -> Arc<dyn ModuleDb> {
        self.db.clone()
    }

    /// Забрать хранилище целиком: модуль владеет им всю свою жизнь.
    pub fn into_db(self) -> Arc<dyn ModuleDb> {
        self.db
    }

    /// Своё хранилище модуля: либо срез общей базы, либо отдельный файл — по
    /// объявленному [`StoreKind`]. Расположение и надёжность выбирает ядро.
    ///
    /// Для [`StoreKind::Shared`] возвращается тот же срез, что и [`Self::db`],
    /// поэтому явно объявлять общее хранилище смысла нет — оно есть по умолчанию.
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

    /// Что уже объявлено этим модулем (для диагностики и очистки кэша).
    pub fn stores(&self) -> Vec<StoreInfo> {
        match self.stores.as_ref() {
            Some(factory) => factory.list(&self.id),
            None => Vec::new(),
        }
    }

    /// Доступ к состоянию, которое положило ядро (например `AppHandle`).
    pub fn host<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.host.as_ref().and_then(|host| host.downcast_ref::<T>())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Жизненный цикл модуля
// ═══════════════════════════════════════════════════════════════════════════════

/// Статус выполнения диагностической проверки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

/// Результат одной диагностической проверки модуля или подсистемы ядра.
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

/// Контекст для сухих функциональных проверок (`udtk doctor -modules`).
/// Изолирован от боевых баз данных на диске.
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

/// Модуль Shell. Ядро держит набор таких и вызывает их на старте/остановке.
pub trait Module: Send + Sync {
    /// Стабильный идентификатор: совпадает с папкой в `modules/<id>/`.
    fn id(&self) -> &'static str;

    /// Декларативный манифест системных зависимостей модуля (бинарники, DLL, ассеты, задачи).
    fn dependencies(&self) -> &'static [manifest::ModuleDependency] {
        &[]
    }

    /// Инициализация: подписки на события, открытие своих таблиц.
    fn init(&self, ctx: ModuleContext) -> Result<(), String>;

    /// Остановка с бюджетом времени: снять фоновые потоки, закрыть процессы модуля.
    fn shutdown(&self, _budget: std::time::Duration) -> Result<(), String> {
        Ok(())
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
        // Свежий шинный кэш в тесте уже мог быть заполнен другим тестом того же
        // процесса, поэтому проверяем именно последнее записанное значение.
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
        // Ничего не должно паниковать, даже если ядро не подставило логгер.
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
