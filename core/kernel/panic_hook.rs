//! Crash reporting for `panic = "abort"` builds.
//!
//! On panic the hook first drains the log queue synchronously (the process dies
//! without running destructors, so anything still queued would be lost), then
//! writes the panic location, payload and a backtrace to
//! `%LOCALAPPDATA%\UniDTK\crash.log` with a hard `sync_all`.

use std::io::Write;
use std::path::PathBuf;

/// Install the global panic hook. Call as the very first thing in `main()`.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let report = format_panic(info);
        // 1. Keep the panic visible in the unified log store, then survive as
        //    much queued log context as possible.
        let summary = report
            .lines()
            .filter(|line| !line.is_empty())
            .take(4)
            .collect::<Vec<_>>()
            .join(" | ");
        crate::logging::log_backend("panic", summary);
        crate::logging::flush();
        // 2. Synchronous crash report (independent of the logging pipeline).
        write_crash_log(&report);
        // 3. Keep the default stderr output for dev runs.
        default_hook(info);
    }));
}

fn crash_log_path() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("UniDTK").join("crash.log")
}

fn format_panic(info: &std::panic::PanicHookInfo<'_>) -> String {
    let thread = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_string();
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "unknown".to_string());
    let backtrace = std::backtrace::Backtrace::force_capture();
    format!(
        "==== panic ====\nthread '{thread}' panicked at {location}\n{info}\n{backtrace}\n"
    )
}

fn write_crash_log(report: &str) {
    let path = crash_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{report}");
        let _ = file.flush();
        let _ = file.sync_all();
    }
}
