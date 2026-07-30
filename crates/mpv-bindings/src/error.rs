//! Error type for libmpv FFI calls.

use thiserror::Error;

/// A libmpv error code, mapped from `mpv_error` integers.
///
/// See: <https://github.com/mpv-player/mpv/blob/master/libmpv/client.h>
#[derive(Debug, Error)]
pub enum MpvError {
    #[error("mpv: event queue full")]
    EventQueueFull,
    #[error("mpv: memory allocation failed")]
    NoMem,
    #[error("mpv: uninitialized")]
    Uninitialized,
    #[error("mpv: invalid parameter")]
    InvalidParameter,
    #[error("mpv: option not found")]
    OptionNotFound,
    #[error("mpv: option format mismatch")]
    OptionFormat,
    #[error("mpv: option error")]
    OptionError,
    #[error("mpv: property not found")]
    PropertyNotFound,
    #[error("mpv: property format mismatch")]
    PropertyFormat,
    #[error("mpv: property unavailable")]
    PropertyUnavailable,
    #[error("mpv: property exists (read-only, cannot set)")]
    PropertyReadOnly,
    #[error("mpv: property error: {0}")]
    PropertyError(i32),
    #[error("mpv: command failed: {0}")]
    CommandError(i32),
    #[error("mpv: loading failed")]
    LoadingFailed,
    #[error("mpv: AO init failed")]
    AoInitFailed,
    #[error("mpv: VO init failed")]
    VoInitFailed,
    #[error("mpv: nothing to play")]
    NothingToPlay,
    #[error("mpv: unknown format")]
    UnknownFormat,
    #[error("mpv: unsupported")]
    Unsupported,
    #[error("mpv: not implemented")]
    NotImplemented,
    #[error("mpv: generic error code: {0}")]
    Generic(i32),
    #[error("mpv: null pointer returned")]
    NullPointer,
    #[error("mpv: nul byte in string")]
    NulByte(#[from] std::ffi::NulError),
    #[error("mpv: utf-8 conversion failed")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("mpv: handle was already terminated")]
    Terminated,
}

impl MpvError {
    /// Map a libmpv return code to `MpvResult<()>`.
    /// `0` (MPV_ERROR_SUCCESS) becomes `Ok(())`; anything else is an `Err`.
    pub fn from_code(code: i32) -> MpvResult<()> {
        use sys::*;
        match code {
            mpv_error_MPV_ERROR_SUCCESS => Ok(()),
            mpv_error_MPV_ERROR_EVENT_QUEUE_FULL => Err(MpvError::EventQueueFull),
            mpv_error_MPV_ERROR_NOMEM => Err(MpvError::NoMem),
            mpv_error_MPV_ERROR_UNINITIALIZED => Err(MpvError::Uninitialized),
            mpv_error_MPV_ERROR_INVALID_PARAMETER => Err(MpvError::InvalidParameter),
            mpv_error_MPV_ERROR_OPTION_NOT_FOUND => Err(MpvError::OptionNotFound),
            mpv_error_MPV_ERROR_OPTION_FORMAT => Err(MpvError::OptionFormat),
            mpv_error_MPV_ERROR_OPTION_ERROR => Err(MpvError::OptionError),
            mpv_error_MPV_ERROR_PROPERTY_NOT_FOUND => Err(MpvError::PropertyNotFound),
            mpv_error_MPV_ERROR_PROPERTY_FORMAT => Err(MpvError::PropertyFormat),
            mpv_error_MPV_ERROR_PROPERTY_UNAVAILABLE => Err(MpvError::PropertyUnavailable),
            mpv_error_MPV_ERROR_PROPERTY_ERROR => Err(MpvError::PropertyError(code)),
            mpv_error_MPV_ERROR_COMMAND => Err(MpvError::CommandError(code)),
            mpv_error_MPV_ERROR_LOADING_FAILED => Err(MpvError::LoadingFailed),
            mpv_error_MPV_ERROR_AO_INIT_FAILED => Err(MpvError::AoInitFailed),
            mpv_error_MPV_ERROR_VO_INIT_FAILED => Err(MpvError::VoInitFailed),
            mpv_error_MPV_ERROR_NOTHING_TO_PLAY => Err(MpvError::NothingToPlay),
            mpv_error_MPV_ERROR_UNKNOWN_FORMAT => Err(MpvError::UnknownFormat),
            mpv_error_MPV_ERROR_UNSUPPORTED => Err(MpvError::Unsupported),
            mpv_error_MPV_ERROR_NOT_IMPLEMENTED => Err(MpvError::NotImplemented),
            mpv_error_MPV_ERROR_GENERIC => Err(MpvError::Generic(code)),
            _ => Err(MpvError::Generic(code)),
        }
    }
}

pub type MpvResult<T> = Result<T, MpvError>;

use crate::sys;
