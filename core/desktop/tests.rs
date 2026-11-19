use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, EVENT_OBJECT_DESTROY, EVENT_OBJECT_LOCATIONCHANGE, OBJID_CLIENT, OBJID_WINDOW,
    WS_CHILD, WS_VISIBLE,
};

#[test]
fn desktop_shell_classes_are_excluded_for_explorer_layers_only() {
    for cls in ["Progman", "WorkerW", "SHELLDLL_DefView", "SysListView32"] {
        assert!(rune::win32_occlusion::is_desktop_shell_class(cls), "expected desktop layer: {cls}");
    }
    for cls in ["CabinetWClass", "ExploreWClass", "Chrome_WidgetWin_1", ""] {
        assert!(!rune::win32_occlusion::is_desktop_shell_class(cls), "did not expect desktop layer: {cls}");
    }
}

#[test]
fn win_event_filter_skips_third_party_child_destroys() {
    assert!(!rune::win32_occlusion::should_process_win_event(
        EVENT_OBJECT_DESTROY,
        OBJID_WINDOW.0,
        CHILDID_SELF as i32,
        Some(WS_CHILD.0 | WS_VISIBLE.0),
        false,
    ));
    assert!(rune::win32_occlusion::should_process_win_event(
        EVENT_OBJECT_DESTROY,
        OBJID_WINDOW.0,
        CHILDID_SELF as i32,
        Some(WS_VISIBLE.0),
        true,
    ));
    assert!(rune::win32_occlusion::should_process_win_event(
        EVENT_OBJECT_DESTROY,
        OBJID_WINDOW.0,
        CHILDID_SELF as i32,
        None,
        false,
    ));
    assert!(rune::win32_occlusion::should_process_win_event(
        EVENT_OBJECT_LOCATIONCHANGE,
        OBJID_WINDOW.0,
        CHILDID_SELF as i32,
        Some(WS_VISIBLE.0),
        true,
    ));
    assert!(!rune::win32_occlusion::should_process_win_event(
        EVENT_OBJECT_LOCATIONCHANGE,
        OBJID_WINDOW.0,
        CHILDID_SELF as i32,
        Some(WS_CHILD.0),
        false,
    ));
    assert!(!rune::win32_occlusion::should_process_win_event(
        EVENT_OBJECT_LOCATIONCHANGE,
        OBJID_CLIENT.0,
        CHILDID_SELF as i32,
        Some(WS_VISIBLE.0),
        true,
    ));
}

#[test]
fn lock_screen_inaccessible_desktop_marks_window_covered() {
    assert_eq!(
        rune::win32_occlusion::fallback_found_for(rune::win32_occlusion::DesktopAccess::Inaccessible, true),
        rune::win32_occlusion::ProbeFound::Covered,
    );
    assert_eq!(
        rune::win32_occlusion::fallback_found_for(rune::win32_occlusion::DesktopAccess::Other, true),
        rune::win32_occlusion::ProbeFound::Seen,
    );
    assert_eq!(
        rune::win32_occlusion::fallback_found_for(rune::win32_occlusion::DesktopAccess::Other, false),
        rune::win32_occlusion::ProbeFound::Hidden,
    );
}

#[test]
fn child_overlap_clips_to_child_rect_and_region_not_secondary_monitor() {
    unsafe {
        // Multi-monitor Progman root spanning 0..3840 x 0..1080; child sits only on primary 0..1920.
        let root_rect = windows_sys_rect(0, 0, 3840, 1080);
        let child_rect = windows_sys_rect(0, 0, 1920, 1080);
        // Uncovered region exists ONLY on monitor 2 (1920..3840); primary is completely covered.
        let left_monitor2 = CreateRectRgn(1920, 0, 3840, 1080);
        assert_eq!(
            rune::win32_occlusion::child_overlap_with_left(child_rect, root_rect, None, to_sys_hrgn(left_monitor2)),
            rune::win32_occlusion::ProbeFound::Covered,
        );
        let _ = DeleteObject(left_monitor2);

        // Child has SetWindowRgn covering card (24,24..344,104). Only right side of primary (400..1920) is uncovered -> Covered.
        let card_rgn = CreateRectRgn(24, 24, 344, 104);
        let left_right_side = CreateRectRgn(400, 0, 1920, 1080);
        assert_eq!(
            rune::win32_occlusion::child_overlap_with_left(
                child_rect,
                root_rect,
                Some(to_sys_hrgn(card_rgn)),
                to_sys_hrgn(left_right_side),
            ),
            rune::win32_occlusion::ProbeFound::Covered,
        );
        let _ = DeleteObject(left_right_side);

        // Once (0..1920) becomes uncovered, the card region intersects `left` -> Seen.
        let left_full_primary = CreateRectRgn(0, 0, 1920, 1080);
        assert_eq!(
            rune::win32_occlusion::child_overlap_with_left(
                child_rect,
                root_rect,
                Some(to_sys_hrgn(card_rgn)),
                to_sys_hrgn(left_full_primary),
            ),
            rune::win32_occlusion::ProbeFound::Seen,
        );
        let _ = DeleteObject(left_full_primary);
        let _ = DeleteObject(card_rgn);

        // Explicit empty SetWindowRgn (0,0,0,0) -> Covered even if left is full screen.
        let empty_rgn = CreateRectRgn(0, 0, 0, 0);
        let left_full = CreateRectRgn(0, 0, 1920, 1080);
        assert_eq!(
            rune::win32_occlusion::child_overlap_with_left(
                child_rect,
                root_rect,
                Some(to_sys_hrgn(empty_rgn)),
                to_sys_hrgn(left_full),
            ),
            rune::win32_occlusion::ProbeFound::Covered,
        );
        let _ = DeleteObject(left_full);
        let _ = DeleteObject(empty_rgn);
    }
}

#[test]
fn single_instance_mutex_blocks_duplicate_and_recovers_from_abandoned() {
    use std::sync::mpsc;
    let unique = format!(
        "Local\\UniDTK.DesktopHost.Test.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );

    let first = crate::desktop_host::acquire_named_mutex(&unique).expect("first mutex acquire must succeed");
    let same_name = unique.clone();
    let duplicate = std::thread::spawn(move || crate::desktop_host::acquire_named_mutex(&same_name).err())
        .join()
        .expect("duplicate probe thread should not panic");
    assert!(duplicate.is_some(), "live mutex owner must block duplicate instance");
    drop(first);

    let second = crate::desktop_host::acquire_named_mutex(&unique).expect("re-acquire after Drop must succeed");
    drop(second);

    // Simulate owning thread dying without ReleaseMutex while the handle remains alive.
    let (tx, rx) = mpsc::channel::<usize>();
    let abandon_name = unique.clone();
    std::thread::spawn(move || {
        let guard = crate::desktop_host::acquire_named_mutex(&abandon_name).expect("thread acquire");
        let leaked = guard.into_raw_for_test();
        let _ = tx.send(leaked);
    })
    .join()
    .expect("abandoning thread exits cleanly");

    let leaked_handle = rx.recv().expect("receive raw handle");
    let recovered = crate::desktop_host::acquire_named_mutex(&unique)
        .expect("WAIT_ABANDONED after dead owner thread must recover ownership");
    drop(recovered);
    unsafe {
        let _ = windows::Win32::Foundation::CloseHandle(windows::Win32::Foundation::HANDLE(leaked_handle as *mut _));
    }
}

#[test]
fn private_file_uses_dynamic_user_sid_dacl_not_ow() {
    let sddl = crate::desktop_host::user_owner_system_sddl().expect("resolve current TokenUser SDDL");
    assert!(sddl.starts_with("D:P(A;;GA;;;S-1-"), "unexpected user SID DACL: {sddl}");
    assert!(sddl.ends_with(")(A;;GA;;;SY)"), "missing SYSTEM ACE in DACL: {sddl}");
    assert!(!sddl.contains("OW"), "SDDL must not hardcode literal OW: {sddl}");

    let dir = std::env::temp_dir().join(format!(
        "unidtk-desktop-acl-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let target = dir.join("nested").join("meta.json");
    crate::desktop_host::write_private_file_atomic(&target, b"first-payload").expect("initial atomic write");
    assert_eq!(std::fs::read(&target).unwrap(), b"first-payload");

    crate::desktop_host::write_private_file_atomic(&target, b"second").expect("atomic replace");
    assert_eq!(std::fs::read(&target).unwrap(), b"second");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn supervised_inspector_binds_ephemeral_port_writes_pid_metadata_and_reserves_priority_slot() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::{Duration, Instant};

    let dir = std::env::temp_dir().join(format!(
        "unidtk-inspector-sup-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let foreign_standalone = dir.join("host-standalone.json");
    std::fs::write(&foreign_standalone, r#"{"pid":999999,"port":26541,"token":"foreign","supervised":false}"#).unwrap();

    let inspector = crate::inspector::Inspector::new_in(dir.clone()).unwrap();
    let quit_flag = Arc::new(AtomicBool::new(false));
    let flag_clone = quit_flag.clone();
    inspector.start(true, move || {
        flag_clone.store(true, Ordering::Release);
    }).expect("start supervised inspector");

    let port = inspector.port();
    assert_ne!(port, 0);
    assert_ne!(port, crate::protocol::PORT, "supervised inspector must bind ephemeral port");

    let pid_meta_path = dir.join(format!("host-inspector.{}.json", std::process::id()));
    let pid_meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&pid_meta_path).unwrap()).unwrap();
    assert_eq!(pid_meta["pid"].as_u64(), Some(std::process::id() as u64));
    assert_eq!(pid_meta["port"].as_u64(), Some(port as u64));
    assert_eq!(pid_meta["supervised"].as_bool(), Some(true));
    let token = pid_meta["token"].as_str().expect("token string").to_string();
    assert_eq!(token.len(), 64);

    // Hold all 4 normal worker slots with idle sockets.
    let mut idle = Vec::new();
    for _ in 0..4 {
        idle.push(TcpStream::connect(("127.0.0.1", port)).unwrap());
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while inspector.active_normal_clients() < 4 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(inspector.active_normal_clients(), 4, "4 idle clients must occupy all normal slots");

    // 5th `tree` must be rejected cleanly with busy because only priority `ping`/`quit` can use the reserve slot.
    let mut tree_sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
    writeln!(tree_sock, r#"{{"cmd":"tree"}}"#).unwrap();
    tree_sock.flush().unwrap();
    let mut line = String::new();
    BufReader::new(&mut tree_sock).read_line(&mut line).unwrap();
    let tree_reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(tree_reply["ok"].as_bool(), Some(false));
    assert!(
        tree_reply["error"].as_str().unwrap_or("").to_lowercase().contains("busy"),
        "expected busy rejection for 5th tree: {tree_reply}"
    );
    drop(tree_sock);
    let slot_deadline = Instant::now() + Duration::from_secs(1);
    while inspector.active_priority_clients() > 0 && Instant::now() < slot_deadline {
        std::thread::sleep(Duration::from_millis(2));
    }

    // 5th/next `ping` uses reserved priority slot and succeeds immediately.
    let mut ping_sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
    writeln!(ping_sock, r#"{{"cmd":"ping"}}"#).unwrap();
    ping_sock.flush().unwrap();
    line.clear();
    BufReader::new(&mut ping_sock).read_line(&mut line).unwrap();
    let ping_reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(ping_reply["ok"].as_bool(), Some(true));
    assert_eq!(ping_reply["result"]["port"].as_u64(), Some(port as u64));
    drop(ping_sock);
    let slot_deadline = Instant::now() + Duration::from_secs(1);
    while inspector.active_priority_clients() > 0 && Instant::now() < slot_deadline {
        std::thread::sleep(Duration::from_millis(2));
    }

    // `quit` uses reserved priority slot even while all 4 normal sockets remain idle.
    let mut quit_sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
    writeln!(quit_sock, "{}", serde_json::json!({ "cmd": "quit", "args": { "token": token } })).unwrap();
    quit_sock.flush().unwrap();
    line.clear();
    BufReader::new(&mut quit_sock).read_line(&mut line).unwrap();
    let quit_reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(quit_reply["ok"].as_bool(), Some(true));
    let quit_deadline = Instant::now() + Duration::from_millis(500);
    while !quit_flag.load(Ordering::Acquire) && Instant::now() < quit_deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(quit_flag.load(Ordering::Acquire));

    inspector.stop();
    drop(idle);
    assert!(!pid_meta_path.exists(), "own PID metadata must be cleaned up on stop");
    assert!(foreign_standalone.exists(), "supervised stop must not delete foreign standalone metadata");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn standalone_inspector_writes_standalone_metadata_and_falls_back_on_busy_port() {
    use std::net::TcpListener;

    let dir = std::env::temp_dir().join(format!(
        "unidtk-inspector-standalone-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();

    // Case 1: preferred port is free -> binds exact preferred port and writes both files.
    let probe = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let free_port = probe.local_addr().unwrap().port();
    drop(probe);

    let insp1 = crate::inspector::Inspector::new_in(dir.clone()).unwrap();
    insp1.start_on_port(false, free_port, || {}).expect("bind free preferred port");
    assert_eq!(insp1.port(), free_port);
    let standalone_path = dir.join("host-standalone.json");
    let standalone_meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&standalone_path).unwrap()).unwrap();
    assert_eq!(standalone_meta["port"].as_u64(), Some(free_port as u64));
    assert_eq!(standalone_meta["supervised"].as_bool(), Some(false));
    insp1.stop();
    assert!(!standalone_path.exists(), "standalone metadata must be cleaned up on own stop");

    // Case 2: preferred port is occupied -> falls back to ephemeral port cleanly.
    let blocker = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let busy_port = blocker.local_addr().unwrap().port();
    let insp2 = crate::inspector::Inspector::new_in(dir.clone()).unwrap();
    insp2.start_on_port(false, busy_port, || {}).expect("fallback to ephemeral port when busy");
    assert_ne!(insp2.port(), busy_port);
    assert_ne!(insp2.port(), 0);
    insp2.stop();
    drop(blocker);
    let _ = std::fs::remove_dir_all(&dir);
}

fn windows_sys_rect(left: i32, top: i32, right: i32, bottom: i32) -> rune::win32_occlusion::Rect {
    rune::win32_occlusion::Rect { left, top, right, bottom }
}

fn to_sys_hrgn(rgn: windows::Win32::Graphics::Gdi::HRGN) -> rune::win32_occlusion::Hrgn {
    rgn.0 as rune::win32_occlusion::Hrgn
}

#[test]
fn surface_counts_records_wait_cancelled_separately_from_wakes() {
    use std::time::{Duration, Instant};
    let mut counts = rune::win32_telemetry::SurfaceCounts::default();
    let cancelled = rune::win32_telemetry::StartCause::WaitCancelled {
        start: Instant::now(),
        requested_resume: Some(Instant::now() + Duration::from_secs(1)),
    };
    let timer = rune::win32_telemetry::StartCause::ResumeTimeReached {
        start: Instant::now(),
        requested_resume: Instant::now() + Duration::from_millis(16),
    };
    assert!(rune::win32_telemetry::is_wait_cancelled(cancelled));
    assert!(!rune::win32_telemetry::is_wait_cancelled(timer));
    counts.record_turn(false, true);
    counts.record_turn(true, false);
    assert_eq!(counts.turns, 2);
    assert_eq!(counts.wakes, 1);
    assert_eq!(counts.wait_cancelled, 1);
}

#[test]
fn readback_staging_layout_pads_rows_and_beats_4mib_general_heap() {
    assert!(rune::win32_telemetry::readback_staging_layout(0, 10).is_err());
    let (row_small, size_small) = rune::win32_telemetry::readback_staging_layout(10, 10).unwrap();
    assert_eq!(row_small, 256);
    assert!(size_small > 4 * 1024 * 1024, "staging buffer {size_small} must exceed 4 MiB general heap");

    let (row_hd, size_hd) = rune::win32_telemetry::readback_staging_layout(1280, 720).unwrap();
    assert_eq!(row_hd, 5120);
    assert!(size_hd > 4 * 1024 * 1024, "1280x720 buffer {size_hd} must exceed 4 MiB general heap");

    let (row_fhd, size_fhd) = rune::win32_telemetry::readback_staging_layout(1920, 1080).unwrap();
    assert_eq!(row_fhd, 7680);
    assert_eq!(size_fhd, 7680 * 1080);

    assert!(rune::win32_telemetry::readback_staging_layout(4096, 4096).is_ok());
    assert!(
        rune::win32_telemetry::readback_staging_layout(4097, 4096).is_err(),
        "readback staging layout must reject captures larger than 16 MP"
    );
}

#[test]
fn unpremultiply_to_straight_rgba_preserves_transparent_alpha_and_swaps_bgra() {
    let mut pixels = vec![
        12, 34, 56, 0,    // transparent BGRA with fringe -> zeroed RGBA
        64, 128, 32, 128, // premultiplied BGRA (B=64, G=128, R=32, A=128)
        10, 20, 30, 255,  // opaque BGRA
    ];
    rune::win32_telemetry::unpremultiply_to_straight_rgba(&mut pixels, true);
    assert_eq!(&pixels[0..4], &[0, 0, 0, 0]);
    assert_eq!(pixels[7], 128);
    assert_eq!(pixels[4], 64);  // R: (32*255 + 64)/128 = 64
    assert_eq!(pixels[5], 255); // G: min(255)
    assert_eq!(pixels[6], 128); // B: (64*255 + 64)/128 = 128
    assert_eq!(&pixels[8..12], &[30, 20, 10, 255]);
}

#[test]
fn encode_rgba_png_roundtrips_true_transparent_backdrop_and_widget_pixel() {
    let rgba = vec![
        0, 0, 0, 0,
        19, 20, 25, 255,
    ];
    let bytes = crate::desktop_host::encode_rgba_png(2, 1, &rgba).expect("png encode");
    assert!(!bytes.is_empty());
    let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
    let mut reader = decoder.read_info().expect("png decode header");
    let mut decoded = vec![0u8; reader.output_buffer_size().expect("output_buffer_size")];
    let info = reader.next_frame(&mut decoded).expect("png decode frame");
    assert_eq!((info.width, info.height), (2, 1));
    assert_eq!(&decoded[..8], &rgba[..]);
}

#[test]
fn modifiers_state_from_mods_maps_ctrl_alt_shift_and_logo_cleanly() {
    let mods = rune::Mods { shift: true, cmd: true, alt: true, ctrl: true, logo: false };
    let state = rune::win32_telemetry::modifiers_state_from_mods(mods);
    assert!(state.shift_key());
    assert!(state.control_key());
    assert!(state.alt_key());
    assert!(!state.super_key());

    let cmd_only = rune::Mods { shift: false, cmd: true, alt: false, ctrl: false, logo: true };
    let cmd_state = rune::win32_telemetry::modifiers_state_from_mods(cmd_only);
    assert!(cmd_state.control_key());
    assert!(cmd_state.super_key());
}

#[test]
fn supervised_stdin_reader_parses_control_lines_and_triggers_stop_on_eof() {
    use crate::supervisor::{read_control_stream, ControlSink, StartHandshake};

    #[derive(Default)]
    struct RecordingSink {
        started: Option<StartHandshake>,
        taskbar_reattached: usize,
        shows: usize,
        stops: Vec<String>,
    }

    impl ControlSink for RecordingSink {
        fn on_start(&mut self, handshake: StartHandshake) {
            self.started = Some(handshake);
        }
        fn on_taskbar_created(&mut self) {
            self.taskbar_reattached += 1;
        }
        fn on_show(&mut self) {
            self.shows += 1;
        }
        fn on_stop(&mut self, reason: &str) {
            self.stops.push(reason.to_string());
        }
    }

    let input = concat!(
        r#"{"cmd":"start","generation":7,"instance_nonce":"nonce-7","ws_port":26599,"ws_token":"secret"}"#, "\n",
        r#"{"cmd":"taskbar_created"}"#, "\n",
        r#"{"cmd":"show"}"#, "\n",
    );
    let mut sink = RecordingSink::default();
    read_control_stream(std::io::Cursor::new(input), &mut sink).unwrap();

    assert_eq!(
        sink.started,
        Some(StartHandshake {
            generation: 7,
            instance_nonce: "nonce-7".into(),
            ws_port: Some(26599),
            ws_token: Some("secret".into()),
        })
    );
    assert_eq!(sink.taskbar_reattached, 1);
    assert_eq!(sink.shows, 1);
    assert_eq!(sink.stops, vec!["eof"]);
}

#[test]
fn a_wheel_line_from_the_kernel_parses() {
    use crate::supervisor::{read_control_stream, ControlSink, StartHandshake};

    #[derive(Default)]
    struct Wheels(Vec<(u64, i32, i32, i32, bool)>);

    impl ControlSink for Wheels {
        fn on_start(&mut self, _: StartHandshake) {}
        fn on_taskbar_created(&mut self) {}
        fn on_show(&mut self) {}
        fn on_stop(&mut self, _: &str) {}
        fn on_wheel(&mut self, generation: u64, x: i32, y: i32, delta: i32, touchpad: bool) {
            self.0.push((generation, x, y, delta, touchpad));
        }
    }

    // The line `host_supervisor::tests::a_wheel_is_one_line_the_host_can_read` pins.
    let input = concat!(
        r#"{"cmd":"wheel","generation":2,"x":-40,"y":612,"delta":-120,"touchpad":true}"#, "\n",
        r#"{"cmd":"wheel","generation":2,"x":5,"y":6,"delta":120}"#, "\n",
    );
    let mut sink = Wheels::default();
    read_control_stream(std::io::Cursor::new(input), &mut sink).unwrap();
    assert_eq!(sink.0, vec![(2, -40, 612, -120, true), (2, 5, 6, 120, false)]);
}

#[test]
fn desktop_discovery_budget_and_progman_fallback_gate() {
    assert_eq!(crate::host::DISCOVERY_ATTEMPTS, 120);
    assert_eq!(crate::host::DISCOVERY_INTERVAL, std::time::Duration::from_millis(250));
    // Early cold boot before Shell_TrayWnd exists must not grab immature Progman.
    assert!(!crate::desktop_host::allow_progman_fallback(false, 2));
    assert!(!crate::desktop_host::allow_progman_fallback(true, 2));
    // Once Shell_TrayWnd is up and WorkerW split attempt window passed, allow fallback.
    assert!(crate::desktop_host::allow_progman_fallback(true, 4));
    // At 15s (60 * 250ms), allow fallback even if tray heuristic isn't present.
    assert!(crate::desktop_host::allow_progman_fallback(false, 60));
}

#[test]
fn inspector_gates_ready_and_tree_on_committed_tree_and_waits_for_tree_after_click() {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    let dir = std::env::temp_dir().join(format!(
        "unidtk-inspector-committed-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let inspector = crate::inspector::Inspector::new_in(dir.clone()).unwrap();

    assert!(!inspector.is_ready(true).unwrap());
    assert!(inspector.tree_snapshot().is_err());

    {
        let mut s = inspector.snapshot.lock().unwrap();
        s.committed = 1;
        s.drawn = 0; // Still covered on cold boot: no GPU present yet.
        s.elements = vec![serde_json::json!({ "key": "g0-card", "text": "Presses: 0" })];
        s.region = vec![[24, 24, 320, 80]];
    }
    inspector.tree_seq.store(1, Ordering::Release);

    assert!(
        inspector.is_ready(true).unwrap(),
        "attached host with committed tree + region must report ready even while covered (drawn=0)"
    );
    let tree = inspector.tree_snapshot().expect("tree available after commit");
    assert_eq!(tree[0]["text"], "Presses: 0");

    // Simulate Rune pressing first (`input_count`), then committing updated tree (`tree_seq`) 15ms later.
    let before_input = inspector.input_count.load(Ordering::Acquire);
    let before_tree = inspector.tree_seq.load(Ordering::Acquire);
    let insp_bg = inspector.clone();
    let updater = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        insp_bg.input_count.fetch_add(1, Ordering::AcqRel);
        std::thread::sleep(Duration::from_millis(15));
        {
            let mut s = insp_bg.snapshot.lock().unwrap();
            s.committed += 1;
            s.elements = vec![serde_json::json!({ "key": "g0-card", "text": "Presses: 1" })];
        }
        insp_bg.tree_seq.fetch_add(1, Ordering::AcqRel);
    });

    let count = inspector
        .wait_for_click_ack(
            before_input,
            before_tree,
            Duration::from_secs(1),
            Duration::from_millis(200),
        )
        .expect("click ack with tree barrier");
    assert_eq!(count, before_input + 1);
    let updated = inspector.tree_snapshot().unwrap();
    assert_eq!(updated[0]["text"], "Presses: 1");
    let _ = updater.join();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rune_tree_committed_fires_after_covered_boot_and_dedups_unchanged_commits() {
    use std::cell::RefCell;
    use std::rc::Rc;

    // Verifies the App builder surface compiles symmetrically with `on_frame_drawn`.
    let spec = rune::AppSpec::new(
        "test",
        (200.0, 100.0),
        || 0u32,
        |n: &u32, _: &mut rune::Cx<u32>| rune::el().key("g0-card").child(rune::text(format!("Presses: {n}"))),
    );
    let _app = rune::App::new(spec).on_tree_committed(|_rt| {});

    let mut rt = rune::Runtime::new(
        0u32,
        |n: &u32, _: &mut rune::Cx<u32>| {
            rune::el().key("g0-card").child(rune::text(format!("Presses: {n}")))
        },
        (200.0, 100.0),
        rune::Env::default(),
    );
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    let mut last_committed = 0u64;
    let mut on_commit = |rt: &rune::Runtime<u32>| {
        let text = rt
            .elements()
            .into_iter()
            .filter_map(|e| e.text)
            .collect::<Vec<_>>()
            .join(",");
        seen.borrow_mut().push(text);
    };

    assert!(!rune::win32_telemetry::check_commit_transition(&rt, &mut last_committed, &mut on_commit));
    assert!(seen.borrow().is_empty(), "uncommitted runtime must not fire on_tree_committed");

    // Cold covered boot (`covered_step`) on `!has_committed()` builds the initial tree once.
    rune::win32_telemetry::covered_step(&mut rt);
    assert!(rt.has_committed(), "covered_step on cold boot must perform initial commit+build");
    assert!(rune::win32_telemetry::check_commit_transition(&rt, &mut last_committed, &mut on_commit));
    assert_eq!(seen.borrow().as_slice(), ["Presses: 0"]);

    // Subsequent `covered_step` while idle neither re-commits nor re-fires the hook.
    rune::win32_telemetry::covered_step(&mut rt);
    assert!(!rune::win32_telemetry::check_commit_transition(&rt, &mut last_committed, &mut on_commit));
    assert_eq!(seen.borrow().len(), 1);

    // State update + frame builds "Presses: 1" without needing a Drawn swapchain present.
    rt.update(|n, _| *n = 1);
    rt.frame(0.0);
    assert!(rune::win32_telemetry::check_commit_transition(&rt, &mut last_committed, &mut on_commit));
    assert_eq!(seen.borrow().as_slice(), ["Presses: 0", "Presses: 1"]);
}

// Rune's own tests are not built (core/rune/UPSTREAM.md item 3), so the `maxlength` it learned
// for the menu's fields is pinned here.
#[test]
fn a_text_edit_keeps_only_what_fits_and_code_is_not_cut() {
    let mut edit = rune::TextEdit::new("ab").max_chars(4);
    edit.insert("cdef");
    assert_eq!(edit.text, "abcd");
    edit.insert("x");
    assert_eq!(edit.text, "abcd", "a full field takes nothing more");
    // Characters, not bytes: Cyrillic is two bytes each in UTF-8.
    let mut cyrillic = rune::TextEdit::new("").max_chars(3);
    cyrillic.insert("папка");
    assert_eq!(cyrillic.text, "пап");
    // Text set in code is not cut, as `maxlength` does not cut a value set by script.
    assert_eq!(rune::TextEdit::new("long name").max_chars(3).text, "long name");
}

// A covered desk commits a layout without drawing, and its springs do not step: a card the layout
// moved still stands, presented, at its old place. The window region is made from where cards
// are and where their layout puts them, so the first press after a start lands on the card
// (`r[shell.window-region]`).
#[test]
fn the_window_region_follows_a_card_the_layout_moved_while_the_desk_was_covered() {
    let mut rt = rune::Runtime::new(
        20.0f32,
        |x: &f32, _: &mut rune::Cx<f32>| {
            rune::el().size(800.0, 400.0).child(rune::el().key("widget:a").absolute(*x, 20.0).size(50.0, 50.0))
        },
        (800.0, 400.0),
        rune::Env::default(),
    );
    rune::win32_telemetry::covered_step(&mut rt);
    assert!(rt.has_committed());
    rt.update(|x, _| *x = 300.0);
    rt.frame(0.0);
    let regions = rt.keyed_regions(&["card:", "widget:"]);
    assert_eq!(regions.len(), 1, "{regions:?}");
    let [x, _, w, _] = regions[0].0;
    assert!(x <= 300.0 && x + w >= 350.0, "the region must cover the card where the layout put it: {regions:?}");
}
