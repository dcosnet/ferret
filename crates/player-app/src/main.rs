//! ferret — main binary.
//!
//! Architecture:
//!   1. Parse CLI args (just the file path).
//!   2. Create winit event loop (single event loop, multiple windows).
//!   3. Create the "video window" — a plain winit window we hand to libmpv
//!      via the `wid` option (X11 only today; Wayland waits on the libmpv
//!      render-context API).
//!   4. Create the "overlay window" — transparent, borderless, always-on-top,
//!      sized to overlap the video window. egui + wgpu renders controls here.
//!   5. Construct the engine with the video window's XID as `wid`.
//!   6. Run the event loop. Each RedrawRequested:
//!      - For video window: do nothing (libmpv renders into it directly).
//!      - For overlay window: pull latest state, render egui frame.
//!   7. Send Cmds to the engine based on user input (keyboard, mouse, UI events).
//!
//! File dialogs (Load File / Folder / Playlist / Export Markers) run on a
//! worker thread because `rfd` blocks while the dialog is open. The worker
//! sends the result back via a channel polled from `about_to_wait`.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use crossbeam_channel::unbounded;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::WindowId;

use player_core::cmd::LoadModeKind;
use player_core::options::EngineOptions;
use player_core::{Cmd, PlayerEngine};
use player_ui::OverlayRenderer;

use x11rb::protocol::xproto;

mod keymap;
mod windows;

use windows::{WindowKind, WindowManager};

/// Result from a background ffmpeg export worker.
enum DialogResult {
    /// Export succeeded; the file was written to this path. `cropped` says
    /// whether the current zoom/pan focus area was baked in as a crop.
    File { path: String, cropped: bool },
    /// Export failed with this error message.
    Error(String),
}

/// Only one background operation remains: ffmpeg video export. All file
/// selection is now in-UI (see `player_ui::file_dialog`). The kind tags the
/// dialog-result channel so future worker kinds can be distinguished.
enum DialogKind {
    /// Result of an ffmpeg A-B loop export worker.
    ExportVideo,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn,ferret=info")),
        )
        .with_target(false)
        .init();

    // X11 today. Wayland waits on the libmpv render-context API.
    if std::env::var("WAYLAND_DISPLAY").is_ok() && std::env::var("DISPLAY").is_ok() {
        if std::env::var("FERRET_FORCE_WAYLAND").is_err() {
            // SAFETY: `set_var` is unsafe as of Rust 2024 due to getenv races.
            // We run before spawning any thread, so no concurrent reader exists.
            unsafe { std::env::set_var("WAYLAND_DISPLAY", ""); }
            info!("Wayland detected; locking to X11 (set FERRET_FORCE_WAYLAND=1 to override)");
        }
    }

    let args: Vec<String> = std::env::args().collect();
    let file_path = if args.len() >= 2 {
        Some(args[1].clone())
    } else {
        None
    };
    if let Some(ref p) = file_path {
        info!("will load: {p}");
    } else {
        info!("no file argument; launching empty (use: ferret <path>)");
    }

    let event_loop = EventLoop::new()?;
    let engine_options = EngineOptions::default();
    let mut app = FerretApp::new(engine_options, file_path);
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct FerretApp {
    engine_options: EngineOptions,
    initial_file: Option<String>,
    windows: WindowManager,
    engine: Option<PlayerEngine>,
    overlay: Option<OverlayRenderer>,
    last_overlay_render: Instant,
    overlay_mouse_pos: Option<egui::Pos2>,
    fullscreen: bool,
    /// Channel for commands emitted by the overlay UI (forwarded to the engine).
    cmd_tx: crossbeam_channel::Sender<Cmd>,
    cmd_rx: crossbeam_channel::Receiver<Cmd>,
    /// Channel for dialog results coming back from worker threads.
    /// We pair each result with the kind of dialog that produced it so we
    /// know what Cmd to emit once we have the path.
    dialog_result_rx: crossbeam_channel::Receiver<(DialogKind, DialogResult)>,
    dialog_result_tx: crossbeam_channel::Sender<(DialogKind, DialogResult)>,
    /// Whether either of our windows currently has keyboard focus.
    /// When false, the overlay drops from AlwaysOnTop so it doesn't block
    /// other applications.
    has_focus: bool,
    /// Latest keyboard modifier state (ctrl/alt/shift/super), tracked via
    /// WindowEvent::ModifiersChanged. winit 0.30 KeyEvents do not carry
    /// modifiers, so we keep the state here for key→egui translation.
    keyboard_modifiers: ModifiersState,
    /// The overlay bounding shape last applied to the X server. Kept so we
    /// only issue an XShape request when egui's painted rects actually
    /// changed (show/hide of bars, opening a dialog, tooltips, ...).
    last_shape_rects: Vec<egui::Rect>,
    /// Last pointer position while a pan drag is in progress on the video
    /// area (the "movable object on a canvas" gesture). None = not
    /// dragging. Pointer events only reach the video window through the
    /// holes in the overlay's X11 shape, so a drag that starts here is by
    /// construction over the video — never over the bars or sidebar.
    video_pan_drag: Option<(f64, f64)>,
}

impl FerretApp {
    fn new(engine_options: EngineOptions, initial_file: Option<String>) -> Self {
        let (cmd_tx, cmd_rx) = unbounded::<Cmd>();
        let (dialog_result_tx, dialog_result_rx) = unbounded::<(DialogKind, DialogResult)>();
        Self {
            engine_options,
            initial_file,
            windows: WindowManager::new(),
            engine: None,
            overlay: None,
            last_overlay_render: Instant::now(),
            overlay_mouse_pos: None,
            fullscreen: false,
            cmd_tx,
            cmd_rx,
            dialog_result_rx,
            dialog_result_tx,
            has_focus: true,
            keyboard_modifiers: ModifiersState::empty(),
            last_shape_rects: Vec::new(),
            video_pan_drag: None,
        }
    }

    fn setup(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        // 1. Create video window.
        let video_window = self.windows.create_video_window(event_loop)?;
        let video_window_arc = Arc::new(video_window);

        // 1b. Set the X11 background pixel on the video window. winit creates
        // windows with background_pixel=None, which means the X server shows
        // framebuffer garbage (stale content from other windows) during a
        // resize — before libmpv has a chance to repaint. Setting the
        // background to dark grey (#141416, matching the VLC theme) makes the
        // X server fill the window with that color on resize, eliminating the
        // "mirrored desktop" artifact. This MUST happen before libmpv attaches
        // via wid.
        set_x11_window_background(&video_window_arc, 0x141416);

        // 2. Get its X11 XID.
        let wid = extract_x11_xid(&video_window_arc)?;
        info!("video window XID: {wid}");

        // 3. Construct engine with wid patched in.
        self.engine_options.wid = Some(wid.to_string());
        let mut engine = PlayerEngine::new(self.engine_options.clone())?;
        engine.start().context("engine start")?;

        // 4. Take the event receiver (single-consumer).
        let event_rx = engine
            .take_event_receiver()
            .ok_or_else(|| anyhow::anyhow!("engine event receiver already taken"))?;

        // 5. Load initial file.
        if let Some(path) = self.initial_file.clone() {
            engine.send(Cmd::LoadFile {
                path,
                options: player_core::cmd::LoadOptions {
                    mode: LoadModeKind::Replace,
                    pause: false,
                },
            })?;
        }

        self.engine = Some(engine);

        // 6. Create overlay window.
        let overlay_window = self
            .windows
            .create_overlay_window(event_loop, &video_window_arc)?;
        let overlay_window_arc = Arc::new(overlay_window);

        // 7. Create overlay renderer.
        let state = self.engine.as_ref().unwrap().state();
        let overlay_renderer = OverlayRenderer::new(
            overlay_window_arc.clone(),
            state,
            event_rx,
            self.cmd_tx.clone(),
        )?;

        self.overlay = Some(overlay_renderer);
        self.windows.video = Some(video_window_arc);
        self.windows.overlay = Some(overlay_window_arc);

        // 8. Tell X11 the overlay is a helper window for the video window.
        //    This makes the WM:
        //    - Skip the overlay in the taskbar / alt-tab list
        //    - Keep the overlay visually grouped with the video window
        //    - Un-fullscreen both when focus leaves
        set_x11_overlay_hints(
            self.windows.video.as_ref().unwrap(),
            self.windows.overlay.as_ref().unwrap(),
        );

        self.request_redraw_both();
        Ok(())
    }

    /// Pop any pending ffmpeg export results and convert them to info
    /// toast messages. The only remaining use of the dialog-result channel
    /// — all file selection is now in-UI.
    fn poll_dialog_results(&mut self) -> Vec<String> {
        let mut infos: Vec<String> = Vec::new();
        while let Ok((kind, result)) = self.dialog_result_rx.try_recv() {
            match (kind, result) {
                (DialogKind::ExportVideo, DialogResult::File { path, cropped }) => {
                    infos.push(if cropped {
                        format!("A-B loop exported (with zoom/pan focus area) to {path}")
                    } else {
                        format!("A-B loop exported to {path}")
                    });
                }
                (DialogKind::ExportVideo, DialogResult::Error(msg)) => {
                    infos.push(format!("Export failed: {msg}"));
                }
            }
        }
        infos
    }

    fn render_overlay(&mut self, event_loop: &ActiveEventLoop) {
        let Some(engine) = self.engine.as_ref() else { return; };

        // Forward commands from the overlay UI to the engine. Three
        // commands are intercepted here (not sent to the engine):
        //   - ToggleFullscreen: main-app window concern
        //   - ExportABLoopVideo: main-app runs ffmpeg (engine no-ops it)
        //   - Shutdown: menu Quit must exit the whole event loop — the
        //     engine alone only stops the playback thread and the app
        //     window would hang around forever.
        let mut pending_fullscreen_toggle = false;
        let mut pending_quit = false;
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match &cmd {
                Cmd::Shutdown => {
                    // File → Quit. Exit the winit event loop; FerretApp (and
                    // with it the engine + windows) is dropped on return,
                    // which cleanly shuts the engine thread down.
                    pending_quit = true;
                }
                Cmd::ToggleFullscreen => {
                    // Window-level concern — handled after the render below.
                    pending_fullscreen_toggle = true;
                }
                Cmd::ExportABLoopVideo { path } => {
                    // The in-UI file dialog already validated A/B markers
                    // and sent this with the real output path. Read A/B +
                    // input from engine state, then spawn ffmpeg.
                    let st = engine.state();
                    let a = st.marker_a;
                    let b = st.marker_b;
                    let input = st.path.clone();
                    // Bake the current zoom/pan focus area into the clip:
                    // map the visible window region back to source pixels
                    // and crop. Only when the view is actually realigned,
                    // and only without rotation — the export doesn't remap
                    // rotated coordinates (it never did).
                    let crop = if st.video_rotate == 0 {
                        self.windows.video.as_ref().and_then(|w| {
                            let size = w.inner_size();
                            player_core::crop::visible_crop(
                                st.video_width,
                                st.video_height,
                                size.width,
                                size.height,
                                st.video_zoom,
                                st.video_pan_x,
                                st.video_pan_y,
                            )
                        })
                    } else {
                        None
                    };
                    drop(st);
                    match (a, b, input) {
                        (Some(start), Some(end), Some(inp)) if end > start => {
                            spawn_ffmpeg_export(
                                inp,
                                start,
                                end,
                                path.clone(),
                                crop,
                                self.dialog_result_tx.clone(),
                            );
                        }
                        _ => {
                            let _ = self.dialog_result_tx.send((
                                DialogKind::ExportVideo,
                                DialogResult::Error(
                                    "Set both A and B markers before exporting".into(),
                                ),
                            ));
                        }
                    }
                }
                _ => {
                    let _ = engine.send(cmd);
                }
            }
        }

        // Poll for ffmpeg export results.
        let _ = engine;
        let infos = self.poll_dialog_results();
        for info in infos {
            if let Some(overlay) = self.overlay.as_mut() {
                overlay.app.show_info(info);
            }
        }

        let Some(overlay) = self.overlay.as_mut() else { return; };
        let Some(engine) = self.engine.as_ref() else { return; };
        let state = engine.state();
        if let Err(e) = overlay.render(state, self.overlay_mouse_pos) {
            warn!("overlay render: {e}");
        }
        self.last_overlay_render = Instant::now();

        // Mirror this frame's painted rects onto the overlay window as its
        // X11 bounding shape. Everything egui did NOT paint becomes a hole
        // in the window — the libmpv video window underneath shows through
        // unconditionally (no compositor / alpha-mode / visual involved).
        // This is the structural fix for the recurring "no video output"
        // class of bugs: the overlay can no longer blanket the video with a
        // possibly-opaque surface.
        let painted = overlay.painted_rects.clone();
        let overlay_window = self.windows.overlay.clone();
        if let Some(w) = overlay_window {
            if painted != self.last_shape_rects {
                apply_overlay_shape(&w, &painted);
                self.last_shape_rects = painted;
            }
        }

        if pending_quit {
            info!("quit requested via menu — exiting event loop");
            event_loop.exit();
            return;
        }
        if pending_fullscreen_toggle {
            self.toggle_fullscreen();
        }
    }

    /// Forward a pointer position received on the VIDEO window into the
    /// egui overlay (the overlay's bounding shape has holes over the video,
    /// so those events arrive here instead). Same coordinate space.
    fn forward_video_pointer(&mut self, position: winit::dpi::PhysicalPosition<f64>) {
        let pos = egui::pos2(position.x as f32, position.y as f32);
        self.overlay_mouse_pos = Some(pos);
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.app.push_event(egui::Event::PointerMoved(pos));
            // Moving the mouse over the video is user activity: (re)show
            // the menu bar and control bar.
            overlay.app.note_user_activity();
        }
        self.request_redraw_overlay();
    }

    /// Feed a video-window pointer move into an active pan drag. Deltas are
    /// converted to screen fractions and sent to the engine as
    /// `Cmd::AdjustVideoPan` — the video follows the pointer like an object
    /// being dragged around a canvas, letting the user realign whatever
    /// area they want in focus.
    fn pan_video_by_drag(&mut self, position: winit::dpi::PhysicalPosition<f64>) {
        let Some((last_x, last_y)) = self.video_pan_drag else { return };
        let Some(video) = self.windows.video.clone() else { return };
        let size = video.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let dx = (position.x - last_x) as f32 / size.width as f32;
        let dy = (position.y - last_y) as f32 / size.height as f32;
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        self.video_pan_drag = Some((position.x, position.y));
        if let Some(engine) = self.engine.as_ref() {
            let _ = engine.send(Cmd::AdjustVideoPan { dx, dy });
        }
    }

    /// Request a redraw on both windows. Single dispatch point so callers
    /// never have to repeat the `if let Some(w) = ...` dance.
    fn request_redraw_both(&self) {
        if let Some(w) = self.windows.video.as_ref() {
            w.request_redraw();
        }
        if let Some(w) = self.windows.overlay.as_ref() {
            w.request_redraw();
        }
    }

    /// Request a redraw on the overlay window only.
    fn request_redraw_overlay(&self) {
        if let Some(w) = &self.windows.overlay {
            w.request_redraw();
        }
    }

    fn handle_keyboard(&mut self, event_loop: &ActiveEventLoop, key_event: &KeyEvent) {
        // If the overlay UI has a focused text field (e.g. the save-dialog
        // filename input), route the keystroke into egui instead of the
        // global hotkeys. Otherwise typing "q" in a filename would quit,
        // space would pause, etc.
        let ui_wants_keyboard = self
            .overlay
            .as_ref()
            .map(|o| o.app.ui_wants_keyboard)
            .unwrap_or(false);
        if ui_wants_keyboard {
            let events = key_event_to_egui_events(key_event, self.keyboard_modifiers);
            if !events.is_empty() {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.app.push_events(events);
                }
                self.request_redraw_overlay();
                return;
            }
        }

        // Hotkeys only act on key PRESS; releases are only interesting to
        // egui (handled above).
        if key_event.state != ElementState::Pressed {
            return;
        }

        // `q` and `f` are window-level concerns; they never reach the engine.
        match &key_event.logical_key {
            Key::Character(s) if s == "q" || s == "Q" => {
                event_loop.exit();
                return;
            }
            Key::Character(s) if s == "f" || s == "F" => {
                self.toggle_fullscreen();
                return;
            }
            _ => {}
        }
        let Some(engine) = self.engine.as_ref() else { return; };
        // Snapshot the current playback state for keys that depend on
        // UI-derived values (loop mode, speed). Keeps `keymap.rs` decoupled
        // from `player-ui`.
        let state = engine.state();
        if let Some(cmd) = keymap::key_to_cmd(&key_event.logical_key, &state) {
            let _ = engine.send(cmd);
        }
        // Reveal the controls briefly (VLC-style) so the effect of the key
        // (pause/play toggle, seek jump, ...) is immediately visible.
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.app.note_user_activity();
        }
    }

    fn toggle_fullscreen(&mut self) {
        let Some(video) = self.windows.video.clone() else { return; };
        self.fullscreen = !self.fullscreen;
        if self.fullscreen {
            video.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
            // Also fullscreen the overlay so it covers the entire screen.
            // Without this, the overlay stays at the old windowed size while
            // the video fills the screen — controls would be cut off.
            if let Some(overlay_win) = self.windows.overlay.as_ref() {
                overlay_win.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
            }
        } else {
            video.set_fullscreen(None);
            if let Some(overlay_win) = self.windows.overlay.as_ref() {
                overlay_win.set_fullscreen(None);
            }
        }
        // Sync state to overlay so the fullscreen button reflects it.
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.app.set_fullscreen(self.fullscreen);
        }
    }

    /// Update the overlay window level based on focus state.
    /// When we have focus: overlay is AlwaysOnTop (controls visible above video).
    /// When we lose focus: overlay drops to Normal so it doesn't block other apps.
    fn update_overlay_focus(&mut self) {
        let Some(overlay_win) = self.windows.overlay.as_ref() else { return; };
        if self.has_focus {
            overlay_win.set_window_level(winit::window::WindowLevel::AlwaysOnTop);
        } else {
            overlay_win.set_window_level(winit::window::WindowLevel::Normal);
        }
    }
}

impl ApplicationHandler for FerretApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.engine.is_none() {
            if let Err(e) = self.setup(event_loop) {
                error!("setup failed: {e:#}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let kind = self.windows.classify(window_id);

        match kind {
            WindowKind::Video => match event {
                WindowEvent::Focused(gained) => {
                    self.has_focus = gained;
                    self.update_overlay_focus();
                }
                WindowEvent::ModifiersChanged(m) => {
                    self.keyboard_modifiers = m.state();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    // The X11 bounding shape routes pointer events that fall
                    // in the holes (over the video) to the VIDEO window, not
                    // the overlay. Re-route them into the egui overlay so
                    // hover state, bar auto-show, and widget interaction
                    // keep working over the whole window. Coordinates are
                    // identical: the overlay covers the video window's inner
                    // area exactly and pixels_per_point is 1.0.
                    self.forward_video_pointer(position);
                    // An active drag over the video pans it.
                    self.pan_video_by_drag(position);
                }
                WindowEvent::CursorLeft { .. } => {
                    self.overlay_mouse_pos = None;
                    self.video_pan_drag = None;
                    if let Some(overlay) = self.overlay.as_mut() {
                        overlay.app.push_event(egui::Event::PointerGone);
                    }
                    self.request_redraw_overlay();
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    // Click on the video area (delivered here because the
                    // overlay's shape has a hole there). Forward as an egui
                    // event so UI state (e.g. closing dropdowns) stays
                    // consistent with clicks on the overlay itself.
                    let egui_button = match button {
                        MouseButton::Left => egui::PointerButton::Primary,
                        MouseButton::Right => egui::PointerButton::Secondary,
                        MouseButton::Middle => egui::PointerButton::Middle,
                        _ => { return; }
                    };
                    let pressed = state == ElementState::Pressed;
                    if let (Some(overlay), Some(pos)) =
                        (self.overlay.as_mut(), self.overlay_mouse_pos)
                    {
                        overlay.app.push_event(egui::Event::PointerButton {
                            pos,
                            button: egui_button,
                            pressed,
                            modifiers: egui::Modifiers::default(),
                        });
                        // Clicking is user activity: (re)show the bars.
                        overlay.app.note_user_activity();
                    }
                    // Left-drag on the video = pan (movable-object gesture).
                    // Only meaningful with something on screen.
                    if matches!(button, MouseButton::Left) {
                        if pressed {
                            let has_file = self
                                .engine
                                .as_ref()
                                .map(|e| e.state().path.is_some())
                                .unwrap_or(false);
                            if has_file {
                                if let Some(pos) = self.overlay_mouse_pos {
                                    self.video_pan_drag = Some((pos.x as f64, pos.y as f64));
                                }
                            }
                        } else {
                            self.video_pan_drag = None;
                        }
                    }
                    self.request_redraw_overlay();
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    // Ctrl+wheel over the video zooms (next to the pan
                    // gesture). Without ctrl the wheel stays unused, as
                    // before. Wheel-up = zoom in.
                    if self.keyboard_modifiers.control_key() {
                        let step = match delta {
                            winit::event::MouseScrollDelta::LineDelta(_, y) => {
                                y * 0.1
                            }
                            winit::event::MouseScrollDelta::PixelDelta(p) => {
                                (p.y as f32 / 400.0).clamp(-0.5, 0.5)
                            }
                        };
                        if step != 0.0 {
                            if let Some(engine) = self.engine.as_ref() {
                                let _ = engine.send(Cmd::AdjustVideoZoom(step));
                            }
                            // Show the bars so the zoom status is visible.
                            if let Some(overlay) = self.overlay.as_mut() {
                                overlay.app.note_user_activity();
                            }
                        }
                    }
                    self.request_redraw_overlay();
                }
                WindowEvent::KeyboardInput { event: key_event, .. } => {
                    self.handle_keyboard(event_loop, &key_event);
                    self.request_redraw_overlay();
                }
                WindowEvent::Resized(_) | WindowEvent::Moved(_) => {
                    if let Err(e) = self.windows.sync_overlay_to_video() {
                        warn!("overlay sync: {e}");
                    }
                    // Proactively resize the overlay's wgpu surface to match
                    // the video window's new geometry. We can't wait for the
                    // overlay's own Resized event because:
                    //   1. During a MOVE, the overlay's size doesn't change,
                    //      so its Resized event never fires.
                    //   2. During a RESIZE, there's a delay between
                    //      request_inner_size() and the overlay's Resized
                    //      event. During that delay, get_current_texture()
                    //      returns Outdated and we can't paint.
                    // By calling resize() here, we reconfigure the surface
                    // immediately — the next render succeeds and paints an
                    // opaque dark grey frame that hides the video window's
                    // resize gap. resize() is a no-op if the size hasn't
                    // changed (e.g., during a pure move).
                    if let (Some(overlay), Some(video)) = (&mut self.overlay, &self.windows.video) {
                        let size = video.inner_size();
                        overlay.resize(size.width, size.height);
                    }
                    self.request_redraw_overlay();
                }
                WindowEvent::CloseRequested => {
                    event_loop.exit();
                }
                WindowEvent::RedrawRequested => {
                    // The video window itself doesn't render anything (libmpv
                    // owns it). But a RedrawRequested on the video window
                    // means the WM wants us to repaint — forward it to the
                    // overlay so the controls stay in sync.
                    self.render_overlay(event_loop);
                    self.request_redraw_overlay();
                }
                _ => {}
            },
            WindowKind::Overlay => match event {
                WindowEvent::Focused(gained) => {
                    self.has_focus = gained;
                    self.update_overlay_focus();
                }
                WindowEvent::ModifiersChanged(m) => {
                    self.keyboard_modifiers = m.state();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    // The renderer sets pixels_per_point=1.0, so egui's
                    // coordinate system matches physical pixels directly.
                    let pos = egui::pos2(position.x as f32, position.y as f32);
                    self.overlay_mouse_pos = Some(pos);
                    if let Some(overlay) = self.overlay.as_mut() {
                        overlay.app.push_event(egui::Event::PointerMoved(pos));
                        // Moving the mouse is user activity: (re)show the
                        // menu bar and control bar.
                        overlay.app.note_user_activity();
                    }
                    self.request_redraw_overlay();
                }
                WindowEvent::CursorLeft { .. } => {
                    self.overlay_mouse_pos = None;
                    if let Some(overlay) = self.overlay.as_mut() {
                        overlay.app.push_event(egui::Event::PointerGone);
                    }
                    self.request_redraw_overlay();
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    let egui_button = match button {
                        MouseButton::Left => egui::PointerButton::Primary,
                        MouseButton::Right => egui::PointerButton::Secondary,
                        MouseButton::Middle => egui::PointerButton::Middle,
                        _ => { return; }
                    };
                    let pressed = state == ElementState::Pressed;
                    if let (Some(overlay), Some(pos)) = (self.overlay.as_mut(), self.overlay_mouse_pos) {
                        overlay.app.push_event(egui::Event::PointerButton {
                            pos,
                            button: egui_button,
                            pressed,
                            modifiers: egui::Modifiers::default(),
                        });
                        // Clicking is user activity: (re)show the bars.
                        overlay.app.note_user_activity();
                    }
                    // Process the click immediately so dropdown menus open
                    // without waiting for the next render cycle. The
                    // RedrawRequested handler has a 16ms rate limit that can
                    // skip the render, and about_to_wait only fires on a 33ms
                    // timer — so without this eager render, the click sits in
                    // pending_events and the dropdown never appears until the
                    // mouse moves.
                    self.render_overlay(event_loop);
                    self.request_redraw_overlay();
                }
                WindowEvent::KeyboardInput { event: key_event, .. } => {
                    self.handle_keyboard(event_loop, &key_event);
                    self.request_redraw_overlay();
                }
                WindowEvent::Resized(_) | WindowEvent::Moved(_) => {
                    // The overlay itself was resized or moved. When the overlay
                    // moves (because we called set_outer_position in the video
                    // window's Moved handler), its wgpu surface can become
                    // stale. Re-suppress transparency and request a redraw so
                    // the renderer reconfigures the surface and paints a fresh
                    // opaque frame. Without this, the old transparent frame
                    // stays visible and the desktop shows through.
                    let new_size = self.windows.overlay.as_ref().map(|w| w.inner_size());
                    if let (Some(overlay), Some(size)) = (self.overlay.as_mut(), new_size) {
                        overlay.resize(size.width, size.height);
                    }
                    self.request_redraw_overlay();
                }
                WindowEvent::RedrawRequested => {
                    // Always render on RedrawRequested. The previous 16ms rate
                    // limit caused skipped frames during resize bursts and
                    // delayed dropdown menu opening. wgpu's PresentMode already
                    // throttles to the display refresh rate.
                    self.render_overlay(event_loop);
                }
                WindowEvent::CloseRequested => {
                    event_loop.exit();
                }
                _ => {}
            },
            WindowKind::Unknown => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Self-sustaining ~30fps repaint ticker.
        //
        // winit's default ControlFlow::Wait parks the event loop once the
        // last input event has been processed, and egui's
        // `Context::request_repaint_after` is not wired to winit here. Before
        // this ticker existed the overlay only repainted while events kept
        // arriving, which caused four visible bugs: a freshly loaded video
        // stayed black behind the last (opaque) startup frame until the mouse
        // moved, the progress bar / time display froze, the pause button icon
        // never flipped after clicking it, and the auto-hide never triggered.
        // Scheduling a WaitUntil wakeup at every frame boundary keeps engine
        // state flowing into the UI even with zero user input.
        if self.overlay.is_none() || self.engine.is_none() {
            // Setup hasn't completed — nothing to tick.
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }

        let has_pending_events = self
            .overlay
            .as_ref()
            .map(|o| !o.app.pending_events.is_empty())
            .unwrap_or(false);
        let tick_due = self.last_overlay_render.elapsed() > Duration::from_millis(33);
        if tick_due || !self.dialog_result_rx.is_empty() || has_pending_events {
            // Paint via the RedrawRequested path — request_redraw wakes the
            // loop and the overlay's RedrawRequested handler does the actual
            // rendering (single render per tick, throttled by vsync).
            self.request_redraw_overlay();
        }
        // Wake up at the next frame boundary even without input events.
        let wake_at = (self.last_overlay_render + Duration::from_millis(33)).max(Instant::now());
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake_at));
    }
}

/// Translate a winit `KeyEvent` into egui input events, so the overlay's
/// text fields (save-dialog filename) receive keyboard input. Mirrors the
/// essential parts of egui-winit's translation:
///
/// * printable text (no ctrl held) → `Event::Text`
/// * named keys (Enter, Backspace, arrows, ...) → `Event::Key`, on both
///   press and release so egui's key-down tracking stays consistent
/// * ctrl/alt/meta + character → `Event::Key` with the character mapped to
///   `egui::Key`, so shortcuts like ctrl+A / C / V / X work in text fields
///
/// The modifiers come from the caller's tracked `ModifiersState` (winit
/// delivers modifiers as separate `ModifiersChanged` events).
fn key_event_to_egui_events(
    event: &KeyEvent,
    mods: ModifiersState,
) -> Vec<egui::Event> {
    translate_key(
        &event.logical_key,
        event.text.as_deref(),
        event.state == ElementState::Pressed,
        event.repeat,
        mods,
    )
}

/// The testable core of the key translation: takes the logical key, the
/// text winit produced for it, press state, repeat flag and modifier
/// state. Split out from `key_event_to_egui_events` because winit's
/// `KeyEvent` cannot be constructed outside the crate (it has a
/// `pub(crate)` field), which would make it untestable.
fn translate_key(
    logical: &Key,
    text: Option<&str>,
    pressed: bool,
    repeat: bool,
    mods: ModifiersState,
) -> Vec<egui::Event> {
    let egui_mods = egui::Modifiers {
        alt: mods.alt_key(),
        ctrl: mods.control_key(),
        shift: mods.shift_key(),
        mac_cmd: false,
        // Linux: ctrl is the "command" key for egui's shortcut matching.
        command: mods.control_key(),
    };
    let mut events: Vec<egui::Event> = Vec::new();

    match logical {
        Key::Named(named) => {
            let key = match named {
                NamedKey::Enter => Some(egui::Key::Enter),
                NamedKey::Backspace => Some(egui::Key::Backspace),
                NamedKey::Escape => Some(egui::Key::Escape),
                NamedKey::Tab => Some(egui::Key::Tab),
                NamedKey::Space => Some(egui::Key::Space),
                NamedKey::ArrowLeft => Some(egui::Key::ArrowLeft),
                NamedKey::ArrowRight => Some(egui::Key::ArrowRight),
                NamedKey::ArrowUp => Some(egui::Key::ArrowUp),
                NamedKey::ArrowDown => Some(egui::Key::ArrowDown),
                NamedKey::Delete => Some(egui::Key::Delete),
                NamedKey::Home => Some(egui::Key::Home),
                NamedKey::End => Some(egui::Key::End),
                NamedKey::PageUp => Some(egui::Key::PageUp),
                NamedKey::PageDown => Some(egui::Key::PageDown),
                NamedKey::Insert => Some(egui::Key::Insert),
                _ => None,
            };
            if let Some(key) = key {
                events.push(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat,
                    modifiers: egui_mods,
                });
            }
            // egui's text insertion only reacts to Event::Text — a bare
            // Key::Space event inserts nothing. Emit the text too so typing
            // spaces into the save-dialog filename works.
            if pressed && matches!(named, NamedKey::Space) {
                events.push(egui::Event::Text(" ".into()));
            }
        }
        Key::Character(ch) => {
            if mods.control_key() || mods.alt_key() || mods.super_key() {
                // Shortcut combo (ctrl+A, ctrl+C, ...). Emit a Key event so
                // egui's text editing shortcuts engage. Only single
                // characters map cleanly to egui::Key.
                if ch.chars().count() == 1 {
                    if let Some(key) = egui::Key::from_name(&ch.to_lowercase()) {
                        events.push(egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat,
                            modifiers: egui_mods,
                        });
                    }
                }
            } else if pressed {
                // Plain typing — forward the produced text as-is.
                let text = match text.filter(|t| !t.is_empty()) {
                    Some(t) => Some(t),
                    None => (!ch.is_empty()).then_some(ch.as_str()),
                };
                if let Some(t) = text {
                    if !t.chars().any(|c| c.is_control()) {
                        events.push(egui::Event::Text(t.to_owned()));
                    }
                }
            }
        }
        _ => {}
    }

    events
}


/// Extract the X11 XID from a winit window.
fn extract_x11_xid(window: &Arc<winit::window::Window>) -> Result<u64> {
    use raw_window_handle::HasWindowHandle;
    let handle = window.window_handle()?.as_raw();
    match handle {
        raw_window_handle::RawWindowHandle::Xlib(x) => Ok(x.window),
        raw_window_handle::RawWindowHandle::Xcb(x) => Ok(x.window.get() as u64),
        other => Err(anyhow::anyhow!(
            "video window is not on X11 (got {other:?}). Wayland requires libmpv's render-context API, which is on the roadmap."
        )),
    }
}

// ---------------------------------------------------------------------------
// X11 bounding shape (XShape)
// ---------------------------------------------------------------------------
//
// The overlay window used to cover the video window with a full-screen
// wgpu surface and relied on *transparency* (ARGB visual + compositor +
// surface alpha mode) for the video to show through. That chain broke in
// the field over and over — wgpu's `CompositeAlphaMode::Auto` only ever
// resolves to Opaque/Inherit (never a transparent mode), some drivers
// write opaque alpha, some setups run without a compositor — and each
// break produced the same user-visible bug: "app has no video output"
// while the UI kept working.
//
// The bounding shape removes the dependency on that entire chain. Every
// frame, egui's actually-painted rects (see `renderer::painted_pixel_rects`)
// become the overlay window's X11 *bounding region*. Pixels outside the
// region are a literal hole in the X window: the video window underneath
// shows through because the overlay simply does not exist there, whatever
// the GPU, driver, compositor or alpha mode may say. The input region
// defaults to the bounding region, so pointer events in the holes are
// delivered to the video window — we re-route them into egui (see
// `forward_video_pointer`) to keep hover/auto-show behavior.

/// Dedicated XCB connection for shape requests. Separate from winit's
/// connection so we never interleave requests on its socket.
static SHAPE_CONN: OnceLock<Option<Arc<x11rb::rust_connection::RustConnection>>> = OnceLock::new();

fn shape_conn() -> Option<&'static Arc<x11rb::rust_connection::RustConnection>> {
    SHAPE_CONN
        .get_or_init(|| {
            match x11rb::connect(None) {
                Ok((conn, _screen)) => Some(Arc::new(conn)),
                Err(e) => {
                    warn!("X11 shape: cannot open X connection: {e}");
                    None
                }
            }
        })
        .as_ref()
}

/// The overlay window's X11 window ID as the X server knows it.
fn overlay_xid(overlay: &Arc<winit::window::Window>) -> Option<u32> {
    use raw_window_handle::HasWindowHandle;
    let handle = overlay.window_handle().ok()?.as_raw();
    match handle {
        raw_window_handle::RawWindowHandle::Xlib(x) => Some(x.window as u32),
        raw_window_handle::RawWindowHandle::Xcb(x) => Some(x.window.get()),
        _ => None,
    }
}

/// Convert an egui rect (window-local pixels, pixels_per_point = 1.0) into
/// an X11 protocol rectangle. Window dims are clamped to 16384 upstream,
/// so the protocol's i16/u16 ranges always hold.
fn to_x_rectangle(r: egui::Rect) -> xproto::Rectangle {
    let x = r.min.x.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    let y = r.min.y.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    // Width/height are span deltas, always >= 0, clamped to the protocol max.
    let w = (r.max.x.round() - r.min.x.round()).clamp(0.0, u16::MAX as f32) as u16;
    let h = (r.max.y.round() - r.min.y.round()).clamp(0.0, u16::MAX as f32) as u16;
    xproto::Rectangle { x, y, width: w, height: h }
}

/// Set the overlay window's X11 *bounding shape* to exactly `rects`
/// (window-local pixel rects). Pixels outside the union of the rects are a
/// hole: the video window beneath shows through unconditionally, and
/// pointer events in the holes go to the video window. An empty list is a
/// well-defined *empty* region (the server unions zero rectangles): the
/// overlay becomes fully invisible and fully click-through — e.g. while
/// the bars are auto-hidden.
fn apply_overlay_shape(overlay: &Arc<winit::window::Window>, rects: &[egui::Rect]) {
    use x11rb::protocol::shape::{self as shape_ext, SK, SO};
    use x11rb::protocol::xproto::ClipOrdering;

    let Some(xid) = overlay_xid(overlay) else { return; };
    let Some(conn) = shape_conn() else { return; };

    let xrects: Vec<xproto::Rectangle> = rects.iter().copied().map(to_x_rectangle).collect();
    match shape_ext::rectangles(
        conn.as_ref(),
        SO::SET,
        SK::BOUNDING,
        ClipOrdering::UNSORTED,
        xid,
        0,
        0,
        &xrects,
    ) {
        Ok(cookie) => {
            if let Err(e) = cookie.check() {
                warn!("X11 shape update error: {e}");
            }
        }
        Err(e) => warn!("X11 shape request failed: {e}"),
    }
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    #[test]
    fn x_rectangle_rounding_and_clamping() {
        let r = egui::Rect::from_min_max(egui::pos2(10.4, 20.6), egui::pos2(60.2, 70.8));
        let x = to_x_rectangle(r);
        assert_eq!((x.x, x.y, x.width, x.height), (10, 21, 50, 50));
    }

    #[test]
    fn x_rectangle_saturates_on_origin_clamp() {
        // Negative origins (shouldn't occur — egui clips to the window —
        // but must not wrap into huge u16s).
        let r = egui::Rect::from_min_max(egui::pos2(-5.0, -5.0), egui::pos2(5.0, 5.0));
        let x = to_x_rectangle(r);
        assert_eq!((x.x, x.y, x.width, x.height), (-5, -5, 10, 10));
    }
}

/// Set the X11 background pixel on a winit window.
///
/// winit creates windows with `background_pixel = None`, which means the X
/// server does NOT fill the window on resize/expose — it shows whatever is in
/// the framebuffer (stale content from other windows, GPU garbage). This is
/// the root cause of the "mirrored desktop" artifact during window resize/move.
///
/// By calling `XSetWindowBackground` with a dark grey pixel, we tell the X
/// server to fill the window with that color whenever it needs to clear or
/// resize the window. libmpv's video rendering paints on top of this
/// background, so normal playback is unaffected — but during the resize gap
/// (before libmpv repaints), the window shows dark grey instead of garbage.
///
/// The pixel value is in X11's native format: `0x00RRGGBB` (on most displays,
/// this is a 24-bit color packed into a 32-bit `unsigned long`).
fn set_x11_window_background(window: &Arc<winit::window::Window>, pixel: u64) {
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

    // raw-window-handle 0.6 split the display handle into a separate type.
    // XlibWindowHandle only carries the window ID; the display pointer lives
    // in XlibDisplayHandle, accessed via HasDisplayHandle.
    let Ok(win_handle) = window.window_handle() else { return; };
    let Ok(disp_handle) = window.display_handle() else { return; };

    let win_raw = win_handle.as_raw();
    let disp_raw = disp_handle.as_raw();

    match (win_raw, disp_raw) {
        (
            raw_window_handle::RawWindowHandle::Xlib(x),
            raw_window_handle::RawDisplayHandle::Xlib(d),
        ) => {
            // SAFETY: We're calling Xlib functions with the display pointer
            // and window ID from the raw handles. Both are valid as long as
            // the window exists (winit owns them). XSetWindowBackground is
            // thread-safe and doesn't allocate. XFlush ensures the request
            // is sent to the X server immediately.
            // Link against libX11 — XSetWindowBackground and XFlush live there.
            // winit's X11 backend already pulls in libX11 transitively, but
            // the linker still needs the explicit #[link] to resolve our
            // direct extern "C" references.
            #[link(name = "X11")]
            extern "C" {
                fn XSetWindowBackground(
                    display: *mut std::os::raw::c_void,
                    w: std::os::raw::c_ulong,
                    pixel: std::os::raw::c_ulong,
                ) -> std::os::raw::c_int;
                fn XFlush(display: *mut std::os::raw::c_void) -> std::os::raw::c_int;
            }
            unsafe {
                // d.display is Option<NonNull<c_void>> in raw-window-handle 0.6.
                // Unwrap and convert to a raw pointer. None would mean winit
                // didn't provide a display handle — skip in that case.
                let Some(display_ptr) = d.display else {
                    warn!("XlibDisplayHandle.display is None; cannot set background pixel");
                    return;
                };
                let display = display_ptr.as_ptr();
                XSetWindowBackground(
                    display,
                    x.window as std::os::raw::c_ulong,
                    pixel as std::os::raw::c_ulong,
                );
                XFlush(display);
            }
            info!("set X11 background pixel to {:#010x} on video window", pixel);
        }
        (
            raw_window_handle::RawWindowHandle::Xcb(_),
            raw_window_handle::RawDisplayHandle::Xcb(_),
        ) => {
            // XCB path: would need xcb_change_window_attributes. For now,
            // only the Xlib path is implemented — winit on X11 uses Xlib
            // by default, so this covers the common case.
            warn!("XCB backend detected; X11 background pixel not set (Xlib path only)");
        }
        _ => {
            // Not X11 — Wayland, macOS, Windows, etc. No background pixel
            // concept; the compositing model handles this differently.
        }
    }
}

/// Set X11 hints on the overlay window so the window manager treats it as a
/// helper/child of the video window rather than a standalone top-level window.
///
/// Three hints are set:
///
/// 1. **`_NET_WM_WINDOW_TYPE = DIALOG`** — tells EWMH-compliant WMs that the
///    overlay is a dialog/helper window, not a normal application window. Most
///    WMs respond by:
///    - Skipping it in the taskbar and alt-tab list
///    - Grouping it with its parent window
///    - Not giving it its own virtual desktop entry
///
/// 2. **`XSetTransientForHint`** — X11 ICCCM hint that marks the overlay as a
///    "transient" (short-lived) window belonging to the video window. WMs use
///    this to:
///    - Center the dialog over its parent
///    - Keep it on the same screen/monitor
///    - Un-map it when the parent is withdrawn or iconified
///
/// 3. **`_NET_WM_STATE: _NET_WM_STATE_SKIP_TASKBAR`** — explicit hint for WMs
///    that don't respect _NET_WM_WINDOW_TYPE=DIALOG for taskbar suppression.
///
/// Together, these hints ensure the overlay appears as a single integrated UI
/// with the video window, not as a separate top-level window that clutters
/// the taskbar and blocks other apps when fullscreen.
fn set_x11_overlay_hints(
    video: &Arc<winit::window::Window>,
    overlay: &Arc<winit::window::Window>,
) {
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

    let Ok(video_handle) = video.window_handle() else { return; };
    let Ok(overlay_handle) = overlay.window_handle() else { return; };
    let Ok(disp_handle) = overlay.display_handle() else { return; };

    let video_raw = video_handle.as_raw();
    let overlay_raw = overlay_handle.as_raw();
    let disp_raw = disp_handle.as_raw();

    match (
        video_raw,
        overlay_raw,
        disp_raw,
    ) {
        (
            raw_window_handle::RawWindowHandle::Xlib(video_x),
            raw_window_handle::RawWindowHandle::Xlib(overlay_x),
            raw_window_handle::RawDisplayHandle::Xlib(d),
        ) => {
            #[link(name = "X11")]
            extern "C" {
                fn XInternAtom(
                    display: *mut std::os::raw::c_void,
                    name: *const std::os::raw::c_char,
                    only_if_exists: std::os::raw::c_int,
                ) -> std::os::raw::c_ulong;
                fn XSetTransientForHint(
                    display: *mut std::os::raw::c_void,
                    w: std::os::raw::c_ulong,
                    prop_window: std::os::raw::c_ulong,
                ) -> std::os::raw::c_int;
                fn XChangeProperty(
                    display: *mut std::os::raw::c_void,
                    w: std::os::raw::c_ulong,
                    property: std::os::raw::c_ulong,
                    atype: std::os::raw::c_ulong,
                    format: std::os::raw::c_int,
                    mode: std::os::raw::c_int,
                    data: *const std::os::raw::c_uchar,
                    nelements: std::os::raw::c_int,
                ) -> std::os::raw::c_int;
                fn XFlush(display: *mut std::os::raw::c_void) -> std::os::raw::c_int;
            }

            let Some(display_ptr) = d.display else {
                warn!("XlibDisplayHandle.display is None; cannot set overlay hints");
                return;
            };
            let display = display_ptr.as_ptr();
            unsafe {
                let video_xid = video_x.window as std::os::raw::c_ulong;
                let overlay_xid = overlay_x.window as std::os::raw::c_ulong;

                // 1. Set _NET_WM_WINDOW_TYPE = DIALOG on the overlay.
                let atom_window_type =
                    XInternAtom(display, b"_NET_WM_WINDOW_TYPE\0".as_ptr() as *const _, 0);
                let atom_dialog =
                    XInternAtom(display, b"_NET_WM_WINDOW_TYPE_DIALOG\0".as_ptr() as *const _, 0);
                let xa_atom = XInternAtom(display, b"ATOM\0".as_ptr() as *const _, 0);

                XChangeProperty(
                    display,
                    overlay_xid,
                    atom_window_type,
                    xa_atom,
                    32,
                    0, // PropModeReplace
                    &atom_dialog as *const _ as *const std::os::raw::c_uchar,
                    1,
                );

                // 2. Set _NET_WM_STATE = _NET_WM_STATE_SKIP_TASKBAR.
                let atom_wm_state =
                    XInternAtom(display, b"_NET_WM_STATE\0".as_ptr() as *const _, 0);
                let atom_skip_taskbar =
                    XInternAtom(display, b"_NET_WM_STATE_SKIP_TASKBAR\0".as_ptr() as *const _, 0);
                XChangeProperty(
                    display,
                    overlay_xid,
                    atom_wm_state,
                    xa_atom,
                    32,
                    0,
                    &atom_skip_taskbar as *const _ as *const std::os::raw::c_uchar,
                    1,
                );

                // 3. Set transient-for hint: overlay belongs to video window.
                XSetTransientForHint(display, overlay_xid, video_xid);

                XFlush(display);
            }
            info!(
                "set overlay X11 hints: DIALOG type + SKIP_TASKBAR + transient-for video window"
            );
        }
        (
            raw_window_handle::RawWindowHandle::Xcb(_),
            raw_window_handle::RawWindowHandle::Xcb(_),
            raw_window_handle::RawDisplayHandle::Xcb(_),
        ) => {
            warn!("XCB backend detected; overlay WM hints not set (Xlib path only)");
        }
        _ => {
            // Not X11 — no WM hints needed (Wayland compositors handle this
            // via their own protocol; macOS/iOS use NSPanel/etc.).
        }
    }
}

/// Spawn a worker thread to run ffmpeg for A-B loop video export. The
/// thread runs the encode and sends the result back on `tx` when done.
/// Runs in the background so the UI stays responsive during encoding.
/// `crop` optionally carries a source-space rectangle (x, y, w, h) to crop
/// the output to — the zoom/pan focus area the user had aligned.
fn spawn_ffmpeg_export(
    input: String,
    start: f64,
    end: f64,
    output: String,
    crop: Option<player_core::crop::CropRect>,
    tx: crossbeam_channel::Sender<(DialogKind, DialogResult)>,
) {
    let cropped = crop.is_some();
    std::thread::Builder::new()
        .name("ferret-ffmpeg".into())
        .spawn(move || {
            let result = export_video_segment(&input, start, end, &output, crop)
                .map(|_| DialogResult::File { path: output.clone(), cropped })
                .unwrap_or_else(|e| DialogResult::Error(e.to_string()));
            let _ = tx.send((DialogKind::ExportVideo, result));
        })
        .ok();
}

/// Run ffmpeg to extract the video segment [start, end] from `input` into
/// `output`. Re-encodes video (libx264) for frame accuracy and maximum
/// compatibility. Audio is re-encoded to AAC. When `crop` is Some, the
/// output is cropped to that source-pixel rectangle first — used to bake
/// the zoom/pan focus area into exported A-B clips.
fn export_video_segment(
    input: &str,
    start: f64,
    end: f64,
    output: &str,
    crop: Option<player_core::crop::CropRect>,
) -> std::io::Result<()> {
    let duration = end - start;
    info!("exporting video segment: {input} [{start:.3}..{end:.3}] → {output}");

    // Check that ffmpeg is available.
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "ffmpeg not found in PATH. Install ffmpeg to export video segments.",
        ));
    }

    // Use -ss before -i for fast seeking. Re-encode video (libx264) for
    // frame accuracy and maximum compatibility. Use -y to overwrite output.
    let mut ffmpeg = std::process::Command::new("ffmpeg");
    ffmpeg
        .arg("-y")
        .arg("-ss").arg(format!("{start:.3}"))
        .arg("-i").arg(input)
        .arg("-t").arg(format!("{duration:.3}"));
    if let Some((cx, cy, cw, ch)) = crop {
        ffmpeg.arg("-vf").arg(format!("crop={cw}:{ch}:{cx}:{cy}"));
    }
    ffmpeg
        .arg("-c:v").arg("libx264")
        .arg("-preset").arg("fast")
        .arg("-crf").arg("18")
        .arg("-c:a").arg("aac")
        .arg("-b:a").arg("192k")
        .arg(output);
    let out = ffmpeg.output()?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg = stderr.lines().last().unwrap_or("unknown ffmpeg error");
        return Err(std::io::Error::other(format!("ffmpeg: {msg}")));
    }
    Ok(())
}

#[cfg(test)]
mod key_tests {
    use super::*;
    use winit::keyboard::ModifiersState;

    fn char_key(ch: &str) -> Key {
        Key::Character(ch.into())
    }

    fn is_text(events: &[egui::Event], t: &str) -> bool {
        events.iter().any(|e| matches!(e, egui::Event::Text(s) if s == t))
    }

    fn is_key(events: &[egui::Event], k: egui::Key, pressed: bool) -> bool {
        events.iter().any(
            |e| matches!(e, egui::Event::Key { key, pressed: p, .. } if *key == k && *p == pressed),
        )
    }

    #[test]
    fn plain_characters_become_text() {
        // Typing 'q' in the filename field must go to egui — NOT quit.
        let ev = translate_key(&char_key("q"), Some("q"), true, false, ModifiersState::empty());
        assert!(is_text(&ev, "q"));
        assert_eq!(ev.len(), 1);
    }

    #[test]
    fn space_types_a_space() {
        let ev = translate_key(
            &Key::Named(NamedKey::Space),
            Some(" "),
            true,
            false,
            ModifiersState::empty(),
        );
        assert!(is_text(&ev, " "));
        assert!(is_key(&ev, egui::Key::Space, true));
    }

    #[test]
    fn named_keys_map_to_egui_keys() {
        let ev = translate_key(
            &Key::Named(NamedKey::Backspace),
            None,
            true,
            false,
            ModifiersState::empty(),
        );
        assert!(is_key(&ev, egui::Key::Backspace, true));
        // Releases are forwarded too so egui's key-down tracking stays sane.
        let ev = translate_key(
            &Key::Named(NamedKey::Backspace),
            None,
            false,
            false,
            ModifiersState::empty(),
        );
        assert!(is_key(&ev, egui::Key::Backspace, false));
    }

    #[test]
    fn enter_maps_to_key_not_text() {
        // winit gives Enter text "\r" — egui wants Key::Enter, no Text.
        let ev = translate_key(
            &Key::Named(NamedKey::Enter),
            Some("\r"),
            true,
            false,
            ModifiersState::empty(),
        );
        assert!(is_key(&ev, egui::Key::Enter, true));
        assert!(!events_have_text(&ev));
    }

    #[test]
    fn ctrl_char_maps_to_key_event() {
        // ctrl+A must reach egui as a Key event for select-all to work.
        let ev = translate_key(
            &char_key("a"),
            None,
            true,
            false,
            ModifiersState::CONTROL,
        );
        assert!(is_key(&ev, egui::Key::A, true));
        assert!(!events_have_text(&ev));
    }

    fn events_have_text(events: &[egui::Event]) -> bool {
        events.iter().any(|e| matches!(e, egui::Event::Text(_)))
    }
}
