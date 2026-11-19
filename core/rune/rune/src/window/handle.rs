//! `WindowHandle`: a window's native reference that outlives any one host, and the requests
//! made through it before the host exists.

use super::*;

pub(super) type Request = Box<dyn FnOnce(usize) -> WakeEvent + Send>;

#[derive(Default)]
pub(super) struct NativeWindow {
    pub(super) window: Mutex<Weak<Window>>,
    pub(super) number: AtomicU32,
    pub(super) ready: AtomicBool,
    /// What the host's meter last published.
    pub(super) stats: Arc<Mutex<SurfaceStats>>,
    pub(super) route: Mutex<Route>,
}

/// Where a window's requests go.
pub(super) enum Route {
    /// No host yet, or a headless run, which takes them each turn (`take_headless`).
    Unbound { queued: Vec<Request> },
    /// The host's event loop and slot.
    Bound { proxy: EventLoopProxy<WakeEvent>, slot: usize },
}

impl Default for Route {
    fn default() -> Self { Route::Unbound { queued: Vec::new() } }
}

/// What [`WindowHandle::gpu_memory`] returns.
#[derive(Clone, Debug)]
pub struct GpuMemory {
    pub counters: wgpu::InternalCounters,
    pub layers: crate::render::LayerStats,
    pub report: Option<wgpu::AllocatorReport>,
}

/// A forwarded keyboard event delivered through [`WindowHandle::inject_key`].
#[derive(Clone, Debug, PartialEq)]
pub struct InjectedKey {
    pub physical: crate::PhysicalKeyCode,
    pub pressed: bool,
    pub repeat: bool,
    pub mods: crate::Mods,
    pub logical: Option<crate::KeyEvent>,
    pub key_base: Option<char>,
}

pub(crate) struct PendingCapture {
    pub(crate) device: wgpu::Device,
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) submission: wgpu::SubmissionIndex,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) padded_row: u32,
    pub(crate) bgra: bool,
    pub(crate) map_rx: std::sync::mpsc::Receiver<Result<(), String>>,
}

impl PendingCapture {
    pub(crate) fn finish(self) -> Result<(u32, u32, Vec<u8>), String> {
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(self.submission),
            timeout: Some(std::time::Duration::from_secs(3)),
        });
        self.map_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .map_err(|e| format!("GPU readback map wait failed: {e}"))??;
        let row_bytes = (self.width as usize).saturating_mul(4);
        let padded = self.padded_row as usize;
        let mut rgba = Vec::with_capacity(row_bytes.saturating_mul(self.height as usize));
        {
            let slice = self.buffer.slice(..);
            let data = slice.get_mapped_range().map_err(|e| format!("readback map range failed: {e}"))?;
            for row in 0..self.height as usize {
                let start = row * padded;
                rgba.extend_from_slice(&data[start..start + row_bytes]);
            }
        }
        self.buffer.unmap();
        drop(self.buffer);
        crate::render::unpremultiply_to_straight_rgba(&mut rgba, self.bgra);
        Ok((self.width, self.height, rgba))
    }
}

/// A window's native reference, refreshed across App close/reopen without owning the window.
#[derive(Clone)]
pub struct WindowHandle {
    pub(super) native: Arc<NativeWindow>,
    pub(super) key: Arc<str>,
}
impl WindowHandle {
    pub(crate) fn new(key: &str) -> Self { Self { native: Arc::default(), key: key.into() } }

    /// Routes requests to the window's host, starting with those queued before it existed.
    pub(super) fn bind(&self, proxy: EventLoopProxy<WakeEvent>, slot: usize) {
        let mut route = self.native.route.lock().unwrap();
        if let Route::Unbound { queued } = &mut *route {
            for request in std::mem::take(queued) { let _ = proxy.send_event(request(slot)); }
        }
        *route = Route::Bound { proxy, slot };
    }

    /// A headless run never binds a host; it takes the queued requests instead.
    pub(crate) fn take_headless(&self) -> Vec<WakeEvent> {
        let queued = match &mut *self.native.route.lock().unwrap() {
            Route::Unbound { queued } => std::mem::take(queued),
            Route::Bound { .. } => Vec::new(),
        };
        queued.into_iter().map(|request| request(0)).collect()
    }

    pub(crate) fn headless_pending(&self) -> bool {
        matches!(&*self.native.route.lock().unwrap(), Route::Unbound { queued } if !queued.is_empty())
    }

    pub(super) fn send(&self, request: impl FnOnce(usize) -> WakeEvent + Send + 'static) {
        match &mut *self.native.route.lock().unwrap() {
            Route::Bound { proxy, slot } => { let _ = proxy.send_event(request(*slot)); }
            Route::Unbound { queued } => queued.push(Box::new(request)),
        }
    }

    /// The first frame has completed for the current native lifetime. For a launcher
    /// this confirms GPU completion of an alpha-zero frame before reveal. False after destruction.
    pub fn is_ready(&self) -> bool { self.window().is_ok() && self.native.ready.load(Ordering::Acquire) }
    pub fn key(&self) -> &str { &self.key }
    /// The window's frame counts: redraws, presents, failed drawable acquisitions, loop turns
    /// and pacing: current totals, and per-second rates updated once a second while it draws.
    pub fn surface_stats(&self) -> SurfaceStats { crate::frame::surface_stats::read(&self.native.stats) }
    pub(super) fn window(&self) -> Result<Arc<Window>, String> {
        self.native.window.lock().unwrap().upgrade().ok_or_else(|| "window is closed".into())
    }
    /// macOS CGWindowID for ScreenCaptureKit exclusion; None before registration or after destruction.
    pub fn window_number(&self) -> Option<u32> {
        let _window = self.window().ok()?;
        let number = self.native.number.load(Ordering::Acquire);
        (number != 0).then_some(number)
    }
    /// Where the window is on the desktop and whether it's maximized, minimized or full screen,
    /// to save and reopen it there. None while it's closed, and on Wayland, which hides window
    /// positions. Call it on the UI thread.
    pub fn placement(&self) -> Option<crate::windows::WindowPlacement> {
        let window = self.window().ok()?;
        let (outer, inner) = (window.outer_position().ok()?, window.inner_position().ok()?);
        let size = window.inner_size();
        Some(crate::windows::WindowPlacement {
            position: (outer.x, outer.y),
            content_position: (inner.x, inner.y),
            content_size: (size.width, size.height),
            scale: window.scale_factor(),
            maximized: window.is_maximized(),
            minimized: window.is_minimized().unwrap_or(false),
            full_screen: window.fullscreen().is_some(),
        })
    }
    pub fn set_click_through(&self, click_through: bool) -> Result<(), String> {
        self.window()?.set_cursor_hittest(!click_through).map_err(|e| e.to_string())
    }
    pub fn set_always_on_top(&self, on: bool) {
        self.send(move |slot| WakeEvent::Level(slot, on));
    }
    /// Physical desktop pixels; retained across recreation. Unsupported on Wayland.
    pub fn set_position(&self, position: (i32, i32)) {
        self.send(move |slot| WakeEvent::Position(slot, position));
    }
    pub fn place(&self, display: crate::DisplayTarget, placement: crate::OverlayPlacement) {
        let place = crate::windows::PlaceRequest::new(display, placement);
        self.send(move |slot| WakeEvent::Place(slot, place));
    }
    /// [`Tx::resize_window`] from outside a handler, on `tok::motion::DEFAULT`.
    pub fn resize(&self, size: (f32, f32)) {
        self.frame(crate::window::frame::FrameRequest { size: Some(size), animate: true, ..Default::default() });
    }
    /// [`Tx::resize_window_instant`] from outside a handler.
    pub fn resize_instant(&self, size: (f32, f32)) {
        self.frame(crate::window::frame::FrameRequest { size: Some(size), ..Default::default() });
    }
    /// [`Tx::center_window`] from outside a handler.
    pub fn center(&self) {
        self.frame(crate::window::frame::FrameRequest { center: true, ..Default::default() });
    }
    pub(super) fn frame(&self, request: crate::window::frame::FrameRequest) {
        self.send(move |slot| WakeEvent::Frame(slot, request));
    }
    /// Show/recreate, or hide while keeping state. A launcher takes keys and reveals cached
    /// pixels; an ordinary window shows without requesting focus.
    pub fn set_visible(&self, visible: bool) {
        let request = if visible { WindowRequest::Show } else { WindowRequest::Hide };
        self.send(move |slot| WakeEvent::Window(slot, request));
    }
    pub fn show(&self) { self.set_visible(true); }
    pub fn hide(&self) { self.set_visible(false); }
    /// Focus/activate an already-visible window, restoring it if minimized. Wayland requests attention.
    pub fn focus(&self) { self.send(WakeEvent::Focus); }
    /// Show, restore and activate, creating the window if needed (see `AppHandle::open`).
    /// A default close destroys its host; use App::keep_running to reopen.
    /// Windows added with `App::window` always reopen. Requests made before the loop runs
    /// apply once the window's host starts.
    pub fn open(&self) { self.send(|slot| WakeEvent::Window(slot, WindowRequest::Open(None))); }
    pub fn close(&self) { self.send(|slot| WakeEvent::Window(slot, WindowRequest::Close)); }
    /// Borrow a raw native handle while alive. Platform thread restrictions still apply;
    /// do not retain an extracted pointer beyond the callback.
    pub fn with_raw_handle<R>(&self, f: impl for<'a> FnOnce(winit::raw_window_handle::WindowHandle<'a>) -> R) -> Result<R, String> {
        let window = self.window()?;
        let handle = window.window_handle().map_err(|e| e.to_string())?;
        Ok(f(handle))
    }

    /// Re-evaluates Win32 occlusion immediately (for example after reparenting or updating `SetWindowRgn`).
    pub fn recheck_occlusion(&self) {
        #[cfg(target_os = "windows")]
        crate::platform::win32::occlusion::recheck();
    }

    /// Delivers a lossless keyboard event directly to this window's runtime without OS focus switching.
    pub fn inject_key(&self, key: InjectedKey) {
        self.send(move |slot| WakeEvent::InjectKey(slot, key));
    }

    /// Releases any held forwarded keys and delivers `PhysicalInput::FocusLost` on the UI thread.
    pub fn inject_focus_lost(&self) {
        self.send(WakeEvent::InjectFocusLost);
    }

    /// Renders this window into an isolated offscreen target (even while desktop-covered) and returns
    /// straight-alpha RGBA8 `(width, height, pixels)`. GPU polling and pixel conversion run off the UI thread.
    pub fn capture_rgba(&self) -> Result<(u32, u32, Vec<u8>), String> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        {
            let route = self.native.route.lock().unwrap();
            match &*route {
                Route::Bound { proxy, slot } => {
                    proxy
                        .send_event(WakeEvent::Capture(*slot, tx))
                        .map_err(|_| "window event loop closed".to_string())?;
                }
                Route::Unbound { .. } => return Err("window host is not running yet".into()),
            }
        }
        let pending = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| format!("offscreen capture timed out or cancelled: {e}"))??;
        pending.finish()
    }

    /// What this window's GPU device holds: `wgpu`'s counters of textures and buffers (every
    /// resource, committed or suballocated), the renderer's layer targets, and the allocator's
    /// report by label where the backend suballocates (DX12 on Iris Xe does not, so it is
    /// empty there). `None` while the window's host is not running. Swap-chain buffers are the
    /// driver's and are in none of them.
    pub fn gpu_memory(&self) -> Option<GpuMemory> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        match &*self.native.route.lock().unwrap() {
            Route::Bound { proxy, slot } => proxy.send_event(WakeEvent::GpuMemory(*slot, tx)).ok()?,
            Route::Unbound { .. } => return None,
        }
        rx.recv_timeout(std::time::Duration::from_secs(2)).ok().flatten()
    }
}

#[cfg(target_os = "macos")]
pub(super) fn native_window_number(window: &Window) -> Option<u32> {
    // SAFETY: called on winit's event thread.
    u32::try_from(unsafe { crate::platform::macos::ns_window(window)?.windowNumber() }).ok().filter(|n| *n != 0)
}
#[cfg(not(target_os = "macos"))]
pub(super) fn native_window_number(_: &Window) -> Option<u32> { None }

