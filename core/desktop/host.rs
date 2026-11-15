//! G0's standalone native window and an observable card. Production stays on WebView2.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use rune::el::{col, el, text};
use rune::{App, AppSpec, Color, FrameOutcome, WindowOptions};
use serde_json::json;
use crate::inspector::Inspector;
use crate::supervisor::{ControlCommand, ControlSink, StartHandshake};
use crate::ws_client::{HostRegistrationInfo, WsClientHandle};

struct Host {
    inspector: Arc<Inspector>,
    clicks: u64,
}

fn view(state: &Host, _: &mut rune::runtime::Cx<Host>) -> rune::el::El {
    col().w_full().h_full().padding(24.0).child(
        el().key("g0-card").label("Native inspector test card")
            .self_start().w(320.0).h(80.0)
            .fill(Color::BLACK.alpha(0.45)).radius(14.0).padding(12.0)
            .on_press(|state: &mut Host| {
                state.clicks += 1;
                state.inspector.input_count.fetch_add(1, Ordering::AcqRel);
            })
            .child(text(format!("UniDTK · Rune · G0\nPresses: {}", state.clicks)).color(Color::WHITE)),
    )
}

const SAMPLES: usize = 512;
pub(crate) const DISCOVERY_ATTEMPTS: usize = 120;
pub(crate) const DISCOVERY_INTERVAL: Duration = Duration::from_millis(250);

static REATTACH_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

fn spawn_attach_worker(
    name: &'static str,
    hwnd: isize,
    max_attempts: usize,
    inspector: Arc<Inspector>,
    ui: rune::AppHandle<Host>,
    fail_on_exhaust: bool,
    ws_slot: Arc<Mutex<Option<WsClientHandle>>>,
) {
    let spawn = std::thread::Builder::new().name(name.into()).spawn(move || {
        let mut error = "desktop unavailable".to_string();
        for attempt in 0..max_attempts {
            if inspector.stopping.load(Ordering::Acquire) {
                if !fail_on_exhaust {
                    REATTACH_IN_PROGRESS.store(false, Ordering::Release);
                }
                return;
            }
            match crate::desktop_host::discover_with_attempt(attempt) {
                Ok(parent) => {
                    let attached_ws = ws_slot.clone();
                    ui.update(move |state, _| {
                        if state.inspector.stopping.load(Ordering::Acquire) {
                            return;
                        }
                        match crate::desktop_host::attach_to(hwnd, parent) {
                            Ok(()) => {
                                state.inspector.hwnd.store(hwnd, Ordering::Release);
                                if let Err(error) = publish_region(&state.inspector) {
                                    state.inspector.fail(error);
                                    return;
                                }
                                let window = state.inspector.window.lock().ok().and_then(|g| g.clone());
                                if let Some(w) = window {
                                    w.recheck_occlusion();
                                }
                                sync_registration(&state.inspector, &attached_ws);
                                crate::desktop_host::note(&format!(
                                    "host attached under {}",
                                    crate::desktop_host::host_description()
                                ));
                            }
                            Err(error) => state.inspector.fail(error),
                        }
                    });
                    if !fail_on_exhaust {
                        REATTACH_IN_PROGRESS.store(false, Ordering::Release);
                    }
                    return;
                }
                Err(next) => error = next,
            }
            std::thread::sleep(DISCOVERY_INTERVAL);
        }
        if fail_on_exhaust {
            inspector.fail(format!("desktop discovery exhausted: {error}"));
        } else {
            crate::desktop_host::note(&format!("reattach exhausted: {error}"));
            REATTACH_IN_PROGRESS.store(false, Ordering::Release);
        }
    });
    if let Err(error) = spawn {
        if !fail_on_exhaust {
            REATTACH_IN_PROGRESS.store(false, Ordering::Release);
        } else {
            // Best-effort log; caller holds inspector.
            crate::desktop_host::note(&format!("desktop worker failed: {error}"));
        }
    }
}

pub fn run() -> Result<(), String> {
    crate::desktop_host::ensure_dpi_awareness();
    let _instance = crate::desktop_host::acquire_single_instance()?;
    let supervised = std::env::args().any(|arg| arg == "--supervised");
    let inspector = Inspector::new()?;
    let (width, height) = crate::desktop_host::primary_size();
    let state_inspector = inspector.clone();
    let spec = AppSpec::new("UniDTK desktop", (width as f32, height as f32), || unreachable!("init is supplied"), view);
    let app = App::new(spec).init(move || Host { inspector: state_inspector, clicks: 0 })
        .open_inactive()
        .window_options(WindowOptions {
            transparent: true, borderless: true,
            // Safe even if a covered launch never draws. Enable input only once a
            // successful attach and an actual widget region have both been published.
            click_through: true, resizable: false,
            // DirectComposition draws the window; a GDI surface under it leaves a title bar
            // visible through every clear pixel of the desktop child.
            no_redirection_bitmap: true,
            physical_size: Some((width as u32, height as u32)), position: Some((0, 0)),
            ..Default::default()
        });
    let handle = app.handle();
    let quit_handle = handle.clone();
    inspector.start(supervised, move || quit_handle.quit())?;

    let ws_slot: Arc<Mutex<Option<WsClientHandle>>> = Arc::new(Mutex::new(None));
    let active_generation = Arc::new(AtomicU64::new(0));
    let focused_input = Arc::new(AtomicBool::new(false));
    if supervised {
        struct SupervisedSink {
            inspector: Arc<Inspector>,
            ui: rune::AppHandle<Host>,
            ws_slot: Arc<Mutex<Option<WsClientHandle>>>,
            active_generation: Arc<AtomicU64>,
        }
        impl ControlSink for SupervisedSink {
            fn on_start(&mut self, handshake: StartHandshake) {
                self.active_generation.store(handshake.generation, Ordering::Release);
                crate::desktop_host::note(&format!(
                    "supervisor: start gen={} nonce={} ws_port={:?}",
                    handshake.generation, handshake.instance_nonce, handshake.ws_port
                ));
                if let (Some(port), Some(token)) = (handshake.ws_port, handshake.ws_token) {
                    let hwnd = self.inspector.hwnd.load(Ordering::Acquire);
                    let attached = hwnd != 0 && crate::desktop_host::window_is_attached(hwnd);
                    let ready = self.inspector.is_ready(attached).unwrap_or(false);
                    let info = HostRegistrationInfo {
                        pid: std::process::id(),
                        generation: handshake.generation,
                        instance_nonce: handshake.instance_nonce,
                        inspector_port: self.inspector.port(),
                        attached,
                        ready,
                    };
                    match crate::ws_client::connect_with_registration(port, token, info) {
                        Ok(client) => {
                            // Stopping joins the old link's thread, which can wait seconds in a
                            // handshake: never while holding the lock the UI thread takes.
                            let old = self.ws_slot.lock().ok().and_then(|mut slot| slot.replace(client));
                            if let Some(old) = old {
                                old.stop();
                            }
                            // Close the race window if a commit or attach finished while connecting.
                            sync_registration(&self.inspector, &self.ws_slot);
                        }
                        Err(err) => crate::desktop_host::note(&format!("ws_client start: {err}")),
                    }
                }
            }

            fn on_taskbar_created(&mut self) {
                let hwnd = self.inspector.hwnd.load(Ordering::Acquire);
                if hwnd != 0
                    && REATTACH_IN_PROGRESS
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    crate::desktop_host::note("supervisor: TaskbarCreated -> re-attaching desktop");
                    spawn_attach_worker(
                        "desktop-reattach",
                        hwnd,
                        DISCOVERY_ATTEMPTS,
                        self.inspector.clone(),
                        self.ui.clone(),
                        false,
                        self.ws_slot.clone(),
                    );
                }
            }

            fn on_show(&mut self) {
                let hwnd = self.inspector.hwnd.load(Ordering::Acquire);
                if hwnd != 0
                    && !crate::desktop_host::window_is_attached(hwnd)
                    && REATTACH_IN_PROGRESS
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    spawn_attach_worker(
                        "desktop-show",
                        hwnd,
                        20,
                        self.inspector.clone(),
                        self.ui.clone(),
                        false,
                        self.ws_slot.clone(),
                    );
                }
            }

            fn on_inject_key(&mut self, cmd: ControlCommand) {
                let current_gen = self.active_generation.load(Ordering::Acquire);
                if let ControlCommand::InjectKey { generation, .. } = cmd {
                    if generation != current_gen {
                        return;
                    }
                }
                let Some(ev) = to_injected_key(&cmd) else { return };
                let window = self.inspector.window.lock().ok().and_then(|g| g.clone());
                if let Some(w) = window {
                    w.inject_key(ev);
                }
            }

            fn on_focus_lost(&mut self, generation: u64) {
                if generation != self.active_generation.load(Ordering::Acquire) {
                    return;
                }
                let window = self.inspector.window.lock().ok().and_then(|g| g.clone());
                if let Some(w) = window {
                    w.inject_focus_lost();
                }
            }

            fn on_stop(&mut self, reason: &str) {
                crate::desktop_host::note(&format!("supervisor: stop reason={reason}"));
                self.inspector.request_quit();
            }
        }

        let sup_inspector = inspector.clone();
        let sup_ui = handle.clone();
        let sup_ws = ws_slot.clone();
        let sup_gen = active_generation.clone();
        let _ = std::thread::Builder::new()
            .name("host-stdin".into())
            .spawn(move || {
                let mut sink = SupervisedSink {
                    inspector: sup_inspector,
                    ui: sup_ui,
                    ws_slot: sup_ws,
                    active_generation: sup_gen,
                };
                let stdin = std::io::stdin();
                let _ = crate::supervisor::read_control_stream(stdin.lock(), &mut sink);
            });
    }

    let on_window = inspector.clone();
    let on_window_ws = ws_slot.clone();
    let on_encoded = inspector.clone();
    let on_committed = inspector.clone();
    let on_committed_ws = ws_slot.clone();
    let on_committed_focus = focused_input.clone();
    let last_commit_drawn_c = std::rc::Rc::new(std::cell::Cell::new(None::<u64>));
    let on_drawn = inspector.clone();
    let on_drawn_ws = ws_slot.clone();
    let on_drawn_focus = focused_input.clone();
    let last_commit_drawn_d = last_commit_drawn_c.clone();
    let result = app.on_window(move |window| {
        *on_window.window.lock().unwrap() = Some(window.clone());
        let hwnd = window.with_raw_handle(|raw| match raw.as_raw() {
            raw_window_handle::RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
            _ => None,
        });
        let Ok(Some(hwnd)) = hwnd else {
            on_window.fail("no Win32 window handle".into());
            return;
        };
        let inspector = on_window.clone();
        let ui = handle.clone();
        // r[kernel.no-blocking-main-thread], r[kernel.main-thread-once]: shell
        // discovery and retries stay on a worker; only the final attach is on UI.
        spawn_attach_worker(
            "desktop-discovery",
            hwnd,
            DISCOVERY_ATTEMPTS,
            inspector,
            ui,
            true,
            on_window_ws.clone(),
        );
    })
    .on_frame_encoded(move |frame| {
        if let Ok(mut snapshot) = on_encoded.snapshot.lock() {
            match frame.outcome() {
                FrameOutcome::Drawn => snapshot.drawn += 1,
                FrameOutcome::Skipped => snapshot.skipped += 1,
                _ => snapshot.unchanged += 1,
            }
            let sample = |ring: &mut std::collections::VecDeque<f64>, value: Option<f64>| {
                if let Some(value) = value {
                    if ring.len() == SAMPLES { ring.pop_front(); }
                    ring.push_back(value);
                }
            };
            sample(&mut snapshot.encode_ms, frame.encode_ms());
            sample(&mut snapshot.update_ms, frame.update_ms());
            sample(&mut snapshot.cpu_frame_ms, frame.update_ms().zip(frame.encode_ms()).map(|(update, encode)| update + encode));
            snapshot.last_frame = Some(Instant::now());
        }
    })
    .on_tree_committed(move |rt| {
        let drawn_seq = sync_tree_snapshot(
            rt,
            &on_committed,
            &on_committed_ws,
            &on_committed_focus,
            true,
        );
        last_commit_drawn_c.set(Some(drawn_seq));
    })
    .on_frame_drawn(move |rt| {
        let drawn_seq = on_drawn.snapshot.lock().map(|s| s.drawn).unwrap_or(0);
        if last_commit_drawn_d.take() == Some(drawn_seq) {
            return;
        }
        let _ = sync_tree_snapshot(rt, &on_drawn, &on_drawn_ws, &on_drawn_focus, false);
    }).try_run();
    inspector.stop();
    if let Ok(mut slot) = ws_slot.lock() {
        if let Some(client) = slot.take() {
            client.stop();
        }
    }
    result?;
    if let Some(error) = inspector.failure.lock().map_err(|e| e.to_string())?.clone() { return Err(error); }
    Ok(())
}

/// A window region passes clicks across process boundaries; HTTRANSPARENT alone
/// only walks windows on the same thread. The margin preserves painted shadows.
fn hit_region(rects: &[[f32; 4]], scale: f64) -> Vec<[i32; 4]> {
    const SHADOW: f64 = 16.0;
    rects.iter().map(|rect| {
        let [x, y, w, h] = rect.map(f64::from);
        let left = ((x - SHADOW) * scale).floor() as i32;
        let top = ((y - SHADOW) * scale).floor() as i32;
        let right = ((x + w + SHADOW) * scale).ceil() as i32;
        let bottom = ((y + h + SHADOW) * scale).ceil() as i32;
        [left, top, right - left, bottom - top]
    }).collect()
}

fn sync_registration(inspector: &Inspector, ws_slot: &Arc<Mutex<Option<WsClientHandle>>>) {
    let hwnd = inspector.hwnd.load(Ordering::Acquire);
    let attached = hwnd != 0 && crate::desktop_host::window_is_attached(hwnd);
    let ready = inspector.is_ready(attached).unwrap_or(false);
    if let Ok(slot) = ws_slot.lock() {
        if let Some(client) = slot.as_ref() {
            client.update_registration(inspector.port(), attached, ready);
        }
    }
}

fn sync_tree_snapshot(
    rt: &rune::Runtime<Host>,
    inspector: &Inspector,
    ws_slot: &Arc<Mutex<Option<WsClientHandle>>>,
    focused_input: &AtomicBool,
    from_commit: bool,
) -> u64 {
    let infos = rt.elements();
    let elements = infos.iter().map(|element| json!({
        "path": element.path, "key": element.key, "role": element.role,
        "label": element.label, "text": element.text, "rect": element.rect,
        "opacity": element.opacity,
    })).collect();
    let has_focused_text = infos.iter().any(|element| element.focused && element.focusable && !element.exiting);
    if focused_input.swap(has_focused_text, Ordering::AcqRel) != has_focused_text {
        if let Ok(slot) = ws_slot.lock() {
            if let Some(client) = slot.as_ref() {
                client.report_input_focus(has_focused_text);
            }
        }
    }
    let drawn_seq = {
        let mut snapshot = inspector.snapshot.lock().unwrap();
        // ElementInfo.parent indexes runtime nodes, not this filtered list.
        // G0 has exactly one interactive surface, identified by its stable key.
        snapshot.widget_rects = infos.iter().filter(|element| {
            element.key.as_deref() == Some("g0-card") && !element.exiting
        }).map(|element| element.rect).collect();
        snapshot.elements = elements;
        if from_commit {
            snapshot.committed += 1;
        }
        snapshot.drawn
    };
    if let Err(error) = publish_region(inspector) {
        inspector.fail(error);
    }
    inspector.tree_seq.fetch_add(1, Ordering::AcqRel);
    sync_registration(inspector, ws_slot);
    drawn_seq
}

/// Both events run on Rune's UI thread. A covered window may draw before attach
/// and never draw again, so attaching must publish the last presented geometry.
fn publish_region(inspector: &Inspector) -> Result<(), String> {
    let hwnd = inspector.hwnd.load(Ordering::Acquire);
    if hwnd == 0 { return Ok(()); }
    let (rects, previous) = {
        let snapshot = inspector.snapshot.lock().map_err(|e| e.to_string())?;
        (snapshot.widget_rects.clone(), snapshot.region.clone())
    };
    if rects.is_empty() && previous.is_empty() { return Ok(()); }
    let region = hit_region(&rects, crate::desktop_host::window_scale(hwnd));
    if previous == region { return Ok(()); }
    crate::desktop_host::set_hit_region(hwnd, &region)?;
    if previous.is_empty() && !region.is_empty() {
        // winit still thinks this HWND is top-level. Its style setter would
        // restore WS_POPUP and erase our WS_CHILD after SetParent.
        crate::desktop_host::enable_widget_input(hwnd)?;
    }
    let window = inspector.window.lock().ok().and_then(|g| g.clone());
    inspector.snapshot.lock().map_err(|e| e.to_string())?.region = region;
    if let Some(w) = window {
        w.recheck_occlusion();
    }
    Ok(())
}

pub(crate) fn to_injected_key(cmd: &ControlCommand) -> Option<rune::InjectedKey> {
    let ControlCommand::InjectKey {
        vk,
        scan_code: _,
        extended,
        pressed,
        repeat,
        shift,
        ctrl,
        alt,
        win,
        text,
        key_base,
        ..
    } = cmd
    else {
        return None;
    };

    let physical = vk_to_physical(*vk, *extended)?;
    let mods = rune::Mods {
        shift: *shift,
        cmd: *ctrl,
        alt: *alt,
        ctrl: *ctrl,
        logo: *win,
    };

    let key = match *vk {
        0x0D => Some(rune::Key::Enter),
        0x1B => Some(rune::Key::Escape),
        0x08 => Some(rune::Key::Backspace),
        0x09 => Some(rune::Key::Tab),
        0x2E => Some(rune::Key::Delete),
        0x2D => Some(rune::Key::Insert),
        0x24 => Some(rune::Key::Home),
        0x23 => Some(rune::Key::End),
        0x21 => Some(rune::Key::PageUp),
        0x22 => Some(rune::Key::PageDown),
        0x25 => Some(rune::Key::Left),
        0x26 => Some(rune::Key::Up),
        0x27 => Some(rune::Key::Right),
        0x28 => Some(rune::Key::Down),
        0x5D => Some(rune::Key::ContextMenu),
        0x70..=0x87 => Some(rune::Key::Function((*vk - 0x70 + 1) as u8)),
        0x20 => Some(rune::Key::Text(" ".into())),
        _ if mods.cmd && !(mods.ctrl && mods.alt) => physical_to_command_char(physical).map(rune::Key::Command),
        _ => text.as_ref().filter(|s| !s.is_empty()).map(|s| rune::Key::Text(s.clone())),
    };

    Some(rune::InjectedKey {
        physical,
        pressed: *pressed,
        repeat: *repeat,
        mods,
        logical: key.map(|k| rune::KeyEvent { key: k, mods }),
        key_base: *key_base,
    })
}

fn vk_to_physical(vk: u32, extended: bool) -> Option<rune::PhysicalKeyCode> {
    use rune::PhysicalKeyCode::*;
    Some(match vk {
        0x41 => KeyA, 0x42 => KeyB, 0x43 => KeyC, 0x44 => KeyD, 0x45 => KeyE,
        0x46 => KeyF, 0x47 => KeyG, 0x48 => KeyH, 0x49 => KeyI, 0x4A => KeyJ,
        0x4B => KeyK, 0x4C => KeyL, 0x4D => KeyM, 0x4E => KeyN, 0x4F => KeyO,
        0x50 => KeyP, 0x51 => KeyQ, 0x52 => KeyR, 0x53 => KeyS, 0x54 => KeyT,
        0x55 => KeyU, 0x56 => KeyV, 0x57 => KeyW, 0x58 => KeyX, 0x59 => KeyY,
        0x5A => KeyZ,
        0x30 => Digit0, 0x31 => Digit1, 0x32 => Digit2, 0x33 => Digit3, 0x34 => Digit4,
        0x35 => Digit5, 0x36 => Digit6, 0x37 => Digit7, 0x38 => Digit8, 0x39 => Digit9,
        0x60 => Numpad0, 0x61 => Numpad1, 0x62 => Numpad2, 0x63 => Numpad3, 0x64 => Numpad4,
        0x65 => Numpad5, 0x66 => Numpad6, 0x67 => Numpad7, 0x68 => Numpad8, 0x69 => Numpad9,
        0x6A => NumpadMultiply, 0x6B => NumpadAdd, 0x6D => NumpadSubtract,
        0x6E => NumpadDecimal, 0x6F => NumpadDivide,
        0x70 => F1, 0x71 => F2, 0x72 => F3, 0x73 => F4, 0x74 => F5, 0x75 => F6,
        0x76 => F7, 0x77 => F8, 0x78 => F9, 0x79 => F10, 0x7A => F11, 0x7B => F12,
        0x7C => F13, 0x7D => F14, 0x7E => F15, 0x7F => F16, 0x80 => F17, 0x81 => F18,
        0x82 => F19, 0x83 => F20, 0x84 => F21, 0x85 => F22, 0x86 => F23, 0x87 => F24,
        0x0D if extended => NumpadEnter,
        0x0D => Enter,
        0x1B => Escape,
        0x08 => Backspace,
        0x09 => Tab,
        0x20 => Space,
        0x2E => Delete,
        0x2D => Insert,
        0x24 => Home,
        0x23 => End,
        0x21 => PageUp,
        0x22 => PageDown,
        0x25 => ArrowLeft,
        0x26 => ArrowUp,
        0x27 => ArrowRight,
        0x28 => ArrowDown,
        0xBD => Minus,
        0xBB => Equal,
        0xBC => Comma,
        0xBE => Period,
        0xBF => Slash,
        0xBA => Semicolon,
        0xDE => Quote,
        0xDB => BracketLeft,
        0xDD => BracketRight,
        0xDC => Backslash,
        0xC0 => Backquote,
        0x10 | 0xA0 => ShiftLeft,
        0xA1 => ShiftRight,
        0x11 | 0xA2 => ControlLeft,
        0xA3 => ControlRight,
        0x12 | 0xA4 => AltLeft,
        0xA5 => AltRight,
        0x5B => SuperLeft,
        0x5C => SuperRight,
        0x5D => ContextMenu,
        _ => return None,
    })
}

fn physical_to_command_char(physical: rune::PhysicalKeyCode) -> Option<char> {
    use rune::PhysicalKeyCode::*;
    Some(match physical {
        KeyA => 'a', KeyB => 'b', KeyC => 'c', KeyD => 'd', KeyE => 'e',
        KeyF => 'f', KeyG => 'g', KeyH => 'h', KeyI => 'i', KeyJ => 'j',
        KeyK => 'k', KeyL => 'l', KeyM => 'm', KeyN => 'n', KeyO => 'o',
        KeyP => 'p', KeyQ => 'q', KeyR => 'r', KeyS => 's', KeyT => 't',
        KeyU => 'u', KeyV => 'v', KeyW => 'w', KeyX => 'x', KeyY => 'y',
        KeyZ => 'z',
        Digit0 => '0', Digit1 => '1', Digit2 => '2', Digit3 => '3', Digit4 => '4',
        Digit5 => '5', Digit6 => '6', Digit7 => '7', Digit8 => '8', Digit9 => '9',
        Minus => '-', Equal => '=', Comma => ',', Period => '.', Slash => '/',
        Semicolon => ';', Quote => '\'', BracketLeft => '[', BracketRight => ']',
        Backslash => '\\', Backquote => '`',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn input_edges_include_shadow_and_round_outward_at_fractional_dpi() {
        assert_eq!(super::hit_region(&[[24.0, 24.0, 320.0, 80.0]], 1.5), vec![[12, 12, 528, 168]]);
        assert_eq!(super::hit_region(&[[16.1, 16.1, 1.0, 1.0]], 1.25), vec![[0, 0, 42, 42]]);
        assert!(super::hit_region(&[], 1.5).is_empty());
    }

    #[test]
    fn inject_key_translation_handles_ctrl_a_enter_escape_and_russian_layout() {
        let ctrl_a = crate::supervisor::ControlCommand::InjectKey {
            generation: 1,
            vk: 0x41,
            scan_code: 30,
            extended: false,
            pressed: true,
            repeat: false,
            shift: false,
            ctrl: true,
            alt: false,
            win: false,
            text: None,
            key_base: Some('ф'),
        };
        let mapped_a = super::to_injected_key(&ctrl_a).expect("ctrl+a maps to InjectedKey");
        assert_eq!(mapped_a.physical, rune::PhysicalKeyCode::KeyA);
        assert!(mapped_a.mods.ctrl && mapped_a.mods.cmd);
        assert_eq!(mapped_a.key_base, Some('ф'));
        assert!(mapped_a.logical.as_ref().is_some_and(|ev| ev.is_command('a')));

        let enter = crate::supervisor::ControlCommand::InjectKey {
            generation: 1, vk: 0x0D, scan_code: 28, extended: false, pressed: true, repeat: false,
            shift: false, ctrl: false, alt: false, win: false, text: None, key_base: None,
        };
        let mapped_enter = super::to_injected_key(&enter).expect("enter maps");
        assert_eq!(mapped_enter.physical, rune::PhysicalKeyCode::Enter);
        assert_eq!(mapped_enter.logical.as_ref().map(|ev| &ev.key), Some(&rune::Key::Enter));

        let esc = crate::supervisor::ControlCommand::InjectKey {
            generation: 1, vk: 0x1B, scan_code: 1, extended: false, pressed: true, repeat: false,
            shift: false, ctrl: false, alt: false, win: false, text: None, key_base: None,
        };
        let mapped_esc = super::to_injected_key(&esc).expect("escape maps");
        assert_eq!(mapped_esc.physical, rune::PhysicalKeyCode::Escape);
        assert_eq!(mapped_esc.logical.as_ref().map(|ev| &ev.key), Some(&rune::Key::Escape));

        let russian = crate::supervisor::ControlCommand::InjectKey {
            generation: 1, vk: 0xDE, scan_code: 40, extended: false, pressed: true, repeat: false,
            shift: false, ctrl: false, alt: false, win: false, text: Some("э".into()), key_base: Some('э'),
        };
        let mapped_ru = super::to_injected_key(&russian).expect("russian text maps");
        assert_eq!(mapped_ru.physical, rune::PhysicalKeyCode::Quote);
        assert_eq!(mapped_ru.key_base, Some('э'));
        assert_eq!(mapped_ru.logical.as_ref().map(|ev| &ev.key), Some(&rune::Key::Text("э".into())));
    }
}
