//! OS interface/route changes wake consumers without polling or blocking callbacks.
use std::sync::{atomic::{AtomicBool, Ordering}, OnceLock};
use windows::Win32::{Foundation::{HANDLE, NO_ERROR}, NetworkManagement::IpHelper::{
    NotifyIpInterfaceChange, NotifyRouteChange2, CancelMibChangeNotify2,
    MIB_IPINTERFACE_ROW, MIB_IPFORWARD_ROW2, MIB_NOTIFICATION_TYPE,
}, Networking::WinSock::AF_UNSPEC};

static CHANGED: AtomicBool = AtomicBool::new(false);
static WORKER: OnceLock<std::thread::Thread> = OnceLock::new();
fn signal() {
    CHANGED.store(true, Ordering::Release);
    if let Some(worker) = WORKER.get() { worker.unpark(); }
}
unsafe extern "system" fn interface_changed(_: *const std::ffi::c_void,
    _: *const MIB_IPINTERFACE_ROW, _: MIB_NOTIFICATION_TYPE) { signal(); }
unsafe extern "system" fn route_changed(_: *const std::ffi::c_void,
    _: *const MIB_IPFORWARD_ROW2, _: MIB_NOTIFICATION_TYPE) { signal(); }

pub fn install() {
    std::thread::Builder::new().name("network-events".into()).spawn(|| {
        let _ = WORKER.set(std::thread::current());
        let mut interface = HANDLE::default();
        let mut route = HANDLE::default();
        let interface_ok = unsafe { NotifyIpInterfaceChange(AF_UNSPEC,
            Some(interface_changed), None, false, &mut interface) } == NO_ERROR;
        let route_ok = unsafe { NotifyRouteChange2(AF_UNSPEC,
            Some(route_changed), std::ptr::null(), false, &mut route) } == NO_ERROR;
        crate::log_line(format!("network: OS notifications interface={interface_ok} route={route_ok}"));
        while !crate::is_shutting_down() {
            std::thread::park_timeout(std::time::Duration::from_secs(30));
            if CHANGED.swap(false, Ordering::AcqRel) {
                // Coalesce a burst from one DHCP/VPN operation, away from OS callbacks.
                std::thread::sleep(std::time::Duration::from_millis(250));
                CHANGED.store(false, Ordering::Release);
                if !crate::is_shutting_down() {
                    unidtk_api::emit("network-changed", serde_json::json!({}));
                }
            }
        }
        unsafe {
            if interface_ok { let _ = CancelMibChangeNotify2(interface); }
            if route_ok { let _ = CancelMibChangeNotify2(route); }
        }
    }).expect("spawn network observer");
}
