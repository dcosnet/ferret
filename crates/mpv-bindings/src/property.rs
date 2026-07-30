//! Property get/set helpers.
//!
//! libmpv properties are typed values identified by string names like
//! `time-pos`, `duration`, `volume`, `pause`. We model the supported formats
//! as a `Format` enum and use `Property` as a typed bag that can carry any
//! of the value kinds we care about.

use crate::error::{MpvError, MpvResult};

/// The libmpv property formats we support. Mirrors `mpv_format` but trimmed
/// to what we actually use.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Format {
    String,
    Flag,
    Int64,
    Double,
    Node,
}

impl Format {
    /// Return the raw `mpv_format` value as the type alias emitted by bindgen
    /// (a `c_uint` / `u32`).
    pub fn as_mpv_format(self) -> crate::sys::mpv_format {
        match self {
            Format::String => crate::sys::mpv_format_MPV_FORMAT_STRING,
            Format::Flag => crate::sys::mpv_format_MPV_FORMAT_FLAG,
            Format::Int64 => crate::sys::mpv_format_MPV_FORMAT_INT64,
            Format::Double => crate::sys::mpv_format_MPV_FORMAT_DOUBLE,
            Format::Node => crate::sys::mpv_format_MPV_FORMAT_NODE,
        }
    }
}

/// A typed property value, paired with its name.
///
/// Construction is via the `Property::str()`, `flag()`, `int()`, `double()`
/// constructors. Reading happens through the `as_*` accessors.
pub struct Property {
    name: String,
    value: PropValue,
}

enum PropValue {
    Str(String),
    Flag(bool),
    Int(i64),
    Double(f64),
}

impl Property {
    pub fn str(name: impl Into<String>, v: impl Into<String>) -> Self {
        Self { name: name.into(), value: PropValue::Str(v.into()) }
    }
    pub fn flag(name: impl Into<String>, v: bool) -> Self {
        Self { name: name.into(), value: PropValue::Flag(v) }
    }
    pub fn int(name: impl Into<String>, v: i64) -> Self {
        Self { name: name.into(), value: PropValue::Int(v) }
    }
    pub fn double(name: impl Into<String>, v: f64) -> Self {
        Self { name: name.into(), value: PropValue::Double(v) }
    }

    pub fn name(&self) -> &str { &self.name }
    pub fn format(&self) -> Format {
        match self.value {
            PropValue::Str(_) => Format::String,
            PropValue::Flag(_) => Format::Flag,
            PropValue::Int(_) => Format::Int64,
            PropValue::Double(_) => Format::Double,
        }
    }

    pub fn as_str(&self) -> MpvResult<&str> {
        match &self.value {
            PropValue::Str(s) => Ok(s),
            _ => Err(MpvError::PropertyFormat),
        }
    }
    pub fn as_flag(&self) -> MpvResult<bool> {
        match self.value {
            PropValue::Flag(b) => Ok(b),
            _ => Err(MpvError::PropertyFormat),
        }
    }
    pub fn as_i64(&self) -> MpvResult<i64> {
        match self.value {
            PropValue::Int(i) => Ok(i),
            _ => Err(MpvError::PropertyFormat),
        }
    }
    pub fn as_f64(&self) -> MpvResult<f64> {
        match self.value {
            PropValue::Double(d) => Ok(d),
            _ => Err(MpvError::PropertyFormat),
        }
    }
}
