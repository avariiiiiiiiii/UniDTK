#![windows_subsystem = "windows"]
//! `unidtk-desktop`: the native desktop host, drawn by Rune (`docs/dev/plan-rune-host.md`).
//!
//! G0 starts with the question the whole host depends on: does a transparent Rune window,
//! made a child of the desktop host (WorkerW, or Progman on the newer Windows 11 layout),
//! show what lies under it through its clear pixels? Rune makes its translucent surfaces
//! through DirectComposition, which is proven for top-level windows only.
//!
//! `unidtk-desktop --probe-transparency <report.json>` answers it on this machine: it reads
//! a few pixels of the desktop host as DWM composed it, attaches a full-screen transparent window with one half-transparent
//! card, reads the same pixels again, writes the report and exits.

mod desktop_host;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rune::el::{el, text};
use rune::{App, AppSpec, Color, WindowOptions};

/// The card the probe draws, in Rune's points; the screen has `scale()` pixels per point.
const CARD: (i32, i32, i32, i32) = (120, 520, 320, 200);

/// Physical pixels per point: Rune lays out in points, the reads are in pixels (1.5 at
/// 150 %; the first probes sampled unscaled points and missed the card).
fn scale() -> f32 {
    unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() as f32 / 96.0 }
}

struct Probe;

fn view(_: &Probe, _: &mut rune::runtime::Cx<Probe>) -> rune::el::El {
    let (x, y, w, h) = CARD;
    el().w_full().h_full().child(
        el().absolute(x as f32, y as f32)
            .size(w as f32, h as f32)
            .fill(Color::WHITE.alpha(0.5))
            .radius(24.0)
            .child(text("UniDTK G0")),
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.iter().position(|arg| arg == "--probe-transparency") {
        Some(index) => {
            let report = args.get(index + 1).cloned().unwrap_or_else(|| "transparency.json".into());
            probe_transparency(report.into());
        }
        None => eprintln!("unidtk-desktop: only --probe-transparency <report.json> exists yet (G0)"),
    }
}

/// Points outside the card must not change; the point inside must.
fn sample_points() -> Vec<(&'static str, i32, i32)> {
    let (x, y, w, h) = CARD;
    let px = |points: i32| (points as f32 * scale()).round() as i32;
    vec![
        ("inside", px(x + w / 2), px(y + h / 2)),
        ("outside-left", px(x - 60), px(y + h / 2)),
        ("outside-above", px(x + w / 2), px(y - 60)),
        ("outside-far", px(1000), px(600)),
    ]
}

fn probe_transparency(report: std::path::PathBuf) {
    let points = sample_points();
    let before = desktop_host::read_pixels(&points, "before");
    let attached = Arc::new(AtomicBool::new(false));
    let (width, height) = desktop_host::primary_size();

    let spec = AppSpec::new("UniDTK desktop", (width as f32, height as f32), || Probe, view);
    let mut app = App::new(spec).window_options(WindowOptions {
        transparent: true,
        borderless: true,
        click_through: true,
        resizable: false,
        physical_size: Some((width as u32, height as u32)),
        position: Some((0, 0)),
        ..Default::default()
    });
    let handle = app.handle();
    let on_ready_attached = attached.clone();
    // At creation, not `on_ready`: that one waits for a first frame, and the window is
    // attached before it draws so its first present lands where it will stay.
    app = app.on_frame_encoded(|frame| desktop_host::note(&format!("frame {} {:?} encode {:?} ms", frame.seq(), frame.outcome(), frame.encode_ms())));
    app = app.on_window(move |window| {
        let hwnd = window.with_raw_handle(|raw| match raw.as_raw() {
            raw_window_handle::RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
            _ => None,
        });
        let outcome = match hwnd {
            Ok(Some(hwnd)) => desktop_host::attach(hwnd),
            Ok(None) => Err("not a Win32 window".into()),
            Err(error) => Err(error),
        };
        desktop_host::note(&format!("attach: {outcome:?}"));
        on_ready_attached.store(outcome.is_ok(), Ordering::SeqCst);
    });

    std::thread::spawn(move || {
        // Waits for the attach, then lets a few frames land; how long the window took to
        // appear is part of the answer.
        let started = std::time::Instant::now();
        while !attached.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let attach_ms = started.elapsed().as_millis();
        std::thread::sleep(Duration::from_millis(1500));
        let after = desktop_host::read_pixels(&points, "after");
        let rows: Vec<serde_json::Value> = points
            .iter()
            .zip(before.iter().zip(after.iter()))
            .map(|((name, x, y), (b, a))| {
                serde_json::json!({ "point": name, "x": x, "y": y, "before": b, "after": a, "changed": b != a })
            })
            .collect();
        let outside_kept = rows.iter().filter(|row| row["point"] != "inside").all(|row| row["changed"] == false);
        let inside_changed = rows.iter().any(|row| row["point"] == "inside" && row["changed"] == true);
        // Rune's own steps to the first frame, in milliseconds from the first one.
        let marks: std::collections::BTreeMap<String, u64> = {
            let marks = rune::marks::marks();
            let origin = marks.iter().map(|(_, at)| *at).min();
            marks
                .iter()
                .map(|(name, at)| (name.to_string(), origin.map_or(0, |origin| at.duration_since(origin).as_millis() as u64)))
                .collect()
        };
        let summary = serde_json::json!({
            "attached": attached.load(Ordering::SeqCst),
            "window_error": handle.window_error(),
            "attach_ms": attach_ms,
            "marks": marks,
            "notes": rune::marks::notes(),
            "visible": handle.is_visible(),
            "host": desktop_host::host_description(),
            "scale": scale(),
            "transparent": outside_kept && inside_changed,
            "outside_kept": outside_kept,
            "inside_changed": inside_changed,
            "points": rows,
        });
        let _ = std::fs::write(&report, serde_json::to_string_pretty(&summary).unwrap_or_default());
        handle.quit();
        // The probe must end even if the event loop does not take the request.
        std::thread::sleep(Duration::from_secs(2));
        desktop_host::note("quit was not taken; exiting");
        std::process::exit(0);
    });
    app.run();
}
