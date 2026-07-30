use serde::{Deserialize, Serialize};

use crate::cmd::LoopMode;

/// Engine-level configuration (translates to libmpv options at init time).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineOptions {
    /// Enable high-precision seeking (mpv `hr-seek`).
    /// True = accuracy-first (mpv default). False = keyframe-snapped (faster but imprecise).
    pub hr_seek: bool,

    /// Framedrop policy: "never" | "vo" | "decoder" | "decoder+vo".
    /// "vo" = drop only at display, never on decode. Accuracy-first.
    pub framedrop: String,

    /// Video sync mode: "audio" | "display-resample" | "display-resample-desync" | ...
    /// "display-resample" = resample audio to match display (best A/V sync).
    pub video_sync: String,

    /// Hardware decoding: "no" | "auto" | "auto-safe" | "auto-copy".
    /// "auto-safe" = use hwdec only when known-safe (no copy-back issues).
    pub hwdec: String,

    /// User-configured volume at startup (0.0..=1.0; libmpv uses 0..=100 internally).
    pub initial_volume: f32,

    /// Maximum amplification soft-cap. We DO NOT allow >1.0 (the PipeWire clash
    /// lesson from the research). 1.0 == 100%.
    pub volume_max: f32,

    /// Default log level for libmpv's own diagnostics.
    pub log_level: String,

    /// Native window handle for libmpv to render into (X11 XID as a string,
    /// e.g. "12345678"). None = libmpv creates its own window.
    /// MUST be set before `mpv_initialize` (i.e. before `engine.start()`).
    pub wid: Option<String>,

    /// Video output driver. `"gpu"` for embedded rendering via `wid`.
    /// `"libmpv"` selects the render-context API (target: Wayland support).
    pub vo: String,

    /// Initial loop mode. Off by default. Set to File/Playlist at construction
    /// if you want looping on startup. Can be changed at runtime via Cmd::SetLoopMode.
    pub loop_mode: LoopMode,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            hr_seek: true,
            framedrop: "vo".to_string(),
            video_sync: "display-resample".to_string(),
            hwdec: "auto-safe".to_string(),
            initial_volume: 1.0,
            volume_max: 1.0,
            log_level: "warn".to_string(),
            wid: None,
            vo: "gpu".to_string(),
            loop_mode: LoopMode::Off,
        }
    }
}

impl EngineOptions {
    /// Translate options to `(name, value)` pairs for `mpv_set_option_string`.
    pub fn to_mpv_options(&self) -> Vec<(&'static str, String)> {
        let mut v: Vec<(&'static str, String)> = Vec::new();
        v.push(("hr-seek", if self.hr_seek { "yes".into() } else { "no".into() }));
        v.push(("framedrop", self.framedrop.clone()));
        v.push(("video-sync", self.video_sync.clone()));
        v.push(("hwdec", self.hwdec.clone()));
        v.push(("volume", format!("{}", (self.initial_volume * 100.0).clamp(0.0, 100.0))));
        v.push(("volume-max", format!("{}", (self.volume_max * 100.0).clamp(0.0, 100.0))));
        v.push(("terminal", "no".into()));           // we route logs via events, not stdout
        // mpv's msg-level parser requires `module=level` form. A bare level
        // (e.g. "warn") was accepted by older mpv builds but is rejected by
        // libmpv 2.x with MPV_ERROR_OPTION_ERROR, which aborts engine init.
        // Use `all=<level>` to set the global default for every module.
        v.push(("msg-level", format!("all={}", self.log_level)));
        v.push(("config", "no".into()));              // don't read ~/.config/mpv/mpv.conf
        v.push(("input-default-bindings", "no".into())); // we own the input
        v.push(("input-builtin-bindings", "no".into()));
        v.push(("osc", "no".into()));                  // we render our own UI
        v.push(("cursor-autohide", "no".into()));     // we handle cursor hiding in UI
        v.push(("input-vo-keyboard", "no".into()));   // we feed keys ourselves
        v.push(("vo", self.vo.clone()));
        // Initial loop mode. `loop-file=inf` is the canonical form. We push
        // both `loop-file` and `loop-playlist`; mpv honors each at its own
        // scope (current file vs entire playlist). Table-driven so the three
        // modes share one source of truth.
        const LOOP_TABLE: [(LoopMode, &str, &str); 3] = [
            (LoopMode::Off, "no", "no"),
            (LoopMode::File, "inf", "no"),
            (LoopMode::Playlist, "no", "inf"),
        ];
        let (_, file_v, list_v) = LOOP_TABLE
            .iter()
            .copied()
            .find(|(m, _, _)| *m == self.loop_mode)
            .expect("LoopMode is exhaustive over LOOP_TABLE");
        v.push(("loop-file", file_v.into()));
        v.push(("loop-playlist", list_v.into()));
        if let Some(wid) = &self.wid {
            v.push(("wid", wid.clone()));
        }
        v
    }
}
