//! In-UI file browser — replaces external zenity/kdialog/rfd dialogs.
//!
//! The overlay is `WindowLevel::AlwaysOnTop`, which means external file
//! dialogs (separate OS windows) pop *under* the overlay — invisible to
//! the user. Drawing the file browser inside the egui overlay eliminates
//! this z-order conflict entirely: the browser is part of the overlay,
//! so it's always visible and always on top of the video window.
//!
//! Supported kinds:
//!   - LoadFile       (single select, media extensions)
//!   - LoadFolder     (directory select)
//!   - LoadPlaylist   (multi select, media + playlist extensions)
//!   - SaveMarkers    (save with filename, txt/json)
//!   - LoadSubtitle   (single select, subtitle extensions)
//!   - ImportMarkers  (single select, txt/json)
//!   - ExportVideo    (save with filename, mp4/mkv/webm)

use std::path::PathBuf;
use std::time::Instant;

use egui::{Color32, Context, Layout, Vec2};

use player_core::cmd::MarkerExportFormat;

use crate::theme::Theme;

/// Media file extensions for LoadFile / LoadFolder / LoadPlaylist.
const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "webm", "avi", "mov", "flv",
    "mp3", "ogg", "wav", "flac", "aac", "m4a",
    "ts", "m2ts", "vob", "wmv", "3gp",
];

/// Playlist file extensions for LoadPlaylist.
const PLAYLIST_EXTENSIONS: &[&str] = &["m3u", "m3u8", "pls"];

/// Subtitle file extensions for LoadSubtitle.
const SUBTITLE_EXTENSIONS: &[&str] = &[
    "srt", "ass", "ssa", "sub", "idx", "sup", "vtt", "smi", "lrc",
];

/// Marker file extensions for ImportMarkers.
const MARKER_EXTENSIONS: &[&str] = &["txt", "json"];

/// What kind of dialog to show. Determines title, filter, multi-select,
/// and what Cmd gets sent on confirm.
#[derive(Clone, Debug)]
pub enum FileDialogKind {
    LoadFile,
    LoadFolder,
    LoadPlaylist,
    /// Save the current queue (in the user-organized order) as an .m3u file.
    SavePlaylist,
    SaveMarkers(MarkerExportFormat),
    LoadSubtitle,
    ImportMarkers,
    ExportVideo,
}

impl FileDialogKind {
    fn title(&self) -> &'static str {
        match self {
            FileDialogKind::LoadFile => "Open File",
            FileDialogKind::LoadFolder => "Open Folder",
            FileDialogKind::LoadPlaylist => "Open Playlist (select multiple files)",
            FileDialogKind::SavePlaylist => "Save Playlist As",
            FileDialogKind::SaveMarkers(_) => "Export Markers",
            FileDialogKind::LoadSubtitle => "Open Subtitle File",
            FileDialogKind::ImportMarkers => "Import Markers",
            FileDialogKind::ExportVideo => "Export A-B Loop Video",
        }
    }

    fn is_save(&self) -> bool {
        matches!(
            self,
            FileDialogKind::SaveMarkers(_) | FileDialogKind::ExportVideo | FileDialogKind::SavePlaylist
        )
    }

    fn is_multi(&self) -> bool {
        matches!(self, FileDialogKind::LoadPlaylist)
    }

    fn is_folder(&self) -> bool {
        matches!(self, FileDialogKind::LoadFolder)
    }

    fn extensions(&self) -> &[&str] {
        match self {
            FileDialogKind::LoadFile => MEDIA_EXTENSIONS,
            FileDialogKind::LoadFolder => &[],
            FileDialogKind::LoadPlaylist => &[], // we filter in-code (media OR playlist)
            FileDialogKind::SavePlaylist => &["m3u", "m3u8"],
            FileDialogKind::SaveMarkers(fmt) => match fmt {
                MarkerExportFormat::Text => &["txt"],
                MarkerExportFormat::Json => &["json"],
            },
            FileDialogKind::LoadSubtitle => SUBTITLE_EXTENSIONS,
            FileDialogKind::ImportMarkers => MARKER_EXTENSIONS,
            FileDialogKind::ExportVideo => &["mp4", "mkv", "webm"],
        }
    }

    /// For LoadPlaylist, accept either media or playlist extensions.
    fn accepts(&self, ext: &str) -> bool {
        match self {
            FileDialogKind::LoadFile => MEDIA_EXTENSIONS.contains(&ext),
            FileDialogKind::LoadFolder => false, // folders handled separately
            FileDialogKind::LoadPlaylist => {
                MEDIA_EXTENSIONS.contains(&ext) || PLAYLIST_EXTENSIONS.contains(&ext)
            }
            FileDialogKind::SaveMarkers(_) => true, // save accepts any extension
            FileDialogKind::SavePlaylist => true,   // save accepts any extension
            FileDialogKind::LoadSubtitle => SUBTITLE_EXTENSIONS.contains(&ext),
            FileDialogKind::ImportMarkers => MARKER_EXTENSIONS.contains(&ext),
            FileDialogKind::ExportVideo => true, // save accepts any extension
        }
    }
}

/// One directory entry, cached for rendering.
#[derive(Clone, Debug)]
struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

/// The result of a completed dialog.
#[derive(Clone, Debug)]
pub enum FileDialogResult {
    /// User cancelled.
    Cancel,
    /// Single file or folder selected.
    Path(String),
    /// Multiple files selected (LoadPlaylist only).
    Paths(Vec<String>),
}

pub struct FileDialog {
    pub kind: FileDialogKind,
    current_dir: PathBuf,
    entries: Vec<Entry>,
    selected: Option<PathBuf>,
    /// Multi-select (LoadPlaylist). Stored as a set of paths.
    selected_multi: Vec<PathBuf>,
    /// Filename text input for save dialogs.
    filename: String,
    /// Error message (permission denied, etc.).
    error: Option<String>,
    /// When the dialog was opened — used for focus management.
    opened_at: Instant,
}

impl FileDialog {
    pub fn open(kind: FileDialogKind) -> Self {
        Self::open_with_filename(kind, None)
    }

    /// Open a dialog, optionally pre-filling the filename field (save
    /// dialogs). `suggested` gives the user a one-click default instead of
    /// forcing them to either type blind or click an existing file.
    pub fn open_with_filename(kind: FileDialogKind, suggested: Option<String>) -> Self {
        let start_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let mut dlg = Self {
            kind,
            current_dir: start_dir,
            entries: Vec::new(),
            selected: None,
            selected_multi: Vec::new(),
            filename: suggested.unwrap_or_default(),
            error: None,
            opened_at: Instant::now(),
        };
        dlg.refresh();
        dlg
    }

    /// Re-read the current directory. Sorts: directories first (alpha), then
    /// files (alpha). Clears on error and sets `error`.
    fn refresh(&mut self) {
        self.entries.clear();
        self.error = None;
        match std::fs::read_dir(&self.current_dir) {
            Ok(rd) => {
                let mut dirs: Vec<Entry> = Vec::new();
                let mut files: Vec<Entry> = Vec::new();
                for entry in rd.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let path = entry.path();
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let e = Entry { name, path, is_dir };
                    if is_dir {
                        dirs.push(e);
                    } else {
                        files.push(e);
                    }
                }
                dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                self.entries = dirs;
                self.entries.extend(files);
            }
            Err(e) => {
                self.error = Some(format!("Cannot read directory: {e}"));
            }
        }
    }

    /// Navigate to a directory. Falls back to the parent if it fails.
    fn navigate_to(&mut self, path: PathBuf) {
        if path.is_dir() {
            self.current_dir = path;
            self.selected = None;
            self.selected_multi.clear();
            self.refresh();
        }
    }

    /// Go up one level.
    fn go_up(&mut self) {
        if let Some(parent) = self.current_dir.parent() {
            self.navigate_to(parent.to_path_buf());
        }
    }

    /// Go to the user's home directory.
    fn go_home(&mut self) {
        if let Some(home) = std::env::var_os("HOME") {
            self.navigate_to(PathBuf::from(home));
        }
    }

    /// Confirm the selection. Returns the result if valid, None if the user
    /// needs to select something first.
    fn confirm(&self) -> Option<FileDialogResult> {
        if self.kind.is_folder() {
            // Folder select: return the selected directory, or fall back to
            // the current directory so the user can open the folder they're
            // already browsing without navigating up first.
            if let Some(sel) = &self.selected {
                if sel.is_dir() {
                    return Some(FileDialogResult::Path(sel.to_string_lossy().into_owned()));
                }
            }
            // No subfolder selected — use the current directory itself.
            return Some(FileDialogResult::Path(self.current_dir.to_string_lossy().into_owned()));
        }
        if self.kind.is_save() {
            // Save: need a filename.
            if self.filename.trim().is_empty() {
                return None;
            }
            let path = self.current_dir.join(&self.filename);
            return Some(FileDialogResult::Path(path.to_string_lossy().into_owned()));
        }
        if self.kind.is_multi() {
            // Multi-select: return all selected files.
            if self.selected_multi.is_empty() {
                return None;
            }
            let paths: Vec<String> = self
                .selected_multi
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            return Some(FileDialogResult::Paths(paths));
        }
        // Single file select.
        if let Some(sel) = &self.selected {
            return Some(FileDialogResult::Path(sel.to_string_lossy().into_owned()));
        }
        None
    }

    /// Draw the dialog as a modal overlay. Returns `Some(result)` when the
    /// user confirms or cancels, `None` while the dialog is still open.
    pub fn draw(&mut self, ctx: &Context, theme: &Theme) -> Option<FileDialogResult> {
        let mut result: Option<FileDialogResult> = None;

        let win_size = ctx.screen_rect().size();
        let dlg_w = 700.0_f32.min(win_size.x - 40.0).max(400.0);
        let dlg_h = 500.0_f32.min(win_size.y - 40.0).max(300.0);
        let dlg_x = (win_size.x - dlg_w) / 2.0;
        let dlg_y = (win_size.y - dlg_h) / 2.0;

        // Dim the background behind the dialog.
        let dim_rect = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("ferret_file_dialog_dim"),
        ));
        painter.rect_filled(
            dim_rect,
            0.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, 160),
        );

        let fg = theme.fg_color32();
        let fg_dim = theme.fg_dim_color32();
        let bg = theme.bg_color32();
        let accent = theme.accent_color32();

        egui::Area::new(egui::Id::new("ferret_file_dialog"))
            .fixed_pos(egui::pos2(dlg_x, dlg_y))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(bg)
                    .rounding(8.0)
                    .stroke(egui::Stroke::new(1.0_f32, accent))
                    .inner_margin(egui::Margin::symmetric(0.0, 0.0))
                    .show(ui, |ui| {
                        ui.set_min_size(Vec2::new(dlg_w, dlg_h));
                        ui.set_max_size(Vec2::new(dlg_w, dlg_h));

                        ui.vertical(|ui| {
                            // ---- Title bar ----
                            ui.horizontal(|ui| {
                                ui.add_space(12.0);
                                ui.label(
                                    egui::RichText::new(self.kind.title())
                                        .color(fg)
                                        .size(14.0)
                                        .strong(),
                                );
                                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.button("×").clicked() {
                                        result = Some(FileDialogResult::Cancel);
                                    }
                                });
                            });
                            ui.separator();

                            // ---- Path bar ----
                            ui.horizontal(|ui| {
                                ui.add_space(8.0);
                                if ui.button("↑ Up").clicked() {
                                    self.go_up();
                                }
                                if ui.button("Home").clicked() {
                                    self.go_home();
                                }
                                ui.label(
                                    egui::RichText::new("📁")
                                        .color(accent)
                                        .size(13.0),
                                );
                                ui.label(
                                    egui::RichText::new(
                                        self.current_dir.to_string_lossy().into_owned(),
                                    )
                                    .color(fg_dim)
                                    .size(12.0)
                                    .monospace(),
                                );
                            });
                            ui.separator();

                            // ---- File list ----
                            egui::ScrollArea::vertical()
                                .max_height(dlg_h - 140.0)
                                .show(ui, |ui| {
                                    if let Some(err) = &self.error {
                                        ui.add_space(8.0);
                                        ui.label(
                                            egui::RichText::new(err)
                                                .color(Color32::from_rgb(220, 80, 80))
                                                .size(12.0),
                                        );
                                    }
                                    if self.entries.is_empty() && self.error.is_none() {
                                        ui.add_space(8.0);
                                        ui.label(
                                            egui::RichText::new("(empty directory)")
                                                .color(fg_dim)
                                                .size(12.0),
                                        );
                                    }
                                    // Collect navigation requests — can't call
                                    // self.navigate_to() inside the for loop
                                    // because &self.entries borrows self immutably.
                                    let mut navigate_to: Option<PathBuf> = None;
                                    for entry in &self.entries {
                                        let is_selected = if self.kind.is_multi() {
                                            self.selected_multi.contains(&entry.path)
                                        } else {
                                            self.selected.as_ref() == Some(&entry.path)
                                        };
                                        let icon = if entry.is_dir { "📁" } else { "📄" };
                                        let label = format!("{icon}  {}", entry.name);

                                        // For non-folder dialogs, skip files that
                                        // don't match the extension filter.
                                        if !entry.is_dir && !self.kind.is_folder() {
                                            let ext = entry
                                                .path
                                                .extension()
                                                .and_then(|e| e.to_str())
                                                .map(|s| s.to_lowercase())
                                                .unwrap_or_default();
                                            if !self.kind.accepts(&ext) {
                                                continue;
                                            }
                                        }

                                        let row_resp = ui.add_sized(
                                            Vec2::new(ui.available_width(), 22.0),
                                            egui::SelectableLabel::new(is_selected, label),
                                        );
                                        if row_resp.clicked() {
                                            if entry.is_dir {
                                                // Single-click selects, double-click navigates.
                                                if self.kind.is_folder() {
                                                    self.selected = Some(entry.path.clone());
                                                }
                                                if row_resp.double_clicked() {
                                                    navigate_to = Some(entry.path.clone());
                                                }
                                            } else {
                                                if self.kind.is_multi() {
                                                    // Toggle selection.
                                                    if let Some(idx) =
                                                        self.selected_multi.iter().position(|p| p == &entry.path)
                                                    {
                                                        self.selected_multi.remove(idx);
                                                    } else {
                                                        self.selected_multi.push(entry.path.clone());
                                                    }
                                                } else {
                                                    self.selected = Some(entry.path.clone());
                                                    // For save dialogs, prefill the filename.
                                                    if self.kind.is_save() {
                                                        self.filename = entry.name.clone();
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    // Apply navigation after the borrow ends.
                                    if let Some(path) = navigate_to {
                                        self.navigate_to(path);
                                    }
                                });
                            ui.separator();

                            // ---- Filename input (save dialogs only) ----
                            if self.kind.is_save() {
                                ui.horizontal(|ui| {
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new("Filename:")
                                            .color(fg_dim)
                                            .size(12.0),
                                    );
                                    let resp = ui.add_sized(
                                        Vec2::new(dlg_w - 120.0, 20.0),
                                        egui::TextEdit::singleline(&mut self.filename),
                                    );
                                    // Focus the filename field on open.
                                    if self.opened_at.elapsed().as_millis() < 200 {
                                        resp.request_focus();
                                    }
                                });
                                ui.add_space(4.0);
                            }

                            // ---- Bottom buttons ----
                            ui.horizontal(|ui| {
                                ui.add_space(8.0);
                                let confirm_label = if self.kind.is_save() { "Save" } else { "Open" };
                                let can_confirm = self.confirm().is_some();
                                ui.add_enabled_ui(can_confirm, |ui| {
                                    if ui.button(confirm_label).clicked() {
                                        if let Some(r) = self.confirm() {
                                            result = Some(r);
                                        }
                                    }
                                });
                                if ui.button("Cancel").clicked() {
                                    result = Some(FileDialogResult::Cancel);
                                }
                                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                                    let count = if self.kind.is_multi() {
                                        self.selected_multi.len()
                                    } else if self.selected.is_some() {
                                        1
                                    } else {
                                        0
                                    };
                                    ui.label(
                                        egui::RichText::new(format!("{count} selected"))
                                            .color(fg_dim)
                                            .size(11.0),
                                    );
                                    ui.add_space(8.0);
                                });
                            });
                        });
                    });
            });

        result
    }
}
