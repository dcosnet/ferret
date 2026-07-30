//! State snapshots published by the engine thread.

use serde::{Serialize, Serializer};

use crate::cmd::LoopMode;

/// One mpv track-list entry. Used for both audio and subtitle tracks — they
/// have the same shape, just different `type` values ("audio" vs "sub").
#[derive(Debug, Clone)]
pub struct Track {
    /// mpv track id (1-based). Pass this back via `Cmd::SetAudioTrack` /
    /// `Cmd::SetSubtitleTrack`.
    pub id: i64,
    /// Track type — "audio" or "sub". (We keep this so a single Vec<Track>
    /// could be used if needed, though we currently split them.)
    #[allow(dead_code)]
    pub kind: String,
    /// Track title if present (e.g. "Director's Commentary", "Forced").
    pub title: Option<String>,
    /// Language code if present (e.g. "eng", "spa").
    pub lang: Option<String>,
    /// Is this the currently-selected track?
    pub selected: bool,
    /// Is this the default track?
    pub default: bool,
    /// For subtitle tracks: is this a forced/forced-only track? (mpv
    /// `track-list/N/forced`.) Audio tracks always set this to false.
    pub forced: bool,
}

impl Track {
    /// Short label for the dropdown: "1: eng [default]" or "2: Commentary".
    pub fn label(&self) -> String {
        let mut s = format!("{}", self.id);
        if let Some(lang) = &self.lang {
            s.push_str(&format!(": {lang}"));
        } else if let Some(title) = &self.title {
            s.push_str(&format!(": {title}"));
        } else {
            s.push_str(": (untitled)");
        }
        if self.forced {
            s.push_str(" [forced]");
        }
        if self.default {
            s.push_str(" [default]");
        }
        s
    }
}

/// Convenience alias for callers that consume only audio tracks. `Track`
/// models both audio and subtitle tracks via the `kind` field.
pub type AudioTrack = Track;

/// Snapshot of playback state, sent whenever something changes.
#[derive(Debug, Clone, Default)]
pub struct PlaybackState {
    /// Current position in seconds. None if no file loaded.
    pub time_pos: Option<f64>,
    /// Total duration in seconds. None if unknown.
    pub duration: Option<f64>,
    /// Is playback currently paused?
    pub paused: bool,
    /// Volume 0..=1 (clamped). Mapped 1:1 with libmpv's 0..=100.
    pub volume: f32,
    /// Muted?
    pub muted: bool,
    /// Path of the currently-loaded file (set on FileLoaded).
    pub path: Option<String>,
    /// Title metadata if available.
    pub title: Option<String>,
    /// Playback speed multiplier (1.0 = normal).
    pub speed: f32,
    /// Current loop mode. Mirrors the engine's last `SetLoopMode` cmd.
    pub loop_mode: LoopMode,
    /// Available audio tracks. Empty until `track-list/count` is observed.
    pub audio_tracks: Vec<Track>,
    /// id of the currently-selected audio track, or None if auto.
    pub current_audio_track: Option<i64>,
    /// Available subtitle tracks. Empty until `track-list/count` is observed.
    pub subtitle_tracks: Vec<Track>,
    /// id of the currently-selected subtitle track, or None if no subs.
    pub current_subtitle_track: Option<i64>,
    /// Are subtitles currently visible? (mpv `sub-visibility`.)
    pub sub_visibility: bool,

    /// Video rotation in degrees (0, 90, 180, 270). Mirrors mpv's
    /// `video-rotate` property.
    pub video_rotate: u16,
    /// Is horizontal flip (mirror) enabled? Tracked locally — mpv's vf
    /// list is harder to read back reliably.
    pub video_flip_h: bool,
    /// Is vertical flip (upside-down) enabled? Tracked locally.
    pub video_flip_v: bool,

    /// A/B marker positions in seconds. None = not set.
    /// Mirrored to mpv's `ab-loop-a` / `ab-loop-b` so mpv itself can drive
    /// the looping; we cache them here for UI rendering.
    pub marker_a: Option<f64>,
    pub marker_b: Option<f64>,
    /// Is A→B loop currently enabled?
    pub marker_loop_enabled: bool,
}

// Serialize Track for the JSON marker export. We do it manually so the
// output stays stable across versions.
impl Serialize for Track {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = ser.serialize_struct("Track", 7)?;
        s.serialize_field("id", &self.id)?;
        s.serialize_field("type", &self.kind)?;
        s.serialize_field("title", &self.title)?;
        s.serialize_field("lang", &self.lang)?;
        s.serialize_field("selected", &self.selected)?;
        s.serialize_field("default", &self.default)?;
        s.serialize_field("forced", &self.forced)?;
        s.end()
    }
}

impl PlaybackState {
    /// Position as a 0..=1 fraction of duration, if both are known.
    pub fn progress(&self) -> Option<f32> {
        self.fraction_of_duration(self.time_pos)
    }

    /// Position of marker A as a 0..=1 fraction of duration, if known.
    pub fn marker_a_frac(&self) -> Option<f32> {
        self.fraction_of_duration(self.marker_a)
    }

    /// Position of marker B as a 0..=1 fraction of duration, if known.
    pub fn marker_b_frac(&self) -> Option<f32> {
        self.fraction_of_duration(self.marker_b)
    }

    /// Shared helper: project a timestamp onto the [0,1] span of `duration`.
    /// Returns `None` when either operand is missing or duration is non-positive.
    fn fraction_of_duration(&self, t: Option<f64>) -> Option<f32> {
        match (t, self.duration) {
            (Some(t), Some(d)) if d > 0.0 => Some((t / d) as f32),
            _ => None,
        }
    }

    /// Pretty-print position as "MM:SS / MM:SS".
    pub fn time_str(&self) -> String {
        let fmt = |s: f64| {
            let s = s.max(0.0) as u64;
            let h = s / 3600;
            let m = (s % 3600) / 60;
            let sec = s % 60;
            if h > 0 {
                format!("{h}:{m:02}:{sec:02}")
            } else {
                format!("{m:02}:{sec:02}")
            }
        };
        let pos = self.time_pos.map(fmt).unwrap_or_else(|| "00:00".into());
        let dur = self.duration.map(fmt).unwrap_or_else(|| "--:--".into());
        format!("{pos} / {dur}")
    }

    pub fn pos_duration(&self) -> (Option<std::time::Duration>, Option<std::time::Duration>) {
        let p = self.time_pos.map(|t| std::time::Duration::from_secs_f64(t.max(0.0)));
        let d = self.duration.map(|t| std::time::Duration::from_secs_f64(t.max(0.0)));
        (p, d)
    }
}

/// High-level player status (state machine position).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerStatus {
    /// Engine constructed but no file loaded.
    Idle,
    /// File loaded and playing.
    Playing,
    /// File loaded but paused.
    Paused,
    /// Playback ended (reached EOF or was stopped).
    Ended,
    /// Engine has shut down.
    Stopped,
}

impl Default for PlayerStatus {
    fn default() -> Self { PlayerStatus::Idle }
}
