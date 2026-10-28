//! Two-finger scroll read straight from a Precision Touchpad (raw HID).
//!
//! Over a WebView2 window Windows does not synthesize `WM_MOUSEWHEEL` from a
//! touchpad pan: the gesture goes to the window's DirectManipulation viewport
//! instead. Our WebView lives under WorkerW and is never activated, so that
//! path scrolls nothing, and the `WH_MOUSE_LL` hook sees no wheel at all
//! (measured: ~100 HID reports/s from the pad, zero wheel events). So the
//! kernel reads the pad's contacts itself and turns a two-finger vertical pan
//! into wheel travel for the shell window only (see `input::send_touchpad_wheel`).
//!
//! Everything here runs on the raw-input window's thread; no locks.

use std::{cell::RefCell, collections::HashMap, time::Instant};

use windows::Win32::{
    Devices::HumanInterfaceDevice::{
        HidP_GetCaps, HidP_GetUsageValue, HidP_GetUsages, HidP_GetValueCaps, HidP_Input,
        HIDP_CAPS, HIDP_STATUS_SUCCESS, HIDP_VALUE_CAPS, PHIDP_PREPARSED_DATA,
    },
    Foundation::HANDLE,
    System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
    UI::Input::{GetRawInputDeviceInfoW, RIDI_PREPARSEDDATA},
};

const PAGE_GENERIC: u16 = 0x01;
const PAGE_DIGITIZER: u16 = 0x0D;
const USAGE_TOUCHPAD: u16 = 0x05;
const USAGE_Y: u16 = 0x31;
const USAGE_TIP_SWITCH: u16 = 0x42;
const USAGE_CONTACT_ID: u16 = 0x51;
const USAGE_CONTACT_COUNT: u16 = 0x54;

/// Wheel units (WHEEL_DELTA = 120 per notch) per millimetre of finger travel.
/// Calibrated against Windows' own conversion on this machine: one swipe over
/// a normal window produced 240–480 units.
const WHEEL_PER_MM: f32 = 10.0;
/// Pad height assumed when the descriptor has no usable physical units.
const FALLBACK_PAD_MM: f32 = 60.0;
/// A pause longer than this starts a new gesture (also covers contacts left
/// "down" across sleep — their stale positions must not become a jump).
const GESTURE_GAP_MS: u128 = 120;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub id: u32,
    pub tip: bool,
    pub y: i32,
}

/// Turns complete touchpad frames into vertical two-finger travel.
#[derive(Default)]
pub struct Gesture {
    prev: Vec<(u32, i32)>,
    last_ms: Option<u128>,
}

impl Gesture {
    /// Feed one complete frame; returns the pan in logical Y units (positive =
    /// fingers moved down) when exactly the same two fingers moved together.
    pub fn frame(&mut self, contacts: &[Contact], now_ms: u128) -> Option<f32> {
        let stale = self.last_ms.map_or(true, |t| now_ms.saturating_sub(t) > GESTURE_GAP_MS);
        self.last_ms = Some(now_ms);
        let down: Vec<(u32, i32)> = contacts.iter().filter(|c| c.tip).map(|c| (c.id, c.y)).collect();
        if down.len() != 2 {
            self.prev.clear();
            return None;
        }
        let prev = std::mem::replace(&mut self.prev, down.clone());
        if stale {
            return None;
        }
        let moved = |(id, y): (u32, i32)| prev.iter().find(|p| p.0 == id).map(|p| (y - p.1) as f32);
        let (Some(d1), Some(d2)) = (moved(down[0]), moved(down[1])) else { return None };
        // Opposite directions is a pinch, not a scroll.
        if d1 * d2 < 0.0 {
            return None;
        }
        let dy = (d1 + d2) / 2.0;
        (dy != 0.0).then_some(dy)
    }
}

/// Millimetres per logical unit from a value cap's physical extent.
fn mm_per_unit(cap: &HIDP_VALUE_CAPS) -> f32 {
    let logical = (cap.LogicalMax - cap.LogicalMin) as f32;
    if logical <= 0.0 {
        return 0.0;
    }
    // Units nibble 0: system (1 = SI linear → cm, 3 = English linear → inch),
    // nibble 1: length exponent. UnitsExp is a signed 4-bit power of ten.
    let unit_mm = match (cap.Units & 0xF, (cap.Units >> 4) & 0xF) {
        (1, 1) => 10.0,
        (3, 1) => 25.4,
        _ => 0.0,
    };
    let exp = cap.UnitsExp & 0xF;
    let exp = if exp >= 8 { exp as i32 - 16 } else { exp as i32 };
    let mm = (cap.PhysicalMax - cap.PhysicalMin) as f32 * unit_mm * 10f32.powi(exp);
    let mm = if (10.0..=300.0).contains(&mm) { mm } else { FALLBACK_PAD_MM };
    mm / logical
}

struct Device {
    preparsed: Vec<u64>,
    /// Finger link collections in report order.
    fingers: Vec<u16>,
    count_link: Option<u16>,
    mm_per_unit: f32,
}

impl Device {
    fn pp(&self) -> PHIDP_PREPARSED_DATA {
        PHIDP_PREPARSED_DATA(self.preparsed.as_ptr() as isize)
    }

    fn open(handle: HANDLE) -> Option<Device> {
        unsafe {
            let mut size = 0u32;
            GetRawInputDeviceInfoW(handle, RIDI_PREPARSEDDATA, None, &mut size);
            if size == 0 {
                return None;
            }
            let mut preparsed = vec![0u64; (size as usize).div_ceil(8)];
            if GetRawInputDeviceInfoW(handle, RIDI_PREPARSEDDATA, Some(preparsed.as_mut_ptr().cast()), &mut size) == u32::MAX {
                return None;
            }
            let pp = PHIDP_PREPARSED_DATA(preparsed.as_ptr() as isize);
            let mut caps = HIDP_CAPS::default();
            if HidP_GetCaps(pp, &mut caps) != HIDP_STATUS_SUCCESS
                || caps.UsagePage != PAGE_DIGITIZER
                || caps.Usage != USAGE_TOUCHPAD
            {
                return None;
            }
            let mut n = caps.NumberInputValueCaps;
            let mut values = vec![HIDP_VALUE_CAPS::default(); n as usize];
            if HidP_GetValueCaps(HidP_Input, values.as_mut_ptr(), &mut n, pp) != HIDP_STATUS_SUCCESS {
                return None;
            }
            values.truncate(n as usize);
            let has = |cap: &HIDP_VALUE_CAPS, page: u16, usage: u16| {
                cap.UsagePage == page
                    && if cap.IsRange.as_bool() {
                        (cap.Anonymous.Range.UsageMin..=cap.Anonymous.Range.UsageMax).contains(&usage)
                    } else {
                        cap.Anonymous.NotRange.Usage == usage
                    }
            };
            let mut ys: Vec<&HIDP_VALUE_CAPS> = values.iter().filter(|c| has(c, PAGE_GENERIC, USAGE_Y)).collect();
            ys.sort_by_key(|c| c.LinkCollection);
            ys.dedup_by_key(|c| c.LinkCollection);
            let first = *ys.first()?;
            Some(Device {
                fingers: ys.iter().map(|c| c.LinkCollection).collect(),
                count_link: values.iter().find(|c| has(c, PAGE_DIGITIZER, USAGE_CONTACT_COUNT)).map(|c| c.LinkCollection),
                mm_per_unit: mm_per_unit(first),
                preparsed,
            })
        }
    }

    fn value(&self, page: u16, link: u16, usage: u16, report: &[u8]) -> Option<u32> {
        let mut v = 0u32;
        let status = unsafe { HidP_GetUsageValue(HidP_Input, page, link, usage, &mut v, self.pp(), report) };
        (status == HIDP_STATUS_SUCCESS).then_some(v)
    }

    fn tip(&self, link: u16, report: &mut [u8]) -> bool {
        let mut usages = [0u16; 16];
        let mut len = usages.len() as u32;
        let status = unsafe {
            HidP_GetUsages(HidP_Input, PAGE_DIGITIZER, link, usages.as_mut_ptr(), &mut len, self.pp(), report)
        };
        status == HIDP_STATUS_SUCCESS && usages[..len as usize].contains(&USAGE_TIP_SWITCH)
    }
}

#[derive(Default)]
struct State {
    devices: HashMap<isize, Option<Device>>,
    /// Contacts of the frame being assembled (hybrid mode spreads one frame
    /// over several reports; only the first carries the contact count).
    frame: Vec<Contact>,
    expected: usize,
    gesture: Gesture,
    carry: f32,
    natural: Option<bool>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    static EPOCH: Instant = Instant::now();
}

/// Windows' "Scrolling direction" for the touchpad. 0 (default) = "down
/// motion scrolls up" (content follows the fingers); 0xFFFFFFFF = reversed.
fn read_natural() -> bool {
    let mut value = 0u32;
    let mut size = 4u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            windows::core::w!("Software\\Microsoft\\Windows\\CurrentVersion\\PrecisionTouchPad"),
            windows::core::w!("ScrollDirection"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    status.is_err() || value == 0
}

/// Settings may have changed (WM_SETTINGCHANGE): re-read the direction lazily.
pub fn settings_changed() {
    STATE.with(|s| s.borrow_mut().natural = None);
}

/// A device arrived or left: its handle may be reused, drop the cached parse.
pub fn device_changed(handle: isize) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.devices.remove(&handle);
        s.frame.clear();
        s.expected = 0;
    });
}

/// One WM_INPUT HID packet: `reports` holds `count` reports of `size` bytes.
/// Returns whole wheel units to scroll (positive = wheel up), if any.
pub fn handle_reports(handle: isize, size: usize, reports: &mut [u8]) -> Option<i32> {
    let now_ms = EPOCH.with(|e| e.elapsed().as_millis());
    STATE.with(|state| {
        let mut guard = state.borrow_mut();
        let s = &mut *guard;
        let device = s
            .devices
            .entry(handle)
            .or_insert_with(|| Device::open(HANDLE(handle as _)))
            .as_ref()?;
        let mut travel = 0.0f32;
        for report in reports.chunks_exact_mut(size.max(1)) {
            match device.count_link {
                Some(link) => {
                    let count = device.value(PAGE_DIGITIZER, link, USAGE_CONTACT_COUNT, report).unwrap_or(0) as usize;
                    if count > 0 {
                        s.frame.clear();
                        s.expected = count;
                    } else if s.expected == 0 {
                        continue;
                    }
                }
                // No contact count in the descriptor: every report is a frame.
                None => {
                    s.frame.clear();
                    s.expected = device.fingers.len();
                }
            }
            for &link in &device.fingers {
                if s.frame.len() >= s.expected {
                    break;
                }
                let tip = device.tip(link, report);
                let id = device.value(PAGE_DIGITIZER, link, USAGE_CONTACT_ID, report).unwrap_or(link as u32);
                let y = device.value(PAGE_GENERIC, link, USAGE_Y, report).unwrap_or(0) as i32;
                s.frame.push(Contact { id, tip, y });
            }
            if s.frame.len() >= s.expected {
                if let Some(dy) = s.gesture.frame(&s.frame, now_ms) {
                    travel += dy * device.mm_per_unit;
                }
                s.frame.clear();
                s.expected = 0;
            }
        }
        if travel == 0.0 {
            return None;
        }
        let natural = *s.natural.get_or_insert_with(read_natural);
        // Y grows downward. Natural: fingers down → content down → wheel up (+).
        s.carry += travel * WHEEL_PER_MM * if natural { 1.0 } else { -1.0 };
        let whole = s.carry.trunc();
        s.carry -= whole;
        (whole != 0.0).then_some(whole as i32)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: u32, y: i32) -> Contact {
        Contact { id, tip: true, y }
    }

    #[test]
    fn two_fingers_moving_together_scroll() {
        let mut g = Gesture::default();
        assert_eq!(g.frame(&[c(1, 100), c(2, 110)], 0), None);
        assert_eq!(g.frame(&[c(1, 110), c(2, 120)], 8), Some(10.0));
        assert_eq!(g.frame(&[c(1, 104), c(2, 112)], 16), Some(-7.0));
    }

    #[test]
    fn pinch_and_one_or_three_fingers_do_not_scroll() {
        let mut g = Gesture::default();
        g.frame(&[c(1, 100), c(2, 200)], 0);
        assert_eq!(g.frame(&[c(1, 90), c(2, 210)], 8), None);
        assert_eq!(g.frame(&[c(1, 80)], 16), None);
        assert_eq!(g.frame(&[c(1, 80), c(2, 210), c(3, 50)], 24), None);
    }

    #[test]
    fn finger_swap_and_lift_reset_the_baseline() {
        let mut g = Gesture::default();
        g.frame(&[c(1, 100), c(2, 100)], 0);
        // Finger 2 replaced by finger 3 at a far position: no jump.
        assert_eq!(g.frame(&[c(1, 100), c(3, 400)], 8), None);
        let lifted = [c(1, 100), Contact { id: 3, tip: false, y: 400 }];
        assert_eq!(g.frame(&lifted, 16), None);
        assert_eq!(g.frame(&[c(1, 100), c(3, 400)], 24), None);
    }

    #[test]
    fn pause_starts_a_new_gesture() {
        let mut g = Gesture::default();
        g.frame(&[c(1, 100), c(2, 100)], 0);
        // Contacts "stuck" down across sleep must not turn into one huge jump.
        assert_eq!(g.frame(&[c(1, 900), c(2, 900)], 60_000), None);
        assert_eq!(g.frame(&[c(1, 905), c(2, 905)], 60_008), Some(5.0));
    }

    #[test]
    fn physical_extent_converts_to_millimetres() {
        let mut cap = HIDP_VALUE_CAPS::default();
        cap.LogicalMin = 0;
        cap.LogicalMax = 1000;
        cap.PhysicalMin = 0;
        cap.PhysicalMax = 700; // 7.00 cm with exponent -2
        cap.Units = 0x11;
        cap.UnitsExp = 0xE;
        assert!((mm_per_unit(&cap) - 0.07).abs() < 1e-6);
        cap.Units = 0; // no units → fallback height
        assert!((mm_per_unit(&cap) - FALLBACK_PAD_MM / 1000.0).abs() < 1e-6);
    }
}
