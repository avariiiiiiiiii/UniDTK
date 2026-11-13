//! Supervised lifecycle link: parses NDJSON commands from stdin and shuts the
//! native host down cleanly on EOF or explicit `stop`.

use std::io::BufRead;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum ControlCommand {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartHandshake {
    pub generation: u64,
    pub instance_nonce: String,
    pub ws_port: Option<u16>,
    pub ws_token: Option<String>,
}

pub trait ControlSink {
    fn on_start(&mut self, handshake: StartHandshake);
    fn on_taskbar_created(&mut self);
    fn on_show(&mut self);
    fn on_stop(&mut self, reason: &str);
}

pub fn read_control_stream<R: BufRead, S: ControlSink>(
    mut reader: R,
    sink: &mut S,
) -> Result<(), String> {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                sink.on_stop("eof");
                return Ok(());
            }
            Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => {
                sink.on_stop("eof");
                return Ok(());
            }
            Err(err) => {
                sink.on_stop("eof");
                return Err(format!("stdin read error: {err}"));
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<ControlCommand>(trimmed) {
                    Ok(ControlCommand::Start {
                        generation,
                        instance_nonce,
                        ws_port,
                        ws_token,
                    }) => sink.on_start(StartHandshake {
                        generation,
                        instance_nonce,
                        ws_port,
                        ws_token,
                    }),
                    Ok(ControlCommand::TaskbarCreated) => sink.on_taskbar_created(),
                    Ok(ControlCommand::Show) => sink.on_show(),
                    Ok(ControlCommand::Stop) => {
                        sink.on_stop("stop");
                        return Ok(());
                    }
                    Err(err) => {
                        crate::desktop_host::note(&format!(
                            "supervisor: ignored malformed control line: {err}"
                        ));
                    }
                }
            }
        }
    }
}
