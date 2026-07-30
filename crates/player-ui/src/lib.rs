//! egui + wgpu overlay UI for ferret.
//!
//! Architecture:
//!   - `OverlayApp`   : the egui::App impl that renders the control bar
//!   - `OverlayState` : latest playback state (mirrored from engine via channels)
//!   - `OverlayRenderer` : wgpu surface + egui_wgpu integration
//!
//! The overlay runs in the SAME winit event loop as the video window — they
//! are two windows owned by one EventLoop. This keeps input routing simple
//! and avoids IPC between windows.

#![allow(dead_code)]

pub mod app;
pub mod file_dialog;
pub mod icons;
pub mod renderer;
pub mod theme;
pub mod widgets;

pub use app::OverlayApp;
pub use file_dialog::{FileDialog, FileDialogKind, FileDialogResult};
pub use renderer::OverlayRenderer;
pub use theme::Theme;
