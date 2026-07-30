//! Typed command builder for libmpv.
//!
//! libmpv commands take an argv-style array of strings, terminated by NULL.
//! Example: `["loadfile", "/path/to/video.mp4", "replace"]`

use std::ffi::CString;
use std::os::raw::c_char;
use std::ptr;

use crate::error::MpvResult;

/// A command to be sent to libmpv.
pub struct Command {
    // We hold the CStrings so the pointers stay valid for the duration of the
    // mpv_command call.
    args: Vec<CString>,
}

impl Command {
    pub fn new() -> Self {
        Self { args: Vec::new() }
    }

    /// Add a string argument.
    pub fn arg(mut self, s: impl Into<String>) -> MpvResult<Self> {
        self.args.push(CString::new(s.into())?);
        Ok(self)
    }

    /// Build the argv array with a trailing NULL sentinel, as expected by
    /// `mpv_command`. The returned vector's pointers are valid only while
    /// `self` is alive.
    pub(crate) fn argv_with_null(&self) -> Vec<*const c_char> {
        let mut v: Vec<*const c_char> = self.args.iter().map(|s| s.as_ptr() as *const c_char).collect();
        v.push(ptr::null());
        v
    }
}

impl Default for Command {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience constructors for the commands we use most.
impl Command {
    /// `loadfile <path> [replace|append]`
    pub fn loadfile(path: impl Into<String>, mode: LoadMode) -> MpvResult<Self> {
        Command::new()
            .arg("loadfile")?
            .arg(path)?
            .arg(match mode {
                LoadMode::Replace => "replace",
                LoadMode::Append => "append",
                LoadMode::AppendPlay => "append-play",
            })
    }

    /// `seek <target> [relative|absolute|relative-percent|absolute-percent] [default|exact|keyframes]`
    pub fn seek(target_secs: f64, mode: SeekMode, flags: SeekFlags) -> MpvResult<Self> {
        Command::new()
            .arg("seek")?
            .arg(format!("{target_secs}"))?
            .arg(match mode {
                SeekMode::Relative => "relative",
                SeekMode::Absolute => "absolute",
                SeekMode::RelativePercent => "relative-percent",
                SeekMode::AbsolutePercent => "absolute-percent",
            })?
            .arg(match flags {
                SeekFlags::Default => "default",
                SeekFlags::Exact => "exact",
                SeekFlags::Keyframes => "keyframes",
            })
    }

    /// Stop playback and clear the playlist.
    pub fn stop() -> Self {
        // stop has no args that we care about for MVP
        let mut c = Command::new();
        c.args.push(CString::new("stop").unwrap());
        c
    }

    /// Frame-step forward.
    pub fn frame_step() -> Self {
        let mut c = Command::new();
        c.args.push(CString::new("frame-step").unwrap());
        c
    }

    /// Frame-step backward.
    pub fn frame_back_step() -> Self {
        let mut c = Command::new();
        c.args.push(CString::new("frame-back-step").unwrap());
        c
    }
}

#[derive(Copy, Clone, Debug)]
pub enum LoadMode { Replace, Append, AppendPlay }

#[derive(Copy, Clone, Debug)]
pub enum SeekMode { Relative, Absolute, RelativePercent, AbsolutePercent }

#[derive(Copy, Clone, Debug)]
pub enum SeekFlags { Default, Exact, Keyframes }
