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
