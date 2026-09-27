//! The overlay UI logic — VLC-styled.
//!
//! Layout (bottom of screen):
//!   ┌──────────────────────────────────────────────────────────────────┐
//!   │  [≡]                                                              │
//!   │  00:12  ████████●░░░░░░░░░░░░░░░░░░  01:30  ← seek bar w/ time    │
//!   │  ┌────┬────┬────┬────┬────┐         ┌─────────┐  ┌────────────┐  │
//!   │  │ ▶  │ ■  │ ⏮  │ ⏭  │ ↙  │ 00:12   │ 🔊━━━━○ │  │ ⛶ fullscr  │  │
//!   │  └────┴────┴────┴────┴────┘         └─────────┘  └────────────┘  │
//!   │  ┌────┬────┬────┬────┬────┬────┬────┐                            │
//!   │  │ A  │ B  │ AB │ ⟳  │ ½× │ 1× │ 2× │  Audio: [eng ▾]            │
//!   │  └────┴────┴────┴────┴────┴────┴────┘                            │
//!   └──────────────────────────────────────────────────────────────────┘
//!
//! - Top-left ≡ hamburger opens the File menu (Load File / Folder /
//!   Playlist, Save Playlist As..., Queue Sidebar, Quit).
//! - Seek bar with A/B marker pins rendered on top.
//! - Bottom row 1: transport | time | volume + fullscreen.
//! - Bottom row 2: A/B markers, AB-loop toggle, loop-mode toggle, speed
//!   presets + slider, audio track dropdown.
//! - The queue button (next to shuffle in the transport row) toggles a
//!   persistent right-hand sidebar listing the playlist in play order;
//!   entries can be reordered by drag-and-drop or by typing a new
//!   position number, played on click, removed, and the whole queue saved
//!   as an .m3u playlist. A-B markers live under Playback → Loop /
//!   A-B Markers.
//! - The video acts as a movable object on a canvas: drag it to pan,
//!   Ctrl+wheel to zoom (Video → Zoom / Pan for steps, values, reset).

use std::time::Instant;

use crossbeam_channel::Receiver;
use egui::{Color32, Context, Layout, Ui, Vec2};

use player_core::event::EngineEvent;
use player_core::state::PlaybackState;
use player_core::{Cmd, LoopMode, RandomMode};

use crate::icons;
use crate::theme::Theme;
use crate::widgets::seek_bar;

pub struct OverlayApp {
    pub state: PlaybackState,
    pub event_rx: Receiver<EngineEvent>,
    pub cmd_tx: crossbeam_channel::Sender<Cmd>,
    pub theme: Theme,
    /// Timestamp of the last user activity (mouse move/click or keypress).
    /// Drives auto-hide of the menu bar and control bar.
    pub last_mouse_move: Instant,
    pub auto_hide_secs: f64,
    pub visible: bool,
    pub seeking: bool,
    pub seek_drag_pos: f64,
    pub window_size: Vec2,
    pub error_msg: Option<String>,
    pub error_expiry: Option<Instant>,
    /// Is the cursor currently inside the overlay window?
    pub mouse_inside: bool,
    /// Latest pointer position in overlay coordinates (None = cursor left).
    pub pointer_pos: Option<egui::Pos2>,
    /// Are we in fullscreen mode? (Mirrored from main app.)
    pub fullscreen: bool,

    /// True when egui currently has a focused widget that wants keyboard
    /// input (e.g. the save-dialog filename `TextEdit`). While true, the
    /// main app routes keystrokes into egui instead of the global hotkeys —
    /// so typing "q" in a filename field doesn't quit.
    /// Updated by the renderer after each egui pass.
    pub ui_wants_keyboard: bool,

    /// True when a dropdown menu (File/Playback/...) or combo popup was open
    /// at the end of the previous frame. Acts as a sticky condition for
    /// visibility: an open menu must never auto-hide its own bar out from
    /// under the user.
    pub menu_popup_open: bool,

    /// Sticky info toast (e.g. "Markers exported to ...").
    pub info_msg: Option<String>,
    pub info_expiry: Option<Instant>,

    /// Local speed slider value while dragging — avoids fighting with the
    /// engine's property-change echo. Reset to None when not dragging.
    pub speed_drag: Option<f32>,

    /// Buffer of egui input events (mouse moved, clicked, etc.) accumulated
    /// since the last frame. The renderer drains this into RawInput.events
    /// at the start of each render. Without this, egui never sees mouse
    /// events and no buttons/sliders/dropdowns respond to clicks.
    pub pending_events: Vec<egui::Event>,

    /// Is the About panel visible? Toggled by the Help → About menu item.
    /// When true, a persistent panel renders in the overlay showing ferret
    /// version, author, website, and license info.
    pub about_visible: bool,

    /// Is the queue sidebar (right-hand panel) visible? Toggled by the
    /// File → Window menu, or the queue button in the control bar. The
    /// sidebar shows the mpv playlist in order and supports reordering by
    /// drag-and-drop or by typing a new position number, plus click-to-play
    /// and per-entry removal. Drawn like the About panel: persistent,
    /// independent of the auto-hiding bars.
    pub queue_visible: bool,

    /// Active position-edit in the queue sidebar: (row index, text buffer).
    /// While Some, that row's position number renders as a `TextEdit`;
    /// committing (Enter or focus lost) moves the entry to the typed
    /// 1-based position. None = no row is being edited.
    queue_edit: Option<(usize, String)>,
    /// One-frame flag: request keyboard focus for the queue position edit
    /// on the frame it opens (the TextEdit must exist before it can be
    /// focused, so the click that opens the edit can't focus it directly).
    queue_edit_focus: bool,
    /// Queue sidebar drag-and-drop state: index of the row being dragged.
    queue_drag_from: Option<usize>,
    /// Queue sidebar drag-and-drop state: index the drag hovers over
    /// (insert-before target). `Some(len)` = below the last row = append.
    queue_drag_to: Option<usize>,

    /// Active in-UI file dialog (if any). When `Some`, the dialog renders as
    /// a modal overlay covering the controls. Replaces external zenity/kdialog
    /// /rfd dialogs which popped under the overlay's AlwaysOnTop window.
    pub file_dialog: Option<crate::file_dialog::FileDialog>,

    /// When the current file finished loading (FileLoaded event). Used by
    /// the VO health check: if mpv still reports `vo-configured == false`
    /// a couple of seconds after load, the video output failed and the user
    /// gets a visible warning instead of a silent black window.
    file_loaded_at: Option<Instant>,
    /// One-shot guard so the VO-failure warning only fires once per file.
    vo_warned: bool,
}

impl OverlayApp {
    pub fn new(
        state: PlaybackState,
        event_rx: Receiver<EngineEvent>,
        cmd_tx: crossbeam_channel::Sender<Cmd>,
    ) -> Self {
        Self {
            state,
            event_rx,
            cmd_tx,
            theme: Theme::vlc_dark(),
            last_mouse_move: Instant::now(),
            auto_hide_secs: 3.0,
            visible: true,
            seeking: false,
            seek_drag_pos: 0.0,
            window_size: Vec2::new(1280.0, 720.0),
            error_msg: None,
            error_expiry: None,
            mouse_inside: false,
            pointer_pos: None,
            fullscreen: false,
            ui_wants_keyboard: false,
            menu_popup_open: false,
            info_msg: None,
            info_expiry: None,
            speed_drag: None,
            pending_events: Vec::new(),
            about_visible: false,
            queue_visible: false,
            queue_edit: None,
            queue_edit_focus: false,
            queue_drag_from: None,
            queue_drag_to: None,
            file_dialog: None,
            file_loaded_at: None,
            vo_warned: false,
        }
    }

    /// Push an egui input event (mouse move, click, etc.) into the buffer.
    /// Called by the main app's window event handler. The renderer drains
    /// these into RawInput at the start of each frame.
    pub fn push_event(&mut self, event: egui::Event) {
        self.pending_events.push(event);
    }

    /// Push several egui input events at once (e.g. a translated keypress).
    pub fn push_events(&mut self, events: impl IntoIterator<Item = egui::Event>) {
        self.pending_events.extend(events);
    }

    /// Drain all pending egui events. Called by the renderer before building
    /// RawInput for the next frame.
    pub fn drain_events(&mut self) -> Vec<egui::Event> {
        std::mem::take(&mut self.pending_events)
    }

    pub fn poll_events(&mut self) {
        while let Ok(ev) = self.event_rx.try_recv() {
            match ev {
                EngineEvent::Ready | EngineEvent::StartFile => {}
                EngineEvent::FileLoaded { path, title } => {
                    self.state.path = Some(path);
                    self.state.title = title;
                    self.file_loaded_at = Some(Instant::now());
                    self.vo_warned = false;
                }
                EngineEvent::StateChanged => {}
                EngineEvent::EndReached { reason: _ } => {
                    self.state.paused = true;
                    self.state.time_pos = None;
                }
                EngineEvent::Error { message } => {
                    self.error_msg = Some(message);
                    self.error_expiry = Some(Instant::now() + std::time::Duration::from_secs(5));
                    self.visible = true;
                    self.last_mouse_move = Instant::now();
                }
                EngineEvent::Log { .. } | EngineEvent::Shutdown => {}
            }
        }
        if let Some(exp) = self.error_expiry {
            if Instant::now() > exp {
                self.error_msg = None;
                self.error_expiry = None;
            }
        }
        if let Some(exp) = self.info_expiry {
            if Instant::now() > exp {
                self.info_msg = None;
                self.info_expiry = None;
            }
        }
    }

    pub fn update_state(&mut self, s: PlaybackState) {
        if !self.seeking {
            self.state = s;
        } else {
            // Preserve time_pos so the seek thumb doesn't snap back while dragging.
            let old_pos = self.state.time_pos;
            self.state = s;
            self.state.time_pos = old_pos;
        }
    }

    /// Record user activity (mouse move/click, keypress, widget hover).
    /// Resets the auto-hide timer and re-shows the menu bar + control bar.
    pub fn note_user_activity(&mut self) {
        self.last_mouse_move = Instant::now();
        self.visible = true;
    }

    pub fn set_mouse_inside(&mut self, inside: bool) {
        // Only record cursor presence. Do NOT reset the auto-hide timer
        // here: a cursor resting anywhere over the video is NOT activity.
        // The timer advances only from real input (see note_user_activity),
        // otherwise the bars would never hide while the mouse sits still
        // inside the window.
        self.mouse_inside = inside;
    }

    pub fn set_fullscreen(&mut self, fs: bool) {
        self.fullscreen = fs;
    }

    /// Recompute menu-bar/control-bar visibility. Hides both after
    /// `auto_hide_secs` without input, unless a sticky condition holds
    /// (open dropdown/dialog/panel/toast) or the cursor is resting on top
    /// of the currently-visible bars.
    pub fn compute_visibility(&mut self) -> bool {
        let sticky = self.error_msg.is_some()
            || self.info_msg.is_some()
            || self.file_dialog.is_some()
            || self.about_visible
            || self.menu_popup_open;
        if sticky {
            self.visible = true;
        } else if self.last_mouse_move.elapsed().as_secs_f64() > self.auto_hide_secs {
            // Keep the bars up while the cursor rests ON them (even in the
            // gaps between widgets) — but a cursor resting over the video
            // area lets them hide.
            let resting_on_ui = self.mouse_inside
                && self
                    .pointer_pos
                    .map(|p| Self::in_menu_strip(self.window_size, p) || Self::in_control_bar(self.window_size, p))
                    .unwrap_or(false);
            if !resting_on_ui {
                self.visible = false;
            }
        }
        self.visible
    }

    /// Pointer is within the top menu-bar strip.
    fn in_menu_strip(win: Vec2, p: egui::Pos2) -> bool {
        p.y <= 32.0 && p.x >= 0.0 && p.x <= win.x
    }

    /// Pointer is within the bottom control-bar zone (118px tall).
    fn in_control_bar(win: Vec2, p: egui::Pos2) -> bool {
        p.y >= win.y - 118.0 && p.y <= win.y
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Show a transient info toast (e.g. "Markers exported to ...").
    pub fn show_info(&mut self, msg: impl Into<String>) {
        self.info_msg = Some(msg.into());
        self.info_expiry = Some(Instant::now() + std::time::Duration::from_secs(4));
        self.visible = true;
        self.last_mouse_move = Instant::now();
    }

    pub fn draw(&mut self, ctx: &Context) {
        ctx.request_repaint_after(std::time::Duration::from_millis(33));

        // VO health check: a file is loaded but mpv never configured a
        // video output (bad GL context, driver issue, ...). Warn visibly —
        // a silent black window is the worst failure mode and historically
        // got reported as "no video output" with no hint of the cause.
        if self.state.path.is_some()
            && !self.state.vo_configured
            && !self.vo_warned
            && self.error_msg.is_none()
            && self.info_msg.is_none()
        {
            if let Some(t) = self.file_loaded_at {
                if t.elapsed() > std::time::Duration::from_secs(2) {
                    self.show_info(
                        "Video output failed to initialize (vo=gpu,xv,x11 all failed?) — \
                         check GL drivers / try software rendering",
                    );
                    self.vo_warned = true;
                }
            }
        }

        // The menu bar now auto-hides together with the control bar: it is
        // only drawn while the UI is "visible" (mouse moved within the last
        // `auto_hide_secs`, or a sticky condition holds — see
        // `compute_visibility`). Moving the mouse brings it back. A modal
        // file dialog replaces all other interaction while it is open.
        let menu_visible = self.visible || self.about_visible;
        if self.file_dialog.is_none() && menu_visible {
            self.draw_file_menu(ctx);
        } else {
            // No menus were drawn this frame — nothing can be open.
            self.menu_popup_open = false;
        }

        // If a file dialog is open, render it as a modal and process its
        // result. This replaces all external (zenity/kdialog/rfd) dialogs.
        if let Some(dialog) = &mut self.file_dialog {
            // Capture the kind BEFORE we clear the dialog — handle_file_dialog_result
            // needs it to know which Cmd to send. If we clear first, the kind is lost.
            let kind = dialog.kind.clone();
            let result = dialog.draw(ctx, &self.theme);
            if let Some(res) = result {
                self.file_dialog = None;
                self.handle_file_dialog_result(res, kind);
            }
            return;
        }

        let visible = self.visible;
        if !visible {
            // The About panel and queue sidebar are persistent panels, not
            // tied to the bars.
            if self.queue_visible {
                self.draw_queue_panel(ctx);
            }
            if self.about_visible {
                self.draw_about_panel(ctx);
            }
            return;
        }

        let win_w = self.window_size.x;
        let win_h = self.window_size.y;

        // ----- Title strip (top center, only when there's a file loaded) -----
        if let Some(title) = self.state.title.clone().or(self.state.path.clone()) {
            let title_short = if title.len() > 80 {
                format!("{}…", &title[..77])
            } else {
                title.clone()
            };
            let title_w = (title_short.len() as f32 * 7.5).min(win_w - 80.0).max(200.0);
            egui::Area::new(egui::Id::new("ferret_title_strip"))
                .fixed_pos(egui::pos2((win_w - title_w) / 2.0, 12.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let bg = self.theme.bg_color32();
                    let bg = egui::Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 200);
                    egui::Frame::none()
                        .fill(bg)
                        .rounding(6.0)
                        .inner_margin(egui::Margin::symmetric(12.0, 6.0))
                        .show(ui, |ui| {
                            ui.set_min_width(title_w);
                            ui.label(
                                egui::RichText::new(title_short)
                                    .color(self.theme.fg_color32())
                                    .size(12.0),
                            );
                        });
                });
        }

        // ----- Bottom control bar -----
        let bar_h = 118.0;
        let bar_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, win_h - bar_h),
            Vec2::new(win_w, bar_h),
        );

        egui::Area::new(egui::Id::new("ferret_overlay"))
            .fixed_pos(bar_rect.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let painter = ui.painter();
                // Semi-transparent fill — the video shows through the control
                // bar at all times while it is visible.
                let bg = self.theme.bg_color32();
                let bg = egui::Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 175);
                painter.rect_filled(bar_rect, 0.0, bg);

                let top_line = egui::Rect::from_min_size(
                    bar_rect.min,
                    Vec2::new(bar_rect.width(), 1.0),
                );
                painter.rect_filled(top_line, 0.0, self.theme.accent_color32());

                let inner = bar_rect.shrink2(Vec2::new(16.0, 8.0));
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(inner), |ui| {
                    self.draw_controls(ui);
                });
            });

        // ----- Error toast (top center) -----
        if let Some(msg) = self.error_msg.clone() {
            let toast_w: f32 = 600.0_f32.min(win_w - 40.0);
            egui::Area::new(egui::Id::new("ferret_error_toast"))
                .fixed_pos(egui::pos2((win_w - toast_w) / 2.0, 48.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(egui::Color32::from_rgb(120, 30, 30))
                        .rounding(6.0)
                        .show(ui, |ui| {
                            ui.set_min_width(toast_w);
                            ui.label(
                                egui::RichText::new(msg)
                                    .color(egui::Color32::WHITE)
                                    .size(13.0),
                            );
                        });
                });
        }

        // ----- Info toast (below error toast if any) -----
        if let Some(msg) = self.info_msg.clone() {
            let toast_w: f32 = 500.0_f32.min(win_w - 40.0);
            let y = if self.error_msg.is_some() { 88.0 } else { 48.0 };
            egui::Area::new(egui::Id::new("ferret_info_toast"))
                .fixed_pos(egui::pos2((win_w - toast_w) / 2.0, y))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(egui::Color32::from_rgb(20, 60, 30))
                        .rounding(6.0)
                        .show(ui, |ui| {
                            ui.set_min_width(toast_w);
                            ui.label(
                                egui::RichText::new(msg)
                                    .color(egui::Color32::WHITE)
                                    .size(12.0),
                            );
                        });
                });
        }

        // ----- Queue sidebar (persistent, independent of bar visibility) -----
        if self.queue_visible {
            self.draw_queue_panel(ctx);
        }

        // ----- About panel (persistent, independent of bar visibility) -----
        if self.about_visible {
            self.draw_about_panel(ctx);
        }

        // Track combo-box popups (audio track dropdown) for visibility
        // stickiness — the audio dropdown lives in the control bar.
        if self.visible && ctx.memory(|m| m.any_popup_open()) {
            self.menu_popup_open = true;
        }
    }

    /// Draw the menu bar (File / Playback / Audio / Subtitles / Video / Help)
    /// at the top-left corner. Auto-hides together with the control bar (see
    /// `compute_visibility`); an open dropdown keeps the whole UI visible.
    /// Uses egui's built-in `menu_button` which handles popup open/close,
    /// click-outside-to-dismiss, and Escape-to-close automatically.
    fn draw_file_menu(&mut self, ctx: &Context) {
        // Collect commands to send after the UI closure (can't borrow self
        // for send() while the closure also borrows self for theme/state).
        let mut pending_cmds: Vec<Cmd> = Vec::new();
        let mut pending_about_toggle = false;
        // Toggle the queue sidebar (File → Window). Applied after the pass.
        let mut pending_queue_toggle = false;
        // Collect file-dialog-open requests — can't open the dialog inside
        // the closure because it borrows self for the theme snapshot.
        let mut pending_dialog: Option<crate::file_dialog::FileDialogKind> = None;
        // True when any dropdown menu is open this frame — sticky condition
        // so the bar never auto-hides while the user is reading a menu.
        let mut menu_open = false;

        // Snapshot the theme colors we need so the closure doesn't borrow self.
        let fg = self.theme.fg_color32();
        let fg_dim = self.theme.fg_dim_color32();
        let bg = self.theme.bg_color32();
        let loop_mode = self.state.loop_mode;
        let speed = self.state.speed;
        let has_audio = !self.state.audio_tracks.is_empty();
        let sub_vis = self.state.sub_visibility;
        let video_rotate = self.state.video_rotate;
        let video_flip_h = self.state.video_flip_h;
        let video_flip_v = self.state.video_flip_v;
        let video_zoom = self.state.video_zoom;
        let video_pan_x = self.state.video_pan_x;
        let video_pan_y = self.state.video_pan_y;
        let about_visible = self.about_visible;
        let queue_visible = self.queue_visible;
        let marker_loop = self.state.marker_loop_enabled;

        egui::Area::new(egui::Id::new("ferret_menu_bar"))
            .fixed_pos(egui::pos2(4.0, 4.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                // Use a Frame for the background — it auto-sizes to the content
                // and paints the fill behind the widgets. (Painting manually
                // before the widgets doesn't work because ui.max_rect() is
                // empty at that point.)
                egui::Frame::none()
                    .fill(egui::Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 220))
                    .rounding(4.0)
                    .inner_margin(egui::Margin::symmetric(6.0, 3.0))
                    .show(ui, |ui| {
                        // Horizontal layout: menus sit side by side, left to right.
                        ui.horizontal(|ui| {
                            ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
                            ui.spacing_mut().item_spacing.x = 2.0;

                            // ---- File menu ----
                            let file_resp = ui.menu_button(
                                egui::RichText::new("File").color(fg).size(13.0),
                                |ui| {
                                    ui.set_min_width(200.0);
                                    ui.style_mut().visuals.override_text_color = Some(fg);

                                    ui.label(
                                        egui::RichText::new("Open")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);

                                    if ui.button("Load File...").clicked() {
                                        pending_dialog = Some(crate::file_dialog::FileDialogKind::LoadFile);
                                        ui.close_menu();
                                    }
                                    if ui.button("Load Folder...").clicked() {
                                        pending_dialog = Some(crate::file_dialog::FileDialogKind::LoadFolder);
                                        ui.close_menu();
                                    }
                                    if ui.button("Load Playlist...").clicked() {
                                        pending_dialog = Some(crate::file_dialog::FileDialogKind::LoadPlaylist);
                                        ui.close_menu();
                                    }
                                    if ui.button("Save Playlist As...").clicked() {
                                        pending_dialog = Some(crate::file_dialog::FileDialogKind::SavePlaylist);
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    ui.label(
                                        egui::RichText::new("Window")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);
                                    if ui.button("Toggle Fullscreen  (F)").clicked() {
                                        pending_cmds.push(Cmd::ToggleFullscreen);
                                        ui.close_menu();
                                    }
                                    if ui.selectable_label(queue_visible, "Queue Sidebar").clicked() {
                                        pending_queue_toggle = true;
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    if ui.button("Quit  (Q)").clicked() {
                                        pending_cmds.push(Cmd::Shutdown);
                                        ui.close_menu();
                                    }
                                },
                            );
                            if file_resp.inner.is_some() {
                                menu_open = true;
                            }

                            // ---- Playback menu ----
                            let playback_resp = ui.menu_button(
                                egui::RichText::new("Playback").color(fg).size(13.0),
                                |ui| {
                                    ui.set_min_width(220.0);
                                    ui.style_mut().visuals.override_text_color = Some(fg);

                                    if ui.button("Play / Pause  (Space)").clicked() {
                                        pending_cmds.push(Cmd::PlayPause);
                                        ui.close_menu();
                                    }
                                    if ui.button("Stop").clicked() {
                                        pending_cmds.push(Cmd::Stop);
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    ui.label(
                                        egui::RichText::new("Seek")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);
                                    if ui.button("Forward 5s  (→)").clicked() {
                                        pending_cmds.push(Cmd::Seek {
                                            target_secs: 5.0,
                                            mode: mpv_bindings::command::SeekMode::Relative,
                                            flags: mpv_bindings::command::SeekFlags::Keyframes,
                                        });
                                        ui.close_menu();
                                    }
                                    if ui.button("Backward 5s  (←)").clicked() {
                                        pending_cmds.push(Cmd::Seek {
                                            target_secs: -5.0,
                                            mode: mpv_bindings::command::SeekMode::Relative,
                                            flags: mpv_bindings::command::SeekFlags::Keyframes,
                                        });
                                        ui.close_menu();
                                    }
                                    if ui.button("Frame Step Forward  (.)").clicked() {
                                        pending_cmds.push(Cmd::FrameStep);
                                        ui.close_menu();
                                    }
                                    if ui.button("Frame Step Backward  (,)").clicked() {
                                        pending_cmds.push(Cmd::FrameBackStep);
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    ui.label(
                                        egui::RichText::new("Playlist")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);
                                    if ui.button("Next  (N)").clicked() {
                                        pending_cmds.push(Cmd::PlaylistNext);
                                        ui.close_menu();
                                    }
                                    if ui.button("Previous  (P)").clicked() {
                                        pending_cmds.push(Cmd::PlaylistPrev);
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    ui.label(
                                        egui::RichText::new("Volume")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);
                                    if ui.button("Volume Up 5%  (↑)").clicked() {
                                        pending_cmds.push(Cmd::AdjustVolume(0.05));
                                        ui.close_menu();
                                    }
                                    if ui.button("Volume Down 5%  (↓)").clicked() {
                                        pending_cmds.push(Cmd::AdjustVolume(-0.05));
                                        ui.close_menu();
                                    }
                                    if ui.button("Toggle Mute  (M)").clicked() {
                                        pending_cmds.push(Cmd::ToggleMute);
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    ui.label(
                                        egui::RichText::new("Loop")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);
                                    let modes = [
                                        ("Off", LoopMode::Off),
                                        ("Loop File", LoopMode::File),
                                        ("Loop Playlist", LoopMode::Playlist),
                                    ];
                                    for (label, mode) in modes {
                                        let checked = loop_mode == mode;
                                        if ui.selectable_label(checked, label).clicked() {
                                            pending_cmds.push(Cmd::SetLoopMode(mode));
                                            ui.close_menu();
                                        }
                                    }
                                    if ui.button("Cycle Loop Mode  (L)").clicked() {
                                        pending_cmds.push(Cmd::SetLoopMode(loop_mode.cycle()));
                                        ui.close_menu();
                                    }
                                    if ui.selectable_label(marker_loop, "A-B Loop").clicked() {
                                        pending_cmds.push(Cmd::ToggleMarkerLoop);
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    ui.label(
                                        egui::RichText::new("A-B Markers")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);
                                    if ui.button("Set A Marker  ([)").clicked() {
                                        pending_cmds.push(Cmd::SetMarkerA);
                                        ui.close_menu();
                                    }
                                    if ui.button("Set B Marker  (])").clicked() {
                                        pending_cmds.push(Cmd::SetMarkerB);
                                        ui.close_menu();
                                    }
                                    if ui.button("Clear Markers  (\\)").clicked() {
                                        pending_cmds.push(Cmd::ClearMarkers);
                                        ui.close_menu();
                                    }
                                    if ui.button("Export A-B Loop Video...").clicked() {
                                        pending_dialog = Some(crate::file_dialog::FileDialogKind::ExportVideo);
                                        ui.close_menu();
                                    }
                                    if ui.button("Import Markers...").clicked() {
                                        pending_dialog = Some(crate::file_dialog::FileDialogKind::ImportMarkers);
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    ui.label(
                                        egui::RichText::new("Random / Shuffle")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);
                                    let random_modes = [
                                        ("Off", RandomMode::Off),
                                        ("Same Folder", RandomMode::SameFolder),
                                        ("Whole Tree", RandomMode::WholeTree),
                                        ("Step-Aware", RandomMode::StepAware),
                                    ];
                                    let random_mode = self.state.random_mode;
                                    for (label, mode) in random_modes {
                                        let checked = random_mode == mode;
                                        if ui.selectable_label(checked, label).clicked() {
                                            pending_cmds.push(Cmd::SetRandomMode(mode));
                                            ui.close_menu();
                                        }
                                    }
                                    if ui.button("Random Next  (R)").clicked() {
                                        pending_cmds.push(Cmd::RandomNext);
                                        ui.close_menu();
                                    }
                                    if ui.button("Cycle Random Mode  (S)").clicked() {
                                        pending_cmds.push(Cmd::SetRandomMode(random_mode.cycle()));
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    ui.label(
                                        egui::RichText::new("Speed")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);
                                    // No discrete preset entries here — the
                                    // control-bar slider (0.25×–4×) plus these
                                    // up/down steps cover the whole range.
                                    if ui.button("Speed Up +0.25×  (=)").clicked() {
                                        pending_cmds.push(Cmd::SetSpeed((speed + 0.25).min(4.0)));
                                        ui.close_menu();
                                    }
                                    if ui.button("Speed Down -0.25×  (-)").clicked() {
                                        pending_cmds.push(Cmd::SetSpeed((speed - 0.25).max(0.25)));
                                        ui.close_menu();
                                    }
                                },
                            );
                            if playback_resp.inner.is_some() {
                                menu_open = true;
                            }

                            // ---- Audio menu (only if tracks are available) ----
                            if has_audio {
                                let tracks = self.state.audio_tracks.clone();
                                let current = self.state.current_audio_track;
                                let audio_resp = ui.menu_button(
                                    egui::RichText::new("Audio").color(fg).size(13.0),
                                    |ui| {
                                        ui.set_min_width(180.0);
                                        ui.style_mut().visuals.override_text_color = Some(fg);

                                        let auto_checked = current.is_none();
                                        if ui.selectable_label(auto_checked, "Auto").clicked() {
                                            pending_cmds.push(Cmd::SetAudioTrack(None));
                                            ui.close_menu();
                                        }
                                        ui.separator();
                                        for track in &tracks {
                                            let checked = current == Some(track.id);
                                            if ui.selectable_label(checked, track.label()).clicked() {
                                                pending_cmds.push(Cmd::SetAudioTrack(Some(track.id)));
                                                ui.close_menu();
                                            }
                                        }
                                    },
                                );
                                if audio_resp.inner.is_some() {
                                    menu_open = true;
                                }
                            }

                            // ---- Subtitles menu ----
                            {
                                let tracks = self.state.subtitle_tracks.clone();
                                let current = self.state.current_subtitle_track;
                                let subs_resp = ui.menu_button(
                                    egui::RichText::new("Subtitles").color(fg).size(13.0),
                                    |ui| {
                                        ui.set_min_width(200.0);
                                        ui.style_mut().visuals.override_text_color = Some(fg);

                                        if ui.button("Load Subtitle File...").clicked() {
                                            pending_dialog = Some(crate::file_dialog::FileDialogKind::LoadSubtitle);
                                            ui.close_menu();
                                        }
                                        ui.separator();

                                        if ui.selectable_label(sub_vis, "Show Subtitles  (V)").clicked() {
                                            pending_cmds.push(Cmd::ToggleSubVisibility);
                                            ui.close_menu();
                                        }
                                        ui.separator();

                                        let none_checked = current.is_none();
                                        if ui.selectable_label(none_checked, "None").clicked() {
                                            pending_cmds.push(Cmd::SetSubtitleTrack(None));
                                            ui.close_menu();
                                        }
                                        if !tracks.is_empty() {
                                            ui.separator();
                                            for track in &tracks {
                                                let checked = current == Some(track.id);
                                                if ui.selectable_label(checked, track.label()).clicked() {
                                                    pending_cmds.push(Cmd::SetSubtitleTrack(Some(track.id)));
                                                    ui.close_menu();
                                                }
                                            }
                                        } else {
                                            ui.label(
                                                egui::RichText::new("(no embedded subtitle tracks)")
                                                    .color(fg_dim).size(10.0),
                                            );
                                        }
                                    },
                                );
                                if subs_resp.inner.is_some() {
                                    menu_open = true;
                                }
                            }

                            // ---- Video menu (rotate + flip) ----
                            let video_resp = ui.menu_button(
                                egui::RichText::new("Video").color(fg).size(13.0),
                                |ui| {
                                    ui.set_min_width(180.0);
                                    ui.style_mut().visuals.override_text_color = Some(fg);

                                    ui.label(
                                        egui::RichText::new("Rotate")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);
                                    for &deg in &[0u16, 90, 180, 270] {
                                        let checked = video_rotate == deg;
                                        let label = if deg == 0 { "0° (normal)".to_string() } else { format!("{}°", deg) };
                                        if ui.selectable_label(checked, label).clicked() {
                                            pending_cmds.push(Cmd::SetVideoRotate(deg));
                                            ui.close_menu();
                                        }
                                    }

                                    ui.separator();

                                    ui.label(
                                        egui::RichText::new("Flip")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);
                                    if ui.selectable_label(video_flip_h, "Flip Horizontal (mirror)").clicked() {
                                        pending_cmds.push(Cmd::SetVideoFlipH(!video_flip_h));
                                        ui.close_menu();
                                    }
                                    if ui.selectable_label(video_flip_v, "Flip Vertical (upside-down)").clicked() {
                                        pending_cmds.push(Cmd::SetVideoFlipV(!video_flip_v));
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    ui.label(
                                        egui::RichText::new("Zoom / Pan")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);
                                    if ui.button("Zoom In  (Ctrl+Wheel)").clicked() {
                                        pending_cmds.push(Cmd::AdjustVideoZoom(0.25));
                                        ui.close_menu();
                                    }
                                    if ui.button("Zoom Out  (Ctrl+Wheel)").clicked() {
                                        pending_cmds.push(Cmd::AdjustVideoZoom(-0.25));
                                        ui.close_menu();
                                    }
                                    if ui.button("Reset Zoom & Pan").clicked() {
                                        pending_cmds.push(Cmd::ResetVideoPanZoom);
                                        ui.close_menu();
                                    }
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "zoom {:.2}× · pan {:+.2} / {:+.2}",
                                            2.0_f32.powf(video_zoom),
                                            video_pan_x,
                                            video_pan_y,
                                        ))
                                        .color(fg_dim).size(10.0),
                                    );
                                    ui.label(
                                        egui::RichText::new("drag the video to pan")
                                            .color(fg_dim).size(10.0),
                                    );
                                },
                            );
                            if video_resp.inner.is_some() {
                                menu_open = true;
                            }

                            // ---- Help menu ----
                            let help_resp = ui.menu_button(
                                egui::RichText::new("Help").color(fg).size(13.0),
                                |ui| {
                                    ui.set_min_width(200.0);
                                    ui.style_mut().visuals.override_text_color = Some(fg);

                                    if ui.selectable_label(about_visible, "About ferret").clicked() {
                                        pending_about_toggle = true;
                                        ui.close_menu();
                                    }
                                    ui.separator();
                                    ui.label(
                                        egui::RichText::new("ferret 1.2.0")
                                            .color(fg_dim).size(10.0),
                                    );
                                    ui.label(
                                        egui::RichText::new("GPL-2.0-or-later")
                                            .color(fg_dim).size(10.0),
                                    );
                                },
                            );
                            if help_resp.inner.is_some() {
                                menu_open = true;
                            }

                            // ---- Status line ----
                            ui.add_space(8.0);
                            let status = if self.state.paused { "paused" } else { "playing" };
                            let speed_str = if (speed - 1.0).abs() < 0.01 {
                                String::new()
                            } else {
                                format!(" {:.2}x", speed)
                            };
                            let loop_str = match loop_mode {
                                LoopMode::Off => String::new(),
                                LoopMode::File => " loop:file".into(),
                                LoopMode::Playlist => " loop:list".into(),
                            };
                            let random_mode = self.state.random_mode;
                            let rand_str = if random_mode == RandomMode::Off {
                                String::new()
                            } else {
                                format!(" {}", random_mode.short_label())
                            };
                            let zoom_str = if video_zoom.abs() > 0.001 {
                                format!(" zoom:{:.2}x", 2.0_f32.powf(video_zoom))
                            } else {
                                String::new()
                            };
                            let pan_str = if video_pan_x.abs() > 0.005 || video_pan_y.abs() > 0.005 {
                                format!(" pan:{:+.2},{:+.2}", video_pan_x, video_pan_y)
                            } else {
                                String::new()
                            };
                            ui.label(
                                egui::RichText::new(format!(
                                    "{status}{speed_str}{loop_str}{rand_str}{zoom_str}{pan_str}"
                                ))
                                    .color(fg_dim).size(10.0),
                            );
                        });
                    });
            });

        // Remember whether any dropdown menu was open this frame — used as a
        // sticky condition by compute_visibility so an open menu never hides
        // its own bar while the user reads it.
        self.menu_popup_open = menu_open;

        // Send any commands that were collected during the UI pass.
        for cmd in pending_cmds {
            self.send(cmd);
        }
        // Apply the About toggle after the closure (can't mutate self inside).
        if pending_about_toggle {
            self.about_visible = !self.about_visible;
        }
        // Same for the queue sidebar toggle.
        if pending_queue_toggle {
            self.queue_visible = !self.queue_visible;
        }
        // Open the in-UI file dialog if a menu item requested one.
        if let Some(kind) = pending_dialog {
            // For ExportVideo, pre-check that A/B markers are set.
            if matches!(kind, crate::file_dialog::FileDialogKind::ExportVideo) {
                let (a, b) = (self.state.marker_a, self.state.marker_b);
                match (a, b) {
                    (Some(start), Some(end)) if end > start => {
                        // Suggest a default clip filename derived from the
                        // input file and the A/B timestamps so the user can
                        // just hit Save (or type their own — the filename
                        // field takes keyboard input now).
                        let suggestion = build_export_suggestion(&self.state);
                        self.file_dialog = Some(
                            crate::file_dialog::FileDialog::open_with_filename(kind, suggestion),
                        );
                    }
                    _ => {
                        self.show_info("Set both A and B markers before exporting");
                    }
                }
            } else {
                self.file_dialog = Some(crate::file_dialog::FileDialog::open(kind));
            }
        }
    }

    /// Handle the result of a completed file dialog. Sends the appropriate
    /// `Cmd` (or Cmds) to the engine via `cmd_tx`. The `kind` is passed in
    /// because the dialog has already been cleared from `self.file_dialog`
    /// by the time this is called.
    fn handle_file_dialog_result(
        &mut self,
        result: crate::file_dialog::FileDialogResult,
        kind: crate::file_dialog::FileDialogKind,
    ) {
        use crate::file_dialog::{FileDialogKind, FileDialogResult};
        use player_core::cmd::{LoadModeKind, LoadOptions};

        match result {
            FileDialogResult::Cancel => {}
            FileDialogResult::Path(path) => match kind {
                FileDialogKind::LoadFile => {
                    let _ = self.cmd_tx.send(Cmd::LoadFile {
                        path: path.clone(),
                        options: LoadOptions { mode: LoadModeKind::Replace, pause: false },
                    });
                    self.show_info(format!("Loading: {path}"));
                }
                FileDialogKind::LoadFolder => {
                    // Enumerate media files in the folder, load as playlist.
                    if let Some(paths) = collect_folder_as_playlist(&path) {
                        if paths.is_empty() {
                            self.show_info("No video files found in folder");
                        } else {
                            // Use try_send (non-blocking) — if the engine's
                            // command channel is full (64 slots), skip the
                            // remaining files rather than blocking the UI thread.
                            let mut sent = 0usize;
                            for (i, p) in paths.iter().enumerate() {
                                let mode = if i == 0 {
                                    LoadModeKind::Replace
                                } else {
                                    LoadModeKind::AppendPlay
                                };
                                if self.cmd_tx.try_send(Cmd::LoadFile {
                                    path: p.clone(),
                                    options: LoadOptions { mode, pause: false },
                                }).is_ok() {
                                    sent += 1;
                                }
                            }
                            self.show_info(format!("Loaded {}/{} files from folder", sent, paths.len()));
                        }
                    } else {
                        self.show_info(format!("Could not read folder: {path}"));
                    }
                }
                FileDialogKind::SaveMarkers(fmt) => {
                    let _ = self.cmd_tx.send(Cmd::ExportMarkers { path: path.clone(), format: fmt });
                    self.show_info(format!("Markers exported to {path}"));
                }
                FileDialogKind::LoadSubtitle => {
                    let _ = self.cmd_tx.send(Cmd::LoadSubtitleFile { path });
                    self.show_info("Subtitle file loaded");
                }
                FileDialogKind::ImportMarkers => {
                    let _ = self.cmd_tx.send(Cmd::ImportMarkers { path: path.clone() });
                    self.show_info(format!("Markers imported from {path}"));
                }
                FileDialogKind::ExportVideo => {
                    // ffmpeg needs a container extension to pick a muxer — if
                    // the user typed a filename without one (or with an
                    // unknown one), default to .mp4 instead of failing.
                    let path = ensure_video_extension(path);
                    // Send the real export command with the chosen path.
                    // Main app intercepts all ExportABLoopVideo Cmds and runs ffmpeg.
                    let _ = self.cmd_tx.send(Cmd::ExportABLoopVideo { path });
                }
                FileDialogKind::SavePlaylist => {
                    // Save the queue — in exactly the order the user
                    // organized it — as an .m3u playlist.
                    if self.state.playlist.is_empty() {
                        self.show_info("Queue is empty — nothing to save");
                    } else {
                        let path = ensure_m3u_extension(path.clone());
                        let count = self.state.playlist.len();
                        match write_m3u(&path, &self.state.playlist) {
                            Ok(()) => self.show_info(format!(
                                "Playlist saved — {count} entries → {path}"
                            )),
                            Err(e) => self.show_info(format!("Playlist save failed: {e}")),
                        }
                    }
                }
                FileDialogKind::LoadPlaylist => {
                    // Single file from a multi-select dialog (shouldn't happen,
                    // but handle gracefully). A playlist file expands to its
                    // entries; a media file loads directly.
                    let (expanded, _) = expand_playlist_selection(std::slice::from_ref(&path));
                    if expanded.is_empty() {
                        self.show_info("No files selected");
                    } else {
                        for (i, p) in expanded.iter().enumerate() {
                            let mode = if i == 0 {
                                LoadModeKind::Replace
                            } else {
                                LoadModeKind::AppendPlay
                            };
                            let _ = self.cmd_tx.try_send(Cmd::LoadFile {
                                path: p.clone(),
                                options: LoadOptions { mode, pause: false },
                            });
                        }
                        self.show_info(format!("Loaded {} files", expanded.len()));
                    }
                }
            },
            FileDialogResult::Paths(paths) => {
                // Multi-select arrives only for Load Playlist. Any .m3u /
                // .m3u8 / .pls selections expand to their entries in order.
                if let FileDialogKind::LoadPlaylist = kind {
                    if paths.is_empty() {
                        self.show_info("No files selected");
                    } else {
                        // Expand any .m3u/.m3u8/.pls selections into their
                        // entries (preserving order), then load the result.
                        let (expanded, skipped) = expand_playlist_selection(&paths);
                        if expanded.is_empty() {
                            self.show_info("Playlist contained no entries");
                        } else {
                            // Use try_send (non-blocking) to avoid UI deadlock
                            // if the engine's command channel is full.
                            let mut sent = 0usize;
                            for (i, p) in expanded.iter().enumerate() {
                                let mode = if i == 0 {
                                    LoadModeKind::Replace
                                } else {
                                    LoadModeKind::AppendPlay
                                };
                                if self.cmd_tx.try_send(Cmd::LoadFile {
                                    path: p.clone(),
                                    options: LoadOptions { mode, pause: false },
                                }).is_ok() {
                                    sent += 1;
                                }
                            }
                            let skipped_note = if skipped > 0 {
                                format!(" ({skipped} playlist file(s) unreadable)")
                            } else {
                                String::new()
                            };
                            self.show_info(format!(
                                "Loaded {}/{} files{skipped_note}",
                                sent,
                                expanded.len()
                            ));
                        }
                    }
                }
            }
        }
    }

    /// Draw a persistent About panel in the overlay. This is NOT a popup —
    /// it stays visible until the user toggles it off via Help → About.
    /// Positioned at top-right so it doesn't overlap the menu bar.
    fn draw_about_panel(&mut self, ctx: &Context) {
        let win_w = self.window_size.x;
        let panel_w = 320.0_f32.min(win_w - 20.0);
        let panel_x = win_w - panel_w - 10.0;
        let panel_y = 40.0;

        let fg = self.theme.fg_color32();
        let fg_dim = self.theme.fg_dim_color32();
        let bg = self.theme.bg_color32();
        let accent = self.theme.accent_color32();

        egui::Area::new(egui::Id::new("ferret_about_panel"))
            .fixed_pos(egui::pos2(panel_x, panel_y))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(egui::Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 240))
                    .rounding(6.0)
                    .inner_margin(egui::Margin::symmetric(14.0, 10.0))
                    .stroke(egui::Stroke::new(1.0_f32, accent))
                    .show(ui, |ui| {
                        ui.set_min_width(panel_w - 28.0);
                        ui.set_max_width(panel_w - 28.0);

                        // Title
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("ferret")
                                    .color(accent)
                                    .size(20.0)
                                    .strong(),
                            );
                            ui.label(
                                egui::RichText::new("1.2.0")
                                    .color(fg_dim)
                                    .size(13.0),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("×").clicked() {
                                    self.about_visible = false;
                                }
                            });
                        });

                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(4.0);

                        // Description
                        ui.label(
                            egui::RichText::new("A modern, accuracy-first video player for Linux.")
                                .color(fg).size(12.0),
                        );

                        ui.add_space(6.0);

                        // Author
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Author:").color(fg_dim).size(11.0));
                            ui.label(egui::RichText::new("Jeremy Anderson").color(fg).size(11.0));
                        });

                        // Website
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Website:").color(fg_dim).size(11.0));
                            ui.label(
                                egui::RichText::new("http://git.dcos.net/dcosnet/ferret")
                                    .color(accent).size(11.0),
                            );
                        });

                        // License
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("License:").color(fg_dim).size(11.0));
                            ui.label(egui::RichText::new("GPL-2.0-or-later").color(fg).size(11.0));
                        });

                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(4.0);

                        // Tech credits
                        ui.label(
                            egui::RichText::new("Built with Rust, libmpv, egui, wgpu, and winit.")
                                .color(fg_dim).size(10.0),
                        );
                        ui.label(
                            egui::RichText::new("Copyright © 2026 Jeremy Anderson.")
                                .color(fg_dim).size(10.0),
                        );
                    });
            });
    }

    /// Draw the queue sidebar: a persistent right-hand panel showing the mpv
    /// playlist in play order. Reordering works two ways:
    ///
    ///   * **Drag and drop** — grab any row (or its grip) and drop it on the
    ///     row it should precede, or below the last row to append.
    ///   * **Type a position** — click a row's number, type a new 1-based
    ///     position, press Enter (Esc cancels; clicking away commits).
    ///
    /// Clicking a row's name plays that entry; the × removes it. The Save
    /// button writes the queue — in exactly the order shown — to an .m3u
    /// file via the save dialog.
    fn draw_queue_panel(&mut self, ctx: &Context) {
        enum QueueAction {
            Close,
            SaveAs,
            Play(usize),
            Remove(usize),
            Move { from: usize, to: usize },
            StartEdit(usize),
        }

        let win = self.window_size;
        let panel_w = 300.0_f32.min(win.x - 24.0).max(140.0);
        let panel_x = (win.x - panel_w - 8.0).max(8.0);
        let panel_y = 36.0;
        // Stop above where the control bar sits (118px) so nothing overlaps.
        let panel_h = ((win.y - 126.0) - panel_y).max(140.0);

        // Snapshots so the Area closure never borrows self.
        let theme = self.theme;
        let fg = theme.fg_color32();
        let fg_dim = theme.fg_dim_color32();
        let bg = theme.bg_color32();
        let accent = theme.accent_color32();
        let entries: Vec<String> = self.state.playlist.clone();
        let playing_pos = self.state.playlist_pos;
        let len = entries.len();

        let mut actions: Vec<QueueAction> = Vec::new();
        let mut edit = self.queue_edit.take();
        let mut edit_focus = self.queue_edit_focus;
        let mut drag_from = self.queue_drag_from;
        let mut drag_to = self.queue_drag_to;
        let mut hovered_any = false;

        egui::Area::new(egui::Id::new("ferret_queue_panel"))
            .fixed_pos(egui::pos2(panel_x, panel_y))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(egui::Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 235))
                    .rounding(6.0)
                    .stroke(egui::Stroke::new(1.0_f32, accent))
                    .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                    .show(ui, |ui| {
                        ui.set_min_width(panel_w - 18.0);
                        ui.set_max_width(panel_w - 18.0);

                        // ---- Header ----
                        ui.horizontal(|ui| {
                            let n_label = if len == 1 { "entry" } else { "entries" };
                            ui.label(
                                egui::RichText::new(format!("Queue — {len} {n_label}"))
                                    .color(fg)
                                    .size(13.0)
                                    .strong(),
                            );
                            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("×").clicked() {
                                    actions.push(QueueAction::Close);
                                }
                                if ui.button("Save...").clicked() {
                                    actions.push(QueueAction::SaveAs);
                                }
                            });
                        });
                        ui.separator();

                        // ---- Rows ----
                        egui::ScrollArea::vertical()
                            .max_height((panel_h - 96.0).max(80.0))
                            // Row drags reorder; they must not scroll.
                            .drag_to_scroll(false)
                            .show(ui, |ui| {
                                if entries.is_empty() {
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new("(queue is empty — load files or a playlist)")
                                            .color(fg_dim)
                                            .size(11.0),
                                    );
                                }

                                let mut last_bottom: Option<f32> = None;
                                for (i, path) in entries.iter().enumerate() {
                                    let row_h = 26.0;
                                    let (rect, resp) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width(), row_h),
                                        egui::Sense::click_and_drag(),
                                    );
                                    last_bottom = Some(rect.max.y);
                                    let is_playing = i as i64 == playing_pos;
                                    let is_source = drag_from == Some(i);
                                    let is_target = drag_to == Some(i) && !is_source;

                                    // Sub-rects: [ # ] [ name ....... ] [≡] [×]
                                    let num_rect = egui::Rect::from_min_max(
                                        egui::pos2(rect.min.x + 2.0, rect.min.y),
                                        egui::pos2(rect.min.x + 36.0, rect.max.y),
                                    );
                                    let x_rect = egui::Rect::from_min_max(
                                        egui::pos2(rect.max.x - 20.0, rect.min.y),
                                        egui::pos2(rect.max.x, rect.max.y),
                                    );
                                    let grip_rect = egui::Rect::from_min_max(
                                        egui::pos2(rect.max.x - 40.0, rect.min.y),
                                        egui::pos2(rect.max.x - 20.0, rect.max.y),
                                    );
                                    let name_rect = egui::Rect::from_min_max(
                                        egui::pos2(num_rect.max.x + 6.0, rect.min.y),
                                        egui::pos2(grip_rect.min.x - 4.0, rect.max.y),
                                    );

                                    let painter = ui.painter_at(rect);
                                    let bg_row = if is_source {
                                        egui::Color32::from_rgba_unmultiplied(
                                            theme.accent[0], theme.accent[1], theme.accent[2], 48,
                                        )
                                    } else if is_playing {
                                        egui::Color32::from_rgba_unmultiplied(
                                            theme.accent[0], theme.accent[1], theme.accent[2], 26,
                                        )
                                    } else if resp.hovered() {
                                        theme.button_bg_hover_color32()
                                    } else {
                                        theme.button_bg_color32()
                                    };
                                    painter.rect_filled(rect, 3.0, bg_row);
                                    if is_playing {
                                        painter.rect_filled(
                                            egui::Rect::from_min_size(
                                                rect.min,
                                                Vec2::new(3.0, rect.height()),
                                            ),
                                            1.0,
                                            accent,
                                        );
                                    }
                                    // Insert-before indicator on the target row.
                                    if is_target {
                                        painter.line_segment(
                                            [
                                                egui::pos2(rect.min.x + 2.0, rect.min.y),
                                                egui::pos2(rect.max.x - 2.0, rect.min.y),
                                            ],
                                            egui::Stroke::new(2.0_f32, accent),
                                        );
                                    }

                                    // Position number (skipped while its TextEdit
                                    // is overlaid on this row).
                                    let editing_this =
                                        edit.as_ref().map(|(ei, _)| *ei) == Some(i);
                                    if !editing_this {
                                        painter.text(
                                            num_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            format!("{}", i + 1),
                                            egui::FontId::monospace(11.0),
                                            if is_playing { accent } else { fg_dim },
                                        );
                                    }

                                    // File name.
                                    painter.text(
                                        name_rect.left_center(),
                                        egui::Align2::LEFT_CENTER,
                                        truncate_to_width(&file_basename(path), name_rect.width()),
                                        egui::FontId::proportional(11.0),
                                        fg,
                                    );

                                    // Drag grip (three short lines).
                                    let gc = grip_rect.center();
                                    for dy in [-3.0_f32, 0.0, 3.0] {
                                        painter.line_segment(
                                            [
                                                egui::pos2(gc.x - 5.0, gc.y + dy),
                                                egui::pos2(gc.x + 5.0, gc.y + dy),
                                            ],
                                            egui::Stroke::new(1.5_f32, fg_dim),
                                        );
                                    }

                                    // Remove button (red on hover).
                                    let x_hover = ui
                                        .input(|inp| inp.pointer.hover_pos())
                                        .map(|p| x_rect.contains(p))
                                        .unwrap_or(false);
                                    painter.text(
                                        x_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "×",
                                        egui::FontId::proportional(13.0),
                                        if x_hover {
                                            egui::Color32::from_rgb(220, 80, 80)
                                        } else {
                                            fg_dim
                                        },
                                    );

                                    if resp.hovered() {
                                        hovered_any = true;
                                    }

                                    // ---- Interactions ----
                                    if resp.drag_started() {
                                        drag_from = Some(i);
                                        drag_to = None;
                                    }
                                    if drag_from.is_some() {
                                        if let Some(p) = ui.input(|inp| inp.pointer.hover_pos()) {
                                            if rect.contains(p) {
                                                drag_to = Some(i);
                                            }
                                        }
                                    }
                                    if resp.drag_stopped() {
                                        let from = drag_from.take();
                                        let to = drag_to.take();
                                        drag_to = None;
                                        if let (Some(f), Some(t)) = (from, to) {
                                            let to = t.min(len);
                                            if f != to {
                                                actions.push(QueueAction::Move { from: f, to });
                                            }
                                        }
                                    }
                                    if resp.clicked() {
                                        if let Some(p) = resp.interact_pointer_pos() {
                                            if x_rect.contains(p) {
                                                actions.push(QueueAction::Remove(i));
                                            } else if num_rect.contains(p) && !editing_this {
                                                actions.push(QueueAction::StartEdit(i));
                                            } else {
                                                actions.push(QueueAction::Play(i));
                                            }
                                        }
                                    }

                                    // ---- Position TextEdit overlay ----
                                    if editing_this {
                                        let (_, buf) = edit.as_mut().expect("editing_this implies Some");
                                        let te = egui::TextEdit::singleline(buf)
                                            .desired_width(34.0)
                                            .font(egui::FontId::monospace(11.0));
                                        let te_resp = ui.put(num_rect, te);
                                        if edit_focus {
                                            te_resp.request_focus();
                                            edit_focus = false;
                                        }
                                        let enter =
                                            ui.input(|inp| inp.key_pressed(egui::Key::Enter));
                                        let escape =
                                            ui.input(|inp| inp.key_pressed(egui::Key::Escape));
                                        if escape {
                                            edit = None;
                                            te_resp.surrender_focus();
                                        } else if enter {
                                            let final_pos = buf
                                                .trim()
                                                .parse::<usize>()
                                                .ok()
                                                .filter(|n| *n >= 1 && *n <= len)
                                                .map(|n| n - 1);
                                            if let Some(t) = final_pos {
                                                let (from, to) =
                                                    player_core::cmd::playlist_move_args_for_final(
                                                        i, t, len,
                                                    );
                                                if from != to {
                                                    actions.push(QueueAction::Move { from, to });
                                                }
                                            }
                                            edit = None;
                                            te_resp.surrender_focus();
                                        } else if te_resp.lost_focus() {
                                            // Clicking elsewhere commits the edit.
                                            let final_pos = buf
                                                .trim()
                                                .parse::<usize>()
                                                .ok()
                                                .filter(|n| *n >= 1 && *n <= len)
                                                .map(|n| n - 1);
                                            if let Some(t) = final_pos {
                                                let (from, to) =
                                                    player_core::cmd::playlist_move_args_for_final(
                                                        i, t, len,
                                                    );
                                                if from != to {
                                                    actions.push(QueueAction::Move { from, to });
                                                }
                                            }
                                            edit = None;
                                        }
                                    }
                                }

                                // Drop below the last row = append at the end.
                                if drag_from.is_some() {
                                    if let Some(p) = ui.input(|inp| inp.pointer.hover_pos()) {
                                        if let Some(bottom) = last_bottom {
                                            if p.y > bottom && p.y < bottom + 40.0 {
                                                drag_to = Some(len);
                                            }
                                        }
                                    }
                                    if drag_to == Some(len) {
                                        if let Some(bottom) = last_bottom {
                                            let w = ui.min_rect().width();
                                            ui.painter().line_segment(
                                                [
                                                    egui::pos2(2.0, bottom + 3.0),
                                                    egui::pos2(w - 2.0, bottom + 3.0),
                                                ],
                                                egui::Stroke::new(2.0_f32, accent),
                                            );
                                        }
                                    }
                                }
                            });

                        // ---- Footer hint ----
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(
                                "drag to reorder · click # to type a position · click a name to play",
                            )
                            .color(fg_dim)
                            .size(9.0),
                        );
                    });
            });

        // Write back the interaction mirrors.
        self.queue_edit = edit;
        self.queue_edit_focus = edit_focus;
        self.queue_drag_from = drag_from;
        self.queue_drag_to = drag_to;

        // Apply collected actions.
        for action in actions {
            match action {
                QueueAction::Close => self.queue_visible = false,
                QueueAction::SaveAs => {
                    if self.state.playlist.is_empty() {
                        self.show_info("Queue is empty — nothing to save");
                    } else {
                        self.file_dialog = Some(crate::file_dialog::FileDialog::open_with_filename(
                            crate::file_dialog::FileDialogKind::SavePlaylist,
                            Some("playlist.m3u".to_string()),
                        ));
                    }
                }
                QueueAction::Play(i) => self.send(Cmd::PlaylistPlayIndex { index: i }),
                QueueAction::Remove(i) => {
                    self.send(Cmd::PlaylistRemove { index: i });
                    // Cancel a position-edit on the row that just vanished.
                    if self.queue_edit.as_ref().map(|(ei, _)| *ei) == Some(i) {
                        self.queue_edit = None;
                    }
                }
                QueueAction::Move { from, to } => self.send(Cmd::PlaylistMove { from, to }),
                QueueAction::StartEdit(i) => {
                    self.queue_edit = Some((i, format!("{}", i + 1)));
                    self.queue_edit_focus = true;
                }
            }
        }
        if hovered_any {
            self.note_user_activity();
        }
    }

    fn draw_controls(&mut self, ui: &mut Ui) {
        ui.vertical(|ui| {
            // ===== Row 1: Seek bar with time labels on either side =====
            ui.horizontal(|ui| {
                let cur_time = self
                    .state
                    .time_pos
                    .map(|t| format_time(t))
                    .unwrap_or_else(|| "00:00".to_string());
                ui.label(
                    egui::RichText::new(cur_time)
                        .color(self.theme.fg_color32())
                        .size(11.0)
                        .monospace(),
                );
                ui.add_space(8.0);

                let progress = self.state.progress();
                let (response, new_pos) = seek_bar(ui, progress, None, 18.0);

                // Draw A/B marker pins on top of the seek bar.
                if let Some(dur) = self.state.duration {
                    if dur > 0.0 {
                        let bar_rect = response.rect;
                        let track_y = bar_rect.center().y;
                        let track_w = bar_rect.width() - 4.0;
                        let track_min_x = bar_rect.min.x + 2.0;
                        let painter = ui.painter_at(bar_rect);
                        if let Some(a) = self.state.marker_a {
                            let x = track_min_x + (a / dur).clamp(0.0, 1.0) as f32 * track_w;
                            draw_marker_pin(&painter, egui::pos2(x, track_y - 8.0), Color32::from_rgb(255, 100, 100));
                        }
                        if let Some(b) = self.state.marker_b {
                            let x = track_min_x + (b / dur).clamp(0.0, 1.0) as f32 * track_w;
                            draw_marker_pin(&painter, egui::pos2(x, track_y - 8.0), Color32::from_rgb(100, 180, 255));
                        }
                    }
                }

                if response.hovered() || response.dragged() {
                    self.note_user_activity();
                }
                if let Some(frac) = new_pos {
                    self.seeking = response.dragged();
                    if let Some(dur) = self.state.duration {
                        self.seek_drag_pos = dur * (frac as f64);
                        self.state.time_pos = Some(self.seek_drag_pos);
                        if !response.dragged() {
                            self.send(Cmd::Seek {
                                target_secs: self.seek_drag_pos,
                                mode: mpv_bindings::command::SeekMode::Absolute,
                                flags: mpv_bindings::command::SeekFlags::Exact,
                            });
                            self.seeking = false;
                        }
                    }
                } else if !response.dragged() {
                    self.seeking = false;
                }

                ui.add_space(8.0);
                let dur_str = self
                    .state
                    .duration
                    .map(|t| format_time(t))
                    .unwrap_or_else(|| "--:--".to_string());
                ui.label(
                    egui::RichText::new(dur_str)
                        .color(self.theme.fg_color32())
                        .size(11.0)
                        .monospace(),
                );
            });

            ui.add_space(4.0);

            // ===== Row 2: Transport buttons | time | volume + fullscreen =====
            ui.horizontal(|ui| {
                let btn_size = Vec2::new(32.0, 24.0);

                let paused = self.state.paused;
                let muted = self.state.muted;
                let volume = self.state.volume;

                // Play / Pause
                let play_resp = self.icon_button(ui, "btn_play", btn_size, move |p, r, c| {
                    if paused { icons::play(p, r, c) } else { icons::pause(p, r, c) }
                });
                if play_resp.clicked() { self.send(Cmd::PlayPause); }

                // Stop
                let stop_resp = self.icon_button(ui, "btn_stop", btn_size, |p, r, c| icons::stop(p, r, c));
                if stop_resp.clicked() { self.send(Cmd::Stop); }

                // Previous (playlist)
                let prev_resp = self.icon_button(ui, "btn_prev", btn_size, |p, r, c| icons::previous(p, r, c));
                if prev_resp.clicked() { self.send(Cmd::PlaylistPrev); }

                // Next (playlist)
                let next_resp = self.icon_button(ui, "btn_next", btn_size, |p, r, c| icons::next(p, r, c));
                if next_resp.clicked() { self.send(Cmd::PlaylistNext); }

                ui.add_space(6.0);

                // Frame step back / forward
                let fb_resp = self.icon_button(ui, "btn_frame_back", btn_size, |p, r, c| icons::frame_back(p, r, c));
                if fb_resp.clicked() { self.send(Cmd::FrameBackStep); }
                let ff_resp = self.icon_button(ui, "btn_frame_fwd", btn_size, |p, r, c| icons::frame_forward(p, r, c));
                if ff_resp.clicked() { self.send(Cmd::FrameStep); }

                ui.add_space(6.0);

                // Shuffle button — cycles through random modes (Off → Folder → Tree → Step → Off).
                let rand_active = self.state.random_mode != RandomMode::Off;
                let rand_resp = self.icon_button_toggled(ui, "btn_random", btn_size, rand_active, |p, r, c| {
                    icons::shuffle(p, r, c, rand_active)
                });
                if rand_resp.clicked() { self.send(Cmd::SetRandomMode(self.state.random_mode.cycle())); }
                rand_resp.on_hover_text(format!("Cycle Random Mode — {}", self.state.random_mode.label()));

                // Queue sidebar toggle — shows the playlist panel for
                // reordering / saving. Persistent (not auto-hidden).
                let queue_active = self.queue_visible;
                let queue_resp = self.icon_button_toggled(ui, "btn_queue", btn_size, queue_active, |p, r, c| {
                    icons::queue(p, r, c)
                });
                if queue_resp.clicked() { self.queue_visible = !self.queue_visible; }
                queue_resp.on_hover_text("Toggle Queue Sidebar — reorder & save the queue");

                // Center: time display
                ui.with_layout(Layout::centered_and_justified(egui::Direction::TopDown), |ui| {
                    let time_str = self.state.time_str();
                    ui.label(
                        egui::RichText::new(time_str)
                            .color(self.theme.fg_color32())
                            .size(12.0)
                            .monospace(),
                    );
                });

                // Right: volume + fullscreen
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    let fs_resp = self.icon_button(ui, "btn_fullscreen", btn_size, |p, r, c| icons::fullscreen(p, r, c));
                    if fs_resp.clicked() { self.send(Cmd::ToggleFullscreen); }
                    if fs_resp.hovered() { self.note_user_activity(); }
                    ui.add_space(8.0);

                    let mut vol_pct = self.state.volume * 100.0;
                    let slider = egui::Slider::new(&mut vol_pct, 0.0..=100.0)
                        .show_value(false)
                        .fixed_decimals(0);
                    let slider_resp = ui.add_sized(Vec2::new(100.0, 18.0), slider);
                    if slider_resp.changed() {
                        self.send(Cmd::SetVolume(vol_pct / 100.0));
                        self.state.volume = vol_pct / 100.0;
                    }
                    if slider_resp.hovered() { self.note_user_activity(); }
                    ui.add_space(4.0);

                    let vol_resp = self.icon_button(ui, "btn_vol", btn_size, move |p, r, c| {
                        let level = if muted { 0.0 } else { volume };
                        icons::volume(p, r, c, level, muted)
                    });
                    if vol_resp.clicked() { self.send(Cmd::ToggleMute); }
                });
            });

            ui.add_space(4.0);

            // ===== Row 3: A/B markers | loop | speed presets | audio track =====
            ui.horizontal(|ui| {
                let btn_size = Vec2::new(32.0, 24.0);

                // --- A marker button (with current position label) ---
                let a_label = self.state.marker_a.map(format_time).unwrap_or_else(|| "  A  ".into());
                let a_resp = self.labeled_button(ui, "btn_marker_a", btn_size, &a_label, Color32::from_rgb(255, 100, 100));
                if a_resp.clicked() { self.send(Cmd::SetMarkerA); }

                // --- B marker button ---
                let b_label = self.state.marker_b.map(format_time).unwrap_or_else(|| "  B  ".into());
                let b_resp = self.labeled_button(ui, "btn_marker_b", btn_size, &b_label, Color32::from_rgb(100, 180, 255));
                if b_resp.clicked() { self.send(Cmd::SetMarkerB); }

                // --- AB-loop toggle ---
                let ab_active = self.state.marker_loop_enabled;
                let ab_resp = self.icon_button_toggled(ui, "btn_ab_loop", btn_size, ab_active, |p, r, c| {
                    icons::marker_loop(p, r, c, ab_active)
                });
                if ab_resp.clicked() { self.send(Cmd::ToggleMarkerLoop); }

                ui.add_space(8.0);

                // --- Loop mode toggle (off/file/playlist) ---
                let loop_mode = self.state.loop_mode;
                let loop_label = loop_mode.label();
                let loop_active = loop_mode != LoopMode::Off;
                // Table-driven loop-mode → icon variant mapping. Indices
                // mirror the contract documented on `icons::loop_icon`.
                const LOOP_ICON_IDX: [(LoopMode, u8); 3] = [
                    (LoopMode::Off, 0),
                    (LoopMode::File, 1),
                    (LoopMode::Playlist, 2),
                ];
                let icon_idx = LOOP_ICON_IDX
                    .iter()
                    .copied()
                    .find(|(m, _)| *m == loop_mode)
                    .map(|(_, i)| i)
                    .expect("LoopMode is exhaustive over LOOP_ICON_IDX");
                let loop_resp = self.icon_button_toggled(ui, "btn_loop", btn_size, loop_active, |p, r, c| {
                    icons::loop_icon(p, r, c, icon_idx)
                });
                if loop_resp.clicked() {
                    self.send(Cmd::SetLoopMode(loop_mode.cycle()));
                }
                // Tooltip showing current mode.
                loop_resp.on_hover_text(loop_label);

                ui.add_space(8.0);

                // --- Speed presets: 0.5×, 1×, 1.5×, 2× ---
                let presets = [0.5_f32, 1.0, 1.5, 2.0];
                let current_speed = self.speed_drag.unwrap_or(self.state.speed);
                for &p in &presets {
                    let active = (current_speed - p).abs() < 0.01;
                    let label = format!("{p:.2}×");
                    let resp = self.text_button(ui, &format!("btn_speed_{p}"), btn_size, &label, active);
                    if resp.clicked() {
                        self.send(Cmd::SetSpeed(p));
                        self.speed_drag = Some(p);
                    }
                }

                // Fine-grained speed slider (0.25×–4×).
                let mut speed_val = self.speed_drag.unwrap_or(self.state.speed);
                let slider = egui::Slider::new(&mut speed_val, 0.25..=4.0)
                    .show_value(false)
                    .fixed_decimals(2)
                    .clamping(egui::SliderClamping::Always);
                let slider_resp = ui.add_sized(Vec2::new(110.0, 18.0), slider);
                if slider_resp.drag_started() {
                    self.speed_drag = Some(speed_val);
                }
                if slider_resp.dragged() {
                    self.speed_drag = Some(speed_val);
                }
                if slider_resp.drag_stopped() {
                    if let Some(v) = self.speed_drag.take() {
                        self.send(Cmd::SetSpeed(v));
                    }
                }
                if slider_resp.hovered() { self.note_user_activity(); }

                // Current speed label.
                ui.label(
                    egui::RichText::new(format!("{:.2}×", current_speed))
                        .color(self.theme.fg_color32())
                        .size(11.0)
                        .monospace(),
                );

                ui.add_space(8.0);

                // --- Audio track dropdown ---
                ui.label(
                    egui::RichText::new("Audio:")
                        .color(self.theme.fg_dim_color32())
                        .size(11.0),
                );
                egui::ComboBox::from_id_salt("ferret_audio_track")
                    .selected_text(self.current_audio_label())
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        // "Auto" option.
                        let auto_selected = self.state.current_audio_track.is_none();
                        if ui.selectable_label(auto_selected, "auto").clicked() {
                            self.send(Cmd::SetAudioTrack(None));
                        }
                        ui.separator();
                        for track in &self.state.audio_tracks {
                            let label = track.label();
                            let selected = self.state.current_audio_track == Some(track.id);
                            if ui.selectable_label(selected, label).clicked() {
                                self.send(Cmd::SetAudioTrack(Some(track.id)));
                            }
                        }
                        if self.state.audio_tracks.is_empty() {
                            ui.label(
                                egui::RichText::new("(no audio tracks)")
                                    .color(self.theme.fg_dim_color32())
                                    .size(10.0),
                            );
                        }
                    });
            });
        });
    }

    fn current_audio_label(&self) -> String {
        match self.state.current_audio_track {
            None => "auto".to_string(),
            Some(id) => {
                self.state
                    .audio_tracks
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| t.label())
                    .unwrap_or_else(|| format!("track {id}"))
            }
        }
    }

    /// Draw an icon button with hover state. `draw_fn` receives (painter, rect, color).
    fn icon_button(
        &mut self,
        ui: &mut Ui,
        _id: &str,
        size: Vec2,
        draw_fn: impl Fn(&egui::Painter, egui::Rect, egui::Color32),
    ) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        if response.hovered() {
            self.note_user_activity();
        }
        if ui.is_rect_visible(rect) {
            let painter = ui.painter_at(rect);
            let bg = if response.hovered() || response.has_focus() {
                self.theme.button_bg_hover_color32()
            } else {
                self.theme.button_bg_color32()
            };
            painter.rect_filled(rect, 4.0, bg);
            let icon_color = if response.hovered() {
                self.theme.accent_hover_color32()
            } else {
                self.theme.fg_color32()
            };
            let icon_rect = rect.shrink2(Vec2::splat(5.0));
            draw_fn(&painter, icon_rect, icon_color);
        }
        response
    }

    /// Like `icon_button` but with a "toggled on" visual state — brighter bg
    /// and accent icon color.
    fn icon_button_toggled(
        &mut self,
        ui: &mut Ui,
        _id: &str,
        size: Vec2,
        toggled: bool,
        draw_fn: impl Fn(&egui::Painter, egui::Rect, egui::Color32),
    ) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        if response.hovered() {
            self.note_user_activity();
        }
        if ui.is_rect_visible(rect) {
            let painter = ui.painter_at(rect);
            let bg = if toggled {
                egui::Color32::from_rgba_unmultiplied(
                    self.theme.accent[0],
                    self.theme.accent[1],
                    self.theme.accent[2],
                    60,
                )
            } else if response.hovered() || response.has_focus() {
                self.theme.button_bg_hover_color32()
            } else {
                self.theme.button_bg_color32()
            };
            painter.rect_filled(rect, 4.0, bg);
            let icon_color = if toggled {
                self.theme.accent_color32()
            } else if response.hovered() {
                self.theme.accent_hover_color32()
            } else {
                self.theme.fg_color32()
            };
            let icon_rect = rect.shrink2(Vec2::splat(5.0));
            draw_fn(&painter, icon_rect, icon_color);
        }
        response
    }

    /// A small button with a text label (used for A/B markers + speed presets).
    fn text_button(
        &mut self,
        ui: &mut Ui,
        _id: &str,
        size: Vec2,
        label: &str,
        active: bool,
    ) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        if response.hovered() {
            self.note_user_activity();
        }
        if ui.is_rect_visible(rect) {
            let painter = ui.painter_at(rect);
            let bg = if active {
                egui::Color32::from_rgba_unmultiplied(
                    self.theme.accent[0],
                    self.theme.accent[1],
                    self.theme.accent[2],
                    60,
                )
            } else if response.hovered() {
                self.theme.button_bg_hover_color32()
            } else {
                self.theme.button_bg_color32()
            };
            painter.rect_filled(rect, 4.0, bg);
            let text_color = if active {
                self.theme.accent_color32()
            } else if response.hovered() {
                self.theme.accent_hover_color32()
            } else {
                self.theme.fg_color32()
            };
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(11.0),
                text_color,
            );
        }
        response
    }

    /// A button that shows a small colored pin + a time label below it.
    /// Used for A/B markers.
    fn labeled_button(
        &mut self,
        ui: &mut Ui,
        _id: &str,
        size: Vec2,
        label: &str,
        pin_color: egui::Color32,
    ) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        if response.hovered() {
            self.note_user_activity();
        }
        if ui.is_rect_visible(rect) {
            let painter = ui.painter_at(rect);
            let bg = if response.hovered() {
                self.theme.button_bg_hover_color32()
            } else {
                self.theme.button_bg_color32()
            };
            painter.rect_filled(rect, 4.0, bg);
            // Pin (small triangle at top).
            let pin_rect = egui::Rect::from_min_size(
                egui::pos2(rect.center().x - 6.0, rect.min.y + 3.0),
                Vec2::new(12.0, 8.0),
            );
            let pin_pts = vec![
                egui::pos2(pin_rect.min.x, pin_rect.min.y),
                egui::pos2(pin_rect.max.x, pin_rect.min.y),
                egui::pos2(pin_rect.center().x, pin_rect.max.y),
            ];
            painter.add(egui::Shape::convex_polygon(pin_pts, pin_color, egui::Stroke::NONE));
            // Label.
            painter.text(
                egui::pos2(rect.center().x, rect.max.y - 4.0),
                egui::Align2::CENTER_BOTTOM,
                label,
                egui::FontId::proportional(10.0),
                self.theme.fg_color32(),
            );
        }
        response
    }
}

/// Draw a small downward-pointing pin on the seek bar at (x, y).
fn draw_marker_pin(painter: &egui::Painter, pos: egui::Pos2, color: egui::Color32) {
    let pts = vec![
        egui::pos2(pos.x - 4.0, pos.y),
        egui::pos2(pos.x + 4.0, pos.y),
        egui::pos2(pos.x, pos.y + 6.0),
    ];
    painter.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
}

/// Format a duration in seconds as "HH:MM:SS" if >= 1 hour, else "MM:SS".
fn format_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m:02}:{sec:02}")
    }
}

/// Build a suggested default filename for the A-B loop video export:
/// `<input-stem>_<A-seconds>-<B-seconds>.mp4`. Returns None unless both
/// markers are set (the caller pre-checks anyway).
fn build_export_suggestion(state: &PlaybackState) -> Option<String> {
    let a = state.marker_a?;
    let b = state.marker_b?;
    let stem = state
        .path
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_stem())
        .and_then(|s| s.to_str())
        .map(sanitize_filename_stem)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "clip".to_string());
    Some(format!("{}_{}-{}.mp4", stem, a.max(0.0) as u64, b.max(0.0) as u64))
}

/// Keep only filename-safe characters ([A-Za-z0-9._-]); replace the rest
/// with underscores so the suggestion works across filesystems.
fn sanitize_filename_stem(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Ensure the export path ends in a video container extension ffmpeg can
/// infer a muxer from. Appends `.mp4` when the extension is missing or not
/// one of the supported save formats (mp4/mkv/webm).
fn ensure_video_extension(path: String) -> String {
    const KNOWN: &[&str] = &["mp4", "mkv", "webm"];
    let ext = std::path::Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some(e) if KNOWN.contains(&e) => path,
        _ => format!("{path}.mp4"),
    }
}

/// Final path component of a file path (the name shown in the queue rows).
fn file_basename(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

/// Truncate a label to fit `width` px, appending "…" when cut. Rough
/// per-character estimate (~6px at 11px proportional) — only used for row
/// labels, where being a char off is invisible.
fn truncate_to_width(s: &str, width: f32) -> String {
    let max_chars = ((width / 6.0).floor() as usize).max(1);
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Does this path look like a playlist file we can parse (.m3u/.m3u8/.pls)?
fn is_playlist_file(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "m3u" | "m3u8" | "pls")
}

/// Resolve one playlist entry against the directory the playlist lives in.
/// Absolute paths and URLs pass through untouched.
fn resolve_playlist_entry(base_dir: &std::path::Path, entry: &str) -> String {
    let trimmed = entry.trim();
    if trimmed.starts_with('/')
        || trimmed.contains("://")
        || std::path::Path::new(trimmed).is_absolute()
    {
        return trimmed.to_string();
    }
    base_dir
        .join(trimmed)
        .to_string_lossy()
        .into_owned()
}

/// Parse .m3u/.m3u8 content: one path per line, `#`-lines are directives
/// (EXTM3U, EXTINF, ...) and are skipped. Relative entries resolve against
/// `base_dir`.
fn parse_m3u_content(content: &str, base_dir: &std::path::Path) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| resolve_playlist_entry(base_dir, l))
        .collect()
}

/// Parse .pls content: `File1=path`, `File2=path`, ... entries (key matched
/// case-insensitively; indices keep their numeric order even if the file
/// lists them out of order). Relative entries resolve against `base_dir`.
fn parse_pls_content(content: &str, base_dir: &std::path::Path) -> Vec<String> {
    let mut numbered: Vec<(usize, String)> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('[') || line.starts_with('#') {
            continue;
        }
        let Some((key, val)) = line.split_once('=') else { continue };
        let key = key.trim().to_ascii_lowercase();
        let idx = key.strip_prefix("file").and_then(|n| n.parse::<usize>().ok());
        if let Some(n) = idx {
            numbered.push((n, resolve_playlist_entry(base_dir, val)));
        }
    }
    numbered.sort_by_key(|(n, _)| *n);
    numbered.into_iter().map(|(_, p)| p).collect()
}

/// Read a playlist file from disk and expand it to its entries. Returns
/// None when the file can't be read or the extension is unknown.
fn read_playlist_file(path: &str) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(path).ok()?;
    let base_dir = std::path::Path::new(path).parent()?.to_path_buf();
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "m3u" | "m3u8" => Some(parse_m3u_content(&content, &base_dir)),
        "pls" => Some(parse_pls_content(&content, &base_dir)),
        _ => None,
    }
}

/// Expand a "Load Playlist" selection: plain media files pass through;
/// .m3u/.m3u8/.pls files are replaced by their parsed entries (in order).
/// Returns (paths, skipped) where `skipped` counts playlist files that
/// could not be read or parsed.
fn expand_playlist_selection(paths: &[String]) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for p in paths {
        if is_playlist_file(p) {
            match read_playlist_file(p) {
                Some(entries) if !entries.is_empty() => out.extend(entries),
                _ => skipped += 1,
            }
        } else {
            out.push(p.clone());
        }
    }
    (out, skipped)
}

/// Write an extended .m3u playlist: a header, then an EXTINF line (title =
/// file name) followed by the path, per entry, in the exact order given.
/// Paths are written as-is, so absolute queues stay portable across mounts.
fn write_m3u(path: &str, entries: &[String]) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = String::from("#EXTM3U\n");
    for e in entries {
        let title = file_basename(e);
        out.push_str(&format!("#EXTINF:-1,{title}\n{e}\n"));
    }
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let mut f = std::fs::File::create(path)?;
    f.write_all(out.as_bytes())?;
    Ok(())
}

/// Ensure a save path ends in a playlist extension; appends `.m3u` when the
/// extension is missing or isn't one of m3u/m3u8.
fn ensure_m3u_extension(path: String) -> String {
    const KNOWN: &[&str] = &["m3u", "m3u8"];
    let ext = std::path::Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some(e) if KNOWN.contains(&e) => path,
        _ => format!("{path}.m3u"),
    }
}

/// Enumerate a directory for video/audio files. Returns absolute paths
/// sorted alphabetically. Used by the in-UI "Load Folder" dialog to build
/// an implicit playlist.
fn collect_folder_as_playlist(dir: &str) -> Option<Vec<String>> {
    use std::fs;
    let entries = fs::read_dir(dir).ok()?;
    const EXTENSIONS: &[&str] = &[
        "mp4", "mkv", "webm", "avi", "mov", "flv", "mp3", "ogg", "wav", "flac",
        "aac", "m4a", "ts", "m2ts", "vob", "wmv", "3gp",
    ];
    let mut paths: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            let ext = p.extension()?.to_str()?.to_lowercase();
            EXTENSIONS
                .contains(&ext.as_str())
                .then(|| p.to_string_lossy().into_owned())
        })
        .collect();
    paths.sort();
    Some(paths)
}

#[cfg(test)]
mod queue_playlist_tests {
    use super::*;

    /// Unique temp dir for one test run.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ferret_test_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn m3u_round_trip_preserves_order() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("list.m3u");
        let path_str = path.to_string_lossy().into_owned();
        let entries = vec![
            "/media/a.mp4".to_string(),
            "/media/z ones/b.mkv".to_string(),
            "/media/c.mp3".to_string(),
        ];
        write_m3u(&path_str, &entries).unwrap();
        assert_eq!(read_playlist_file(&path_str).unwrap(), entries);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn m3u_parse_skips_directives_and_resolves_relative() {
        let base = std::path::Path::new("/media/show");
        let content = "#EXTM3U\n\
                       #EXTINF:-1,Episode 1\n\
                       /abs/one.mp4\n\
                       ep2.mkv\n\
                       \n\
                       # comment\n\
                       http://example.com/stream\n";
        assert_eq!(
            parse_m3u_content(content, base),
            vec![
                "/abs/one.mp4".to_string(),
                "/media/show/ep2.mkv".to_string(),
                "http://example.com/stream".to_string(),
            ]
        );
    }

    #[test]
    fn pls_parse_orders_by_file_index() {
        let base = std::path::Path::new("/music");
        let content = "[playlist]\n\
                       NumberOfEntries=2\n\
                       File2=second.ogg\n\
                       Title1=ignored\n\
                       file1=/abs/first.ogg\n";
        assert_eq!(
            parse_pls_content(content, base),
            vec![
                "/abs/first.ogg".to_string(),
                "/music/second.ogg".to_string(),
            ]
        );
    }

    #[test]
    fn expand_selection_mixes_media_and_playlists() {
        let dir = temp_dir("expand");
        let pl = dir.join("pl.m3u");
        std::fs::write(&pl, "#EXTM3U\n/x/a.mp4\n/x/b.mp4\n").unwrap();
        let bad = dir.join("bad.m3u"); // header only → no entries.
        std::fs::write(&bad, "#EXTM3U\n").unwrap();
        let missing = dir.join("missing.m3u"); // does not exist → skipped.

        let (expanded, skipped) = expand_playlist_selection(&[
            "/direct/file.mp4".to_string(),
            pl.to_string_lossy().into_owned(),
            bad.to_string_lossy().into_owned(),
            missing.to_string_lossy().into_owned(),
        ]);
        assert_eq!(
            expanded,
            vec!["/direct/file.mp4".to_string(), "/x/a.mp4".to_string(), "/x/b.mp4".to_string()]
        );
        // The header-only playlist yields no entries and the missing file
        // can't be read — both count as skipped.
        assert_eq!(skipped, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn m3u_extension_is_ensured_on_save() {
        assert_eq!(ensure_m3u_extension("/tmp/list".into()), "/tmp/list.m3u");
        assert_eq!(ensure_m3u_extension("/tmp/list.dat".into()), "/tmp/list.dat.m3u");
        assert_eq!(ensure_m3u_extension("/tmp/list.m3u".into()), "/tmp/list.m3u");
        assert_eq!(ensure_m3u_extension("/tmp/list.M3U8".into()), "/tmp/list.M3U8");
    }

    #[test]
    fn position_edit_converts_final_index_to_mpv_args() {
        // The queue sidebar's number entry feeds this converter; mpv's
        // insert-before semantics are exercised in player-core's tests.
        // Typing "1" on row 3 (of 4) inserts before the entry at 0.
        assert_eq!(player_core::cmd::playlist_move_args_for_final(3, 0, 4), (3, 0));
        // Typing "4" on row 0 (of 4) = last slot = append.
        assert_eq!(player_core::cmd::playlist_move_args_for_final(0, 3, 4), (0, 4));
    }

    #[test]
    fn labels_truncate_with_ellipsis() {
        assert_eq!(truncate_to_width("abc", 100.0), "abc");
        let short = truncate_to_width("a_very_long_filename.mp4", 30.0);
        assert!(short.ends_with('…'));
        assert!(short.chars().count() < "a_very_long_filename.mp4".chars().count());
    }
}

#[cfg(test)]
mod visibility_tests {
    use super::*;
    use crossbeam_channel::unbounded;
    use std::time::Duration;

    fn make_app() -> OverlayApp {
        let (_event_tx, event_rx) = unbounded();
        let (cmd_tx, _cmd_rx) = unbounded();
        OverlayApp::new(PlaybackState::default(), event_rx, cmd_tx)
    }

    /// Simulate the idle timer having run out `secs` seconds ago.
    fn idle_for(app: &mut OverlayApp, secs: u64) {
        app.last_mouse_move = Instant::now() - Duration::from_secs(secs);
    }

    #[test]
    fn hides_after_idle_without_input() {
        let mut app = make_app();
        app.auto_hide_secs = 3.0;
        app.visible = true;
        idle_for(&mut app, 5);
        assert!(!app.compute_visibility());
    }

    #[test]
    fn stationary_cursor_over_video_still_hides() {
        // Regression: the old code reset the idle timer on every frame while
        // the cursor was merely INSIDE the window, so the bars never hid
        // after the mouse stopped moving ("doesn't rehide after mouse
        // movement").
        let mut app = make_app();
        app.auto_hide_secs = 3.0;
        app.visible = true;
        app.set_mouse_inside(true);
        app.pointer_pos = Some(egui::pos2(640.0, 360.0)); // middle of the video
        idle_for(&mut app, 5);
        assert!(!app.compute_visibility());
    }

    #[test]
    fn resting_cursor_on_control_bar_keeps_bars_visible() {
        // A cursor parked ON the visible control bar should not have the bar
        // vanish beneath it.
        let mut app = make_app();
        app.auto_hide_secs = 3.0;
        app.visible = true;
        app.window_size = egui::vec2(1280.0, 720.0);
        app.set_mouse_inside(true);
        app.pointer_pos = Some(egui::pos2(640.0, 720.0 - 60.0));
        idle_for(&mut app, 5);
        assert!(app.compute_visibility());
    }

    #[test]
    fn resting_cursor_on_menu_strip_keeps_bars_visible() {
        let mut app = make_app();
        app.auto_hide_secs = 3.0;
        app.visible = true;
        app.window_size = egui::vec2(1280.0, 720.0);
        app.set_mouse_inside(true);
        app.pointer_pos = Some(egui::pos2(10.0, 10.0));
        idle_for(&mut app, 5);
        assert!(app.compute_visibility());
    }

    #[test]
    fn activity_reveals_bars() {
        // Regression: moving the mouse must re-show the bars.
        let mut app = make_app();
        app.visible = false;
        app.note_user_activity();
        assert!(app.visible);
        assert!(app.compute_visibility());
    }

    #[test]
    fn cursor_leaving_does_not_pin_bars_visible() {
        // set_mouse_inside(false) must not count as activity either.
        let mut app = make_app();
        app.auto_hide_secs = 3.0;
        app.visible = true;
        app.set_mouse_inside(false);
        app.pointer_pos = None;
        idle_for(&mut app, 5);
        assert!(!app.compute_visibility());
    }

    #[test]
    fn open_menu_is_sticky() {
        // A dropdown menu being open must never auto-hide the menu bar out
        // from under the user.
        let mut app = make_app();
        app.auto_hide_secs = 3.0;
        app.menu_popup_open = true;
        idle_for(&mut app, 60);
        assert!(app.compute_visibility());
    }

    #[test]
    fn about_panel_is_sticky() {
        let mut app = make_app();
        app.auto_hide_secs = 3.0;
        app.about_visible = true;
        idle_for(&mut app, 60);
        assert!(app.compute_visibility());
    }

    #[test]
    fn open_file_dialog_is_sticky() {
        let mut app = make_app();
        app.auto_hide_secs = 3.0;
        app.file_dialog =
            Some(crate::file_dialog::FileDialog::open(crate::file_dialog::FileDialogKind::LoadFile));
        idle_for(&mut app, 60);
        assert!(app.compute_visibility());
    }
}

#[cfg(test)]
mod export_name_tests {
    use super::*;

    fn state_with(path: Option<&str>, a: Option<f64>, b: Option<f64>) -> PlaybackState {
        PlaybackState {
            path: path.map(|p| p.to_string()),
            marker_a: a,
            marker_b: b,
            ..PlaybackState::default()
        }
    }

    #[test]
    fn export_suggestion_uses_stem_and_times() {
        let st = state_with(Some("/home/u/movies/My Video!.mp4"), Some(1.5), Some(83.4));
        assert_eq!(
            build_export_suggestion(&st).as_deref(),
            Some("My_Video__1-83.mp4")
        );
    }

    #[test]
    fn export_suggestion_needs_both_markers() {
        assert!(build_export_suggestion(&state_with(Some("/a/b.mp4"), Some(1.0), None)).is_none());
        assert!(build_export_suggestion(&state_with(Some("/a/b.mp4"), None, Some(2.0))).is_none());
    }

    #[test]
    fn export_suggestion_falls_back_to_clip() {
        let st = state_with(None, Some(1.0), Some(2.0));
        // No path, but both markers → the pre-check in draw_file_menu
        // requires a loaded file anyway; the helper stays total.
        // (state_with(None, ..) with markers: helper needs marker_a/b only.)
        assert_eq!(build_export_suggestion(&st).as_deref(), Some("clip_1-2.mp4"));
    }

    #[test]
    fn missing_extension_defaults_to_mp4() {
        assert_eq!(ensure_video_extension("/tmp/out".into()), "/tmp/out.mp4");
        assert_eq!(
            ensure_video_extension("/tmp/out.dat".into()),
            "/tmp/out.dat.mp4"
        );
    }

    #[test]
    fn known_extensions_are_kept() {
        assert_eq!(ensure_video_extension("/tmp/out.mp4".into()), "/tmp/out.mp4");
        assert_eq!(ensure_video_extension("/tmp/out.MKV".into()), "/tmp/out.MKV");
        assert_eq!(ensure_video_extension("/tmp/out.webm".into()), "/tmp/out.webm");
    }
}
