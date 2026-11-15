//! Shared wire limits of the experimental native inspector (r[ipc.host-inspector]).
//! Compiled by both the host and its CLI client; the kernel's managed endpoint follows
//! in the next G0 slice, as recorded in `docs/dev/plan-rune-host.md`.

use std::io::Read;
use std::net::TcpStream;
use std::time::{Duration, Instant};

#[allow(dead_code)]
pub const PORT: u16 = 26541;
pub const VERSION: u32 = 1;
pub const MAX_REQUEST: usize = 16 * 1024;
pub const MAX_PNG: usize = 8 * 1024 * 1024;
#[allow(dead_code)] // The CLI uses this half of the shared wire limits.
pub const MAX_REPLY: usize = 12 * 1024 * 1024;
pub const IO_DEADLINE: Duration = Duration::from_secs(3);

/// One newline-terminated message with an absolute deadline: a slow sender cannot
/// keep a worker alive by sending a byte just before each socket timeout.
#[allow(dead_code)] // The CLI (`opt/cli/host.rs`) compiles and uses this helper.
pub fn read_line(stream: &mut TcpStream, limit: usize) -> Result<Vec<u8>, String> {
    read_line_within(stream, limit, IO_DEADLINE)
}

pub fn read_line_within(stream: &mut TcpStream, limit: usize, window: Duration) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + window;
    let mut bytes = Vec::new();
    loop {
        let remaining = deadline.checked_duration_since(Instant::now()).ok_or("request deadline exceeded")?;
        stream.set_read_timeout(Some(remaining)).map_err(|e| e.to_string())?;
        let mut chunk = [0u8; 4096];
        let count = stream.read(&mut chunk).map_err(|e| format!("read: {e}"))?;
        if count == 0 {
            return Err("connection closed before a complete message".into());
        }
        let end = chunk[..count].iter().position(|b| *b == b'\n');
        let used = end.unwrap_or(count);
        if bytes.len() + used > limit {
            return Err(format!("message exceeds {limit} bytes"));
        }
        bytes.extend_from_slice(&chunk[..used]);
        if end.is_some() {
            return Ok(bytes);
        }
    }
}

#[allow(dead_code)] // The host uses the policy; both binaries exercise it in tests.
pub fn authorize(cmd: &str, presented: Option<&str>, expected: &str) -> Result<(), String> {
    match cmd {
        "ping" | "tree" | "frames" | "png" | "hit" => Ok(()),
        "click" | "quit" | "desk" if presented == Some(expected) => Ok(()),
        "click" | "quit" | "desk" => Err(format!("{cmd} changes the host and requires its per-run token")),
        _ => Err(format!("unknown host command: {cmd}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn write_commands_require_this_runs_token() {
        for command in ["click", "quit", "desk"] {
            assert!(authorize(command, None, "secret").is_err());
            assert!(authorize(command, Some("old-token"), "secret").is_err());
            assert!(authorize(command, Some("secret"), "secret").is_ok());
        }
        assert!(authorize("png", None, "secret").is_ok());
        assert!(authorize("unknown", Some("secret"), "secret").is_err());
    }

    #[test]
    fn unterminated_and_oversized_requests_are_refused() {
        for payload in [b"123456\n".as_slice(), b"short"] {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let mut sender = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (mut receiver, _) = listener.accept().unwrap();
            sender.write_all(payload).unwrap();
            drop(sender);
            assert!(read_line(&mut receiver, 5).is_err());
        }
    }
}
