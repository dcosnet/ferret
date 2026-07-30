//! Headless video player engine.
//!
//! `PlayerEngine` owns a libmpv instance, runs it on a dedicated thread, and
//! exposes a request/response API to the UI via two channels:
//!
//!   - `Cmd` channel (UI → engine)  : commands like LoadFile, Seek, SetVolume
//!   - `Event` channel (engine → UI): state updates like TimePos, EndReached
//!
//! The engine thread does ONE thing: drain libmpv's event queue and translate
//! raw `mpv_event`s into typed `Event`s on the channel. Commands from the UI
//! are applied synchronously on the engine thread (libmpv is thread-safe but
//! we serialize to keep the model simple and the call stack debuggable).
//!
//! ## Accuracy-first error policy
//!
//! The engine configures libmpv for visual accuracy over continuity:
//!   - `hr-seek=yes`        : exact seeks, not keyframe-snapped
//!   - `vd-lavc-fast=no`    : full decode, no shortcuts
//!   - `framedrop=vo`       : only drop frames at the VO if behind, never on decode
//!   - `video-sync=display-resample` : resample audio to match display
//!
//! This matches mpv's defaults; we set them explicitly to prevent user
//! config (~/.config/mpv/mpv.conf) from sneaking in different behavior.

#![allow(dead_code)]

pub mod cmd;
pub mod engine;
pub mod event;
pub mod state;
pub mod error;
pub mod options;

pub use cmd::{
    Cmd, LoadOptions, LoopMode, MarkerExportFormat,
    MAGIC_FILE_DIALOG, MAGIC_FOLDER_DIALOG, MAGIC_PLAYLIST_DIALOG,
    MAGIC_SAVE_DIALOG_TXT, MAGIC_SAVE_DIALOG_JSON,
    MAGIC_SUBTITLE_DIALOG, MAGIC_IMPORT_MARKERS_DIALOG,
    MAGIC_EXPORT_VIDEO_DIALOG,
};
pub use engine::PlayerEngine;
pub use event::{EngineEvent, EngineEventSender};
pub use state::{AudioTrack, PlaybackState, PlayerStatus, Track};
pub use options::EngineOptions;
pub use error::{CoreError, CoreResult};
