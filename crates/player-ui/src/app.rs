//! The overlay UI logic — VLC-styled.
//!
//! Layout (bottom of screen):
//!   ┌──────────────────────────────────────────────────────────────────┐
//!   │  [≡]                                                              │
//!   │  00:12  ████████●░░░░░░░░░░░░░░░░░░  01:30  ← seek bar w/ time    │
//!   │  ┌────┬────┬────┬────┬────┐         ┌─────────┐  ┌────────────┐  │
//!   │  │ ▶  │ ■  │ ⏮  │ ⏭  │ ↙  │ 00:12  │ 🔊━━━━○ │  │ ⛶ fullscr  │  │
//!   │  └────┴────┴────┴────┴────┘         └─────────┘  └────────────┘  │
//!   │  ┌────┬────┬────┬────┬────┬────┬────┐                            │
//!   │  │ A  │ B  │ AB │ ⟳  │ ½× │ 1× │ 2× │  Audio: [eng ▾]            │
//!   │  └────┴────┴────┴────┴────┴────┴────┘                            │
//!   └──────────────────────────────────────────────────────────────────┘
//!
//! - Top-left ≡ hamburger opens the File menu (Load File / Folder / Playlist,
//!   Export Markers, Quit).
//! - Seek bar with A/B marker pins rendered on top.
//! - Bottom row 1: transport | time | volume + fullscreen.
//! - Bottom row 2: A/B markers, AB-loop toggle, loop-mode toggle, speed
//!   presets + slider, audio track dropdown.

use std::time::Instant;

use crossbeam_channel::Receiver;
use egui::{Color32, Context, Layout, Ui, Vec2};

use player_core::event::EngineEvent;
use player_core::state::PlaybackState;
use player_core::{Cmd, LoopMode};

use crate::icons;
use crate::theme::Theme;
use crate::widgets::seek_bar;

pub struct OverlayApp {
    pub state: PlaybackState,
    pub event_rx: Receiver<EngineEvent>,
    pub cmd_tx: crossbeam_channel::Sender<Cmd>,
    pub theme: Theme,
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
    /// Are we in fullscreen mode? (Mirrored from main app.)
    pub fullscreen: bool,

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

    /// Active in-UI file dialog (if any). When `Some`, the dialog renders as
    /// a modal overlay covering the controls. Replaces external zenity/kdialog
    /// /rfd dialogs which popped under the overlay's AlwaysOnTop window.
    pub file_dialog: Option<crate::file_dialog::FileDialog>,
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
            fullscreen: false,
            info_msg: None,
            info_expiry: None,
            speed_drag: None,
            pending_events: Vec::new(),
            about_visible: false,
            file_dialog: None,
        }
    }

    /// Push an egui input event (mouse move, click, etc.) into the buffer.
    /// Called by the main app's window event handler. The renderer drains
    /// these into RawInput at the start of each frame.
    pub fn push_event(&mut self, event: egui::Event) {
        self.pending_events.push(event);
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

    pub fn note_mouse_activity(&mut self) {
        self.last_mouse_move = Instant::now();
        self.visible = true;
    }

    pub fn set_mouse_inside(&mut self, inside: bool) {
        self.mouse_inside = inside;
        if inside {
            self.note_mouse_activity();
        }
    }

    pub fn set_fullscreen(&mut self, fs: bool) {
        self.fullscreen = fs;
    }

    pub fn compute_visibility(&mut self) -> bool {
        if self.mouse_inside || self.error_msg.is_some() || self.info_msg.is_some() {
            self.visible = true;
            self.last_mouse_move = Instant::now();
        } else if self.last_mouse_move.elapsed().as_secs_f64() > self.auto_hide_secs {
            self.visible = false;
        }
        self.visible
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

        // The File menu is ALWAYS visible — it must be reachable even when the
        // rest of the overlay has auto-hidden. This is the primary way to open
        // files, so it can't disappear after 3 seconds of mouse inactivity.
        // But if a file dialog is open, skip the menu — the modal handles all
        // interaction.
        if self.file_dialog.is_none() {
            self.draw_file_menu(ctx);
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
                painter.rect_filled(bar_rect, 0.0, self.theme.bg_color32());

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
    }

    /// Draw the File menu bar at the top-left corner. ALWAYS visible — not
    /// subject to auto-hide. Uses egui's built-in `menu_button` which handles
    /// popup open/close, click-outside-to-dismiss, and Escape-to-close
    /// automatically.
    fn draw_file_menu(&mut self, ctx: &Context) {
        // Collect commands to send after the UI closure (can't borrow self
        // for send() while the closure also borrows self for theme/state).
        let mut pending_cmds: Vec<Cmd> = Vec::new();
        let mut pending_about_toggle = false;
        // Collect file-dialog-open requests — can't open the dialog inside
        // the closure because it borrows self for the theme snapshot.
        let mut pending_dialog: Option<crate::file_dialog::FileDialogKind> = None;

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
        let about_visible = self.about_visible;

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
                            ui.menu_button(
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

                                    ui.separator();

                                    ui.label(
                                        egui::RichText::new("Markers")
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
                                    if ui.button("Toggle A-B Loop").clicked() {
                                        pending_cmds.push(Cmd::ToggleMarkerLoop);
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
                                        egui::RichText::new("Window")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);
                                    if ui.button("Toggle Fullscreen  (F)").clicked() {
                                        pending_cmds.push(Cmd::ToggleFullscreen);
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    if ui.button("Quit  (Q)").clicked() {
                                        pending_cmds.push(Cmd::Shutdown);
                                        ui.close_menu();
                                    }
                                },
                            );

                            // ---- Playback menu ----
                            ui.menu_button(
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

                                    ui.separator();

                                    ui.label(
                                        egui::RichText::new("Speed")
                                            .color(fg_dim).size(10.0).strong(),
                                    );
                                    ui.add_space(2.0);
                                    if ui.button("Speed Up +0.25×  (=)").clicked() {
                                        pending_cmds.push(Cmd::SetSpeed((speed + 0.25).min(4.0)));
                                        ui.close_menu();
                                    }
                                    if ui.button("Speed Down -0.25×  (-)").clicked() {
                                        pending_cmds.push(Cmd::SetSpeed((speed - 0.25).max(0.25)));
                                        ui.close_menu();
                                    }
                                    ui.separator();
                                    for &s in &[0.25_f32, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0] {
                                        let checked = (speed - s).abs() < 0.01;
                                        if ui.selectable_label(checked, format!("{:.2}x", s)).clicked() {
                                            pending_cmds.push(Cmd::SetSpeed(s));
                                            ui.close_menu();
                                        }
                                    }
                                },
                            );

                            // ---- Audio menu (only if tracks are available) ----
                            if has_audio {
                                let tracks = self.state.audio_tracks.clone();
                                let current = self.state.current_audio_track;
                                ui.menu_button(
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
                            }

                            // ---- Subtitles menu ----
                            {
                                let tracks = self.state.subtitle_tracks.clone();
                                let current = self.state.current_subtitle_track;
                                ui.menu_button(
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
                            }

                            // ---- Video menu (rotate + flip) ----
                            ui.menu_button(
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
                                },
                            );

                            // ---- Help menu ----
                            ui.menu_button(
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
                                        egui::RichText::new("ferret 1.0.0")
                                            .color(fg_dim).size(10.0),
                                    );
                                    ui.label(
                                        egui::RichText::new("GPL-2.0-or-later")
                                            .color(fg_dim).size(10.0),
                                    );
                                },
                            );

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
                            ui.label(
                                egui::RichText::new(format!("{status}{speed_str}{loop_str}"))
                                    .color(fg_dim).size(10.0),
                            );
                        });
                    });
            });

        // Send any commands that were collected during the UI pass.
        for cmd in pending_cmds {
            self.send(cmd);
        }
        // Apply the About toggle after the closure (can't mutate self inside).
        if pending_about_toggle {
            self.about_visible = !self.about_visible;
        }
        // Open the in-UI file dialog if a menu item requested one.
        if let Some(kind) = pending_dialog {
            // For ExportVideo, pre-check that A/B markers are set.
            if matches!(kind, crate::file_dialog::FileDialogKind::ExportVideo) {
                let (a, b) = (self.state.marker_a, self.state.marker_b);
                match (a, b) {
                    (Some(start), Some(end)) if end > start => {
                        self.file_dialog = Some(crate::file_dialog::FileDialog::open(kind));
                    }
                    _ => {
                        self.show_info("Set both A and B markers before exporting");
                    }
                }
            } else {
                self.file_dialog = Some(crate::file_dialog::FileDialog::open(kind));
            }
        }
        // Draw the About panel if visible.
        if self.about_visible {
            self.draw_about_panel(ctx);
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
                    // Send the real export command with the chosen path.
                    // Main app intercepts all ExportABLoopVideo Cmds and runs ffmpeg.
                    let _ = self.cmd_tx.send(Cmd::ExportABLoopVideo { path });
                }
                FileDialogKind::LoadPlaylist => {
                    // Single file from a multi-select dialog (shouldn't happen,
                    // but handle gracefully).
                    let _ = self.cmd_tx.send(Cmd::LoadFile {
                        path,
                        options: LoadOptions { mode: LoadModeKind::Replace, pause: false },
                    });
                }
            },
            FileDialogResult::Paths(paths) => match kind {
                FileDialogKind::LoadPlaylist => {
                    if paths.is_empty() {
                        self.show_info("No files selected");
                    } else {
                        // Use try_send (non-blocking) to avoid UI deadlock
                        // if the engine's command channel is full.
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
                        self.show_info(format!("Loaded {}/{} files", sent, paths.len()));
                    }
                }
                _ => {}
            },
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
                                egui::RichText::new("1.0.0")
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
                    self.note_mouse_activity();
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
                    if fs_resp.hovered() { self.note_mouse_activity(); }
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
                    if slider_resp.hovered() { self.note_mouse_activity(); }
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
                if slider_resp.hovered() { self.note_mouse_activity(); }

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
            self.note_mouse_activity();
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
            self.note_mouse_activity();
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
            self.note_mouse_activity();
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
            self.note_mouse_activity();
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
