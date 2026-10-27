//! Докачка модели AI DJ (`discogs-maest-129e`) в каталог данных.
//!
//! Когда модель не пришла с установщиком, модуль достаёт её сам — по явному
//! действию пользователя в виджете: триггер [`start_download`], прогресс —
//! событие `ai-dj-model`. Модель весит ~331 МБ, поэтому правила такие:
//!
//! * файлы идут через `.part`-временные имена и переименовываются атомарно:
//!   недокачанный кусок никогда не похож на готовую модель (валидность
//!   проверяет [`ai_dj::is_valid_onnx_model`]);
//! * HTTP Range resume: после обрыва (таймаут чтения, RST, перезапуск
//!   приложения) докачка продолжается с сохранённого смещения;
//! * сначала прямой HTTPS; при повторных отказах тот же URL уходит через SOCKS
//!   `127.0.0.1:12908` (sing-box, как в модуле vpn). ByeDPI (12909) намеренно
//!   не задействован. Если и SOCKS не отвечает — честная ошибка
//!   «модель недоступна, нужен VPN»;
//! * синглфлайт: повторный запрос не кладёт 331 МБ дважды;
//! * ход не под `ONNX_SESSION`: у докачки своя нить и свой статический мьютекс;
//! * ни `init`, ни автозапуск ничего не сетят — загрузчик будят только из
//!   команды `ai_dj_model_download`.
//!
//! Гарантии integrity: точный размер — из ответа сервера (`Content-Length` /
//! `Content-Range`), SHA-256 — считается поточно во время скачивания и
//! пересчитывается при resume (по префиксу `.part`), результат пишется в
//! sidecar-манифест `download_manifest.json` рядом с моделью. Хеш-константы
//! заранее не хардкодятся: источник может сменить файл, а неверная константа
//! закодировала бы отказ скачивания в бинарник.

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::ai::ai_dj;

// ── Имена файлов и каталог ───────────────────────────────────────────────────

/// Каталог модели внутри `unidtk_api::paths::models_dir()` — выводится из
/// `MODEL_REL` (`discogs-maest-129e/model.onnx`), поэтому раскладка берётся из
/// одного места: контракта загрузчика ([`ai_dj::MODEL_REL`]).
fn model_rel_dir() -> String {
    Path::new(ai_dj::MODEL_REL)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string())
}

/// Имя файла графа (122 КБ) — хвост `MODEL_REL`.
fn graph_file() -> String {
    Path::new(ai_dj::MODEL_REL)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "model.onnx".to_string())
}

/// Внешние веса (347 MB): компаньон `model.onnx.data`, которого требует
/// `is_valid_onnx_model` для графов меньше 50 MB.
fn weights_file() -> String {
    format!("{0}.data", graph_file())
}

/// Порядок скачивания: сначала маленький граф — за один запрос проверяется,
/// что источник отвечает, потом тянется длинная цепочка весов.
const FILE_ORDER: [&str; 2] = ["graph", "weights"];

/// Суффикс временного файла: `model.onnx.data.part`.
const PART_SUFFIX: &str = ".part";

/// Sidecar-манифест по соседству с моделью.
const MANIFEST_NAME: &str = "download_manifest.json";

fn file_for(kind: &str) -> String {
    match kind {
        "weights" => weights_file(),
        _ => graph_file(),
    }
}

fn final_path_for(dir: &Path, kind: &str) -> PathBuf {
    dir.join(file_for(kind))
}

/// `.part`-имя — дословное добавление суффикса к полному имени файла.
/// `with_extension` здесь портит имена (`model.onnx` → `model.part`).
fn part_path_of(final_path: &Path) -> PathBuf {
    let mut os = final_path.as_os_str().to_os_string();
    os.push(PART_SUFFIX);
    PathBuf::from(os)
}

// ── Сеть: таймауты, транспорты, источник ─────────────────────────────────────

/// SOCKS-порт sing-box на локали (как в модуле vpn, вход `socks-in`).
const SOCKS_PORT: u16 = 12908;

/// Сколько всего попыток на файл; внутри попытки — свой список транспортов.
const MAX_ATTEMPTS: u32 = 5;
/// Бэкофф между попытками: 1, 2, 4, 8, 16 c — обрыв на 200 MB не должен
/// откладывать resume на минуты.
const BACKOFF_STEP: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(16);

/// Таймауты соединения. Интент иной, чем у micrographics: тот закрывает
/// короткие ответы метео-API «overall»-таймаутом; докачка живёт минутами,
/// поэтому общий таймаут не ставится вовсе, а `timeout_read` работает как
/// детектор застоя — если за 30 c не пришло ни байта, попытка обрывается и
/// возобновляется со смещения.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

const USER_AGENT: &str = "unidtk-mp3-player/1.0";
/// Окно прогресса в событии `ai-dj-model`: не чаще, чем раз в 250 мс.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// Размер буфера потоковой записи (64 KiB).
const CHUNK: usize = 64 * 1024;

/// Транспорт попытки. ByeDPI (порт 12909) сюда не входит по построению.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Transport {
    Direct,
    Socks,
}

impl Transport {
    const fn label(self) -> &'static str {
        match self {
            Transport::Direct => "direct",
            Transport::Socks => "socks",
        }
    }

    fn agent(self) -> Result<ureq::Agent, String> {
        let builder = ureq::AgentBuilder::new()
            .user_agent(USER_AGENT)
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout_read(STALL_TIMEOUT)
            .timeout_write(WRITE_TIMEOUT);
        let builder = match self {
            Transport::Direct => builder,
            Transport::Socks => {
                let proxy = ureq::Proxy::new(format!("socks5://127.0.0.1:{SOCKS_PORT}"))
                    .map_err(|e| format!("не удалось настроить SOCKS {SOCKS_PORT}: {e}"))?;
                builder.proxy(proxy)
            }
        };
        Ok(builder.build())
    }
}

/// Какие транспорты пробовать в этой попытке.
///
/// Политика «прямой HTTPS first»: первый заход — только прямой; если отказ
/// повторился — сразу SOCKS; дальние попытки идут по цепочке direct → socks,
/// потому что с живой сетью прямой транспорт может ожить.
fn transports_for_attempt(attempt: u32) -> &'static [Transport] {
    match attempt {
        1 => &[Transport::Direct],
        2 => &[Transport::Socks],
        _ => &[Transport::Direct, Transport::Socks],
    }
}

/// Бэкофф: 1s · 2^(n-1) с потолком [`BACKOFF_MAX`].
fn backoff_delay(attempt: u32) -> Duration {
    BACKOFF_STEP
        .saturating_mul(1u32 << (attempt.saturating_sub(1).min(4)))
        .min(BACKOFF_MAX)
}

/// База источника по умолчанию — заявленный UPF-репозиторий на Hugging Face.
///
/// Проверено в рантайме: в репо `mtg-upf/discogs-maest-30s-pw-129e` файлов
/// `model.onnx(.data)` нет ни в корне, ни среди siblings API — ONNX-экспорт
/// делали локально (в чекауте он лежит в `opt/models`). Такой URL честно
/// отвечает 404, и загрузчик отдаёт пользователю понятную ошибку, пока
/// источник не появится. Перекрыть адрес можно без пересборки:
/// `AI_DJ_MODEL_URL=<база-до-файлов>/`.
const DEFAULT_SOURCE_BASE: &str = "https://huggingface.co/mtg-upf/discogs-maest-30s-pw-129e/resolve/main";

/// Переопределяемый источник: база, от которой достраиваются имена файлов.
fn source_url(file_name: &str) -> String {
    let base = match std::env::var("AI_DJ_MODEL_URL") {
        Ok(url) if !url.trim().is_empty() => url.trim().trim_end_matches('/').to_string(),
        _ => DEFAULT_SOURCE_BASE.to_string(),
    };
    format!("{base}/{file_name}")
}

// ── Каталог модели и тестовый шов ─────────────────────────────────────────────

/// Каталог, в котором живёт докачанная модель:
/// `%LOCALAPPDATA%\UniDTK\models\discogs-maest-129e\`.
///
/// Тесты обязаны выставить [`TEST_BASE_DIR`]: без переопределения функция
/// паникует, чтобы unit-тест физически не смог писать в пользовательский
/// AppData.
#[cfg(test)]
static TEST_BASE_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

fn model_dir() -> PathBuf {
    #[cfg(test)]
    {
        let guard = TEST_BASE_DIR.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(dir) = guard.as_ref() {
            return dir.clone();
        }
        panic!("model_fetch tests must set TEST_BASE_DIR (защита от записи в пользовательский AppData)");
    }
    #[cfg(not(test))]
    {
        unidtk_api::paths::models_dir().join(model_rel_dir())
    }
}

/// Тестовый шов для проверки «модель уже есть»: в рантайме предикат лоадера,
/// в тестах — флаг-подмена.
#[cfg(test)]
static TEST_PRESENCE: Mutex<Option<bool>> = Mutex::new(None);

/// «Модель присутствует и валидна» — ровно предикат лоадера
/// ([`ai_dj::find_model_path`]), так что докачка затевается только тогда,
/// когда анализ реально упёрся бы в отсутствие модели.
pub fn is_model_present() -> bool {
    #[cfg(test)]
    {
        let guard = TEST_PRESENCE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(present) = *guard {
            return present;
        }
    }
    ai_dj::find_model_path().is_ok()
}

// ── State machine: синглфлайт ────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Idle,
    Running,
    Error,
}

impl Phase {
    const fn into_static(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Running => "running",
            Phase::Error => "error",
        }
    }
}

#[derive(Debug)]
struct Runtime {
    epoch: u64,
    phase: Phase,
    /// Скачанные байты цепочки: готовые файлы + текущий `.part`.
    bytes_done: u64,
    /// Полный объём цепочки, когда сервер дал размеры.
    bytes_total: Option<u64>,
    file: Option<String>,
    attempt: u32,
    source: Option<&'static str>,
    error: Option<String>,
}

impl Runtime {
    const fn new() -> Self {
        Self {
            epoch: 0,
            phase: Phase::Idle,
            bytes_done: 0,
            bytes_total: None,
            file: None,
            attempt: 0,
            source: None,
            error: None,
        }
    }
}

fn lock_runtime() -> std::sync::MutexGuard<'static, Runtime> {
    static RUNTIME: Mutex<Runtime> = Mutex::new(Runtime::new());
    RUNTIME.lock().unwrap_or_else(|p| p.into_inner())
}

static NEXT_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Замороженный срез состояния докачки — его читает `ai_dj_status`.
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    /// `idle` | `running` | `error`.
    pub phase: &'static str,
    /// Текст последней ошибки (`phase = error`); вычищается при новом запуске.
    pub error: Option<String>,
    /// Файл текущей попытки (`model.onnx.data` / `model.onnx`).
    pub file: Option<String>,
    /// Скачанные байты цепочки.
    pub bytes_done: u64,
    /// Полный объём цепочки, когда сервер дал размеры.
    pub bytes_total: Option<u64>,
    pub attempt: u32,
    /// `direct` | `socks` — транспорт текущей попытки.
    pub source: Option<&'static str>,
}

fn snapshot_of(rt: &Runtime) -> Snapshot {
    Snapshot {
        phase: rt.phase.into_static(),
        error: rt.error.clone(),
        file: rt.file.clone(),
        bytes_done: rt.bytes_done,
        bytes_total: rt.bytes_total,
        attempt: rt.attempt,
        source: rt.source,
    }
}

impl Snapshot {
    /// Прямой ответ команды: результат триггера + живой срез состояния.
    fn into_result(self, result: &'static str) -> serde_json::Value {
        serde_json::json!({
            "result": result,
            "phase": self.phase,
            "file": self.file,
            "bytesDone": self.bytes_done,
            "bytesTotal": self.bytes_total,
            "attempt": self.attempt,
            "source": self.source,
            "error": self.error,
        })
    }
}

pub fn snapshot() -> Snapshot {
    let rt = lock_runtime();
    snapshot_of(&rt)
}

/// Захват состояния под новый запуск. Синглфлайт: уже идущий ход не
/// подменяется, валидная модель не докачивается второй раз.
fn try_begin(update: impl FnOnce(&mut Runtime)) -> Result<&'static str, &'static str> {
    let mut rt = lock_runtime();
    // Уже идёт? Это проверяется раньше presence: во время валидации
    // завершающего хода модель могла уже появиться — пусть доживёт своя нить.
    if rt.phase == Phase::Running {
        return Err("running");
    }
    if is_model_present() {
        return Err("already_present");
    }
    update(&mut rt);
    Ok("started")
}

/// Триггер докачки — не ждёт окончания и не трогает сессию ONNX.
/// Ответ немедленный: `{"result": "started" | "running" | "already_present"}`.
///
/// Ошибку докачки команда не возвращает: долговая работа идёт в своей нити и
/// живёт в событии `ai-dj-model` (`stage:"error"`) и в [Snapshot] / статусе.
pub fn start_download() -> Result<serde_json::Value, String> {
    let epoch = NEXT_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    let outcome = try_begin(|rt: &mut Runtime| {
        rt.epoch = epoch;
        rt.phase = Phase::Running;
        rt.error = None;
        rt.bytes_done = 0;
        rt.bytes_total = None;
        rt.file = None;
        rt.attempt = 0;
        rt.source = None;
    });
    match outcome {
        Ok("started") => {
            // Нить забирает ход и публикует прогресс событиями; команда
            // возвращается сразу, не ожидая 331 МБ.
            match std::thread::Builder::new()
                .name("ai-dj-model-fetch".into())
                .spawn(move || worker(epoch))
            {
                Ok(_handle) => Ok(snapshot().into_result("started")),
                Err(error) => {
                    // Нить не взлетела: честная ошибка вместо «запущено».
                    let message = format!("не удалось запустить поток загрузки: {error}");
                    let mut rt = lock_runtime();
                    if rt.epoch == epoch && rt.phase == Phase::Running {
                        rt.phase = Phase::Error;
                        rt.error = Some(message.clone());
                    }
                    let snap = snapshot_of(&rt);
                    drop(rt);
                    emit_stage(
                        "error",
                        snap.file.clone(),
                        Some(snap.bytes_done),
                        snap.bytes_total,
                        Some(snap.attempt),
                        snap.source,
                        Some(message),
                    );
                    Ok(snap.into_result("started"))
                }
            }
        }
        // Уже есть валидная модель или уже идёт ход — идемпотентные ответы.
        Ok(other) => Ok(snapshot().into_result(other)),
        Err(other) => Ok(snapshot().into_result(other)),
    }
}

// ── Прогресс и события ───────────────────────────────────────────────────────

/// Событие `ai-dj-model`: единственный канал, по которому виджет видит ход
/// докачки и ошибку. С потоком анализа (`ai-dj-progress`) не смешивается.
fn emit_stage(
    stage: &str,
    file: Option<String>,
    bytes_done: Option<u64>,
    bytes_total: Option<u64>,
    attempt: Option<u32>,
    source: Option<&'static str>,
    message: Option<String>,
) {
    let mut payload = serde_json::json!({
        "stage": stage,
        "fileName": file,
        "bytesDone": bytes_done,
        "bytesTotal": bytes_total,
        "attempt": attempt,
        "source": source,
    });
    if let Some(message) = message {
        payload["message"] = serde_json::json!(message);
    }
    unidtk_api::emit("ai-dj-model", payload);
}

/// Срез прогресса одного файла, который нить отдаёт в runtime и в событие.
#[derive(Clone, Copy, Debug)]
struct FileProgress {
    bytes_done: u64,
    bytes_total: Option<u64>,
    attempt: u32,
    source: &'static str,
}

impl FileProgress {
    fn emit(self, stage: &str, file: &str, message: Option<String>) {
        emit_stage(
            stage,
            Some(file.to_string()),
            Some(self.bytes_done),
            self.bytes_total,
            Some(self.attempt),
            Some(self.source),
            message,
        );
    }
}

/// Обновление runtime только если ход всё ещё тот же (нить нового триггера
/// не должна затираться старой нитью и наоборот).
fn report_progress(epoch: u64, done_before: u64, file: &str, progress: FileProgress) {
    let mut rt = lock_runtime();
    if rt.epoch != epoch || rt.phase != Phase::Running {
        return;
    }
    rt.file = Some(file.to_string());
    rt.bytes_done = done_before + progress.bytes_done;
    rt.bytes_total = progress.bytes_total.map(|t| done_before + t);
    rt.attempt = progress.attempt;
    rt.source = Some(progress.source);
}

// ── Ход загрузки ─────────────────────────────────────────────────────────────

fn worker(epoch: u64) {
    let outcome = run_chain(epoch);
    let (message, snap) = {
        let mut rt = lock_runtime();
        // Ход мог быть перебит новым триггером (синглфлайт это не допускает,
        // но актуальное состояние актуальную нить затирать не должна).
        if rt.epoch != epoch || rt.phase != Phase::Running {
            return;
        }
        match &outcome {
            Ok(()) => rt.phase = Phase::Idle,
            Err(message) => {
                rt.phase = Phase::Error;
                rt.error = Some(message.clone());
            }
        }
        (rt.error.clone(), snapshot_of(&rt))
    };
    match outcome {
        Ok(()) => emit_stage(
            "done",
            None,
            Some(snap.bytes_done),
            snap.bytes_total,
            Some(snap.attempt),
            snap.source,
            None,
        ),
        Err(_) => emit_stage(
            "error",
            snap.file,
            Some(snap.bytes_done),
            snap.bytes_total,
            Some(snap.attempt),
            snap.source,
            message,
        ),
    }
}

/// Цепочка по обоим файлам: маленький граф (быстрая проверка источника),
/// затем длинные веса. После завершения — проверка тем же предикатом, что
/// пользуется загрузчик.
fn run_chain(epoch: u64) -> Result<(), String> {
    let dir = model_dir();
    fs::create_dir_all(&dir)
        .map_err(|e| format!("не удалось создать каталог модели {}: {e}", dir.display()))?;

    let mut done_before = 0u64;
    for kind in FILE_ORDER {
        let file_name = file_for(kind);
        let final_path = final_path_for(&dir, kind);
        let part_path = part_path_of(&final_path);
        let result = fetch_file(epoch, kind, done_before, &part_path, &final_path)?;
        record_manifest(
            &dir,
            &file_name,
            ManifestFile {
                url: source_url(&file_name),
                size: result.0,
                sha256: result.1,
                revision: None,
                timestamp: unix_now(),
            },
        )?;
        done_before += result.0;
        unidtk_api::log(format!(
            "mp3-player: AI DJ модель «{file_name}» готова ({} байт), sha256 записан в {MANIFEST_NAME}",
            result.0
        ));
    }

    let graph_path = dir.join(graph_file());
    if !ai_dj::is_valid_onnx_model(&graph_path) {
        return Err(
            "модель скачана, но не прошла проверку графа/весов — попробуйте загрузку заново"
                .to_string(),
        );
    }
    unidtk_api::log("mp3-player: AI DJ модель докачана в каталог данных");
    Ok(())
}

/// Одиночная ошибка попытки: Fatal закрывает файл целиком (источник битый),
/// Retryable сохраняет `.part` и идёт на следующую попытку (опционально
/// со сбросом смещения).
#[derive(Debug)]
enum AttemptFail {
    Fatal(String),
    Retryable { message: String, reset_offset: bool },
}

fn fetch_file(
    epoch: u64,
    kind: &str,
    done_before: u64,
    part_path: &Path,
    final_path: &Path,
) -> Result<(u64, String), String> {
    let file_name = file_for(kind);
    let url = source_url(&file_name);

    // Файл этого хода может быть уже на диске с прошлой цепочки (граф готов,
    // оборвало на весах, наоборот): фиксируем его и не качаем заново.
    if final_path.is_file() {
        if let Ok(meta) = fs::metadata(final_path) {
            let size = meta.len();
            if size > 0 {
                let sha = sha256_file(final_path)?;
                report_progress(
                    epoch,
                    done_before,
                    &file_name,
                    FileProgress {
                        bytes_done: size,
                        bytes_total: Some(size),
                        attempt: 1,
                        source: Transport::Direct.label(),
                    },
                );
                unidtk_api::log(format!(
                    "mp3-player: AI DJ модель «{file_name}» уже на диске ({size} байт), докачиваем остальное"
                ));
                return Ok((size, sha));
            }
        }
    }

    let mut last_fail = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        for transport in transports_for_attempt(attempt) {
            match download_attempt(
                epoch,
                done_before,
                &file_name,
                *transport,
                &url,
                part_path,
                attempt,
            ) {
                Ok(result) => return Ok(result),
                Err(fail) => match fail {
                    AttemptFail::Fatal(message) => {
                        let text = format!("источник недоступен: {message}");
                        unidtk_api::log(format!("mp3-player: AI DJ модель: {text}"));
                        return Err(text);
                    }
                    AttemptFail::Retryable {
                        message,
                        reset_offset,
                    } => {
                        last_fail = message.clone();
                        unidtk_api::log(format!(
                            "mp3-player: AI DJ модель «{file_name}», попытка {attempt}/{MAX_ATTEMPTS} ({}) не удалась: {message}",
                            transport.label()
                        ));
                        if reset_offset {
                            if let Err(seal_error) = truncate_part(part_path) {
                                unidtk_api::log(format!(
                                    "mp3-player: сброс .part не удался: {seal_error}"
                                ));
                                last_fail =
                                    format!("{message}; сброс .part не удался: {seal_error}");
                            }
                        }
                    }
                },
            }
        }
        if attempt < MAX_ATTEMPTS {
            std::thread::sleep(backoff_delay(attempt));
        }
    }

    // Честная ошибка: не «тихая» неудача и не паника скрытого состояния.
    Err(format!(
        "AI DJ модель недоступна: не отвечает ни прямая сеть, ни SOCKS {SOCKS_PORT}. Держите VPN или задайте рабочий источник в AI_DJ_MODEL_URL. Последняя причина: {last_fail}"
    ))
}

/// Одна попытка скачать файл: Range-resume c текущего смещения `.part`,
/// потоковая запись, сквозной SHA-256 и валидация размера/хеша.
#[allow(clippy::too_many_arguments)]
fn download_attempt(
    epoch: u64,
    done_before: u64,
    file_name: &str,
    transport: Transport,
    url: &str,
    part_path: &Path,
    attempt: u32,
) -> Result<(u64, String), AttemptFail> {
    let agent = transport
        .agent()
        .map_err(|message| AttemptFail::Fatal(message))?;
    let offset = resume_offset(part_path);
    let request = if offset > 0 {
        agent
            .get(url)
            .set("Range", &format!("bytes={offset}-"))
    } else {
        agent.get(url)
    };
    let response = request
        .call()
        .map_err(classify_request_error)?;

    let status = response.status();
    // Свидетели integrity: точный размер и (если сервер расскажет) хеш.
    let (total, expected_sha) = read_total(&response);

    let mut offset = offset;
    let total = match status {
        206 => {
            // Проверяем, что сервер согласился с нашим смещением.
            let full = response
                .header("content-range")
                .and_then(parse_content_range)
                .ok_or_else(|| AttemptFail::Retryable {
                    message: "сервер ответил 206 без Content-Range".to_string(),
                    reset_offset: true,
                })?;
            if full <= offset {
                return Err(AttemptFail::Retryable {
                    message: format!(
                        "сервер говорит, что файл {full} байт, а .part уже {offset}"
                    ),
                    reset_offset: true,
                });
            }
            Some(full)
        }
        200 => {
            if offset > 0 {
                // Сервер проигнорировал Range — резюме невозможно, ".part"
                // обязан начаться с нуля. Если сброс не удался, писать поверх
                // нельзя (получится смесь со старым содержимым) — попытка
                // повторяется после честной ошибки.
                truncate_part(part_path).map_err(|e| AttemptFail::Retryable {
                    message: format!("сервер проигнорировал Range, а .part не сброшен: {e}"),
                    reset_offset: true,
                })?;
                offset = 0;
            }
            total
        }
        _ => {
            return Err(AttemptFail::Fatal(format!(
                "неожиданный HTTP-статус {status}"
            )));
        }
    };

    let started = FileProgress {
        bytes_done: offset,
        bytes_total: total,
        attempt,
        source: transport.label(),
    };
    report_progress(epoch, done_before, file_name, started);
    started.emit("download", file_name, None);

    let mut stream = PartStream::open(part_path, offset).map_err(|message| AttemptFail::Retryable {
        message,
        reset_offset: true,
    })?;
    let mut reader = response.into_reader();
    let mut buffer = vec![0u8; CHUNK];
    let mut last_emit = Instant::now() - PROGRESS_INTERVAL;

    loop {
        let read = reader.read(&mut buffer).map_err(|error| AttemptFail::Retryable {
            message: format!("обрыв при чтении на {offset}+{} байт: {error}", stream.written),
            reset_offset: false,
        })?;
        if read == 0 {
            break;
        }
        stream.write_chunk(&buffer[..read]).map_err(|message| {
            AttemptFail::Retryable {
                message,
                reset_offset: true,
            }
        })?;
        if let Some(limit) = total {
            if offset + stream.written > limit {
                return Err(AttemptFail::Retryable {
                    message: format!(
                        "сервер отдал больше байт, чем обещал Content-Length ({limit})"
                    ),
                    reset_offset: true,
                });
            }
        }
        if last_emit.elapsed() >= PROGRESS_INTERVAL {
            let progress = FileProgress {
                bytes_done: offset + stream.written,
                bytes_total: total,
                attempt,
                source: transport.label(),
            };
            report_progress(epoch, done_before, file_name, progress);
            progress.emit("download", file_name, None);
            last_emit = Instant::now();
        }
    }

    let (written, sha) = stream.finish();
    let bytes = offset + written;
    if let Some(limit) = total {
        if bytes != limit {
            return Err(AttemptFail::Retryable {
                message: format!("размер не сходится: получено {bytes} из {limit}"),
                reset_offset: true,
            });
        }
    }
    if let Some(expected) = &expected_sha {
        // Хеш, который дал сервер, не совпал: при resume это смешанные
        // правки/кодировка — та же причина, что и у страницы, меняющейся
        // на ходу. С обнулённым offset это битый источник.
        if *expected != sha {
            let message = format!("SHA-256 не совпал с заявленным сервером ({expected})");
            return Err(if offset > 0 {
                AttemptFail::Retryable {
                    message,
                    reset_offset: true,
                }
            } else {
                AttemptFail::Fatal(message)
            });
        }
    }
    Ok((bytes, sha))
}

/// Классификация ошибки запроса до начала чтения тела.
fn classify_request_error(error: ureq::Error) -> AttemptFail {
    match error {
        ureq::Error::Status(code, response) => {
            let status_text = response.status_text().to_string();
            let base = format!("HTTP {code} {status_text}").trim_end().to_string();
            match code {
                // Источник навеки отвечает «нет»; остальное держим честно.
                401 | 403 | 404 | 405 | 410 => AttemptFail::Fatal(base),
                // Смещение не по зубам серверу — .part больше файла.
                416 => AttemptFail::Retryable {
                    message: format!("{base}: .part несовместим, начинаем заново"),
                    reset_offset: true,
                },
                // Капризы сервера и лимиты: попробовать снова.
                408 | 425 | 429 => AttemptFail::Retryable {
                    message: base,
                    reset_offset: false,
                },
                c if c >= 500 => AttemptFail::Retryable {
                    message: base,
                    reset_offset: false,
                },
                _ => AttemptFail::Fatal(base),
            }
        }
        ureq::Error::Transport(transport) => {
            let message = transport.to_string();
            match transport.kind() {
                // Плохой адрес — это не сбой сети, а ошибка конфигурации.
                ureq::ErrorKind::InvalidUrl
                | ureq::ErrorKind::UnknownScheme
                | ureq::ErrorKind::TooManyRedirects
                | ureq::ErrorKind::InsecureRequestHttpsOnly => AttemptFail::Fatal(message),
                // DNS, обрывы, таймауты, отказ SOCKS — сеть может ожить,
                // или очередь уйдёт на другой транспорт.
                _ => AttemptFail::Retryable {
                    message,
                    reset_offset: false,
                },
            }
        }
    }
}

/// Чтение полного размера и (если дано) хеша из заголовков ответа.
/// Итог: на 206 полный размер приходит в Content-Range; на 200 — в
/// Content-Length. `ETag` со 64-hex хешем тянется как ожидаемый SHA-256
/// (CDN Hugging Face отдаёт sha256 содержимого; слабый `W/…` игнорируется).
fn read_total(response: &ureq::Response) -> (Option<u64>, Option<String>) {
    let total = match response.status() {
        206 => response.header("content-range").and_then(parse_content_range),
        _ => response
            .header("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok()),
    };
    (total, response_hash(response))
}

fn response_hash(response: &ureq::Response) -> Option<String> {
    ["x-linked-etag", "etag"]
        .iter()
        .find_map(|header| {
            let value = response.header(header)?.trim();
            let value = value.trim_matches('"');
            if value.starts_with("W/") {
                return None; // слабый etag не является хешем содержимого
            }
            is_sha256_hex(value).then(|| value.to_string())
        })
}

fn is_sha256_hex(candidate: &str) -> bool {
    candidate.len() == 64
        && candidate
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || (b'A'..=b'F').contains(&b))
}

/// `bytes 0-0/347471872` → 347471872 (полный размер файла).
fn parse_content_range(value: &str) -> Option<u64> {
    let tail = value.trim().rsplit('/').next()?;
    tail.trim().parse::<u64>().ok().filter(|t| *t > 0)
}

/// Сколько байт уже лежит в `.part` (0, если файла нет).
fn resume_offset(part_path: &Path) -> u64 {
    fs::metadata(part_path)
        .map(|meta| if meta.is_file() { meta.len() } else { 0 })
        .unwrap_or(0)
}

/// Сброс `.part` до нулевого размера (truncate+create).
fn truncate_part(part_path: &Path) -> Result<(), String> {
    fs::File::create(part_path)
        .map_err(|e| format!("{}: {e}", part_path.display()))?;
    Ok(())
}

/// Потоковая запись `.part` со сквозным хешем: на resume префикс хешируется
/// в процессе, чтобы SHA-256 итогового файла был честным без перечитывания.
struct PartStream {
    file: fs::File,
    hasher: sha2::Sha256,
    written: u64,
}

impl PartStream {
    fn open(part_path: &Path, existing_offset: u64) -> Result<Self, String> {
        let mut hasher = sha2::Sha256::new();
        if existing_offset > 0 {
            hash_into(part_path, &mut hasher)?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(part_path)
            .map_err(|e| format!("{}: {e}", part_path.display()))?;
        Ok(Self {
            file,
            hasher,
            written: 0,
        })
    }

    fn write_chunk(&mut self, data: &[u8]) -> Result<(), String> {
        self.file
            .write_all(data)
            .map_err(|e| format!("запись .part: {e}"))?;
        self.hasher.update(data);
        self.written += data.len() as u64;
        Ok(())
    }

    /// Durability перед переименованием: сброс на диск, итог (байты, hex).
    fn finish(self) -> (u64, String) {
        let _ = self.file.sync_all();
        let digest = self.hasher.finalize();
        (self.written, format!("{digest:x}"))
    }
}

/// Хешировать существующий файл (префикс `.part` при resume, готовый файл
/// при локальном пропуске).
fn hash_into(path: &Path, hasher: &mut sha2::Sha256) -> Result<(), String> {
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(())
}

/// SHA-256 существующего файла строкой.
fn sha256_file(path: &Path) -> Result<String, String> {
    let mut hasher = sha2::Sha256::new();
    hash_into(path, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Sidecar-манифест ─────────────────────────────────────────────────────────

/// Строка манифеста про один файл: где их взяли и сколько в них байт/хеша.
/// `revision` — коммит источника, если сервер его назвал (HF: x-repo-commit).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManifestFile {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    pub size: u64,
    pub sha256: String,
    /// Момент записи (unix-секунды).
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DownloadManifest {
    pub version: u32,
    /// Файлы по имени: `model.onnx`, `model.onnx.data`.
    pub files: std::collections::BTreeMap<String, ManifestFile>,
}

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join(MANIFEST_NAME)
}

/// Прочитать манифест; битый JSON не рушит ход — просто пусто, запись поверх.
fn read_manifest(dir: &Path) -> Option<DownloadManifest> {
    let path = manifest_path(dir);
    let raw = fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<DownloadManifest>(&raw) {
        Ok(manifest) => Some(manifest),
        Err(error) => {
            unidtk_api::log(format!(
                "mp3-player: {MANIFEST_NAME} не читается ({}), будет переписан: {error}",
                path.display()
            ));
            None
        }
    }
}

/// Дописать строку одного файла, сохранив соседние (graph+weights в одном
/// манифесте, записанные в разные моменты цепочки).
fn record_manifest(dir: &Path, file_name: &str, entry: ManifestFile) -> Result<(), String> {
    let mut manifest = read_manifest(dir).unwrap_or_default();
    manifest.version = 1;
    manifest.files.insert(file_name.to_string(), entry);
    let raw = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("{MANIFEST_NAME}: {e}"))?;
    fs::write(manifest_path(dir), raw).map_err(|e| format!("{MANIFEST_NAME}: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Сериализатор для всех тестов модуля: они крутят глобальное состояние
    /// (runtime, тестовые швы, tempdir), поэтому идут строго по одному.
    static TEST_GUARD: Mutex<()> = Mutex::new(());

    fn fake_model_dir(name: &str) -> (PathBuf, std::sync::MutexGuard<'static, ()>) {
        let guard = TEST_GUARD.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("unidtk-model-fetch-{name}"));
        let base = dir.join("base");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&base).expect("create test base dir");
        *TEST_BASE_DIR.lock().unwrap_or_else(|p| p.into_inner()) = Some(base.clone());
        (dir, guard)
    }

    fn clear_model_dir(guard: &std::sync::MutexGuard<'static, ()>) {
        let _ = guard;
        *TEST_BASE_DIR.lock().unwrap_or_else(|p| p.into_inner()) = None;
        *TEST_PRESENCE.lock().unwrap_or_else(|p| p.into_inner()) = None;
        let mut rt = lock_runtime();
        *rt = Runtime::new();
    }

    fn set_presence(present: bool) {
        *TEST_PRESENCE.lock().unwrap_or_else(|p| p.into_inner()) = Some(present);
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, bytes).expect("write file");
    }

    /// Тестовый ответ с заголовками: у ureq 2.12 нет `Response::set`, зато есть
    /// `FromStr`, который честно разбирает сырой HTTP-текст вместе с заголовками.
    fn response_with(status: u16, text: &str, headers: &[(&str, &str)]) -> ureq::Response {
        let mut raw = format!("HTTP/1.1 {status} {text}\r\n");
        for (name, value) in headers {
            raw.push_str(name);
            raw.push_str(": ");
            raw.push_str(value);
            raw.push_str("\r\n");
        }
        raw.push_str("\r\n");
        raw.parse::<ureq::Response>().expect("build test response")
    }

    // ── имена и раскладка ────────────────────────────────────────────────────

    #[test]
    fn file_layout_follows_the_loader_contract() {
        assert_eq!(ai_dj::MODEL_REL, "discogs-maest-129e/model.onnx");
        assert_eq!(model_rel_dir(), "discogs-maest-129e");
        assert_eq!(graph_file(), "model.onnx");
        assert_eq!(weights_file(), "model.onnx.data");
        assert_eq!(file_for("graph"), "model.onnx");
        assert_eq!(file_for("weights"), "model.onnx.data");
    }

    #[test]
    fn part_path_appends_suffix_without_mangling() {
        let dir = PathBuf::from("target");
        assert_eq!(
            part_path_of(&dir.join("model.onnx")),
            dir.join("model.onnx.part")
        );
        assert_eq!(
            part_path_of(&dir.join("model.onnx.data")),
            dir.join("model.onnx.data.part")
        );
    }

    #[test]
    fn default_source_names_the_declared_upf_repo() {
        // Проверяем дефолт, даже если в окружении пользователя задан
        // AI_DJ_MODEL_URL: снимаем на время проверки и возвращаем на место.
        let saved = std::env::var("AI_DJ_MODEL_URL").ok();
        std::env::remove_var("AI_DJ_MODEL_URL");
        let url = source_url("model.onnx");
        let data_url = source_url(&weights_file());
        if let Some(saved) = saved {
            std::env::set_var("AI_DJ_MODEL_URL", saved);
        }
        assert_eq!(
            url,
            "https://huggingface.co/mtg-upf/discogs-maest-30s-pw-129e/resolve/main/model.onnx"
        );
        assert_eq!(
            data_url,
            "https://huggingface.co/mtg-upf/discogs-maest-30s-pw-129e/resolve/main/model.onnx.data"
        );
    }

    #[test]
    fn source_url_follows_the_env_override() {
        let saved = std::env::var("AI_DJ_MODEL_URL").ok();
        std::env::set_var("AI_DJ_MODEL_URL", "https://mirror.example/base/");
        let url = source_url(&graph_file());
        if let Some(saved) = saved {
            std::env::set_var("AI_DJ_MODEL_URL", saved);
        } else {
            std::env::remove_var("AI_DJ_MODEL_URL");
        }
        assert_eq!(url, "https://mirror.example/base/model.onnx");
    }

    // ── rename/атомарность и resume-offset ──────────────────────────────────

    #[test]
    fn resume_offset_counts_only_real_bytes() {
        let (dir, guard) = fake_model_dir("resume-offset");
        let part = part_path_of(&dir.join("model.onnx"));
        assert_eq!(resume_offset(&part), 0);
        write_file(&part, b"0123456789");
        assert_eq!(resume_offset(&part), 10);
        // Не-файл на месте .part не должен обмануть offset.
        fs::remove_file(&part).expect("remove part");
        fs::create_dir(&part).expect("create dir");
        assert_eq!(resume_offset(&part), 0);
        clear_model_dir(&guard);
    }

    #[test]
    fn truncate_part_resets_to_zero() {
        let (dir, guard) = fake_model_dir("truncate");
        let part = part_path_of(&dir.join("model.onnx.data"));
        write_file(&part, &[0u8; 123]);
        truncate_part(&part).expect("truncate");
        assert_eq!(resume_offset(&part), 0);
        clear_model_dir(&guard);
    }

    #[test]
    fn part_stream_hashes_prefix_and_appended_bytes() {
        let (dir, guard) = fake_model_dir("part-stream");
        let part = part_path_of(&dir.join("model.onnx"));
        write_file(&part, b"abc");
        let mut stream = PartStream::open(&part, 3).expect("open part stream");
        stream.write_chunk(b"def").expect("write chunk");
        let (written, sha) = stream.finish();
        assert_eq!(written, 3);
        assert_eq!(sha, format!("{:x}", sha2::Sha256::digest(b"abcdef")));
        assert_eq!(resume_offset(&part), 6);
        clear_model_dir(&guard);
    }

    #[test]
    fn part_stream_rejects_offset_without_file() {
        let (dir, guard) = fake_model_dir("part-stream-missing");
        let part = part_path_of(&dir.join("model.onnx"));
        // .part отсутствует, но offset > 0 — хешировать префикс не из чего.
        assert!(PartStream::open(&part, 7).is_err());
        clear_model_dir(&guard);
    }

    // ── обработка заголовков и ошибок ureq ──────────────────────────────────

    #[test]
    fn content_range_full_size_is_parsed() {
        assert_eq!(parse_content_range("bytes 0-0/347471872"), Some(347471872));
        assert_eq!(parse_content_range("bytes 123-456/1543"), Some(1543));
        assert_eq!(parse_content_range("bytes 5-9/*"), None);
        assert_eq!(parse_content_range("garbage"), None);
        assert_eq!(parse_content_range("bytes 0-0/0"), None);
    }

    #[test]
    fn strong_hex_etag_is_accepted_weak_is_not() {
        let sha = "a".repeat(64);
        let strong = response_with(206, "Partial Content", &[("etag", &format!("\"{sha}\""))]);
        assert_eq!(response_hash(&strong).as_deref(), Some(sha.as_str()));

        let weak = response_with(200, "OK", &[("etag", &format!("W/\"{sha}\""))]);
        assert_eq!(response_hash(&weak), None);

        let plain = response_with(200, "OK", &[("etag", "\"not-a-hash\"")]);
        assert_eq!(response_hash(&plain), None);

        // x-linked-etag идёт первым в списке источников: при наличии обоих
        // берется он, без аггрегации.
        let linked =
            response_with(200, "OK", &[("x-linked-etag", &format!("\"{sha}\"")), ("etag", "zz")]);
        assert_eq!(response_hash(&linked).as_deref(), Some(sha.as_str()));
    }

    #[test]
    fn fatal_server_statuses_stop_the_chain() {
        let response = ureq::Response::new(404, "Not Found", "").expect("construct response");
        let fail = classify_request_error(ureq::Error::Status(404, response));
        match fail {
            AttemptFail::Fatal(message) => {
                assert!(message.contains("404"), "{message}");
            }
            other => panic!("404 must be fatal: {other:?}"),
        }
    }

    #[test]
    fn transient_statuses_and_broken_socks_are_retried() {
        for code in [500, 502, 429, 408] {
            let response = ureq::Response::new(code, "Err", "").expect("construct response");
            let fail = classify_request_error(ureq::Error::Status(code, response));
            let AttemptFail::Retryable { reset_offset, .. } = fail else {
                panic!("HTTP {code} must be retryable");
            };
            assert!(!reset_offset, "HTTP {code} keeps the .part resume offset");
        }
        let unsatisfiable = ureq::Response::new(416, "Not Satisfiable", "").expect("resp");
        let fail = classify_request_error(ureq::Error::Status(416, unsatisfiable));
        let AttemptFail::Retryable { reset_offset, .. } = fail else {
            panic!("416 must be retryable");
        };
        assert!(reset_offset, "416 drops a stale offset");
    }

    #[test]
    fn transport_errors_split_by_kind() {
        // Битый URL — конфигурация, не сеть: путь Fatal и без сети.
        let error = ureq::get("not a url")
            .call()
            .err()
            .expect("invalid url fails");
        let fail = classify_request_error(error);
        assert!(matches!(fail, AttemptFail::Fatal(_)), "{fail:?}");

        // Битая сеть (io::ConnectionReset) — retryable: сеть может ожить,
        // transport может смениться на SOCKS.
        let error = ureq::Error::from(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "connection reset",
        ));
        let fail = classify_request_error(error);
        let AttemptFail::Retryable { reset_offset, .. } = fail else {
            panic!("io error must be retryable");
        };
        assert!(!reset_offset);
    }

    #[test]
    fn transports_run_direct_first_then_socks() {
        assert_eq!(transports_for_attempt(1), &[Transport::Direct]);
        assert_eq!(transports_for_attempt(2), &[Transport::Socks]);
        assert_eq!(
            transports_for_attempt(3),
            &[Transport::Direct, Transport::Socks]
        );
        assert_eq!(
            transports_for_attempt(5),
            &[Transport::Direct, Transport::Socks]
        );
        // ByeDPI 12909 никогда не появляется в списке транспортов.
        assert_eq!(SOCKS_PORT, 12908);
        assert_ne!(SOCKS_PORT, 12909);
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        assert_eq!(backoff_delay(1), Duration::from_secs(1));
        assert_eq!(backoff_delay(2), Duration::from_secs(2));
        assert_eq!(backoff_delay(3), Duration::from_secs(4));
        assert_eq!(backoff_delay(5), Duration::from_secs(16));
        assert_eq!(backoff_delay(50), BACKOFF_MAX);
    }

    // ── sidecar-манифест ─────────────────────────────────────────────────────

    #[test]
    fn manifest_roundtrip_and_merge() {
        let (dir, guard) = fake_model_dir("manifest");
        let model_dir = dir.join("model");
        fs::create_dir_all(&model_dir).expect("model dir");

        record_manifest(
            &model_dir,
            "model.onnx",
            ManifestFile {
                url: "https://example/com/onnx".into(),
                revision: None,
                size: 122_191,
                sha256: "aa".repeat(32),
                timestamp: 1_760_000_000,
            },
        )
        .expect("record graph");

        // Вторая запись не должна стереть первую: файлы живут в одном
        // манифесте, но пишутся в разные моменты цепочки.
        record_manifest(
            &model_dir,
            "model.onnx.data",
            ManifestFile {
                url: "https://example/com/data".into(),
                revision: Some("abc123".into()),
                size: 347_471_872,
                sha256: "bb".repeat(32),
                timestamp: 1_760_000_001,
            },
        )
        .expect("record weights");

        let manifest = read_manifest(&model_dir).expect("manifest");
        assert_eq!(manifest.version, 1);
        let graph = &manifest.files["model.onnx"];
        assert_eq!(graph.size, 122_191);
        assert_eq!(graph.timestamp, 1_760_000_000);
        assert_eq!(graph.revision, None);
        let weights = &manifest.files["model.onnx.data"];
        assert_eq!(weights.size, 347_471_872);
        assert_eq!(weights.revision.as_deref(), Some("abc123"));

        // Битый манифест не должен рушить ход: перезапишется при записи.
        write_file(&manifest_path(&model_dir), b"{ broken");
        assert!(read_manifest(&model_dir).is_none());
        record_manifest(
            &model_dir,
            "model.onnx",
            ManifestFile {
                url: "https://example/com/onnx".into(),
                revision: None,
                size: 122_191,
                sha256: "aa".repeat(32),
                timestamp: 2,
            },
        )
        .expect("record over broken");
        let manifest = read_manifest(&model_dir).expect("manifest");
        assert!(manifest.files.contains_key("model.onnx"));
        clear_model_dir(&guard);
    }

    // ── state machine: синглфлайт и идемпотентные ответы ────────────────────

    #[test]
    fn try_begin_rejects_running_chain() {
        let (dir, guard) = fake_model_dir("running-chain");
        let _ = dir;
        set_presence(false);
        {
            let mut rt = lock_runtime();
            rt.phase = Phase::Running;
            rt.epoch = 7;
        }
        assert_eq!(try_begin(|_| {}), Err("running"));
        clear_model_dir(&guard);
    }

    #[test]
    fn try_begin_skips_when_model_already_valid() {
        let (dir, guard) = fake_model_dir("already-present");
        let _ = dir;
        set_presence(true);
        assert_eq!(
            try_begin(|rt| rt.phase = Phase::Idle),
            Err("already_present")
        );
        clear_model_dir(&guard);
    }

    #[test]
    fn try_begin_captures_single_flight_epoch() {
        let (dir, guard) = fake_model_dir("single-flight");
        let _ = dir;
        set_presence(false);
        // Первый ход: чистый запуск.
        assert_eq!(
            try_begin(|rt| {
                rt.phase = Phase::Running;
                rt.epoch = 42;
            }),
            Ok("started")
        );
        // Вторая заявка при уже идущем ходе отклоняется — и её ingest не
        // применяется: синглфлайт не должен дать новому триггеру затереть
        // текущее состояние (байты, ошибку, файл) главного хода.
        assert_eq!(
            try_begin(|rt| {
                rt.epoch = 43;
                rt.error = Some("чужой ingest".into());
                rt.bytes_done = 9;
                rt.bytes_total = Some(100);
            }),
            Err("running")
        );
        // Состояние первого хода живо: никто его не переписал.
        let rt = lock_runtime();
        assert_eq!(rt.epoch, 42);
        assert_eq!(rt.phase, Phase::Running);
        assert_eq!(rt.bytes_done, 0);
        assert_eq!(rt.bytes_total, None);
        assert_eq!(rt.error, None);
        drop(rt);
        clear_model_dir(&guard);
    }

    #[test]
    fn try_begin_starts_after_error_state() {
        let (dir, guard) = fake_model_dir("after-error");
        let _ = dir;
        set_presence(false);
        {
            let mut rt = lock_runtime();
            rt.phase = Phase::Error;
            rt.error = Some("модель недоступна, нужен VPN".into());
        }
        assert_eq!(
            try_begin(|rt| {
                rt.phase = Phase::Running;
                rt.epoch = 5;
                rt.error = None;
                rt.bytes_done = 0;
                rt.bytes_total = None;
                rt.file = None;
                rt.attempt = 0;
                rt.source = None;
            }),
            Ok("started")
        );
        assert_eq!(lock_runtime().phase, Phase::Running);
        clear_model_dir(&guard);
    }

    #[test]
    fn report_progress_only_touches_its_own_epoch() {
        let (dir, guard) = fake_model_dir("epoch-guard");
        let _ = dir;
        {
            let mut rt = lock_runtime();
            rt.phase = Phase::Running;
            rt.epoch = 9;
        }
        // Нить со старым epoch не должна затереть актуальное состояние.
        report_progress(
            8,
            10,
            "model.onnx.data",
            FileProgress {
                bytes_done: 50,
                bytes_total: Some(100),
                attempt: 2,
                source: "direct",
            },
        );
        let rt = lock_runtime();
        assert_eq!(rt.bytes_done, 0);
        assert_eq!(rt.file, None);
        assert_eq!(rt.epoch, 9);
        drop(rt);
        // Свой epoch — обновляет.
        report_progress(
            9,
            10,
            "model.onnx.data",
            FileProgress {
                bytes_done: 50,
                bytes_total: Some(100),
                attempt: 2,
                source: "socks",
            },
        );
        let rt = lock_runtime();
        assert_eq!(rt.bytes_done, 60);
        assert_eq!(rt.bytes_total, Some(110));
        assert_eq!(rt.file.as_deref(), Some("model.onnx.data"));
        assert_eq!(rt.source, Some("socks"));
        drop(rt);
        clear_model_dir(&guard);
    }

    #[test]
    fn snapshot_serialises_the_status_contract() {
        let (dir, guard) = fake_model_dir("snapshot-shape");
        let _ = dir;
        {
            let mut rt = lock_runtime();
            rt.phase = Phase::Running;
            rt.epoch = 1;
            rt.bytes_done = 1024;
            rt.bytes_total = Some(347_594_063);
            rt.file = Some("model.onnx.data".into());
            rt.attempt = 2;
            rt.source = Some("direct");
        }
        let snap = snapshot();
        assert_eq!(snap.phase, "running");
        let json = snap.into_result("started");
        assert_eq!(json["result"], "started");
        assert_eq!(json["phase"], "running");
        assert_eq!(json["bytesDone"], 1024);
        assert_eq!(json["bytesTotal"], 347_594_063);
        assert_eq!(json["file"], "model.onnx.data");
        assert_eq!(json["attempt"], 2);
        assert_eq!(json["source"], "direct");
        assert_eq!(json["error"], serde_json::Value::Null);
        clear_model_dir(&guard);
    }

    // ── события ─────────────────────────────────────────────────────────────

    #[test]
    fn model_event_payload_carries_the_contract_fields() {
        let (dir, guard) = fake_model_dir("event-payload");
        let _ = dir;
        let events: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        unidtk_api::subscribe("ai-dj-model", move |payload| {
            if let Ok(mut list) = sink.lock() {
                list.push(payload);
            }
        });

        let progress = FileProgress {
            bytes_done: 123,
            bytes_total: Some(456),
            attempt: 3,
            source: "socks",
        };
        progress.emit("download", "model.onnx.data", None);
        let list = events.lock().unwrap();
        let payload = list.last().expect("event captured").clone();
        drop(list);
        assert_eq!(payload["stage"], "download");
        assert_eq!(payload["fileName"], "model.onnx.data");
        assert_eq!(payload["bytesDone"], 123);
        assert_eq!(payload["bytesTotal"], 456);
        assert_eq!(payload["attempt"], 3);
        assert_eq!(payload["source"], "socks");

        emit_stage(
            "error",
            Some("model.onnx.data".into()),
            Some(9),
            Some(456),
            Some(1),
            Some("direct"),
            Some("HTTP 404 Not Found".to_string()),
        );
        let list = events.lock().unwrap();
        let payload = list.last().expect("error captured").clone();
        drop(list);
        assert_eq!(payload["stage"], "error");
        assert_eq!(payload["message"], "HTTP 404 Not Found");
        clear_model_dir(&guard);
    }

    // ── integrity helpers ───────────────────────────────────────────────────

    #[test]
    fn sha256_file_matches_streaming_hasher() {
        let (dir, guard) = fake_model_dir("sha-file");
        let path = dir.join("blob.bin");
        write_file(&path, b"hello unidtk");
        let hex = sha256_file(&path).expect("hash");
        assert_eq!(hex, format!("{:x}", sha2::Sha256::digest(b"hello unidtk")));
        clear_model_dir(&guard);
    }

    #[test]
    fn response_hash_accepts_all_hex_casing() {
        let upper = "A".repeat(64);
        let mixed = response_with(200, "OK", &[("x-linked-etag", &format!("\"{upper}\""))]);
        assert_eq!(response_hash(&mixed).as_deref(), Some(upper.as_str()));
    }

    #[test]
    fn read_total_prefers_content_range_on_partial_content() {
        let response = response_with(
            206,
            "Partial Content",
            &[("content-range", "bytes 0-0/347471872"), ("content-length", "1")],
        );
        let (total, _) = read_total(&response);
        assert_eq!(total, Some(347471872));
    }

    #[test]
    fn read_total_takes_content_length_on_full_response() {
        let response = response_with(
            200,
            "OK",
            &[("content-length", "347471872")],
        );
        let (total, sha) = read_total(&response);
        assert_eq!(total, Some(347471872));
        assert_eq!(sha, None);
    }
}
