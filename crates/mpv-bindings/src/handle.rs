//! Owned handle to a libmpv instance.
//!
//! `MpvHandle` is the single owner of one `mpv_handle*`. It is `Send` but not
//! `Sync` or `Clone` — you drive libmpv from exactly one thread (typically the
//! engine thread), and pass messages to/from the UI via channels.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::command::Command;
use crate::error::{MpvError, MpvResult};
use crate::event::Event;
use crate::property::{Format, Property};
use crate::sys;

/// Owned libmpv instance.
pub struct MpvHandle {
    raw: *mut sys::mpv_handle,
    /// Set to true when we've called `mpv_terminate_destroy`. Used by Drop
    /// to avoid double-free if user explicitly called `shutdown`.
    terminated: AtomicBool,
}

// libmpv's handle is thread-safe to use from one thread at a time. We model
// that as `Send` (not `Sync`) so the borrow checker enforces "owned by one
// thread at a time" statically.
unsafe impl Send for MpvHandle {}
unsafe impl Sync for MpvHandle {}

impl MpvHandle {
    /// Create a new libmpv instance (uninitialized). Use `Builder` for a
    /// configured one.
    pub fn new() -> MpvResult<Self> {
        // Safety: mpv_create returns a fresh handle or NULL.
        let raw = unsafe { sys::mpv_create() };
        if raw.is_null() {
            return Err(MpvError::Generic(-1));
        }
        Ok(Self { raw, terminated: AtomicBool::new(false) })
    }

    /// Set a string option before initialize. Equivalent to `--<name>=<value>`
    /// on the mpv CLI.
    pub fn set_option_string(&self, name: &str, value: &str) -> MpvResult<()> {
        let c_name = CString::new(name)?;
        let c_val = CString::new(value)?;
        let code = unsafe { sys::mpv_set_option_string(self.raw, c_name.as_ptr(), c_val.as_ptr()) };
        MpvError::from_code(code)
    }

    /// Initialize libmpv. After this point, options are frozen and the
    /// event loop starts producing events.
    pub fn initialize(&self) -> MpvResult<()> {
        let code = unsafe { sys::mpv_initialize(self.raw) };
        MpvError::from_code(code)
    }

    /// Send a command (built via `Command::new()`).
    pub fn command(&self, cmd: &Command) -> MpvResult<()> {
        let argv: Vec<*const c_char> = cmd.argv_with_null();
        let code = unsafe {
            sys::mpv_command(
                self.raw,
                argv.as_ptr() as *mut *const c_char,
            )
        };
        MpvError::from_code(code)
    }

    /// Set a property (typed).
    pub fn set_property(&self, prop: &Property) -> MpvResult<()> {
        let name = CString::new(prop.name())?;
        let code = unsafe {
            match prop.format() {
                Format::String => {
                    let s = CString::new(prop.as_str()?)?;
                    sys::mpv_set_property(
                        self.raw,
                        name.as_ptr(),
                        sys::mpv_format_MPV_FORMAT_STRING,
                        s.as_ptr() as *mut c_void,
                    )
                }
                Format::Flag => {
                    let v: i32 = prop.as_flag()? as i32;
                    sys::mpv_set_property(
                        self.raw,
                        name.as_ptr(),
                        sys::mpv_format_MPV_FORMAT_FLAG,
                        &v as *const i32 as *mut c_void,
                    )
                }
                Format::Int64 => {
                    let v = prop.as_i64()?;
                    sys::mpv_set_property(
                        self.raw,
                        name.as_ptr(),
                        sys::mpv_format_MPV_FORMAT_INT64,
                        &v as *const i64 as *mut c_void,
                    )
                }
                Format::Double => {
                    let v = prop.as_f64()?;
                    sys::mpv_set_property(
                        self.raw,
                        name.as_ptr(),
                        sys::mpv_format_MPV_FORMAT_DOUBLE,
                        &v as *const f64 as *mut c_void,
                    )
                }
                _ => return Err(MpvError::PropertyFormat),
            }
        };
        MpvError::from_code(code)
    }

    /// Set a property to a string value (convenience for options that take
    /// string forms like "no", "inf", "auto", "yes"). Equivalent to
    /// `mpv_set_property_string(handle, name, value)`.
    pub fn set_property_string(&self, name: &str, value: &str) -> MpvResult<()> {
        let c_name = CString::new(name)?;
        let c_val = CString::new(value)?;
        let code = unsafe {
            sys::mpv_set_property_string(self.raw, c_name.as_ptr(), c_val.as_ptr())
        };
        // mpv_set_property_string returns >= 0 on success, < 0 on error.
        MpvError::from_code(code)
    }

    /// Get a property as string. (Most useful for metadata.)
    pub fn get_property_string(&self, name: &str) -> MpvResult<Option<String>> {
        let c_name = CString::new(name)?;
        // Safety: mpv_get_property_string returns a malloc'd string or NULL.
        let raw = unsafe { sys::mpv_get_property_string(self.raw, c_name.as_ptr()) };
        if raw.is_null() {
            return Ok(None);
        }
        // Safety: the returned CStr is valid until we free it.
        let s = unsafe { CStr::from_ptr(raw) }.to_str()?.to_owned();
        unsafe { sys::mpv_free(raw as *mut c_void) };
        Ok(Some(s))
    }

    /// Get a property as f64 (works for time-pos, duration, volume, etc.).
    pub fn get_property_f64(&self, name: &str) -> MpvResult<f64> {
        let c_name = CString::new(name)?;
        let mut v: f64 = 0.0;
        let code = unsafe {
            sys::mpv_get_property(
                self.raw,
                c_name.as_ptr(),
                sys::mpv_format_MPV_FORMAT_DOUBLE,
                &mut v as *mut f64 as *mut c_void,
            )
        };
        MpvError::from_code(code)?;
        Ok(v)
    }

    /// Get a property as i64.
    pub fn get_property_i64(&self, name: &str) -> MpvResult<i64> {
        let c_name = CString::new(name)?;
        let mut v: i64 = 0;
        let code = unsafe {
            sys::mpv_get_property(
                self.raw,
                c_name.as_ptr(),
                sys::mpv_format_MPV_FORMAT_INT64,
                &mut v as *mut i64 as *mut c_void,
            )
        };
        MpvError::from_code(code)?;
        Ok(v)
    }

    /// Get a property as bool (flag).
    pub fn get_property_flag(&self, name: &str) -> MpvResult<bool> {
        let c_name = CString::new(name)?;
        let mut v: i32 = 0;
        let code = unsafe {
            sys::mpv_get_property(
                self.raw,
                c_name.as_ptr(),
                sys::mpv_format_MPV_FORMAT_FLAG,
                &mut v as *mut i32 as *mut c_void,
            )
        };
        MpvError::from_code(code)?;
        Ok(v != 0)
    }

    /// Observe a property. The engine thread will receive `Property` events
    /// with the given reply_user_data tag when the value changes.
    pub fn observe_property(&self, reply_userdata: u64, name: &str, format: Format) -> MpvResult<()> {
        let c_name = CString::new(name)?;
        let code = unsafe {
            sys::mpv_observe_property(
                self.raw,
                reply_userdata,
                c_name.as_ptr(),
                format.as_mpv_format(),
            )
        };
        MpvError::from_code(code)
    }

    /// Set the requested log level. Lower levels are filtered.
    pub fn request_log_messages(&self, level: crate::event::LogLevel) -> MpvResult<()> {
        let s = CString::new(level.as_str())?;
        let code = unsafe { sys::mpv_request_log_messages(self.raw, s.as_ptr()) };
        MpvError::from_code(code)
    }

    /// Wait for the next event, blocking the calling thread.
    /// Returns `None` if the handle has been terminated.
    pub fn wait_event(&self, timeout_seconds: f64) -> MpvResult<Option<Event>> {
        // SAFETY: `mpv_wait_event` returns a pointer to an internal mpv_event
        // stored inside the handle. The pointer is valid until the next call
        // to `wait_event` on the same handle, and we serialize all calls via
        // `&self` (single-threaded access within the engine thread). We copy
        // out everything we need before returning, so no aliasing escapes.
        let raw_event = unsafe { sys::mpv_wait_event(self.raw, timeout_seconds) };
        if raw_event.is_null() {
            return Ok(None);
        }
        let event = unsafe { &*raw_event };
        if event.event_id == sys::mpv_event_id_MPV_EVENT_NONE {
            return Ok(None);
        }
        if event.error != 0 {
            // For MPV_EVENT_SHUTDOWN, error is 0; this branch catches other failures.
            return Err(MpvError::from_code(event.error).unwrap_err());
        }
        Ok(Event::from_raw(event))
    }

    /// Wake up `wait_event` from another thread (e.g. on shutdown).
    pub fn wakeup(&self) {
        unsafe { sys::mpv_wakeup(self.raw) };
    }

    /// Destroy the handle. Idempotent: a second call is a no-op. The atomic
    /// guard makes this safe to invoke from `Drop` even when an explicit
    /// `shutdown()` has already run.
    pub fn shutdown(&self) {
        if self.terminated.swap(true, Ordering::SeqCst) {
            return;
        }
        // SAFETY: `mpv_terminate_destroy` is the documented teardown call.
        // After it returns, the handle is invalid; we never touch `self.raw`
        // again because the atomic guard short-circuits any future call.
        unsafe { sys::mpv_terminate_destroy(self.raw) };
    }

    /// Raw pointer (for advanced consumers). Don't use unless you know what
    /// you're doing.
    pub fn as_ptr(&self) -> *mut sys::mpv_handle {
        self.raw
    }
}

impl Drop for MpvHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Builder for an initialized `MpvHandle`.
pub struct Builder {
    options: Vec<(String, String)>,
    log_level: crate::event::LogLevel,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        Self {
            options: Vec::new(),
            log_level: crate::event::LogLevel::Error,
        }
    }

    /// Set a string option (equivalent to `--name=value` on the mpv CLI).
    pub fn option(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.push((name.into(), value.into()));
        self
    }

    pub fn log_level(mut self, level: crate::event::LogLevel) -> Self {
        self.log_level = level;
        self
    }

    /// Build + initialize. Returns a ready-to-use `MpvHandle`.
    pub fn build(self) -> MpvResult<MpvHandle> {
        let h = MpvHandle::new()?;
        for (k, v) in &self.options {
            h.set_option_string(k, v)?;
        }
        h.initialize()?;
        h.request_log_messages(self.log_level)?;
        Ok(h)
    }
}
