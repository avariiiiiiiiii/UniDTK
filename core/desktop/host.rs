//! G0's standalone native window and an observable card. Production stays on WebView2.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use rune::el::{col, el, text};
use rune::{App, AppSpec, Color, FrameOutcome, WindowOptions};
use serde_json::json;
use crate::inspector::Inspector;

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

pub fn run() -> Result<(), String> {
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
            physical_size: Some((width as u32, height as u32)), position: Some((0, 0)),
            ..Default::default()
        });
    let handle = app.handle();
    let quit_handle = handle.clone();
    inspector.start(move || quit_handle.quit())?;

    let on_window = inspector.clone();
    let on_encoded = inspector.clone();
    let on_drawn = inspector.clone();
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
        let spawn = std::thread::Builder::new().name("desktop-discovery".into()).spawn(move || {
            let mut error = "desktop unavailable".to_string();
            for _ in 0..40 {
                if inspector.stopping.load(Ordering::Acquire) { return; }
                match crate::desktop_host::discover() {
                    Ok(parent) => {
                        ui.update(move |state, _| {
                            if state.inspector.stopping.load(Ordering::Acquire) { return; }
                            match crate::desktop_host::attach_to(hwnd, parent) {
                                Ok(()) => {
                                    state.inspector.hwnd.store(hwnd, Ordering::Release);
                                    if let Err(error) = publish_region(&state.inspector) {
                                        state.inspector.fail(error);
                                        return;
                                    }
                                    crate::desktop_host::note(&format!("host attached under {}", crate::desktop_host::host_description()));
                                }
                                Err(error) => state.inspector.fail(error),
                            }
                        });
                        return;
                    }
                    Err(next) => error = next,
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            inspector.fail(format!("desktop discovery exhausted: {error}"));
        });
        if let Err(error) = spawn { on_window.fail(format!("desktop worker: {error}")); }
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
    .on_frame_drawn(move |rt| {
        let infos = rt.elements();
        let elements = infos.iter().map(|element| json!({
            "path": element.path, "key": element.key, "role": element.role,
            "label": element.label, "text": element.text, "rect": element.rect,
            "opacity": element.opacity,
        })).collect();
        {
            let mut snapshot = on_drawn.snapshot.lock().unwrap();
            // ElementInfo.parent indexes runtime nodes, not this filtered list.
            // G0 has exactly one interactive surface, identified by its stable key.
            snapshot.widget_rects = infos.iter().filter(|element| {
                element.key.as_deref() == Some("g0-card") && !element.exiting
            }).map(|element| element.rect).collect();
            snapshot.elements = elements;
        }
        if let Err(error) = publish_region(&on_drawn) { on_drawn.fail(error); }
    }).try_run();
    inspector.stop();
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
    inspector.snapshot.lock().map_err(|e| e.to_string())?.region = region;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn input_edges_include_shadow_and_round_outward_at_fractional_dpi() {
        assert_eq!(super::hit_region(&[[24.0, 24.0, 320.0, 80.0]], 1.5), vec![[12, 12, 528, 168]]);
        assert_eq!(super::hit_region(&[[16.1, 16.1, 1.0, 1.0]], 1.25), vec![[0, 0, 42, 42]]);
        assert!(super::hit_region(&[], 1.5).is_empty());
    }
}
