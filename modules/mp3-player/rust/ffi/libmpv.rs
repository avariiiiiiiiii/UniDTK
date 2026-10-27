//! Minimal libmpv FFI bindings (raw-dylib).
//!
//! Links directly against `libmpv-2.dll` via `#[link(kind = "raw-dylib")]`,
//! so no import library is needed at build time; the DLL must be present next
//! to `unidtk.exe` at runtime (bundled via tauri.conf.json resources).
//!
//! Only the subset of the libmpv client API used by the player is declared.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::ptr;

pub type mpv_handle = c_void;

pub const MPV_FORMAT_FLAG: c_int = 3;
/// Only used by the test-only `set_property_i64` helper.
#[cfg(test)]
pub const MPV_FORMAT_INT64: c_int = 4;
pub const MPV_FORMAT_DOUBLE: c_int = 5;

pub const MPV_ERROR_SUCCESS: c_int = 0;

#[link(name = "libmpv-2", kind = "raw-dylib")]
extern "C" {
    pub fn mpv_create() -> *mut mpv_handle;
    pub fn mpv_initialize(ctx: *mut mpv_handle) -> c_int;
    pub fn mpv_destroy(ctx: *mut mpv_handle);
    pub fn mpv_set_option_string(ctx: *mut mpv_handle, name: *const c_char, data: *const c_char) -> c_int;
    pub fn mpv_set_property(
        ctx: *mut mpv_handle,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    pub fn mpv_set_property_string(ctx: *mut mpv_handle, name: *const c_char, data: *const c_char) -> c_int;
    pub fn mpv_get_property(
        ctx: *mut mpv_handle,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    pub fn mpv_command(ctx: *mut mpv_handle, args: *const *const c_char) -> c_int;
}

/// Safe wrapper around a raw mpv_handle.
///
/// libmpv is thread-safe; the wrapper asserts Send so it can live in
/// `Arc<Mutex<_>>` across Tauri's blocking threads.
pub struct Libmpv {
    handle: *mut mpv_handle,
}

unsafe impl Send for Libmpv {}

impl Libmpv {
    /// Create + initialize a new libmpv instance.
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { mpv_create() };
        if handle.is_null() {
            return Err("mpv_create failed (out of memory?)".to_string());
        }
        let ret = unsafe { mpv_initialize(handle) };
        if ret != MPV_ERROR_SUCCESS {
            unsafe { mpv_destroy(handle) };
            return Err(format!("mpv_initialize failed: {ret}"));
        }
        Ok(Libmpv { handle })
    }

    pub fn set_option(&mut self, name: &str, value: &str) -> Result<(), String> {
        let n = c(name)?;
        let v = c(value)?;
        let ret = unsafe { mpv_set_option_string(self.handle, n.as_ptr(), v.as_ptr()) };
        if ret != MPV_ERROR_SUCCESS {
            return Err(format!("mpv_set_option({name}={value}) failed: {ret}"));
        }
        Ok(())
    }

    pub fn set_property_str(&mut self, name: &str, value: &str) -> Result<(), String> {
        let n = c(name)?;
        let v = c(value)?;
        let ret = unsafe { mpv_set_property_string(self.handle, n.as_ptr(), v.as_ptr()) };
        if ret != MPV_ERROR_SUCCESS {
            return Err(format!("mpv_set_property({name}={value}) failed: {ret}"));
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn set_property_i64(&mut self, name: &str, value: i64) -> Result<(), String> {
        let n = c(name)?;
        let mut v = value as i64;
        let ret = unsafe {
            mpv_set_property(
                self.handle,
                n.as_ptr(),
                MPV_FORMAT_INT64,
                &mut v as *mut i64 as *mut c_void,
            )
        };
        if ret != MPV_ERROR_SUCCESS {
            return Err(format!("mpv_set_property({name}={value}) failed: {ret}"));
        }
        Ok(())
    }

    pub fn set_property_double(&mut self, name: &str, value: f64) -> Result<(), String> {
        let n = c(name)?;
        let mut v = value as c_double;
        let ret = unsafe {
            mpv_set_property(
                self.handle,
                n.as_ptr(),
                MPV_FORMAT_DOUBLE,
                &mut v as *mut c_double as *mut c_void,
            )
        };
        if ret != MPV_ERROR_SUCCESS {
            return Err(format!("mpv_set_property({name}={value}) failed: {ret}"));
        }
        Ok(())
    }

    pub fn set_property_flag(&mut self, name: &str, value: bool) -> Result<(), String> {
        let n = c(name)?;
        let v = if value { "yes" } else { "no" };
        let v_c = c(v)?;
        let ret = unsafe { mpv_set_property_string(self.handle, n.as_ptr(), v_c.as_ptr()) };
        if ret == MPV_ERROR_SUCCESS {
            return Ok(());
        }
        let mut flag: c_int = if value { 1 } else { 0 };
        let ret2 = unsafe {
            mpv_set_property(
                self.handle,
                n.as_ptr(),
                MPV_FORMAT_FLAG,
                &mut flag as *mut c_int as *mut c_void,
            )
        };
        if ret2 != MPV_ERROR_SUCCESS {
            return Err(format!("mpv_set_property({name}={value}) failed: {ret}"));
        }
        Ok(())
    }

    pub fn get_property_double(&self, name: &str) -> Result<f64, String> {
        let n = c(name)?;
        let mut v: c_double = 0.0;
        let ret = unsafe {
            mpv_get_property(
                self.handle,
                n.as_ptr(),
                MPV_FORMAT_DOUBLE,
                &mut v as *mut c_double as *mut c_void,
            )
        };
        if ret != MPV_ERROR_SUCCESS {
            return Err(format!("mpv_get_property({name}) failed: {ret}"));
        }
        Ok(v as f64)
    }

    pub fn get_property_flag(&self, name: &str) -> Result<bool, String> {
        let n = c(name)?;
        let mut v: c_int = 0;
        let ret = unsafe {
            mpv_get_property(
                self.handle,
                n.as_ptr(),
                MPV_FORMAT_FLAG,
                &mut v as *mut c_int as *mut c_void,
            )
        };
        if ret != MPV_ERROR_SUCCESS {
            return Err(format!("mpv_get_property({name}) failed: {ret}"));
        }
        Ok(v != 0)
    }

    /// Run a command (e.g. `loadfile <path> replace`, `stop`, `seek <pos> absolute`).
    pub fn command(&mut self, args: &[&str]) -> Result<(), String> {
        let cstrs: Vec<CString> = args
            .iter()
            .map(|a| CString::new(*a).map_err(|_| format!("NUL in command arg: {a}")))
            .collect::<Result<_, _>>()?;
        let mut ptrs: Vec<*const c_char> = cstrs.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(ptr::null());
        let ret = unsafe { mpv_command(self.handle, ptrs.as_ptr()) };
        if ret != MPV_ERROR_SUCCESS {
            return Err(format!("mpv_command({args:?}) failed: {ret}"));
        }
        Ok(())
    }
}

impl Drop for Libmpv {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { mpv_destroy(self.handle) };
            self.handle = ptr::null_mut();
        }
    }
}

fn c(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| format!("NUL byte in string: {s:?}"))
}

/// Decode a C string (helper, kept for completeness).
#[allow(dead_code)]
unsafe fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    CStr::from_ptr(p).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_destroy() {
        // Requires libmpv-2.dll next to the test binary. If it's missing,
        // creation fails with a clear error instead of crashing.
        let mpv = Libmpv::new();
        match mpv {
            Ok(mut m) => {
                // Setting a simple option should succeed.
                assert!(m.set_option("config", "no").is_ok());
            }
            Err(e) => {
                panic!("libmpv-2.dll missing? {e}");
            }
        }
    }

    #[test]
    fn test_set_get_properties() {
        if let Ok(mut mpv) = Libmpv::new() {
            let _ = mpv.set_property_i64("volume", 50);
            let _ = mpv.set_property_flag("pause", true);
            // These may fail on a fresh instance (no file loaded), but must not panic.
            let _ = mpv.get_property_double("time-pos");
            let _ = mpv.get_property_flag("eof-reached");
            let _ = mpv.command(&["stop"]);
        }
    }
}
