use super::*;
use serde_json::json;

#[test]
fn a_title_is_cut_at_twenty_characters() {
    assert_eq!(format_title(None), "---");
    assert_eq!(format_title(Some("")), "---");
    assert_eq!(format_title(Some("Short")), "Short");
    assert_eq!(format_title(Some("Двадцать символов ровно")), "Двадцать символов ро…");
    assert_eq!(format_title(Some("12345678901234567890")), "12345678901234567890");
}

#[test]
fn a_cover_is_the_file_or_the_youtube_thumbnail() {
    assert!(is_youtube_id("dQw4w9WgXcQ"));
    assert!(!is_youtube_id("dQw4w9WgXc"));
    assert!(!is_youtube_id("dQw4w9WgXc!"));
    assert_eq!(cover_of(&json!({ "thumbnail_path": "C:\\a.jpg", "id": "dQw4w9WgXcQ" })), Some(Cover::File("C:\\a.jpg".into())));
    assert_eq!(cover_of(&json!({ "id": "dQw4w9WgXcQ" })), Some(Cover::YouTube("dQw4w9WgXcQ".into())));
    assert_eq!(cover_of(&json!({ "id": "local-file" })), None);
}

#[test]
fn a_playlist_named_by_its_address_gets_a_name() {
    assert_eq!(playlist_name(&json!({ "name": "Утро" }), 0), "Утро");
    assert_eq!(playlist_name(&json!({ "name": "https://music.youtube.com/playlist?list=PL1", "track_count": 12 }), 0), "Плейлист 12 tracks");
    assert_eq!(playlist_name(&json!({ "name": "", "source_url": "https://youtube.com/playlist?list=PL1" }), 2), "Плейлист #3");
    assert_eq!(playlist_name(&json!({ "name": "https://youtube.com/watch?v=abc" }), 4), "Трек 5");
    assert_eq!(playlist_name(&json!({ "name": "" }), 1), "Плейлист 2");
}

#[test]
fn a_sync_task_reads_label_status_and_percent() {
    assert_eq!(sync_task_label(&json!({ "label": "Mix", "status": "running", "progress_pct": 41.6 })), "Mix: running 42%");
    assert_eq!(sync_task_label(&json!({})), "Синхронизация: queued");
}

#[test]
fn the_wheel_moves_the_volume_by_five_within_the_range() {
    assert_eq!(nudge_volume(98.0, 5.0), 100);
    assert_eq!(nudge_volume(2.0, -5.0), 0);
    assert_eq!(nudge_volume(42.4, 5.0), 47);
    assert_eq!(volume_toast(47), "ГРОМК: 47%");
}

#[test]
fn the_ring_shows_the_share_played() {
    assert_eq!(ring_share(30.0, 120.0), 0.25);
    assert_eq!(ring_share(30.0, 0.0), 0.0);
    assert_eq!(ring_share(200.0, 120.0), 1.0);
}

#[test]
fn the_arcs_start_at_the_top_and_go_clockwise() {
    let (x, y) = arc_point(50.0, 50.0, 51.0, 0.0);
    assert!((x - 50.0).abs() < 1e-4 && (y + 1.0).abs() < 1e-4, "{x} {y}");
    let (x, _) = arc_point(50.0, 50.0, 51.0, 90.0);
    assert!((x - 101.0).abs() < 1e-4);
}

#[test]
fn bands_hold_then_fade_then_stop() {
    let bands = [0.8, 0.4, 0.2];
    assert_eq!(held_bands(Some((bands, 100.0))), Some(bands));
    let faded = held_bands(Some((bands, 590.0))).unwrap();
    assert!((faded[0] - 0.4).abs() < 1e-9, "{faded:?}");
    assert_eq!(held_bands(Some((bands, 950.0))), None);
    assert_eq!(held_bands(None), None);
}

#[test]
fn the_envelope_attacks_fast_releases_slowly_and_fades_out() {
    let mut envelope = Envelope::default();
    let (first, _) = envelope.step([1.0, 0.0, 0.0], true, 33.0, 0.0);
    assert!((first[0].opacity - 1.0).abs() < 1e-9, "the first frame takes the bands as they are");
    let (down, _) = envelope.step([0.0, 0.0, 0.0], true, 33.0, 0.0);
    // Release 0.2: 1.0 -> 0.8, plus the quiet mix's breath.
    assert!(down[0].opacity > 0.14 + 0.8 * 0.86 - 1e-9 && down[0].opacity < 0.14 + 0.83 * 0.86, "{down:?}");
    let mut rest = Envelope::default();
    rest.step([0.5, 0.5, 0.5], true, 33.0, 0.0);
    let mut energy = 1.0;
    for _ in 0..40 {
        energy = rest.step([0.0; 3], false, 33.0, 0.0).1;
    }
    assert_eq!(energy, 0.0, "stopped music fades the arcs to rest");
    assert_eq!(rest.step([0.0; 3], false, 33.0, 0.0).0, [ARC_REST; 3]);
}

#[test]
fn the_fallback_kicks_within_the_range() {
    for step in 0..200 {
        let bands = fallback_bands(step as f64 * 0.033, [0.5, -0.5, 0.0]);
        assert!(bands.iter().all(|b| (0.0..=1.0).contains(b)), "{bands:?}");
    }
}
