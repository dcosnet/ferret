//! VLC-inspired visual theme.
//!
//! VLC's classic skin uses a dark gray gradient bar with orange highlights
//! for the active/progress state. We approximate that with flat colors
//! (no gradient support in egui without custom shaders).

#[derive(Copy, Clone, Debug)]
pub struct Theme {
    /// Background of the control bar (very dark gray, opaque).
    pub bg: [u8; 4],
    /// Slightly lighter bg used for button "trays" / separators.
    bg_panel: [u8; 4],
    /// Foreground text / icons (near-white).
    pub fg: [u8; 4],
    /// Foreground for disabled / inactive controls.
    pub fg_dim: [u8; 4],
    /// VLC orange — used for the seek bar progress fill, slider thumb, and
    /// active button states.
    pub accent: [u8; 4],
    /// Accent when hovered (slightly brighter).
    pub accent_hover: [u8; 4],
    /// Background of sliders / progress tracks (mid gray).
    pub track: [u8; 4],
    /// Buffered-range fill (dim orange).
    pub buffered: [u8; 4],
    /// Button background (normal state).
    pub button_bg: [u8; 4],
    /// Button background (hover state).
    pub button_bg_hover: [u8; 4],
}

impl Default for Theme {
    fn default() -> Self {
        Self::vlc_dark()
    }
}

impl Theme {
    /// VLC classic dark — black/charcoal bar with orange accents.
    pub fn vlc_dark() -> Self {
        Self {
            bg:              [20, 20, 22, 255],     // #141416 — near-black
            bg_panel:        [32, 32, 36, 255],     // #202024
            fg:              [232, 232, 236, 255],  // #e8e8ec
            fg_dim:          [128, 128, 132, 255],  // #808084
            accent:          [255, 136, 0, 255],    // #ff8800 — VLC orange
            accent_hover:    [255, 168, 40, 255],   // brighter on hover
            track:           [56, 56, 62, 255],     // #38383e
            buffered:        [120, 64, 0, 200],     // dim orange
            button_bg:       [44, 44, 48, 255],     // #2c2c30
            button_bg_hover: [64, 64, 70, 255],     // #404046
        }
    }

    /// VLC alternative — slightly lighter, more modern.
    pub fn vlc_modern() -> Self {
        Self {
            bg:              [28, 28, 32, 245],
            bg_panel:        [40, 40, 46, 255],
            fg:              [240, 240, 244, 255],
            fg_dim:          [140, 140, 148, 255],
            accent:          [255, 152, 0, 255],
            accent_hover:    [255, 183, 60, 255],
            track:           [60, 60, 68, 255],
            buffered:        [120, 72, 0, 200],
            button_bg:       [48, 48, 56, 255],
            button_bg_hover: [72, 72, 80, 255],
        }
    }

    // RGBA conversion helpers (egui uses 0..=1 floats).
    pub fn bg(self) -> [f32; 4] { rgba(self.bg) }
    pub fn bg_panel(self) -> [f32; 4] { rgba(self.bg_panel) }
    pub fn fg(self) -> [f32; 4] { rgba(self.fg) }
    pub fn fg_dim(self) -> [f32; 4] { rgba(self.fg_dim) }
    pub fn accent(self) -> [f32; 4] { rgba(self.accent) }
    pub fn accent_hover(self) -> [f32; 4] { rgba(self.accent_hover) }
    pub fn track(self) -> [f32; 4] { rgba(self.track) }
    pub fn buffered(self) -> [f32; 4] { rgba(self.buffered) }
    pub fn button_bg(self) -> [f32; 4] { rgba(self.button_bg) }
    pub fn button_bg_hover(self) -> [f32; 4] { rgba(self.button_bg_hover) }

    pub fn bg_color32(self) -> egui::Color32 { color32(self.bg) }
    pub fn bg_panel_color32(self) -> egui::Color32 { color32(self.bg_panel) }
    pub fn fg_color32(self) -> egui::Color32 { color32(self.fg) }
    pub fn fg_dim_color32(self) -> egui::Color32 { color32(self.fg_dim) }
    pub fn accent_color32(self) -> egui::Color32 { color32(self.accent) }
    pub fn accent_hover_color32(self) -> egui::Color32 { color32(self.accent_hover) }
    pub fn track_color32(self) -> egui::Color32 { color32(self.track) }
    pub fn buffered_color32(self) -> egui::Color32 { color32(self.buffered) }
    pub fn button_bg_color32(self) -> egui::Color32 { color32(self.button_bg) }
    pub fn button_bg_hover_color32(self) -> egui::Color32 { color32(self.button_bg_hover) }
}

fn rgba(c: [u8; 4]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
}

fn color32(c: [u8; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}
