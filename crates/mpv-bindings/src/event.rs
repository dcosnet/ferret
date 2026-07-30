//! Safe event parsing.
//!
//! libmpv delivers events through `mpv_wait_event`. Each event has an
//! `event_id` (the type) and a `data` pointer whose shape depends on the
//! type. We parse the common ones into a safe `Event` enum here.

use std::ffi::CStr;
use std::os::raw::{c_char, c_void};

use crate::sys;

/// Tag used to correlate observed-property events with their registration.
pub type EventId = u64;

#[derive(Debug, Clone)]
pub enum Event {
    /// mpv finished initializing the audio/video/pipeline.
    StartFile,
    /// A file has been loaded and is ready to play. Carries the playlist
    /// position (1-indexed in libmpv terms).
    FileLoaded,
    /// Playback of the current file ended. `reason` is best-effort.
    EndFile { reason: EndFileReason, error: Option<i32> },
    /// A new log message arrived.
    LogMessage { prefix: String, level: String, text: String },
    /// A property we observed changed.
    PropertyChange {
        reply_userdata: EventId,
        name: String,
        value: PropertyValue,
    },
    /// libmpv is shutting down.
    Shutdown,
    /// Hook event (used internally; we don't expose hooks yet).
    Hook { id: u64, name: String },
    /// Anything we don't model yet.
    Other { event_id: u32 },
}

/// Reason mpv reports for an `MPV_EVENT_END_FILE`. The `#[repr(u8)]` layout
/// makes the discriminant safe to use as a table index — see
/// `engine::END_REASON_MAP`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EndFileReason {
    Eof = 0,
    Stop = 1,
    Quit = 2,
    Error = 3,
    Redirect = 4,
    Unknown = 5,
}

#[derive(Debug, Clone)]
pub enum PropertyValue {
    None,
    Flag(bool),
    Int64(i64),
    Double(f64),
    String(String),
}

/// Log levels, in increasing order of verbosity.
#[derive(Copy, Clone, Debug)]
pub enum LogLevel {
    Quiet,
    Fatal,
    Error,
    Warn,
    Info,
    Status,
    Verbose,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Quiet => "no",
            LogLevel::Fatal => "fatal",
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Status => "status",
            LogLevel::Verbose => "v",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

impl Event {
    /// Parse a `*const mpv_event` into a safe `Event`.
    ///
    /// # Safety
    /// `raw` must point to a valid `mpv_event` returned by `mpv_wait_event`,
    /// and the `data` field (if non-null) must point to a struct of the
    /// correct type for `event.event_id`. Both invariants hold while the
    /// event is being processed inside `MpvHandle::wait_event`.
    pub(crate) fn from_raw(raw: &sys::mpv_event) -> Option<Event> {
        let id = raw.event_id;
        let data = raw.data as *const c_void;
        match id {
            sys::mpv_event_id_MPV_EVENT_START_FILE => Some(Event::StartFile),
            sys::mpv_event_id_MPV_EVENT_FILE_LOADED => Some(Event::FileLoaded),
            sys::mpv_event_id_MPV_EVENT_END_FILE => {
                let reason = if data.is_null() {
                    EndFileReason::Unknown
                } else {
                    let ed = unsafe { &*(data as *const sys::mpv_event_end_file) };
                    let r = match ed.reason {
                        sys::mpv_end_file_reason_MPV_END_FILE_REASON_EOF => EndFileReason::Eof,
                        sys::mpv_end_file_reason_MPV_END_FILE_REASON_STOP => EndFileReason::Stop,
                        sys::mpv_end_file_reason_MPV_END_FILE_REASON_QUIT => EndFileReason::Quit,
                        sys::mpv_end_file_reason_MPV_END_FILE_REASON_ERROR => EndFileReason::Error,
                        sys::mpv_end_file_reason_MPV_END_FILE_REASON_REDIRECT => EndFileReason::Redirect,
                        _ => EndFileReason::Unknown,
                    };
                    if ed.error == 0 { r } else { EndFileReason::Error }
                };
                let err = if data.is_null() {
                    None
                } else {
                    let ed = unsafe { &*(data as *const sys::mpv_event_end_file) };
                    if ed.error == 0 { None } else { Some(ed.error) }
                };
                Some(Event::EndFile { reason, error: err })
            }
            sys::mpv_event_id_MPV_EVENT_LOG_MESSAGE => {
                if data.is_null() {
                    None
                } else {
                    let lm = unsafe { &*(data as *const sys::mpv_event_log_message) };
                    let prefix = unsafe { CStr::from_ptr(lm.prefix as *const c_char) }
                        .to_string_lossy()
                        .into_owned();
                    let level = unsafe { CStr::from_ptr(lm.level as *const c_char) }
                        .to_string_lossy()
                        .into_owned();
                    let text = unsafe { CStr::from_ptr(lm.text as *const c_char) }
                        .to_string_lossy()
                        .into_owned();
                    Some(Event::LogMessage { prefix, level, text })
                }
            }
            sys::mpv_event_id_MPV_EVENT_PROPERTY_CHANGE => {
                if data.is_null() {
                    None
                } else {
                    let pc = unsafe { &*(data as *const sys::mpv_event_property) };
                    let name = unsafe { CStr::from_ptr(pc.name as *const c_char) }
                        .to_string_lossy()
                        .into_owned();
                    let value = parse_property_value(pc.format, pc.data);
                    Some(Event::PropertyChange {
                        reply_userdata: raw.reply_userdata,
                        name,
                        value,
                    })
                }
            }
            sys::mpv_event_id_MPV_EVENT_SHUTDOWN => Some(Event::Shutdown),
            sys::mpv_event_id_MPV_EVENT_HOOK => {
                if data.is_null() {
                    None
                } else {
                    let hd = unsafe { &*(data as *const sys::mpv_event_hook) };
                    let name = unsafe { CStr::from_ptr(hd.name as *const c_char) }
                        .to_string_lossy()
                        .into_owned();
                    Some(Event::Hook { id: hd.id, name })
                }
            }
            sys::mpv_event_id_MPV_EVENT_NONE => None,
            other => Some(Event::Other { event_id: other }),
        }
    }
}

fn parse_property_value(format: sys::mpv_format, data: *const c_void) -> PropertyValue {
    use sys::*;
    if data.is_null() {
        return PropertyValue::None;
    }
    match format {
        mpv_format_MPV_FORMAT_FLAG => {
            let v = unsafe { *(data as *const i32) };
            PropertyValue::Flag(v != 0)
        }
        mpv_format_MPV_FORMAT_INT64 => {
            let v = unsafe { *(data as *const i64) };
            PropertyValue::Int64(v)
        }
        mpv_format_MPV_FORMAT_DOUBLE => {
            let v = unsafe { *(data as *const f64) };
            PropertyValue::Double(v)
        }
        mpv_format_MPV_FORMAT_STRING => {
            let p = data as *const c_char;
            if p.is_null() {
                PropertyValue::None
            } else {
                let s = unsafe { CStr::from_ptr(p) }
                    .to_string_lossy()
                    .into_owned();
                PropertyValue::String(s)
            }
        }
        _ => PropertyValue::None,
    }
}
