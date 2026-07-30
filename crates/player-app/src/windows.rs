//! Multi-window management for ferret.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use crossbeam_channel::Receiver;
use tracing::info;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize, Size};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};

use player_core::Cmd;

/// Hard clamp for window dimensions. X11's CreateWindow takes u16 width/height
/// (max 65535), so anything above that must be clamped BEFORE being handed to
/// winit — otherwise winit's `dimensions.0.try_into().unwrap()` panics with
/// TryFromIntError(PosOverflow). We've seen Xfwm4 return bogus values for
/// `_NET_FRAME_EXTENTS` (which winit uses for `outer_size()`) when libmpv has
/// attached to a window via `wid`, so any code path that reads WM-supplied
/// frame extents needs this guard.
const MAX_WINDOW_DIM: u32 = 16384;

fn clamp_dim(v: u32) -> u32 {
    v.clamp(1, MAX_WINDOW_DIM)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Video,
    Overlay,
    Unknown,
}

pub struct WindowManager {
    pub video: Option<Arc<Window>>,
    pub overlay: Option<Arc<Window>>,
    #[allow(dead_code)]
    pub cmd_rx: Option<Receiver<Cmd>>,
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            video: None,
            overlay: None,
            cmd_rx: None,
        }
    }

    pub fn create_video_window(&self, event_loop: &ActiveEventLoop) -> Result<Window> {
        let attrs = WindowAttributes::default()
            .with_title("ferret")
            .with_inner_size(LogicalSize::new(1280u32, 720u32))
            .with_resizable(true)
            .with_window_level(WindowLevel::Normal)
            .with_visible(true);
        let window = event_loop.create_window(attrs)?;
        info!(
            "created video window: {}x{}",
            window.inner_size().width,
            window.inner_size().height
        );
        Ok(window)
    }

    /// Read the video window's INNER geometry (the actual X11 window, not the
    /// WM-frame outer size). `inner_size()` queries `XGetGeometry` directly
    /// and is robust against WM/libmpv races. `outer_size()` queries
    /// `_NET_FRAME_EXTENTS`, which Xfwm4 has been observed to return bogus
    /// values for after libmpv attaches via `wid` — that path leads to a
    /// `TryFromIntError(PosOverflow)` panic inside winit.
    fn video_inner_geometry(video: &Window) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
        // inner_position can fail with NotSupportedError on some backends;
        // step down to (0,0) which the caller already does for outer_position.
        let pos = video
            .inner_position()
            .unwrap_or_else(|_| PhysicalPosition::new(0, 0));
        let size = video.inner_size();
        let pos = PhysicalPosition::new(pos.x.max(0), pos.y.max(0));
        let size = PhysicalSize::new(clamp_dim(size.width), clamp_dim(size.height));
        (pos, size)
    }

    pub fn create_overlay_window(
        &self,
        event_loop: &ActiveEventLoop,
        video: &Arc<Window>,
    ) -> Result<Window> {
        let (pos, size) = Self::video_inner_geometry(video);

        let attrs = WindowAttributes::default()
            .with_title("ferret-overlay")
            .with_decorations(false)
            .with_transparent(true)
            .with_inner_size(size)
            .with_position(pos)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_resizable(false)
            .with_visible(true)
            .with_min_inner_size(Size::Physical(PhysicalSize::new(64, 64)));

        let window = event_loop.create_window(attrs)?;
        info!(
            "created overlay window at {:?} size {}x{}",
            pos, size.width, size.height
        );
        Ok(window)
    }

    pub fn sync_overlay_to_video(&self) -> Result<()> {
        let video = self.video.as_ref().context("no video window")?;
        let overlay = self.overlay.as_ref().context("no overlay window")?;

        let (pos, size) = Self::video_inner_geometry(video);

        overlay.set_outer_position(pos);
        // request_inner_size returns Option<PhysicalSize<u32>> on X11 (always
        // None on Wayland). Compare inner_size() — outer_size() on the overlay
        // itself is fine because the overlay has no decorations.
        if overlay.inner_size() != size {
            let _ = overlay.request_inner_size(Size::Physical(size));
        }
        Ok(())
    }

    pub fn classify(&self, id: WindowId) -> WindowKind {
        // Step-down: video first, then overlay, then Unknown. Order matters
        // only for diagnostics — a WindowId is owned by exactly one window.
        [
            (self.video.as_ref().map(|w| w.id()), WindowKind::Video),
            (self.overlay.as_ref().map(|w| w.id()), WindowKind::Overlay),
        ]
        .iter()
        .find(|(maybe_id, _)| maybe_id.is_some_and(|x| x == id))
        .map(|(_, k)| *k)
        .unwrap_or(WindowKind::Unknown)
    }
}
