//! Native desktop host lifecycle supervisor (`unidtk-desktop.exe`).
//!
//! Spawns the native Rune host inside an explicit Win32 Job Object
//! (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) with `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`
//! exposing only the anonymous stdin control pipe. Closing stdin triggers a
//! graceful exit even under `WM_ENDSESSION`; closing the job guarantees no
//! orphan host outlives a kernel crash.

use std::ffi::c_void;
use std::fs::File;
use std::io::Write;
use std::mem::size_of;
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, DuplicateHandle, SetHandleInformation, DUPLICATE_SAME_ACCESS, HANDLE,
    HANDLE_FLAG_INHERIT, WAIT_OBJECT_0,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetCurrentProcess, GetExitCodeProcess,
    InitializeProcThreadAttributeList, ResumeThread, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED, EXTENDED_STARTUPINFO_PRESENT,
    INFINITE, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW,
};

const STOP_WAIT_BUDGET: Duration = Duration::from_secs(2);
const STABLE_UPTIME_FOR_BACKOFF_RESET: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum ControlMessage {
    Start {
        generation: u64,
        instance_nonce: String,
        ws_port: Option<u16>,
        ws_token: Option<String>,
    },
    TaskbarCreated,
    Show,
    Stop,
}

pub fn encode_control(msg: &ControlMessage) -> Result<String, String> {
    serde_json::to_string(msg).map_err(|e| format!("encode control: {e}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostStatus {
    pub pid: Option<u32>,
    pub generation: u64,
    pub running: bool,
}

pub struct OwnedHandle(HANDLE);
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    pub fn duplicate(&self) -> Result<Self, String> {
        let mut dup = HANDLE::default();
        unsafe {
            let cur = GetCurrentProcess();
            DuplicateHandle(cur, self.0, cur, &mut dup, 0, false, DUPLICATE_SAME_ACCESS)
                .map_err(|e| format!("DuplicateHandle: {e}"))?;
        }
        Ok(Self(dup))
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

struct AttrList {
    _backing: Vec<u64>,
    ptr: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl AttrList {
    fn for_handle(handle: &HANDLE) -> Result<Self, String> {
        let mut size = 0usize;
        unsafe {
            let _ = InitializeProcThreadAttributeList(
                LPPROC_THREAD_ATTRIBUTE_LIST::default(),
                1,
                0,
                &mut size,
            );
        }
        if size == 0 {
            return Err("InitializeProcThreadAttributeList returned zero size".into());
        }
        let mut backing = vec![0u64; size.div_ceil(size_of::<u64>())];
        let ptr = LPPROC_THREAD_ATTRIBUTE_LIST(backing.as_mut_ptr() as *mut c_void);
        unsafe {
            InitializeProcThreadAttributeList(ptr, 1, 0, &mut size)
                .map_err(|e| format!("InitializeProcThreadAttributeList: {e}"))?;
        }
        let list = Self {
            _backing: backing,
            ptr,
        };
        unsafe {
            UpdateProcThreadAttribute(
                list.ptr,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(handle as *const HANDLE as *const c_void),
                size_of::<HANDLE>(),
                None,
                None,
            )
            .map_err(|e| format!("UpdateProcThreadAttribute: {e}"))?;
        }
        Ok(list)
    }
}

impl Drop for AttrList {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.ptr);
        }
    }
}

pub struct HostProcess {
    pub pid: u32,
    pub generation: u64,
    pub instance_nonce: String,
    stdin_write: Option<File>,
    job: Option<OwnedHandle>,
    process: OwnedHandle,
}

impl HostProcess {
    pub fn write_control(&mut self, msg: &ControlMessage) -> Result<(), String> {
        let Some(pipe) = self.stdin_write.as_mut() else {
            return Err("host stdin pipe already closed".into());
        };
        let line = encode_control(msg)?;
        writeln!(pipe, "{line}").map_err(|e| format!("write host control: {e}"))?;
        pipe.flush()
            .map_err(|e| format!("flush host control: {e}"))?;
        Ok(())
    }

    pub fn close_stdin(&mut self) {
        drop(self.stdin_write.take());
    }

    pub fn duplicate_process_handle(&self) -> Result<OwnedHandle, String> {
        self.process.duplicate()
    }

    pub fn wait_exit(&self, timeout: Duration) -> bool {
        let ms = timeout.as_millis().min((u32::MAX - 1) as u128) as u32;
        unsafe { WaitForSingleObject(self.process.raw(), ms) == WAIT_OBJECT_0 }
    }

    pub fn terminate_job(&self) {
        if let Some(job) = self.job.as_ref() {
            unsafe {
                let _ = TerminateJobObject(job.raw(), 1);
            }
        }
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        self.close_stdin();
        drop(self.job.take());
    }
}

fn wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn build_command_line(exe: &Path, args: &[&str]) -> Vec<u16> {
    let mut cmd = String::with_capacity(128);
    cmd.push('"');
    cmd.push_str(&exe.to_string_lossy());
    cmd.push('"');
    for arg in args {
        cmd.push(' ');
        if arg.contains(' ') || arg.is_empty() {
            cmd.push('"');
            cmd.push_str(arg);
            cmd.push('"');
        } else {
            cmd.push_str(arg);
        }
    }
    wide_null(&cmd)
}

pub fn sibling_host_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "current_exe has no parent directory".to_string())?;
    Ok(dir.join("unidtk-desktop.exe"))
}

static GENERATION: AtomicU64 = AtomicU64::new(0);
static STOPPING: AtomicBool = AtomicBool::new(false);

pub fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::SeqCst) + 1
}

pub fn new_nonce() -> String {
    crate::ws::new_token()
}

/// Spawns `exe` inside a fresh Win32 Job Object (`KILL_ON_JOB_CLOSE`), exposing
/// strictly the anonymous stdin read pipe via `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`.
pub fn spawn_in_job(exe: &Path, args: &[&str]) -> Result<HostProcess, String> {
    static SPAWN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = SPAWN_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|e| format!("spawn lock: {e}"))?;

    let mut read_raw = HANDLE::default();
    let mut write_raw = HANDLE::default();
    unsafe {
        CreatePipe(&mut read_raw, &mut write_raw, None, 0)
            .map_err(|e| format!("CreatePipe: {e}"))?;
    }
    let read_pipe = OwnedHandle(read_raw);
    let write_pipe = unsafe { File::from_raw_handle(write_raw.0) };

    unsafe {
        SetHandleInformation(
            read_pipe.raw(),
            HANDLE_FLAG_INHERIT.0,
            HANDLE_FLAG_INHERIT,
        )
        .map_err(|e| format!("SetHandleInformation: {e}"))?;
    }

    let read_handle = read_pipe.raw();
    let attr_list = AttrList::for_handle(&read_handle)?;

    let job_raw =
        unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map_err(|e| format!("CreateJobObjectW: {e}"))?;
    let job = OwnedHandle(job_raw);

    let mut limit = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limit.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe {
        SetInformationJobObject(
            job.raw(),
            JobObjectExtendedLimitInformation,
            &limit as *const _ as *const c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .map_err(|e| format!("SetInformationJobObject: {e}"))?;
    }

    let si = STARTUPINFOEXW {
        StartupInfo: STARTUPINFOW {
            cb: size_of::<STARTUPINFOEXW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: read_handle,
            ..Default::default()
        },
        lpAttributeList: attr_list.ptr,
    };

    let exe_w = wide_null(&exe.to_string_lossy());
    let mut cmd_w = build_command_line(exe, args);
    let mut pi = PROCESS_INFORMATION::default();

    unsafe {
        CreateProcessW(
            PCWSTR(exe_w.as_ptr()),
            PWSTR(cmd_w.as_mut_ptr()),
            None,
            None,
            true,
            CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &si.StartupInfo,
            &mut pi,
        )
        .map_err(|e| format!("CreateProcessW({}): {e}", exe.display()))?;
    }

    let proc_handle = OwnedHandle(pi.hProcess);
    let thread_handle = OwnedHandle(pi.hThread);
    // Parent closes its copy of the read pipe now that the child owns its duplicate.
    drop(read_pipe);

    unsafe {
        AssignProcessToJobObject(job.raw(), proc_handle.raw())
            .map_err(|e| format!("AssignProcessToJobObject: {e}"))?;
        if ResumeThread(thread_handle.raw()) == u32::MAX {
            return Err("ResumeThread failed".into());
        }
    }
    drop(thread_handle);

    Ok(HostProcess {
        pid: pi.dwProcessId,
        generation: next_generation(),
        instance_nonce: new_nonce(),
        stdin_write: Some(write_pipe),
        job: Some(job),
        process: proc_handle,
    })
}

struct SupervisorState {
    desired_running: bool,
    shutting_down: bool,
    ws_port: Option<u16>,
    ws_token: Option<String>,
    active: Option<HostProcess>,
    restart_attempts: u32,
}

impl Default for SupervisorState {
    fn default() -> Self {
        Self {
            desired_running: false,
            shutting_down: false,
            ws_port: None,
            ws_token: None,
            active: None,
            restart_attempts: 0,
        }
    }
}

struct Supervisor {
    state: Mutex<SupervisorState>,
    exit_cv: Condvar,
}

fn supervisor() -> &'static Arc<Supervisor> {
    static INSTANCE: OnceLock<Arc<Supervisor>> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        Arc::new(Supervisor {
            state: Mutex::new(SupervisorState::default()),
            exit_cv: Condvar::new(),
        })
    })
}

fn spawn_and_watch(
    sup: &Arc<Supervisor>,
    ws_port: Option<u16>,
    ws_token: Option<String>,
) -> Result<(u32, u64), String> {
    let exe = sibling_host_exe()?;
    if !exe.exists() {
        return Err(format!("native host binary missing at {}", exe.display()));
    }
    let mut child = spawn_in_job(&exe, &["--supervised"])?;
    let start_msg = ControlMessage::Start {
        generation: child.generation,
        instance_nonce: child.instance_nonce.clone(),
        ws_port,
        ws_token,
    };
    if let Err(err) = child.write_control(&start_msg) {
        child.terminate_job();
        return Err(err);
    }
    let pid = child.pid;
    let generation = child.generation;
    let wait_handle = child.duplicate_process_handle()?;
    let started_at = Instant::now();

    {
        let mut guard = sup.state.lock().map_err(|e| e.to_string())?;
        if guard.shutting_down || !guard.desired_running {
            child.terminate_job();
            return Err("host supervisor stopped during spawn".into());
        }
        guard.active = Some(child);
    }

    let sup_clone = sup.clone();
    std::thread::Builder::new()
        .name(format!("host-waiter-{generation}"))
        .spawn(move || {
            unsafe {
                let _ = WaitForSingleObject(wait_handle.raw(), INFINITE);
            }
            let mut code = 0u32;
            unsafe {
                let _ = GetExitCodeProcess(wait_handle.raw(), &mut code);
            }
            drop(wait_handle);
            crate::log_line(format!(
                "host_supervisor: gen={generation} pid={pid} exited code={code}"
            ));

            let should_restart = {
                let Ok(mut guard) = sup_clone.state.lock() else {
                    return;
                };
                if guard.active.as_ref().is_some_and(|a| a.generation == generation) {
                    guard.active = None;
                }
                sup_clone.exit_cv.notify_all();
                if guard.shutting_down || STOPPING.load(Ordering::Acquire) || !guard.desired_running {
                    false
                } else {
                    if started_at.elapsed() >= STABLE_UPTIME_FOR_BACKOFF_RESET {
                        guard.restart_attempts = 0;
                    } else {
                        guard.restart_attempts = guard.restart_attempts.saturating_add(1);
                    }
                    true
                }
            };

            if !should_restart {
                return;
            }

            let delay_ms = {
                let guard = sup_clone.state.lock().ok();
                let exp = guard.map(|g| g.restart_attempts.saturating_sub(1).min(4)).unwrap_or(0);
                (500u64 << exp).min(5000)
            };
            if let Ok(guard) = sup_clone.state.lock() {
                let (wait_guard, _) = sup_clone
                    .exit_cv
                    .wait_timeout_while(guard, Duration::from_millis(delay_ms), |s| {
                        !s.shutting_down && s.desired_running
                    })
                    .unwrap_or_else(|e| e.into_inner());
                if wait_guard.shutting_down
                    || STOPPING.load(Ordering::Acquire)
                    || !wait_guard.desired_running
                    || wait_guard.active.is_some()
                {
                    return;
                }
                let ws_port = wait_guard.ws_port;
                let ws_token = wait_guard.ws_token.clone();
                drop(wait_guard);
                if let Err(err) = spawn_and_watch(&sup_clone, ws_port, ws_token) {
                    crate::log_line(format!("host_supervisor: restart failed: {err}"));
                }
            }
        })
        .map_err(|e| format!("spawn host-waiter: {e}"))?;

    crate::log_line(format!(
        "host_supervisor: started unidtk-desktop pid={pid} gen={generation}"
    ));
    Ok((pid, generation))
}

pub fn start_host(ws_port: Option<u16>, ws_token: Option<String>) -> Result<(), String> {
    let sup = supervisor();
    let (already, ws_p, ws_t) = {
        let mut guard = sup.state.lock().map_err(|e| e.to_string())?;
        if guard.shutting_down {
            return Err("application is shutting down".into());
        }
        guard.desired_running = true;
        STOPPING.store(false, Ordering::Release);
        if ws_port.is_some() {
            guard.ws_port = ws_port;
        }
        if ws_token.is_some() {
            guard.ws_token = ws_token;
        }
        (
            guard.active.is_some(),
            guard.ws_port,
            guard.ws_token.clone(),
        )
    };
    if already {
        return Ok(());
    }
    spawn_and_watch(sup, ws_p, ws_t).map(|_| ())
}

pub fn send_control(msg: &ControlMessage) -> Result<(), String> {
    let sup = supervisor();
    let mut guard = sup.state.lock().map_err(|e| e.to_string())?;
    let Some(active) = guard.active.as_mut() else {
        return Err("native host is not running".into());
    };
    active.write_control(msg)
}

pub fn notify_taskbar_created() {
    if let Err(err) = send_control(&ControlMessage::TaskbarCreated) {
        crate::log_line(format!("host_supervisor: TaskbarCreated relay skipped: {err}"));
    }
}

pub fn stop_host(wait: bool) {
    stop_internal(wait, false);
}

pub fn shutdown(wait: bool) {
    stop_internal(wait, true);
}

fn stop_internal(wait: bool, permanent: bool) {
    let sup = supervisor();
    STOPPING.store(true, Ordering::Release);
    let mut guard = match sup.state.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    guard.desired_running = false;
    if permanent {
        guard.shutting_down = true;
    }
    if let Some(active) = guard.active.as_mut() {
        let _ = active.write_control(&ControlMessage::Stop);
        active.close_stdin();
    }
    if !wait {
        if let Some(active) = guard.active.take() {
            active.terminate_job();
        }
        sup.exit_cv.notify_all();
        if !permanent {
            STOPPING.store(false, Ordering::Release);
        }
        return;
    }

    let deadline = Instant::now() + STOP_WAIT_BUDGET;
    let mut g = guard;
    while g.active.is_some() {
        let rem = deadline.saturating_duration_since(Instant::now());
        if rem.is_zero() {
            break;
        }
        g = sup
            .exit_cv
            .wait_timeout(g, rem)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    if let Some(active) = g.active.take() {
        crate::log_line(format!(
            "host_supervisor: stop timed out for pid={}, terminating job",
            active.pid
        ));
        active.terminate_job();
    }
    sup.exit_cv.notify_all();
    if !permanent {
        STOPPING.store(false, Ordering::Release);
    }
}

#[allow(dead_code)]
pub fn status() -> HostStatus {
    let sup = supervisor();
    if let Ok(guard) = sup.state.lock() {
        if let Some(active) = guard.active.as_ref() {
            return HostStatus {
                pid: Some(active.pid),
                generation: active.generation,
                running: true,
            };
        }
    }
    HostStatus {
        pid: None,
        generation: GENERATION.load(Ordering::SeqCst),
        running: false,
    }
}

#[allow(dead_code)]
pub fn wait_for_exit(timeout: Duration) -> bool {
    let sup = supervisor();
    let Ok(mut guard) = sup.state.lock() else {
        return false;
    };
    let deadline = Instant::now() + timeout;
    while guard.active.is_some() {
        let rem = deadline.saturating_duration_since(Instant::now());
        if rem.is_zero() {
            return false;
        }
        guard = sup
            .exit_cv
            .wait_timeout(guard, rem)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_messages_roundtrip_single_line_json() {
        let msg = ControlMessage::Start {
            generation: 3,
            instance_nonce: "deadbeef".into(),
            ws_port: Some(26555),
            ws_token: Some("tok-123".into()),
        };
        let encoded = encode_control(&msg).unwrap();
        assert!(!encoded.contains('\n'), "NDJSON control lines must be single-line");
        let decoded: ControlMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, msg);

        let tb = encode_control(&ControlMessage::TaskbarCreated).unwrap();
        assert_eq!(
            serde_json::from_str::<ControlMessage>(&tb).unwrap(),
            ControlMessage::TaskbarCreated
        );
    }

    #[test]
    fn spawn_in_job_closes_on_stdin_eof_and_kills_on_drop() {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());

        let mut child_eof = spawn_in_job(std::path::Path::new(&comspec), &["/C", "findstr", "."])
            .expect("spawn_in_job must launch subprocess inside JobObject");
        assert!(child_eof.pid > 0);
        child_eof.close_stdin();
        assert!(
            child_eof.wait_exit(Duration::from_secs(3)),
            "child waiting on stdin must exit cleanly after EOF"
        );
        drop(child_eof);

        let child_sleep = spawn_in_job(
            std::path::Path::new(&comspec),
            &["/C", "ping -n 30 127.0.0.1 >nul"],
        )
        .expect("spawn sleeping child in job");
        let proc_watch = child_sleep.duplicate_process_handle().unwrap();
        drop(child_sleep);
        let exited = unsafe { WaitForSingleObject(proc_watch.raw(), 3000) == WAIT_OBJECT_0 };
        assert!(exited, "JobObject KILL_ON_JOB_CLOSE must kill process on drop");
    }
}
