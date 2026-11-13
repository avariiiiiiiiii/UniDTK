//! Whether a window can be seen, as Chromium's `NativeWindowOcclusionTrackerWin` works it out:
//! a thread of its own hears windows move, show, hide, minimise, change virtual desktop or come
//! to the front (`SetWinEventHook`) and, a frame later, walks every window from the top of the
//! z-order down, taking the opaque ones out of what's left of the screens. A tracked window none
//! of whose rect is left, that's minimised, or that's on another virtual desktop is covered: no
//! one can see it, so it stops drawing (`App::on_visibility` hears it) until it shows again.
//!
//! Only on the desktop the user sees. A window on another one (a capture harness's hidden
//! desktop, which several apps share) is never covered, so it keeps drawing for whoever reads
//! its pixels.

use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows_sys::Win32::Graphics::Dwm::{
    DWM_CLOAKED_SHELL, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    COMPLEXREGION, CombineRgn, CreateRectRgn, CreateRectRgnIndirect, DeleteObject, EnumDisplayMonitors,
    GetWindowRgn, HDC, HMONITOR, HRGN, MONITORINFO, MonitorFromWindow, GetMonitorInfoW,
    MONITOR_DEFAULTTONEAREST, NULLREGION, OffsetRgn, RGN_AND, RGN_COPY, RGN_DIFF, RGN_ERROR, RGN_OR, RectInRegion,
};
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_READOBJECTS, GetThreadDesktop, GetUserObjectInformationW, HDESK, OpenInputDesktop, UOI_NAME,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, EVENT_OBJECT_CLOAKED, EVENT_OBJECT_DESTROY, EVENT_OBJECT_LOCATIONCHANGE,
    EVENT_OBJECT_REORDER, EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
    EVENT_SYSTEM_MOVESIZEEND, EnumWindows, GA_ROOT, GWL_EXSTYLE, GWL_STYLE, GetAncestor, GetClassNameW,
    GetLayeredWindowAttributes, GetMessageW, GetShellWindow, GetWindowLongW, GetWindowPlacement,
    GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, KillTimer,
    LWA_ALPHA, LWA_COLORKEY, MSG, OBJID_WINDOW, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SW_MAXIMIZE, SetTimer,
    WINDOWPLACEMENT, WINEVENT_OUTOFCONTEXT, WM_APP, WM_TIMER, WS_CHILD, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT, WS_POPUP,
};

pub type Rect = RECT;
pub type Hrgn = HRGN;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopAccess {
    Same,
    Other,
    Inaccessible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeFound {
    Hidden,
    Covered,
    Seen,
}

pub fn recheck() {
    if let Some(t) = tracker() {
        t.wake();
    }
}

pub fn is_desktop_shell_class(class: &str) -> bool {
    matches!(class, "Progman" | "WorkerW" | "SHELLDLL_DefView" | "SysListView32")
}

pub fn should_process_win_event(
    event: u32,
    object: i32,
    child: i32,
    style: Option<u32>,
    is_root: bool,
) -> bool {
    if object != OBJID_WINDOW || child != CHILDID_SELF as i32 {
        return false;
    }
    if event == EVENT_OBJECT_DESTROY {
        // Destroyed HWNDs may already return 0/`None` for `GetWindowLongW`; only drop confirmed child destroys.
        return style.is_none_or(|s| s & WS_CHILD == 0);
    }
    is_root
}

pub fn fallback_found_for(access: DesktopAccess, visible: bool) -> ProbeFound {
    if !visible {
        ProbeFound::Hidden
    } else if matches!(access, DesktopAccess::Inaccessible) {
        ProbeFound::Covered
    } else {
        ProbeFound::Seen
    }
}

fn intersect_rect(a: RECT, b: RECT) -> Option<RECT> {
    let r = RECT {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

pub fn child_overlap_with_left(
    child_rect: RECT,
    root_rect: RECT,
    child_rgn: Option<HRGN>,
    left: HRGN,
) -> ProbeFound {
    let Some(clipped) = intersect_rect(child_rect, root_rect) else {
        return ProbeFound::Covered;
    };
    let Some(rgn) = child_rgn else {
        // SAFETY: `left` is a valid HRGN from this occlusion pass; `clipped` is initialized.
        return if unsafe { RectInRegion(left, &clipped) } != 0 {
            ProbeFound::Seen
        } else {
            ProbeFound::Covered
        };
    };
    // SAFETY: clone `rgn` into a temporary region so caller handles (`GetWindowRgn` / tests) are never mutated.
    unsafe {
        let local = CreateRectRgn(0, 0, 0, 0);
        let copied = CombineRgn(local, rgn, std::ptr::null_mut(), RGN_COPY);
        if copied == RGN_ERROR {
            DeleteObject(local);
            return if RectInRegion(left, &clipped) != 0 {
                ProbeFound::Seen
            } else {
                ProbeFound::Covered
            };
        }
        OffsetRgn(local, child_rect.left, child_rect.top);
        let bounds_rgn = CreateRectRgnIndirect(&clipped);
        let _ = CombineRgn(local, local, bounds_rgn, RGN_AND);
        DeleteObject(bounds_rgn);
        let overlap = CombineRgn(local, local, left, RGN_AND);
        DeleteObject(local);
        if overlap == NULLREGION || overlap == RGN_ERROR {
            ProbeFound::Covered
        } else {
            ProbeFound::Seen
        }
    }
}

/// Called on the tracker's thread with whether the window is covered now.
pub(crate) type Report = Arc<dyn Fn(bool) + Send + Sync>;

/// A window the tracker watches until this drops.
pub(crate) struct Tracked {
    hwnd: usize,
}

impl Tracked {
    /// Watches `window`, telling `report` each time it becomes covered or uncovered. None when
    /// the tracker's thread couldn't start; the window then never counts as covered.
    pub(crate) fn new(window: &winit::window::Window, report: Report) -> Option<Self> {
        let hwnd = super::hwnd(window)? as usize;
        let tracker = tracker()?;
        tracker.windows.lock().unwrap().push(Watched { hwnd, report, sight: Sight::Unknown });
        tracker.wake();
        Some(Tracked { hwnd })
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        let Some(tracker) = tracker() else { return };
        tracker.windows.lock().unwrap().retain(|w| w.hwnd != self.hwnd);
    }
}

/// What the tracker last told a window's owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sight {
    /// Nothing yet, or the window was off screen (hidden), where its owner starts it uncovered
    /// again when it shows.
    Unknown,
    Seen,
    Covered,
}

/// What a pass found for a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Found {
    /// Hidden (not `IsWindowVisible`): its owner knows, and a show starts it uncovered.
    Hidden,
    Seen,
    Covered,
}

impl Sight {
    /// The report a pass's finding asks for, if any: a change, or the first word on a window
    /// that's on screen.
    fn step(self, found: Found) -> (Sight, Option<bool>) {
        match (self, found) {
            (_, Found::Hidden) => (Sight::Unknown, None),
            (Sight::Seen, Found::Seen) | (Sight::Covered, Found::Covered) => (self, None),
            (_, Found::Seen) => (Sight::Seen, Some(false)),
            (_, Found::Covered) => (Sight::Covered, Some(true)),
        }
    }
}

struct Watched {
    hwnd: usize,
    report: Report,
    sight: Sight,
}

struct Tracker {
    thread: u32,
    windows: Mutex<Vec<Watched>>,
}

/// Posted to the tracker's thread: the tracked windows changed, look now.
const WM_TRACKED: u32 = WM_APP + 1;
/// A frame: how long the tracker lets a burst of window events settle before it looks, as
/// Chromium's does.
const SETTLE_MS: u32 = 16;

impl Tracker {
    fn wake(&self) {
        // SAFETY: posts to a thread whose queue exists (`start` waits for it); no pointers.
        unsafe { PostThreadMessageW(self.thread, WM_TRACKED, 0, 0) };
    }
}

fn tracker() -> Option<&'static Tracker> {
    static TRACKER: OnceLock<Option<Tracker>> = OnceLock::new();
    TRACKER.get_or_init(start).as_ref()
}

/// Starts the tracker's thread, once its message queue exists so posts reach it.
fn start() -> Option<Tracker> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("rune-occlusion".into())
        .spawn(move || {
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            // SAFETY: makes this thread's message queue; `msg` is a valid out pointer.
            unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
            // SAFETY: this thread's id.
            let _ = tx.send(unsafe { GetCurrentThreadId() });
            run();
        })
        .ok()?;
    let thread = rx.recv().ok()?;
    Some(Tracker { thread, windows: Mutex::new(Vec::new()) })
}

/// When the next pass runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pass {
    Idle,
    /// A burst of window events is settling; the timer runs the pass.
    Settling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PassEvent {
    /// A window event that may change what covers what.
    Moved,
    /// A tracked window came or went: look at once.
    Now,
    /// The settle timer fired.
    Timer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PassEffect {
    None,
    StartTimer,
    /// Run the pass, stopping the timer if it runs.
    Run { stop_timer: bool },
}

impl Pass {
    fn step(self, event: PassEvent) -> (Pass, PassEffect) {
        match (self, event) {
            (Pass::Idle, PassEvent::Moved) => (Pass::Settling, PassEffect::StartTimer),
            (Pass::Settling, PassEvent::Moved) => (Pass::Settling, PassEffect::None),
            (Pass::Idle, PassEvent::Now) => (Pass::Idle, PassEffect::Run { stop_timer: false }),
            (Pass::Settling, PassEvent::Now | PassEvent::Timer) => (Pass::Idle, PassEffect::Run { stop_timer: true }),
            (Pass::Idle, PassEvent::Timer) => (Pass::Idle, PassEffect::None),
        }
    }
}

thread_local! {
    static PASS: std::cell::Cell<Pass> = const { std::cell::Cell::new(Pass::Idle) };
}

/// The settle timer's id on the tracker's thread (`SetTimer` with no window picks it).
static TIMER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn pass_event(event: PassEvent) {
    let (next, effect) = PASS.with(|p| p.get()).step(event);
    PASS.with(|p| p.set(next));
    match effect {
        PassEffect::None => {}
        PassEffect::StartTimer => {
            // SAFETY: a thread timer (no window); WM_TIMER comes to this thread's queue.
            let id = unsafe { SetTimer(std::ptr::null_mut(), 0, SETTLE_MS, None) };
            TIMER.store(id, std::sync::atomic::Ordering::Relaxed);
        }
        PassEffect::Run { stop_timer } => {
            if stop_timer {
                // SAFETY: the timer this thread set, or 0, which fails harmlessly.
                unsafe { KillTimer(std::ptr::null_mut(), TIMER.swap(0, std::sync::atomic::Ordering::Relaxed)) };
            }
            run_pass();
        }
    }
}

fn run() {
    let hook = |min: u32, max: u32| {
        // SAFETY: an out-of-context hook on every process's events, called on this thread while
        // it waits for messages; `on_event` is a plain function.
        unsafe { SetWinEventHook(min, max, std::ptr::null_mut(), Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT) }
    };
    let _hooks = [
        hook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        hook(EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MINIMIZEEND),
        hook(EVENT_OBJECT_DESTROY, EVENT_OBJECT_REORDER),
        hook(EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
        hook(EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
    ];
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    // SAFETY: `msg` is a valid out pointer; out-of-context hooks run inside GetMessageW.
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        match msg.message {
            WM_TRACKED => pass_event(PassEvent::Now),
            WM_TIMER => pass_event(PassEvent::Timer),
            _ => {}
        }
    }
}

impl From<ProbeFound> for Found {
    fn from(p: ProbeFound) -> Self {
        match p {
            ProbeFound::Hidden => Found::Hidden,
            ProbeFound::Covered => Found::Covered,
            ProbeFound::Seen => Found::Seen,
        }
    }
}

/// A window event from any process on this desktop. Top-level windows and non-child destroys count;
/// child control destroys (e.g. browser/UI child HWNDs) are ignored so background apps cannot wake
/// the tracker needlessly.
unsafe extern "system" fn on_event(_hook: HWINEVENTHOOK, event: u32, hwnd: HWND, object: i32, child: i32, _thread: u32, _time: u32) {
    if hwnd.is_null() { return; }
    // SAFETY: GetWindowLongW and GetAncestor tolerate stale or cross-process handles.
    let style = (event == EVENT_OBJECT_DESTROY).then(|| {
        let raw = unsafe { GetWindowLongW(hwnd, GWL_STYLE) };
        (raw != 0).then_some(raw as u32)
    }).flatten();
    let is_root = event != EVENT_OBJECT_DESTROY && unsafe { GetAncestor(hwnd, GA_ROOT) } == hwnd;
    if !should_process_win_event(event, object, child, style, is_root) { return; }
    pass_event(PassEvent::Moved);
}

/// Looks at every tracked window and tells the owners of those that changed.
fn run_pass() {
    let Some(tracker) = tracker() else { return };
    let tracked: Vec<usize> = tracker.windows.lock().unwrap().iter().map(|w| w.hwnd).collect();
    if tracked.is_empty() { return; }
    let found = match input_desktop_access() {
        DesktopAccess::Same => look(&tracked),
        access => tracked
            .iter()
            .map(|&h| {
                // SAFETY: IsWindowVisible tolerates any handle.
                let visible = unsafe { IsWindowVisible(h as HWND) } != 0;
                (h, fallback_found_for(access, visible).into())
            })
            .collect(),
    };
    let reports: Vec<(Report, bool)> = {
        let mut windows = tracker.windows.lock().unwrap();
        windows.iter_mut().filter_map(|w| {
            let (_, f) = found.iter().find(|(h, _)| *h == w.hwnd)?;
            let (sight, report) = w.sight.step(*f);
            w.sight = sight;
            report.map(|covered| (w.report.clone(), covered))
        }).collect()
    };
    // Outside the lock: a report may track or untrack windows.
    for (report, covered) in reports { report(covered); }
}

/// How this thread's desktop relates to the active input desktop.
fn input_desktop_access() -> DesktopAccess {
    // SAFETY: desktop handles this function opens it closes; the thread's own isn't closed.
    unsafe {
        let input = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
        if input.is_null() { return DesktopAccess::Inaccessible; }
        let same = desktop_name(input).is_some_and(|name| desktop_name(GetThreadDesktop(GetCurrentThreadId())) == Some(name));
        CloseDesktop(input);
        if same { DesktopAccess::Same } else { DesktopAccess::Other }
    }
}

fn desktop_name(desktop: HDESK) -> Option<Vec<u16>> {
    if desktop.is_null() { return None; }
    let mut name = [0u16; 256];
    let mut needed = 0u32;
    // SAFETY: `name` holds 512 bytes, as passed.
    let ok = unsafe { GetUserObjectInformationW(desktop, UOI_NAME, name.as_mut_ptr().cast(), std::mem::size_of_val(&name) as u32, &mut needed) };
    if ok == 0 { return None; }
    Some(name.iter().copied().take_while(|&c| c != 0).collect())
}

/// One pass down the z-order, as Chromium's `WindowOcclusionCalculator` does: what's left of
/// the screens shrinks by each opaque window, and a tracked window shows if any of what's left
/// is in its rect (or child rect/region clipped by its root host) when the walk reaches its root.
fn look(tracked: &[usize]) -> Vec<(usize, Found)> {
    #[derive(Clone, Copy)]
    struct Target {
        hwnd: usize,
        root: usize,
    }
    struct Walk<'a> {
        targets: &'a [Target],
        shell_pid: Option<u32>,
        left: HRGN,
        found: Vec<(usize, Found)>,
    }
    unsafe extern "system" fn monitor(_m: HMONITOR, _dc: HDC, rect: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the walk's region; `rect` the monitor's, from the system.
        unsafe {
            let left = data as HRGN;
            let r = &*rect;
            let screen = CreateRectRgn(r.left, r.top, r.right, r.bottom);
            CombineRgn(left, left, screen, RGN_OR);
            DeleteObject(screen);
        }
        TRUE
    }
    unsafe extern "system" fn window(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `Walk` `look` passes, alive for the enumeration.
        let walk = unsafe { &mut *(data as *mut Walk) };
        let enum_id = hwnd as usize;
        let mut self_top_level = false;
        for target in walk.targets {
            if target.root == enum_id {
                walk.found.push((target.hwnd, tracked_window(target.hwnd as HWND, target.root as HWND, walk.left)));
            }
            if target.hwnd == enum_id {
                self_top_level = true;
            }
        }
        if let Some(r) = opaque_rect(hwnd, self_top_level, walk.shell_pid) {
            // SAFETY: regions this pass owns.
            unsafe {
                let rgn = CreateRectRgn(r.left, r.top, r.right, r.bottom);
                CombineRgn(walk.left, walk.left, rgn, RGN_DIFF);
                DeleteObject(rgn);
            }
        }
        // Done once every tracked window is found.
        if walk.found.len() == walk.targets.len() { 0 } else { TRUE }
    }
    let targets: Vec<Target> = tracked
        .iter()
        .map(|&hwnd| {
            // SAFETY: GetAncestor tolerates any handle.
            let root = unsafe { GetAncestor(hwnd as HWND, GA_ROOT) };
            let root = if root.is_null() { hwnd } else { root as usize };
            Target { hwnd, root }
        })
        .collect();
    let shell_pid = desktop_shell_pid();
    // SAFETY: the region is this pass's, deleted below; the callbacks get valid pointers.
    unsafe {
        let left = CreateRectRgn(0, 0, 0, 0);
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(monitor), left as LPARAM);
        let mut walk = Walk { targets: &targets, shell_pid, left, found: Vec::new() };
        EnumWindows(Some(window), (&raw mut walk) as LPARAM);
        DeleteObject(left);
        // A tracked window the walk didn't reach (destroyed meanwhile) is off screen.
        targets.iter().map(|t| (t.hwnd, walk.found.iter().find(|(f, _)| *f == t.hwnd).map_or(Found::Hidden, |(_, f)| *f))).collect()
    }
}

/// What a tracked window is, with `left` what other windows leave of the screens above its root.
fn tracked_window(hwnd: HWND, root: HWND, left: HRGN) -> Found {
    // SAFETY: plain queries on live tracked windows.
    unsafe {
        if IsWindowVisible(hwnd) == 0 || (hwnd != root && IsWindowVisible(root) == 0) {
            return Found::Hidden;
        }
        if IsIconic(root) != 0 || cloaked(root).is_some_and(|c| c & DWM_CLOAKED_SHELL != 0) {
            return Found::Covered;
        }
        if hwnd == root {
            // Rune's own first-frame cloak (DWM_CLOAKED_APP) waits for the frame this window draws.
            let Some(r) = bounds(hwnd) else { return Found::Seen };
            return if RectInRegion(left, &r) != 0 { Found::Seen } else { Found::Covered };
        }
        let Some(child_rect) = window_rect(hwnd) else { return Found::Seen };
        let Some(root_rect) = bounds(root).or_else(|| window_rect(root)) else { return Found::Seen };
        let rgn = CreateRectRgn(0, 0, 0, 0);
        let shape = GetWindowRgn(hwnd, rgn);
        let child_rgn = (shape != RGN_ERROR).then_some(rgn);
        let found = child_overlap_with_left(child_rect, root_rect, child_rgn, left);
        DeleteObject(rgn);
        found.into()
    }
}

/// The rect an opaque window covers, or None for one that may let what's under it show, as
/// Chromium's `IsWindowVisibleAndFullyOpaque` decides. `tracked` top-level windows are ours, and
/// cover what's under them whatever their styles; Explorer's desktop shell layers never count as
/// opaque over embedded desktop hosts.
fn opaque_rect(hwnd: HWND, tracked: bool, shell_pid: Option<u32>) -> Option<RECT> {
    // SAFETY: plain queries on a window `EnumWindows` handed over.
    unsafe {
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 { return None; }
        if !tracked {
            if shell_pid.is_some_and(|spid| window_pid(hwnd) == Some(spid))
                && window_class(hwnd).as_deref().is_some_and(is_desktop_shell_class)
            {
                return None;
            }
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            // Clicks fall through it.
            if ex & WS_EX_TRANSPARENT != 0 { return None; }
            // Floating windows can be larger than what they draw; the taskbar isn't.
            if (ex & WS_EX_TOOLWINDOW != 0 || GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_POPUP != 0) && !taskbar(hwnd) { return None; }
            if ex & WS_EX_LAYERED != 0 {
                let (mut key, mut alpha, mut flags) = (0u32, 0u8, 0u32);
                // Fails for a window drawn with UpdateLayeredWindow: per-pixel alpha.
                if GetLayeredWindowAttributes(hwnd, &mut key, &mut alpha, &mut flags) == 0 { return None; }
                if flags & LWA_COLORKEY != 0 || (flags & LWA_ALPHA != 0 && alpha < 255) { return None; }
            }
            let rgn = CreateRectRgn(0, 0, 0, 0);
            let shape = GetWindowRgn(hwnd, rgn);
            DeleteObject(rgn);
            if shape == COMPLEXREGION { return None; }
            // Shown to no one: on another virtual desktop, or a shell window kept for later.
            if cloaked(hwnd).is_some_and(|c| c != 0) { return None; }
        }
        let mut r = bounds(hwnd)?;
        if r.right <= r.left || r.bottom <= r.top { return None; }
        let mut placement: WINDOWPLACEMENT = std::mem::zeroed();
        placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(hwnd, &mut placement) != 0 && placement.showCmd == SW_MAXIMIZE as u32 {
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) != 0 {
                let w = info.rcWork;
                r = RECT { left: r.left.max(w.left), top: r.top.max(w.top), right: r.right.min(w.right), bottom: r.bottom.min(w.bottom) };
            }
        }
        Some(r)
    }
}

fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: `r` is a valid out RECT.
    (unsafe { GetWindowRect(hwnd, &mut r) } != 0).then_some(r)
}

fn desktop_shell_pid() -> Option<u32> {
    // SAFETY: GetShellWindow tolerates any state and returns HWND or null.
    let shell = unsafe { GetShellWindow() };
    (!shell.is_null()).then(|| window_pid(shell)).flatten()
}

fn window_pid(hwnd: HWND) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out pointer.
    let tid = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    (tid != 0 && pid != 0).then_some(pid)
}

fn window_class(hwnd: HWND) -> Option<String> {
    let mut class = [0u16; 64];
    // SAFETY: `class` holds 64 UTF-16 code units, as passed.
    let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    (n > 0).then(|| String::from_utf16_lossy(&class[..n as usize]))
}

/// The rect DWM draws a window in, without its invisible resize borders.
fn bounds(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: `r` is a RECT, as the attribute's size says.
    let hr = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS as u32, (&raw mut r).cast(), std::mem::size_of::<RECT>() as u32) };
    (hr >= 0).then_some(r)
}

/// Why DWM keeps a window off screen, if it does: `DWM_CLOAKED_SHELL` for another virtual
/// desktop, `DWM_CLOAKED_APP` for its app's own cloak.
fn cloaked(hwnd: HWND) -> Option<u32> {
    let mut why = 0u32;
    // SAFETY: `why` is a u32, as the attribute's size says.
    let hr = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, (&raw mut why).cast(), 4) };
    (hr >= 0).then_some(why)
}

fn taskbar(hwnd: HWND) -> bool {
    window_class(hwnd).as_deref() == Some("Shell_TrayWnd")
}

