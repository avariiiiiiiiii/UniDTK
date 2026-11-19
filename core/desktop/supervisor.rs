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
    InjectKey {
        generation: u64,
        vk: u32,
        scan_code: u32,
        extended: bool,
        pressed: bool,
        repeat: bool,
        shift: bool,
        ctrl: bool,
        alt: bool,
        win: bool,
        text: Option<String>,
        key_base: Option<char>,
    },
    FocusLost {
        generation: u64,
    },
    /// A press on the desktop outside the host's region (`host_supervisor::ControlMessage`).
    PointerOutside {
        generation: u64,
    },
    /// Wheel travel over the host's region, in the window's client pixels (`delta` 120 a notch).
    Wheel {
        generation: u64,
        x: i32,
        y: i32,
        delta: i32,
    },
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
    fn on_inject_key(&mut self, _cmd: ControlCommand) {}
    fn on_focus_lost(&mut self, _generation: u64) {}
    fn on_pointer_outside(&mut self, _generation: u64) {}
    fn on_wheel(&mut self, _generation: u64, _x: i32, _y: i32, _delta: i32) {}
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
                    Ok(cmd @ ControlCommand::InjectKey { .. }) => sink.on_inject_key(cmd),
                    Ok(ControlCommand::FocusLost { generation }) => sink.on_focus_lost(generation),
                    Ok(ControlCommand::PointerOutside { generation }) => sink.on_pointer_outside(generation),
                    Ok(ControlCommand::Wheel { generation, x, y, delta }) => sink.on_wheel(generation, x, y, delta),
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
