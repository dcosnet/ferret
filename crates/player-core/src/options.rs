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

    /// Video output driver(s). A comma-separated priority list: mpv tries
    /// each in order, falling back to the next if one fails to initialize.
    /// Default `"gpu,xv,x11"` — `gpu` for the normal GL path, then Xv, and
    /// finally the software `x11` driver which works on ANY X server. This
    /// means a broken GL stack degrades to slow-but-visible video instead
    /// of a silent black window. `"libmpv"` selects the render-context API
    /// (target: Wayland support).
    pub vo: String,

    /// Initial loop mode. Off by default. Set to File/Playlist at construction
    /// if you want looping on startup. Can be changed at runtime via Cmd::SetLoopMode.
    pub loop_mode: LoopMode,

    /// Preferred audio languages for track auto-selection (mpv `alang`), as a
    /// comma-separated list of ISO language codes (e.g. "en", "en,fr").
    /// When set, mpv prefers these languages over the container's
    /// "default"-flagged track — the default comes from the system locale
    /// environment instead of the video's own metadata. `None` leaves mpv's
    /// default behavior (container default flag first).
    pub alang: Option<String>,
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
            // VO fallback chain: GL first, Xv second, software X11 last.
            // See the field docs — this is what keeps video visible when
            // the GPU/GL path fails on exotic setups.
            vo: "gpu,xv,x11".to_string(),
            loop_mode: LoopMode::Off,
            // Default the audio language to the system environment's locale,
            // NOT the video container's "default" flag.
            alang: system_locale_alang(),
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
        // Audio language preference: system-locale derived (see
        // `system_locale_alang`). mpv falls back to the container default
        // when no track matches any listed language.
        if let Some(alang) = &self.alang {
            v.push(("alang", alang.clone()));
        }
        if let Some(wid) = &self.wid {
            v.push(("wid", wid.clone()));
        }
        v
    }
}

/// Derive an mpv `alang` value from the system locale environment.
///
/// Consults `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, and `LANG` (in that order)
/// and extracts the language codes from each entry ("en_US.UTF-8" → "en",
/// "fr_CA" → "fr", "C"/"POSIX" → ignored). `LANGUAGE` is a colon-separated
/// priority list on GNU systems, so every entry is kept in order — the
/// result is a comma-separated preference list for mpv.
///
/// Returns `None` when nothing usable is set (mpv then falls back to its own
/// defaults).
fn system_locale_alang() -> Option<String> {
    let mut langs: Vec<String> = Vec::new();

    // LANGUAGE is an ordered, colon-separated list (GNU gettext convention).
    if let Ok(v) = std::env::var("LANGUAGE") {
        for tag in v.split(':') {
            push_locale_tag(&mut langs, tag);
        }
    }
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            push_locale_tag(&mut langs, &v);
        }
    }

    if langs.is_empty() {
        None
    } else {
        Some(langs.join(","))
    }
}

/// Append one locale tag's language code to `langs` (deduped, lowercased).
/// Accepts "en_US.UTF-8", "fr_CA", "de_DE@euro", "en"; rejects "C",
/// "POSIX", and empty tags.
fn push_locale_tag(langs: &mut Vec<String>, tag: &str) {
    let tag = tag.trim();
    // Strip country / encoding suffixes: "en_US.UTF-8" → "en".
    let lang = tag.split(['_', '.', '@']).next().unwrap_or("");
    // Accept plausible ISO 639 codes (2-3 letters); reject "C", "POSIX".
    let plausible =
        (2..=3).contains(&lang.len()) && lang.chars().all(|c| c.is_ascii_alphabetic());
    if plausible && !langs.iter().any(|l| l.eq_ignore_ascii_case(lang)) {
        langs.push(lang.to_ascii_lowercase());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_common_locale_forms() {
        fn one(tag: &str) -> Option<String> {
            let mut v = Vec::new();
            super::push_locale_tag(&mut v, tag);
            v.into_iter().next()
        }
        assert_eq!(one("en_US.UTF-8").as_deref(), Some("en"));
        assert_eq!(one("fr_CA").as_deref(), Some("fr"));
        assert_eq!(one("de_DE@euro").as_deref(), Some("de"));
        assert_eq!(one("C").as_deref(), None);
        assert_eq!(one("POSIX").as_deref(), None);
        assert_eq!(one("").as_deref(), None);
        assert_eq!(one("en").as_deref(), Some("en"));
    }
}
