//! What the player's card computes, apart from drawing: ported from the page
//! (`modules/mp3-player/ui/widget.js`, `audio-arcs.js`) with its constants, so it can be
//! tested without a window.

use serde_json::Value;

/// `formatMusicTitle`: the title, cut at 20 characters with "…", or "---". The page counts
/// UTF-16 units; here a character is one unit, which differs only for characters beyond
/// the basic plane (an emoji counts once here, twice there).
pub fn format_title(title: Option<&str>) -> String {
    let title = title.filter(|t| !t.is_empty()).unwrap_or("---");
    if title.chars().count() > 20 {
        format!("{}…", title.chars().take(20).collect::<String>())
    } else {
        title.to_string()
    }
}

/// `isYoutubeId`: eleven of `[a-zA-Z0-9_-]`.
pub fn is_youtube_id(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Where a track's cover comes from (`musicCoverUrl`).
#[derive(Clone, Debug, PartialEq)]
pub enum Cover {
    /// A file the library downloaded (`thumbnail_path`).
    File(String),
    /// YouTube's thumbnail of the video id; the host has no HTTP client, the backend fetches it.
    YouTube(String),
}

pub fn cover_of(track: &Value) -> Option<Cover> {
    if let Some(path) = track["thumbnail_path"].as_str().filter(|p| !p.is_empty()) {
        return Some(Cover::File(path.to_string()));
    }
    track["id"].as_str().filter(|id| is_youtube_id(id)).map(|id| Cover::YouTube(id.to_string()))
}

/// `musicPlaylistName`: the playlist's own name, or for a name that is an address a name
/// from what the address is.
pub fn playlist_name(playlist: &Value, index: usize) -> String {
    let raw = playlist["name"].as_str().unwrap_or_default().trim();
    let is_url = |text: &str| {
        let lower = text.to_ascii_lowercase();
        lower.starts_with("http://") || lower.starts_with("https://")
    };
    if !raw.is_empty() && !is_url(raw) {
        return raw.to_string();
    }
    let source = playlist["source_url"].as_str().filter(|s| !s.is_empty()).unwrap_or(raw);
    let count = playlist["track_count"].as_u64().unwrap_or(0);
    let suffix = if count > 0 { format!("{count} tracks") } else { format!("#{}", index + 1) };
    if is_url(source) {
        let query = source.split_once('?').map(|(_, q)| q.split('#').next().unwrap_or_default()).unwrap_or_default();
        let has = |name: &str| query.split('&').any(|pair| pair.split_once('=').is_some_and(|(k, v)| k == name && !v.is_empty()));
        if has("list") {
            return format!("Плейлист {suffix}");
        }
        if has("v") {
            return format!("Трек {}", index + 1);
        }
    }
    format!("Плейлист {}", index + 1)
}

/// `syncTaskLabel`: "label: status NN%".
pub fn sync_task_label(task: &Value) -> String {
    let label = task["label"].as_str().filter(|l| !l.is_empty()).unwrap_or("Синхронизация");
    let status = task["status"].as_str().filter(|s| !s.is_empty()).unwrap_or("queued");
    let progress = task["progress_pct"].as_f64().filter(|p| p.is_finite()).map(|p| format!(" {}%", p.round() as i64)).unwrap_or_default();
    format!("{label}: {status}{progress}")
}

/// `nudgeMusicVolume`: the volume moved by `delta`, rounded and kept in 0..=100.
pub fn nudge_volume(current: f64, delta: f64) -> u8 {
    (current + delta).round().clamp(0.0, 100.0) as u8
}

/// The title line while the wheel moves the volume (`showMusicVolumeToast`), for 1400 ms.
pub fn volume_toast(volume: u8) -> String {
    format!("ГРОМК: {volume}%")
}

pub const VOLUME_TOAST_MS: u64 = 1400;

/// The progress ring: r = 48 in a 100 viewBox, its length `2 * PI * 48` (`.ring-progress`).
pub const RING_RADIUS: f32 = 48.0;

/// How much of the ring is drawn: the share of the track played, 0 without a length.
pub fn ring_share(position: f64, duration: f64) -> f32 {
    if duration > 0.0 && position.is_finite() {
        (position / duration).clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

// ─── The arcs (`audio-arcs.js`) ───

pub const ARC_FRAME_MS: f64 = 33.0;
pub const ARC_IDLE_MS: f64 = 750.0;
pub const BANDS_FRESH_MS: f64 = 280.0;
/// After fresh bands stop, the last sample fades out until this age, then the bands are gone.
pub const BANDS_HOLD_MS: f64 = 900.0;
pub const ARC_ATTACK: f64 = 0.58;
pub const ARC_RELEASE: f64 = 0.2;
pub const ARC_BREATHE: f64 = 0.025;
/// Bass, mid and treble: 100 degrees each, 20 degrees apart, at r = 51 in the 100 viewBox.
pub const ARCS: [(f32, f32); 3] = [(-80.0, 20.0), (40.0, 140.0), (160.0, 260.0)];
pub const ARC_RADIUS: f32 = 51.0;

/// A point of the circle at `angle` degrees, 0 at the top, clockwise (`audioCirclePoint`).
pub fn arc_point(cx: f32, cy: f32, r: f32, angle: f32) -> (f32, f32) {
    let rad = (angle - 90.0).to_radians();
    (cx + r * rad.cos(), cy + r * rad.sin())
}

/// Bass, mid, treble, each 0..=1.
pub type Bands = [f64; 3];

/// `getAudioBands` without the synthetic fallback: the latest sample while fresh, faded
/// linearly over the hold, none after it. `age_ms` is the sample's age.
pub fn held_bands(last: Option<(Bands, f64)>) -> Option<Bands> {
    let (bands, age) = last?;
    if age < BANDS_FRESH_MS {
        return Some(bands);
    }
    if age < BANDS_HOLD_MS {
        let k = (1.0 - (age - BANDS_FRESH_MS) / 620.0).max(0.0);
        return Some(bands.map(|v| v * k));
    }
    None
}

/// `getFallbackAudioBands` while music plays and no bands come (the loopback is dead):
/// kicks so the arcs still move. `noise` is in -0.5..0.5, one value per band (the page uses
/// `Math.random`).
pub fn fallback_bands(seconds: f64, noise: [f64; 3]) -> Bands {
    use std::f64::consts::PI;
    let beat = |rate: f64, sharp: i32| (seconds * rate).sin().max(0.0).powi(sharp);
    let clamp = |v: f64| v.clamp(0.0, 1.0);
    [
        clamp(0.12 + beat(PI * 4.0, 8) * 0.88 + noise[0] * 0.05),
        clamp(0.1 + beat(PI * 6.4 + 0.7, 4) * 0.65 + (seconds * 5.2).sin() * 0.1 + noise[1] * 0.05),
        clamp(0.08 + beat(PI * 9.4 + 1.4, 3) * 0.55 + (seconds * 8.4).sin() * 0.12 + noise[2] * 0.05),
    ]
}

/// The arcs' display envelope (`applyAudioCircleBands`): a fast attack, a slower release, a
/// breath when the mix is quiet, and a fade when the music stops.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Envelope {
    current: Option<[f64; 3]>,
}

/// One arc as drawn: its opacity and its scale about the circle's centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArcLook {
    pub opacity: f64,
    pub scale: f64,
}

/// The arc at rest: the page clears its inline style back to the stylesheet's.
pub const ARC_REST: ArcLook = ArcLook { opacity: 0.2, scale: 1.0 };

impl Envelope {
    /// One frame. `delta_ms` since the last, `seconds` the clock for the breath. Returns how
    /// each arc looks and the loudest arc's energy (the loop idles at 0).
    pub fn step(&mut self, bands: Bands, active: bool, delta_ms: f64, seconds: f64) -> ([ArcLook; 3], f64) {
        let peak = bands.iter().cloned().fold(0.0, f64::max);
        let time_scale = (delta_ms / ARC_FRAME_MS).clamp(0.1, 3.0);
        let mut current = self.current.unwrap_or(if active { bands.map(|b| b.clamp(0.0, 1.0)) } else { [0.0; 3] });
        let mut looks = [ARC_REST; 3];
        let mut max_energy: f64 = 0.0;
        for i in 0..3 {
            let target = bands[i].clamp(0.0, 1.0);
            let mut cur = current[i];
            if active {
                let coef = if target > cur { ARC_ATTACK } else { ARC_RELEASE };
                let alpha = 1.0 - (1.0 - coef).powf(time_scale);
                cur += (target - cur) * alpha;
            } else {
                cur *= 0.75f64.powf(time_scale);
                if cur < 0.01 {
                    cur = 0.0;
                }
            }
            current[i] = cur;
            max_energy = max_energy.max(cur);
            if !active && cur == 0.0 {
                continue;
            }
            let idle = if active && peak < 0.18 {
                ARC_BREATHE * (0.5 + 0.5 * (seconds * 1.8 + i as f64 * 2.1).sin())
            } else {
                0.0
            };
            let v = (cur + idle).min(1.0);
            looks[i] = ArcLook { opacity: 0.14 + v * 0.86, scale: 1.0 + v * 0.09 };
        }
        self.current = Some(current);
        (looks, max_energy)
    }
}

#[cfg(test)]
#[path = "logic_tests.rs"]
mod tests;
