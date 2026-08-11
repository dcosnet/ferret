//! Commands sent from the UI thread to the engine thread.

use crate::error::CoreResult;
use mpv_bindings::command::{LoadMode, SeekFlags, SeekMode};
use serde::{Deserialize, Serialize};

// ---- Random / shuffle mode ----------------------------------------------

/// Describes how the random/shuffle feature selects the next file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RandomMode {
    /// Shuffle is disabled; normal sequential playback.
    Off,
    /// Randomly pick from media files in the **same directory** as the
    /// currently playing file.
    SameFolder,
    /// Randomly pick from media files in the **entire folder tree** rooted at
    /// the current file's parent directory (recursive descent).
    WholeTree,
    /// Two-level random: first uniformly pick a subdirectory that contains
    /// media files, then uniformly pick a file within it.  This avoids the
    /// VLC bug where selecting a subfolder always plays the same first file.
    StepAware,
}

impl RandomMode {
    /// Cycle through modes: Off → SameFolder → WholeTree → StepAware → Off.
    pub fn cycle(self) -> Self {
        const ORDER: [RandomMode; 4] = [
            RandomMode::Off,
            RandomMode::SameFolder,
            RandomMode::WholeTree,
            RandomMode::StepAware,
        ];
        let idx = ORDER
            .iter()
            .position(|m| *m == self)
            .expect("RandomMode is exhaustive over ORDER");
        ORDER[(idx + 1) % ORDER.len()]
    }

    /// Human-readable label for menus.
    pub fn label(self) -> &'static str {
        const TABLE: [(RandomMode, &str); 4] = [
            (RandomMode::Off,       "Random: Off"),
            (RandomMode::SameFolder, "Random: Same Folder"),
            (RandomMode::WholeTree,  "Random: Whole Tree"),
            (RandomMode::StepAware, "Random: Step-Aware"),
        ];
        TABLE
            .iter()
            .copied()
            .find(|(m, _)| *m == self)
            .map(|(_, l)| l)
            .expect("RandomMode is exhaustive over TABLE")
    }

    /// Short label for the status bar.
    pub fn short_label(self) -> &'static str {
        const TABLE: [(RandomMode, &str); 4] = [
            (RandomMode::Off,       "rand:off"),
            (RandomMode::SameFolder, "rand:folder"),
            (RandomMode::WholeTree,  "rand:tree"),
            (RandomMode::StepAware, "rand:step"),
        ];
        TABLE
            .iter()
            .copied()
            .find(|(m, _)| *m == self)
            .map(|(_, l)| l)
            .expect("RandomMode is exhaustive over TABLE")
    }
}

impl Default for RandomMode {
    fn default() -> Self {
        RandomMode::Off
    }
}

// ---- Magic dialog-request strings --------------------------------------------
//
// The overlay UI can't open native file dialogs itself (rfd blocks the event
// loop). Instead, it sends a Cmd with one of these magic path strings. The
// main app (player-app) intercepts them in render_overlay(), spawns an rfd
// worker thread, and forwards the result back as a real Cmd once the user
// picks a file.
//
// Defined here (not in player-app) so both player-ui and player-app can
// reference them without a circular dependency.

/// Sent by the UI to request a native "Load File" open dialog.
pub const MAGIC_FILE_DIALOG: &str = "__file_dialog__";
/// Sent by the UI to request a native "Load Folder" picker.
pub const MAGIC_FOLDER_DIALOG: &str = "__folder_dialog__";
/// Sent by the UI to request a native multi-select "Load Playlist" dialog.
pub const MAGIC_PLAYLIST_DIALOG: &str = "__playlist_dialog__";
/// Sent by the UI to request a native "Save Markers (txt)" dialog.
pub const MAGIC_SAVE_DIALOG_TXT: &str = "__save_dialog_txt__";
/// Sent by the UI to request a native "Save Markers (json)" dialog.
pub const MAGIC_SAVE_DIALOG_JSON: &str = "__save_dialog_json__";
/// Sent by the UI to request a native "Load Subtitle File" dialog.
pub const MAGIC_SUBTITLE_DIALOG: &str = "__subtitle_dialog__";
/// Sent by the UI to request a native "Import Markers" dialog.
pub const MAGIC_IMPORT_MARKERS_DIALOG: &str = "__import_markers_dialog__";
/// Sent by the UI to request a native "Export A-B Loop Video" save dialog.
/// The main app intercepts this, reads A/B markers from engine state, opens
/// a save dialog, then spawns ffmpeg to render the segment.
pub const MAGIC_EXPORT_VIDEO_DIALOG: &str = "__export_video_dialog__";

/// What the UI wants the engine to do.
///
/// Design rule: every variant is *non-blocking* — the engine thread should be
/// able to apply each one in O(1) FFI calls and immediately move on.
#[derive(Debug, Clone)]
pub enum Cmd {
    // ---- Existing / core playback --------------------------------------

    /// Load a file (or URL). Replaces current playback by default.
    LoadFile {
        path: String,
        options: LoadOptions,
    },

    /// Toggle pause.
    PlayPause,

    /// Set pause state explicitly (avoids a round-trip read).
    SetPaused(bool),

    /// Seek by an offset in seconds (relative) or to a position (absolute).
    Seek {
        target_secs: f64,
        mode: SeekMode,
        flags: SeekFlags,
    },

    /// Set volume. 0.0..=1.0 (mapped to 0..=100 in libmpv). NEVER > 1.0 —
    /// software amplification is disabled per the PipeWire clash lesson.
    SetVolume(f32),

    /// Adjust volume by a delta (e.g. wheel scroll). Clamped to 0..=1.0.
    AdjustVolume(f32),

    /// Toggle mute.
    ToggleMute,

    /// Set mute explicitly.
    SetMute(bool),

    /// Stop playback and clear the playlist.
    Stop,

    /// Step one frame forward (pauses automatically).
    FrameStep,

    /// Step one frame backward.
    FrameBackStep,

    /// Tell libmpv to render into a specific native window. Set `None` to detach.
    /// On X11 this is the XID; on Wayland this is a wl_surface* — passed as a
    /// string like "12345" for X11. The engine thread calls mpv_set_option_string
    /// ("wid", value) BEFORE initialize — so this only works pre-init.
    SetWindowId(String),

    /// Toggle fullscreen mode on the video window. Handled by the main app,
    /// not the engine — but we route it through the same channel so the UI
    /// can request it without needing a reference to the window.
    ToggleFullscreen,

    // ---- Loop / playlist -----------------------------------------------

    /// Set the loop mode for the current file. Off = play once, File = loop
    /// the current file forever, Playlist = loop the entire playlist.
    SetLoopMode(LoopMode),

    /// Skip to the next playlist entry. (mpv `playlist-next`.)
    PlaylistNext,

    /// Skip to the previous playlist entry. (mpv `playlist-prev`.)
    PlaylistPrev,

    // ---- Speed ---------------------------------------------------------

    /// Set playback speed. mpv range is 0.01..=100.0; we expose 0.25..=4.0
    /// from the UI and pass through.
    SetSpeed(f32),

    // ---- Audio track selection -----------------------------------------

    /// Select an audio track by mpv track id (1-based). Pass `None` to
    /// auto-select. (mpv `aid` property.)
    SetAudioTrack(Option<i64>),

    // ---- Subtitle track selection --------------------------------------

    /// Select a subtitle track by mpv track id (1-based). Pass `None` to
    /// disable subtitles. (mpv `sid` property.)
    SetSubtitleTrack(Option<i64>),

    /// Toggle subtitle visibility on/off. (mpv `sub-visibility` property.)
    /// Useful for hiding subs without losing the selected track.
    ToggleSubVisibility,

    /// Load an external subtitle file (e.g. .srt, .ass) and attach it to the
    /// current file. (mpv `sub-add` command.) Pass the absolute path.
    LoadSubtitleFile {
        path: String,
    },

    // ---- Video rotation / flip ----------------------------------------

    /// Set video rotation in degrees. Must be 0, 90, 180, or 270.
    /// (mpv `video-rotate` property.)
    SetVideoRotate(u16),

    /// Flip the video horizontally (mirror left-right). Toggles mpv's
    /// `vf` filter `hflip`. Pass `true` to enable, `false` to disable.
    SetVideoFlipH(bool),

    /// Flip the video vertically (upside-down). Toggles mpv's `vf` filter
    /// `vflip`. Pass `true` to enable, `false` to disable.
    SetVideoFlipV(bool),

    // ---- A/B markers ---------------------------------------------------

    /// Drop marker A at the current playback position (mpv `time-pos`).
    /// Stored locally + mirrored to mpv's `ab-loop-a` for visualisation.
    SetMarkerA,

    /// Drop marker B at the current playback position.
    SetMarkerB,

    /// Clear both A and B markers.
    ClearMarkers,

    /// Toggle A→B loop on/off. When on, mpv will loop between `ab-loop-a` and
    /// `ab-loop-b`. We cache the toggle state so the UI can show it.
    ToggleMarkerLoop,

    /// Export the current markers (A, B, plus file path/duration) to disk.
    /// The engine thread handles the file write so the UI doesn't block.
    /// Format is plain text by default; pass `MarkerExportFormat::Json` for
    /// structured output. The path is chosen by the UI (via rfd save dialog).
    ExportMarkers {
        path: String,
        format: MarkerExportFormat,
    },

    /// Import markers from an exported marker file. The engine thread parses
    /// the file and applies the A/B positions to mpv's `ab-loop-a` /
    /// `ab-loop-b` properties. Format is auto-detected from the file
    /// extension (.txt or .json).
    ImportMarkers {
        path: String,
    },

    /// Export the video segment between A and B markers to a new file. The
    /// main app intercepts the magic dialog string, reads A/B from engine
    /// state, opens a save dialog, then spawns ffmpeg to render the clip.
    /// This command never reaches the engine — it's handled entirely in
    /// the main app.
    ExportABLoopVideo {
        path: String,
    },

    // ---- Random / shuffle ------------------------------------------------

    /// Set the random/shuffle mode. Off disables it; other modes control how
    /// the next random file is selected (same folder, whole tree, step-aware).
    SetRandomMode(RandomMode),

    /// Pick a random file according to the current `random_mode` and load it.
    RandomNext,

    // ---- Lifecycle -----------------------------------------------------

    /// Shutdown libmpv and exit the engine thread.
    Shutdown,
}

#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Replace current playlist entry (default) or append.
    pub mode: LoadModeKind,
    /// Pause immediately after load (don't auto-play).
    pub pause: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadModeKind {
    Replace,
    Append,
    AppendPlay,
}

impl Default for LoadModeKind {
    fn default() -> Self { LoadModeKind::Replace }
}

impl LoadModeKind {
    /// Map to the underlying mpv load mode. Single source of truth: the
    /// table below mirrors the `LoadMode` declaration order 1:1.
    pub fn to_mpv(self) -> LoadMode {
        const TABLE: [(LoadModeKind, LoadMode); 3] = [
            (LoadModeKind::Replace, LoadMode::Replace),
            (LoadModeKind::Append, LoadMode::Append),
            (LoadModeKind::AppendPlay, LoadMode::AppendPlay),
        ];
        TABLE
            .iter()
            .copied()
            .find(|(k, _)| *k == self)
            .map(|(_, v)| v)
            .expect("LoadModeKind is exhaustive over TABLE")
    }
}

/// Loop policy for the current file / playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LoopMode {
    /// Play once and stop at EOF (or move to next playlist entry).
    #[default]
    Off,
    /// Repeat the current file indefinitely (mpv `loop-file=inf`).
    File,
    /// Repeat the entire playlist indefinitely (mpv `loop-playlist=inf`).
    Playlist,
}

impl LoopMode {
    /// Cycle to the next loop mode: Off → File → Playlist → Off. Table-driven
    /// so the cycle order lives in exactly one place.
    pub fn cycle(self) -> Self {
        const ORDER: [LoopMode; 3] = [
            LoopMode::Off,
            LoopMode::File,
            LoopMode::Playlist,
        ];
        let idx = ORDER
            .iter()
            .position(|m| *m == self)
            .expect("LoopMode is exhaustive over ORDER");
        ORDER[(idx + 1) % ORDER.len()]
    }

    pub fn label(self) -> &'static str {
        const TABLE: [(LoopMode, &str); 3] = [
            (LoopMode::Off, "Loop: Off"),
            (LoopMode::File, "Loop: File"),
            (LoopMode::Playlist, "Loop: List"),
        ];
        TABLE
            .iter()
            .copied()
            .find(|(m, _)| *m == self)
            .map(|(_, l)| l)
            .expect("LoopMode is exhaustive over TABLE")
    }
}

/// Marker export format. Plain text by default (grep-friendly), JSON for
/// feeding back into another tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerExportFormat {
    /// `A 00:01:23.456\nB 00:02:45.000\n...` — one marker per line.
    Text,
    /// `{ "file": "...", "duration": 180.0, "a": 83.456, "b": 165.0 }`
    Json,
}

impl MarkerExportFormat {
    pub fn extension(self) -> &'static str {
        const TABLE: [(MarkerExportFormat, &str); 2] = [
            (MarkerExportFormat::Text, "txt"),
            (MarkerExportFormat::Json, "json"),
        ];
        TABLE
            .iter()
            .copied()
            .find(|(f, _)| *f == self)
            .map(|(_, e)| e)
            .expect("MarkerExportFormat is exhaustive over TABLE")
    }
}

/// Convenience: build the mpv `loadfile` Command from a `Cmd::LoadFile`.
pub(crate) fn build_loadfile(path: &str, opts: &LoadOptions) -> CoreResult<mpv_bindings::command::Command> {
    let cmd = mpv_bindings::command::Command::loadfile(path, opts.mode.to_mpv())?;
    Ok(cmd)
}

/// Convenience: build the mpv `seek` Command.
pub(crate) fn build_seek(target: f64, mode: SeekMode, flags: SeekFlags) -> CoreResult<mpv_bindings::command::Command> {
    Ok(mpv_bindings::command::Command::seek(target, mode, flags)?)
}
