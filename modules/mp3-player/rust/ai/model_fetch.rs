//! Downloads the AI DJ model (`discogs-maest-129e`) into the data directory.
//!
//! When the model did not arrive with the installer, the module fetches it itself, triggered by an
//! explicit user action in the widget: the trigger is [`start_download`], progress is reported by the
//! `ai-dj-model` event. The model is about 331 MB, so the rules are:
//!
//! * files are written under temporary `.part` names and renamed atomically: a partially downloaded
//!   piece never looks like a finished model (validity is checked by [`ai_dj::is_valid_onnx_model`]);
//! * HTTP Range resume: after an interruption (read timeout, RST, application restart) the download
//!   continues from the saved offset;
//! * direct HTTPS is tried first; after repeated failures the same URL goes through the SOCKS proxy
//!   `127.0.0.1:12908` (sing-box, as in the vpn module). ByeDPI (12909) is intentionally not used.
//!   If SOCKS does not respond either, the error is reported honestly:
//!   "model unavailable, VPN required";
//! * single-flight: a repeated request does not download the 331 MB twice;
//! * the run is not under `ONNX_SESSION`: the download has its own thread and its own static mutex;
//! * neither `init` nor autostart sets anything up; the downloader is woken only by the
//!   `ai_dj_model_download` command.
//!
//! Integrity guarantees: the exact size comes from the server response (`Content-Length` /
//! `Content-Range`); SHA-256 is computed incrementally while downloading and recomputed on resume
//! (from the `.part` prefix); the result is written to the sidecar manifest `download_manifest.json`
//! next to the model. A hash stated by the server is compared only when it is the real content hash
//! (`X-Linked-ETag`): the plain `ETag` of the Xet CDN is a storage hash. Hash constants are not
//! hardcoded in advance: the source may change the file, and a wrong constant would bake a download
//! failure into the binary. The default source names a commit, so its bytes never change.

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

// ── File names and directory ─────────────────────────────────────────────────

/// Model directory inside `unidtk_api::paths::models_dir()`. It is derived from `MODEL_REL`
/// (`discogs-maest-129e/model.onnx`), so the layout comes from one place: the loader's contract
/// ([`ai_dj::MODEL_REL`]).
fn model_rel_dir() -> String {
    Path::new(ai_dj::MODEL_REL)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string())
}

/// Graph file name (122 KB), the tail of `MODEL_REL`.
fn graph_file() -> String {
    Path::new(ai_dj::MODEL_REL)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "model.onnx".to_string())
}

/// External weights (347 MB): the companion file `model.onnx.data`, which
/// `is_valid_onnx_model` requires for graphs smaller than 50 MB.
fn weights_file() -> String {
    format!("{0}.data", graph_file())
}

/// Download order: the small graph first, so a single request confirms the source is responding;
/// then the long chain of weights is fetched.
const FILE_ORDER: [&str; 2] = ["graph", "weights"];

/// Temporary file suffix: `model.onnx.data.part`.
const PART_SUFFIX: &str = ".part";

/// Sidecar manifest next to the model.
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

/// The `.part` name is the literal addition of the suffix to the full file name.
/// `with_extension` would corrupt the names here (`model.onnx` → `model.part`).
fn part_path_of(final_path: &Path) -> PathBuf {
    let mut os = final_path.as_os_str().to_os_string();
    os.push(PART_SUFFIX);
    PathBuf::from(os)
}

// ── Network: timeouts, transports, source ────────────────────────────────────

/// Local sing-box SOCKS port (as in the vpn module, the `socks-in` inbound).
const SOCKS_PORT: u16 = 12908;

/// Total number of attempts per file; each attempt has its own list of transports.
const MAX_ATTEMPTS: u32 = 5;
/// Backoff between attempts: 1, 2, 4, 8, 16 s. An interruption at 200 MB must not
/// delay the resume by minutes.
const BACKOFF_STEP: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(16);

/// Connection timeouts. The intent differs from micrographics: that module cuts off short
/// weather-API responses with an "overall" timeout; a download lasts minutes, so no overall
/// timeout is set at all, and `timeout_read` acts as a stall detector: if no byte arrives for
/// 30 s, the attempt is aborted and resumed from the offset.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

const USER_AGENT: &str = "unidtk-mp3-player/1.0";
/// Progress window for the `ai-dj-model` event: at most once every 250 ms.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// Streaming write buffer size (64 KiB).
const CHUNK: usize = 64 * 1024;

/// Transport of an attempt. ByeDPI (port 12909) is excluded from this by design.
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

/// Which transports to try in this attempt.
///
/// Policy "direct HTTPS first": the first attempt uses direct only; if it fails and the failure
/// repeats, SOCKS is used right away; later attempts follow the chain direct → socks, because
/// with a working network the direct transport may recover.
fn transports_for_attempt(attempt: u32) -> &'static [Transport] {
    match attempt {
        1 => &[Transport::Direct],
        2 => &[Transport::Socks],
        _ => &[Transport::Direct, Transport::Socks],
    }
}

/// Backoff: 1 s · 2^(n-1), capped at [`BACKOFF_MAX`].
fn backoff_delay(attempt: u32) -> Duration {
    BACKOFF_STEP
        .saturating_mul(1u32 << (attempt.saturating_sub(1).min(4)))
        .min(BACKOFF_MAX)
}

/// Default source base: the ONNX export published on Hugging Face, pinned to a commit.
///
/// The upstream repository `mtg-upf/discogs-maest-30s-pw-129e` ships PyTorch weights only and has
/// never had `model.onnx(.data)`. The export made in `opt/models` is therefore published as
/// `avari1250/discogs-maest-30s-pw-129e-onnx` (CC BY-NC-SA 4.0, like the original model). The
/// address names a commit instead of `main`: the downloader trusts the size the server reports,
/// and a commit never changes. To publish a new export, upload it, then replace the revision here
/// and in `docs/modules/mp3-player-ai-dj.md`. The address can be overridden without rebuilding:
/// `AI_DJ_MODEL_URL=<base-up-to-the-files>/`.
const DEFAULT_SOURCE_BASE: &str = "https://huggingface.co/avari1250/discogs-maest-30s-pw-129e-onnx/resolve/03be6480f36b33045a04dc042891679b9599ddc7";

/// Test seam: a local server replaces the source, without touching the process environment.
#[cfg(test)]
static TEST_SOURCE_BASE: Mutex<Option<String>> = Mutex::new(None);

/// Source of one file: the override from `AI_DJ_MODEL_URL`, or the default pinned base.
fn source_url(file_name: &str) -> String {
    #[cfg(test)]
    {
        let guard = TEST_SOURCE_BASE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(base) = guard.as_deref() {
            return source_url_with(Some(base), file_name);
        }
    }
    source_url_with(std::env::var("AI_DJ_MODEL_URL").ok().as_deref(), file_name)
}

/// The URL of one file for a given override (the value of `AI_DJ_MODEL_URL`, if any).
/// Pure, so tests do not need to touch the environment.
fn source_url_with(override_base: Option<&str>, file_name: &str) -> String {
    let base = match override_base.map(str::trim) {
        Some(url) if !url.is_empty() => url.trim_end_matches('/'),
        _ => DEFAULT_SOURCE_BASE,
    };
    format!("{base}/{file_name}")
}

// ── Model directory and test seam ─────────────────────────────────────────────

/// Directory where the downloaded model lives:
/// `%LOCALAPPDATA%\UniDTK\models\discogs-maest-129e\`.
///
/// Tests must set [`TEST_BASE_DIR`]: without the override the function
/// panics, so a unit test physically cannot write into the user's AppData.
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

/// Test seam for checking "model already present": at runtime it is the loader's predicate,
/// in tests it is a flag override.
#[cfg(test)]
static TEST_PRESENCE: Mutex<Option<bool>> = Mutex::new(None);

/// "Model is present and valid": exactly the loader's predicate
/// ([`ai_dj::find_model_path`]), so a download is started only when the analysis
/// would actually fail for lack of the model.
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

// ── State machine: single-flight ─────────────────────────────────────────────

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
    /// Downloaded bytes of the chain: finished files + the current `.part`.
    bytes_done: u64,
    /// Full size of the chain, once the server has reported the sizes.
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

/// Frozen snapshot of the download state; read by `ai_dj_status`.
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    /// `idle` | `running` | `error`.
    pub phase: &'static str,
    /// Text of the last error (`phase = error`); cleared on a new start.
    pub error: Option<String>,
    /// File of the current attempt (`model.onnx.data` / `model.onnx`).
    pub file: Option<String>,
    /// Downloaded bytes of the chain.
    pub bytes_done: u64,
    /// Full size of the chain, once the server has reported the sizes.
    pub bytes_total: Option<u64>,
    pub attempt: u32,
    /// `direct` | `socks`: transport of the current attempt.
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
    /// Direct command response: the trigger result plus a live state snapshot.
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

/// Claims the state for a new run. Single-flight: a run already in progress is not
/// replaced, and a valid model is not downloaded a second time.
fn try_begin(update: impl FnOnce(&mut Runtime)) -> Result<&'static str, &'static str> {
    let mut rt = lock_runtime();
    // Already running? This is checked before presence: during validation of the final
    // step the model may already have appeared, so let its own thread finish.
    if rt.phase == Phase::Running {
        return Err("running");
    }
    if is_model_present() {
        return Err("already_present");
    }
    update(&mut rt);
    Ok("started")
}

/// Download trigger: does not wait for completion and does not touch the ONNX session.
/// The response is immediate: `{"result": "started" | "running" | "already_present"}`.
///
/// The command does not return download errors: the long-running work happens in its own thread and
/// reports through the `ai-dj-model` event (`stage:"error"`) and through [Snapshot] / status.
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
            // The thread takes over the run and publishes progress as events; the command
            // returns immediately without waiting for the 331 MB.
            match std::thread::Builder::new()
                .name("ai-dj-model-fetch".into())
                .spawn(move || worker(epoch))
            {
                Ok(_handle) => Ok(snapshot().into_result("started")),
                Err(error) => {
                    // The thread failed to start: report an honest error instead of "started".
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
        // A valid model is already present or a run is already in progress: idempotent responses.
        Ok(other) => Ok(snapshot().into_result(other)),
        Err(other) => Ok(snapshot().into_result(other)),
    }
}

// ── Progress and events ──────────────────────────────────────────────────────

/// The `ai-dj-model` event: the only channel through which the widget sees the download
/// progress and errors. It is not mixed with the analysis stream (`ai-dj-progress`).
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

/// Progress snapshot of one file, which the thread hands to the runtime and to the event.
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

/// Updates the runtime only if the run is still the same one (a new trigger's thread
/// must not be overwritten by the old thread, or the other way around).
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

// ── Download run ─────────────────────────────────────────────────────────────

fn worker(epoch: u64) {
    let outcome = run_chain(epoch);
    let (message, snap) = {
        let mut rt = lock_runtime();
        // The run may have been preempted by a new trigger (single-flight should not allow this,
        // but the current state must not be overwritten by a thread that is no longer current).
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

/// Chain over both files: the small graph first (a quick check that the source responds),
/// then the long weights. Afterwards the result is verified with the same predicate
/// that the loader uses.
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

/// Single attempt failure: Fatal abandons the whole file (the source is broken),
/// Retryable keeps the `.part` and moves on to the next attempt (optionally
/// with an offset reset).
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

    // This file may already be on disk from a previous chain (graph finished, interrupted during the
    // weights, or the reverse): record it and do not download it again.
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
                Ok(result) => {
                    // The bytes are complete and verified: only now does the file get its final
                    // name. Without this step the loader would look for `model.onnx` while the
                    // data sat in `model.onnx.part`, and the next run would resume past the end.
                    publish_part(part_path, final_path)?;
                    return Ok(result);
                }
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

    // Honest error: not a silent failure and not a panic over hidden state.
    Err(format!(
        "AI DJ модель недоступна: не отвечает ни прямая сеть, ни SOCKS {SOCKS_PORT}. Держите VPN или задайте рабочий источник в AI_DJ_MODEL_URL. Последняя причина: {last_fail}"
    ))
}

/// One attempt to download a file: Range resume from the current `.part` offset,
/// streaming write, end-to-end SHA-256, and size/hash validation.
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
    // Integrity witnesses: the exact size and (if the server reports it) the hash.
    let (total, expected_sha) = read_total(&response);

    let mut offset = offset;
    let total = match status {
        206 => {
            // Check that the server agreed with our offset.
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
                // The server ignored the Range header, so resuming is impossible and ".part"
                // must start from zero. If the reset fails, we must not write over it
                // (the result would be a mix with the old content); the attempt
                // is retried after an honest error.
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
        // The hash the server gave did not match: when resuming, this means mixed
        // edits/encoding, the same cause as a page that changes while it is being fetched.
        // With a zeroed offset, the source itself is broken.
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

/// Classifies a request error before the response body is read.
fn classify_request_error(error: ureq::Error) -> AttemptFail {
    match error {
        ureq::Error::Status(code, response) => {
            let status_text = response.status_text().to_string();
            let base = format!("HTTP {code} {status_text}").trim_end().to_string();
            match code {
                // The source permanently answers "no"; everything else is handled honestly.
                401 | 403 | 404 | 405 | 410 => AttemptFail::Fatal(base),
                // The server cannot serve this offset: .part is larger than the file.
                416 => AttemptFail::Retryable {
                    message: format!("{base}: .part несовместим, начинаем заново"),
                    reset_offset: true,
                },
                // Server quirks and rate limits: try again.
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
                // A bad address is a configuration error, not a network failure.
                ureq::ErrorKind::InvalidUrl
                | ureq::ErrorKind::UnknownScheme
                | ureq::ErrorKind::TooManyRedirects
                | ureq::ErrorKind::InsecureRequestHttpsOnly => AttemptFail::Fatal(message),
                // DNS, dropped connections, timeouts, SOCKS refusal: the network may recover,
                // or the queue will move on to another transport.
                _ => AttemptFail::Retryable {
                    message,
                    reset_offset: false,
                },
            }
        }
    }
}

/// Reads the full size and (if provided) the hash from the response headers.
/// Result: on 206 the full size comes in Content-Range; on 200 it comes in
/// Content-Length. The expected SHA-256 comes only from `X-Linked-ETag` (see [`response_hash`]).
fn read_total(response: &ureq::Response) -> (Option<u64>, Option<String>) {
    let total = match response.status() {
        206 => response.header("content-range").and_then(parse_content_range),
        _ => response
            .header("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok()),
    };
    (total, response_hash(response))
}

/// The content's SHA-256 as the server states it, if it does.
///
/// Only `X-Linked-ETag` counts: Hugging Face puts the real SHA-256 of the file there, on the first
/// answer of `/resolve/`. The plain `ETag` is a storage detail: the Xet CDN behind the redirect
/// answers with its own 64-hex file hash, which is not the SHA-256 of the bytes and would reject
/// every correct download (measured on the published export, 2026-11-03). A weak tag (`W/"…"`)
/// is never a content hash.
fn response_hash(response: &ureq::Response) -> Option<String> {
    let value = response.header("x-linked-etag")?.trim().trim_matches('"');
    if value.starts_with("W/") {
        return None;
    }
    is_sha256_hex(value).then(|| value.to_string())
}

fn is_sha256_hex(candidate: &str) -> bool {
    candidate.len() == 64
        && candidate
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || (b'A'..=b'F').contains(&b))
}

/// `bytes 0-0/347471872` → 347471872 (full file size).
fn parse_content_range(value: &str) -> Option<u64> {
    let tail = value.trim().rsplit('/').next()?;
    tail.trim().parse::<u64>().ok().filter(|t| *t > 0)
}

/// How many bytes are already in `.part` (0 if the file does not exist).
fn resume_offset(part_path: &Path) -> u64 {
    fs::metadata(part_path)
        .map(|meta| if meta.is_file() { meta.len() } else { 0 })
        .unwrap_or(0)
}

/// Resets `.part` to zero size (truncate+create).
fn truncate_part(part_path: &Path) -> Result<(), String> {
    fs::File::create(part_path)
        .map_err(|e| format!("{}: {e}", part_path.display()))?;
    Ok(())
}

/// Publishes a finished `.part` under its final name. The rename replaces an existing file and is
/// atomic within one volume, so the loader never sees a half-written model.
fn publish_part(part_path: &Path, final_path: &Path) -> Result<(), String> {
    fs::rename(part_path, final_path).map_err(|e| {
        format!(
            "не удалось опубликовать {} как {}: {e}",
            part_path.display(),
            final_path.display()
        )
    })
}

/// Streaming write of `.part` with an end-to-end hash: on resume the existing prefix is hashed
/// up front, so the final file's SHA-256 is correct without re-reading it.
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

    /// Durability before the rename: flush to disk, return the result (bytes, hex).
    fn finish(self) -> (u64, String) {
        let _ = self.file.sync_all();
        let digest = self.hasher.finalize();
        (self.written, format!("{digest:x}"))
    }
}

/// Hashes an existing file (the `.part` prefix on resume, or the finished file
/// when skipping locally).
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

/// SHA-256 of an existing file, as a string.
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

// ── Sidecar manifest ─────────────────────────────────────────────────────────

/// Manifest entry for one file: where it came from and its size and hash.
/// `revision` is the source commit, if the server named it (HF: x-repo-commit).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManifestFile {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    pub size: u64,
    pub sha256: String,
    /// Time of writing (Unix seconds).
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DownloadManifest {
    pub version: u32,
    /// Files by name: `model.onnx`, `model.onnx.data`.
    pub files: std::collections::BTreeMap<String, ManifestFile>,
}

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join(MANIFEST_NAME)
}

/// Reads the manifest; broken JSON does not break the run: it is simply treated as empty and overwritten.
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

/// Appends one file's entry while keeping the neighbouring ones (graph and weights share one
/// manifest but are written at different points in the chain).
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

    /// Serializer for all tests in the module: they manipulate global state
    /// (runtime, test seams, tempdir), so they run strictly one at a time.
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
        *TEST_SOURCE_BASE.lock().unwrap_or_else(|p| p.into_inner()) = None;
        let mut rt = lock_runtime();
        *rt = Runtime::new();
    }

    fn set_presence(present: bool) {
        *TEST_PRESENCE.lock().unwrap_or_else(|p| p.into_inner()) = Some(present);
    }

    /// Points the downloader at a local test server instead of the real source.
    fn set_source(base: &str) {
        *TEST_SOURCE_BASE.lock().unwrap_or_else(|p| p.into_inner()) = Some(base.to_string());
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, bytes).expect("write file");
    }

    /// Test response with headers: ureq 2.12 has no `Response::set`, but it has
    /// `FromStr`, which parses raw HTTP text including the headers.
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

    // ── names and layout ─────────────────────────────────────────────────────

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

    // The two address tests use the pure helper: they never touch the process environment,
    // so they cannot race with other tests and do not depend on the developer's own
    // AI_DJ_MODEL_URL.

    #[test]
    fn default_source_is_the_published_export_pinned_to_a_commit() {
        assert_eq!(
            source_url_with(None, "model.onnx"),
            "https://huggingface.co/avari1250/discogs-maest-30s-pw-129e-onnx/resolve/03be6480f36b33045a04dc042891679b9599ddc7/model.onnx"
        );
        assert_eq!(
            source_url_with(None, &weights_file()),
            "https://huggingface.co/avari1250/discogs-maest-30s-pw-129e-onnx/resolve/03be6480f36b33045a04dc042891679b9599ddc7/model.onnx.data"
        );
        // The last segment of the base must be a full commit id: `main` would let the bytes
        // change under a downloader that trusts the size the server reports.
        let revision = DEFAULT_SOURCE_BASE.rsplit('/').next().expect("revision segment");
        assert_eq!(revision.len(), 40, "revision segment: {revision}");
        assert!(
            revision.bytes().all(|b| b.is_ascii_hexdigit()),
            "revision segment is not hex: {revision}"
        );
    }

    #[test]
    fn source_url_with_follows_the_override() {
        assert_eq!(
            source_url_with(Some("https://mirror.example/base/"), &graph_file()),
            "https://mirror.example/base/model.onnx"
        );
        // Padding and trailing slashes are dropped.
        assert_eq!(
            source_url_with(Some("  https://mirror.example/base//  "), &weights_file()),
            "https://mirror.example/base/model.onnx.data"
        );
        // An empty or blank override means "not set".
        assert_eq!(
            source_url_with(Some(""), &graph_file()),
            source_url_with(None, &graph_file())
        );
        assert_eq!(
            source_url_with(Some("   "), &graph_file()),
            source_url_with(None, &graph_file())
        );
    }

    // ── rename, atomicity and resume offset ─────────────────────────────────

    #[test]
    fn resume_offset_counts_only_real_bytes() {
        let (dir, guard) = fake_model_dir("resume-offset");
        let part = part_path_of(&dir.join("model.onnx"));
        assert_eq!(resume_offset(&part), 0);
        write_file(&part, b"0123456789");
        assert_eq!(resume_offset(&part), 10);
        // A non-file at the .part path must not fool the offset.
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
        // .part is missing but offset > 0: there is no prefix to hash.
        assert!(PartStream::open(&part, 7).is_err());
        clear_model_dir(&guard);
    }

    // ── header and ureq error handling ──────────────────────────────────────

    #[test]
    fn content_range_full_size_is_parsed() {
        assert_eq!(parse_content_range("bytes 0-0/347471872"), Some(347471872));
        assert_eq!(parse_content_range("bytes 123-456/1543"), Some(1543));
        assert_eq!(parse_content_range("bytes 5-9/*"), None);
        assert_eq!(parse_content_range("garbage"), None);
        assert_eq!(parse_content_range("bytes 0-0/0"), None);
    }

    #[test]
    fn only_the_linked_etag_is_a_content_hash() {
        let sha = "a".repeat(64);
        let strong = response_with(
            206,
            "Partial Content",
            &[("x-linked-etag", &format!("\"{sha}\""))],
        );
        assert_eq!(response_hash(&strong).as_deref(), Some(sha.as_str()));

        let weak = response_with(200, "OK", &[("x-linked-etag", &format!("W/\"{sha}\""))]);
        assert_eq!(response_hash(&weak), None);

        let plain = response_with(200, "OK", &[("x-linked-etag", "\"not-a-hash\"")]);
        assert_eq!(response_hash(&plain), None);

        // The plain ETag is a storage detail. The Xet CDN of Hugging Face answers with its own
        // 64-hex file hash there, which is not the SHA-256 of the bytes, so it must not be
        // taken for one (measured on the published export, 2026-11-03).
        let cdn = response_with(206, "Partial Content", &[("etag", &format!("\"{sha}\""))]);
        assert_eq!(response_hash(&cdn), None);

        // When both are present, the linked one wins.
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
        // A broken URL is configuration, not network: it takes the Fatal path, without any network access.
        let error = ureq::get("not a url")
            .call()
            .err()
            .expect("invalid url fails");
        let fail = classify_request_error(error);
        assert!(matches!(fail, AttemptFail::Fatal(_)), "{fail:?}");

        // A network I/O error (io::ConnectionReset) is retryable: the network may recover,
        // and the transport may switch to SOCKS.
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
        // ByeDPI 12909 never appears in the transport list.
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

    // ── sidecar manifest ─────────────────────────────────────────────────────

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

        // The second write must not erase the first: the files share one
        // manifest but are written at different points in the chain.
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

        // A broken manifest must not break the run: it is overwritten on the next write.
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

    // ── state machine: single-flight and idempotent responses ───────────────

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
        // First run: clean start.
        assert_eq!(
            try_begin(|rt| {
                rt.phase = Phase::Running;
                rt.epoch = 42;
            }),
            Ok("started")
        );
        // A second request is rejected while a run is in progress, and its ingest is not
        // applied: single-flight must not let the new trigger overwrite the current state
        // (bytes, error, file) of the main run.
        assert_eq!(
            try_begin(|rt| {
                rt.epoch = 43;
                rt.error = Some("чужой ingest".into());
                rt.bytes_done = 9;
                rt.bytes_total = Some(100);
            }),
            Err("running")
        );
        // The first run's state is intact: nothing overwrote it.
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
        // A thread with an old epoch must not overwrite the current state.
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
        // Its own epoch: it updates the state.
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

    // ── events ──────────────────────────────────────────────────────────────

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

    // ── a download against a local HTTP server ───────────────────────────────
    //
    // The success path of the downloader had no test: the only address it knew answered 404,
    // so a finished `.part` that was never published and a CDN hash that was never checked
    // against real headers went unnoticed.

    /// Serves `connections` requests on a loopback port. `respond` receives the request head
    /// and returns the complete raw response. Returns the base URL of the server.
    fn serve(connections: usize, respond: impl Fn(&str) -> Vec<u8> + Send + 'static) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let port = listener.local_addr().expect("local address").port();
        std::thread::spawn(move || {
            for _ in 0..connections {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match stream.read(&mut byte) {
                        Ok(1) => head.push(byte[0]),
                        _ => break,
                    }
                }
                let _ = stream.write_all(&respond(&String::from_utf8_lossy(&head)));
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// A raw HTTP/1.1 response with a correct `Content-Length`.
    fn http_response(status_line: &str, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
        let mut raw = format!(
            "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        for (name, value) in headers {
            raw.push_str(&format!("{name}: {value}\r\n"));
        }
        raw.push_str("\r\n");
        let mut bytes = raw.into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    fn sha_hex(bytes: &[u8]) -> String {
        format!("{:x}", sha2::Sha256::digest(bytes))
    }

    #[test]
    fn finished_download_is_published_under_the_final_name() {
        let (dir, guard) = fake_model_dir("publish");
        let body = vec![7u8; 3000];
        let base = serve(1, {
            let body = body.clone();
            move |_| http_response("200 OK", &[], &body)
        });
        set_source(&base);
        let final_path = dir.join("model.onnx");
        let part_path = part_path_of(&final_path);

        let outcome = fetch_file(0, "graph", 0, &part_path, &final_path);

        assert_eq!(outcome, Ok((3000, sha_hex(&body))));
        assert_eq!(fs::read(&final_path).expect("published file"), body);
        assert!(!part_path.exists(), "the .part must be renamed away");
        clear_model_dir(&guard);
    }

    #[test]
    fn cdn_etag_is_not_mistaken_for_the_content_hash() {
        // The Xet CDN answers with its own 64-hex file hash in ETag. It is not the SHA-256 of
        // the bytes, and a correct download must not be rejected because of it.
        let (dir, guard) = fake_model_dir("cdn-etag");
        let body = vec![9u8; 2048];
        let xet_hash = "ab".repeat(32);
        let base = serve(1, {
            let body = body.clone();
            move |_| http_response("200 OK", &[("ETag", format!("\"{xet_hash}\""))], &body)
        });
        set_source(&base);
        let final_path = dir.join("model.onnx");
        let part_path = part_path_of(&final_path);

        let outcome = fetch_file(0, "graph", 0, &part_path, &final_path);

        assert_eq!(outcome, Ok((2048, sha_hex(&body))));
        assert!(final_path.is_file());
        clear_model_dir(&guard);
    }

    #[test]
    fn linked_etag_is_still_verified() {
        let (dir, guard) = fake_model_dir("linked-etag");
        let body = vec![1u8; 1024];
        let good = sha_hex(&body);
        let wrong = "cd".repeat(32);
        let final_path = dir.join("model.onnx");
        let part_path = part_path_of(&final_path);

        // A matching X-Linked-ETag is accepted.
        let base = serve(1, {
            let body = body.clone();
            move |_| http_response("200 OK", &[("X-Linked-ETag", format!("\"{good}\""))], &body)
        });
        set_source(&base);
        assert!(fetch_file(0, "graph", 0, &part_path, &final_path).is_ok());
        assert!(final_path.is_file());
        fs::remove_file(&final_path).expect("remove the published file");

        // A mismatching one means the source is broken: the file is not published.
        let base = serve(1, {
            let body = body.clone();
            move |_| http_response("200 OK", &[("X-Linked-ETag", format!("\"{wrong}\""))], &body)
        });
        set_source(&base);
        let message = fetch_file(0, "graph", 0, &part_path, &final_path)
            .expect_err("a hash mismatch must fail");
        assert!(message.contains("SHA-256"), "{message}");
        assert!(
            !final_path.exists(),
            "a file that failed verification must not be published"
        );
        clear_model_dir(&guard);
    }

    #[test]
    fn interrupted_download_resumes_with_a_range_request() {
        let (dir, guard) = fake_model_dir("resume");
        let body: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
        let final_path = dir.join("model.onnx");
        let part_path = part_path_of(&final_path);
        // An earlier run was interrupted after 1200 bytes.
        write_file(&part_path, &body[..1200]);
        let base = serve(1, {
            let body = body.clone();
            move |head| {
                assert!(
                    head.to_ascii_lowercase().contains("range: bytes=1200-"),
                    "no resume Range header in: {head}"
                );
                http_response(
                    "206 Partial Content",
                    &[(
                        "Content-Range",
                        format!("bytes 1200-{}/{}", body.len() - 1, body.len()),
                    )],
                    &body[1200..],
                )
            }
        });
        set_source(&base);

        let outcome = fetch_file(0, "graph", 0, &part_path, &final_path);

        assert_eq!(outcome, Ok((3000, sha_hex(&body))));
        assert_eq!(fs::read(&final_path).expect("published file"), body);
        assert!(!part_path.exists());
        clear_model_dir(&guard);
    }

    #[test]
    fn whole_chain_downloads_both_files_and_the_loader_accepts_them() {
        let (_dir, guard) = fake_model_dir("chain");
        let graph = vec![1u8; 4096];
        // Next to a small graph the loader wants more than 10 MiB of weights.
        let weights = vec![2u8; 10 * 1024 * 1024 + 1];
        let base = serve(2, {
            let (graph, weights) = (graph.clone(), weights.clone());
            move |head| {
                if head.starts_with("GET /model.onnx.data ") {
                    http_response("200 OK", &[], &weights)
                } else {
                    http_response("200 OK", &[], &graph)
                }
            }
        });
        set_source(&base);

        run_chain(0).expect("the chain succeeds against a healthy source");

        let dir = model_dir();
        assert_eq!(fs::read(dir.join("model.onnx")).expect("graph"), graph);
        assert_eq!(
            fs::metadata(dir.join("model.onnx.data")).expect("weights").len(),
            weights.len() as u64
        );
        assert!(!part_path_of(&dir.join("model.onnx")).exists());
        assert!(!part_path_of(&dir.join("model.onnx.data")).exists());
        assert!(ai_dj::is_valid_onnx_model(&dir.join(graph_file())));
        let manifest = read_manifest(&dir).expect("manifest");
        assert_eq!(manifest.files["model.onnx"].sha256, sha_hex(&graph));
        assert_eq!(manifest.files["model.onnx.data"].sha256, sha_hex(&weights));
        assert!(manifest.files["model.onnx.data"].url.ends_with("/model.onnx.data"));
        clear_model_dir(&guard);
    }
}
