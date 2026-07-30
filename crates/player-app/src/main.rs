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

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use crossbeam_channel::unbounded;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::Key;
use winit::window::WindowId;

use player_core::cmd::LoadModeKind;
use player_core::options::EngineOptions;
use player_core::{Cmd, PlayerEngine};
use player_ui::OverlayRenderer;

mod keymap;
mod windows;

use windows::{WindowKind, WindowManager};

/// Result from a background ffmpeg export worker.
enum DialogResult {
    /// Export succeeded; the file was written to this path.
    File(String),
    /// Export failed with this error message.
    Error(String),
}

/// Only one background operation remains: ffmpeg video export. All file
/// selection is now in-UI (see `player_ui::file_dialog`).
enum DialogKind {
    ExportVideo {
        input: String,
        start: f64,
        end: f64,
    },
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
                (DialogKind::ExportVideo { .. }, DialogResult::File(path)) => {
                    infos.push(format!("A-B loop exported to {path}"));
                }
                (DialogKind::ExportVideo { .. }, DialogResult::Error(msg)) => {
                    infos.push(format!("Export failed: {msg}"));
                }
                (_, DialogResult::Error(msg)) => {
                    infos.push(msg);
                }
                _ => {}
            }
        }
        infos
    }

    fn render_overlay(&mut self) {
        let Some(engine) = self.engine.as_ref() else { return; };

        // Forward commands from the overlay UI to the engine. Two commands
        // are intercepted here (not sent to the engine):
        //   - ToggleFullscreen: main-app window concern
        //   - ExportABLoopVideo: main-app runs ffmpeg (engine no-ops it)
        let mut pending_fullscreen_toggle = false;
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match &cmd {
                Cmd::ToggleFullscreen => {
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
                    drop(st);
                    match (a, b, input) {
                        (Some(start), Some(end), Some(inp)) if end > start => {
                            spawn_ffmpeg_export(
                                inp,
                                start,
                                end,
                                path.clone(),
                                self.dialog_result_tx.clone(),
                            );
                        }
                        _ => {
                            let _ = self.dialog_result_tx.send((
                                DialogKind::ExportVideo {
                                    input: String::new(),
                                    start: 0.0,
                                    end: 0.0,
                                },
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

        if pending_fullscreen_toggle {
            self.toggle_fullscreen();
        }
    }

    /// Request a redraw on both windows. Single dispatch point so callers
    /// never have to repeat the `if let Some(w) = ...` dance.
    fn request_redraw_both(&self) {
        self.windows.video.as_ref().map(|w| w.request_redraw());
        self.windows.overlay.as_ref().map(|w| w.request_redraw());
    }

    /// Request a redraw on the overlay window only.
    fn request_redraw_overlay(&self) {
        if let Some(w) = &self.windows.overlay {
            w.request_redraw();
        }
    }

    fn handle_keyboard(&mut self, key: &Key, event_loop: &ActiveEventLoop) {
        // `q` and `f` are window-level concerns; they never reach the engine.
        match key {
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
        if let Some(cmd) = keymap::key_to_cmd(key, &state) {
            let _ = engine.send(cmd);
        }
    }

    fn toggle_fullscreen(&mut self) {
        let Some(video) = self.windows.video.clone() else { return; };
        self.fullscreen = !self.fullscreen;
        if self.fullscreen {
            video.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        } else {
            video.set_fullscreen(None);
        }
        // Sync state to overlay so the fullscreen button reflects it.
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.app.set_fullscreen(self.fullscreen);
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
                WindowEvent::KeyboardInput {
                    event:
                        KeyEvent {
                            state: ElementState::Pressed,
                            logical_key,
                            ..
                        },
                    ..
                } => {
                    self.handle_keyboard(&logical_key, event_loop);
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
                    self.render_overlay();
                    self.request_redraw_overlay();
                }
                _ => {}
            },
            WindowKind::Overlay => match event {
                WindowEvent::CursorMoved { position, .. } => {
                    // The renderer sets pixels_per_point=1.0, so egui's
                    // coordinate system matches physical pixels directly.
                    let pos = egui::pos2(position.x as f32, position.y as f32);
                    self.overlay_mouse_pos = Some(pos);
                    if let Some(overlay) = self.overlay.as_mut() {
                        overlay.app.push_event(egui::Event::PointerMoved(pos));
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
                    }
                    // Process the click immediately so dropdown menus open
                    // without waiting for the next render cycle. The
                    // RedrawRequested handler has a 16ms rate limit that can
                    // skip the render, and about_to_wait only fires on a 33ms
                    // timer — so without this eager render, the click sits in
                    // pending_events and the dropdown never appears until the
                    // mouse moves.
                    self.render_overlay();
                    self.request_redraw_overlay();
                }
                WindowEvent::KeyboardInput {
                    event:
                        KeyEvent {
                            state: ElementState::Pressed,
                            logical_key,
                            ..
                        },
                    ..
                } => {
                    self.handle_keyboard(&logical_key, event_loop);
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
                    self.render_overlay();
                }
                WindowEvent::CloseRequested => {
                    event_loop.exit();
                }
                _ => {}
            },
            WindowKind::Unknown => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Re-render the overlay at ~30fps even when no input arrives,
        // promptly whenever a dialog result lands, and immediately when
        // there are queued egui events (clicks, key presses) that the
        // rate-limited RedrawRequested handler might have skipped.
        let has_pending_events = self.overlay
            .as_ref()
            .map(|o| !o.app.pending_events.is_empty())
            .unwrap_or(false);
        let need_render = self.last_overlay_render.elapsed() > Duration::from_millis(33)
            || !self.dialog_result_rx.is_empty()
            || has_pending_events;
        if need_render {
            self.render_overlay();
            self.request_redraw_overlay();
        }
    }
}

/// Extract the X11 XID from a winit window.
fn extract_x11_xid(window: &Arc<winit::window::Window>) -> Result<u64> {
    use raw_window_handle::HasWindowHandle;
    let handle = window.window_handle()?.as_raw();
    match handle {
        raw_window_handle::RawWindowHandle::Xlib(x) => Ok(x.window as u64),
        raw_window_handle::RawWindowHandle::Xcb(x) => Ok(x.window.get() as u64),
        other => Err(anyhow::anyhow!(
            "video window is not on X11 (got {other:?}). Wayland requires libmpv's render-context API, which is on the roadmap."
        )),
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

/// Spawn a worker thread to run ffmpeg for A-B loop video export. The
/// thread runs the encode and sends the result back on `tx` when done.
/// Runs in the background so the UI stays responsive during encoding.
fn spawn_ffmpeg_export(
    input: String,
    start: f64,
    end: f64,
    output: String,
    tx: crossbeam_channel::Sender<(DialogKind, DialogResult)>,
) {
    std::thread::Builder::new()
        .name("ferret-ffmpeg".into())
        .spawn(move || {
            let result = export_video_segment(&input, start, end, &output)
                .map(|_| DialogResult::File(output.clone()))
                .unwrap_or_else(|e| DialogResult::Error(e.to_string()));
            let _ = tx.send((
                DialogKind::ExportVideo {
                    input,
                    start,
                    end,
                },
                result,
            ));
        })
        .ok();
}

/// Run ffmpeg to extract the video segment [start, end] from `input` into
/// `output`. Re-encodes video (libx264) for frame accuracy and maximum
/// compatibility. Audio is re-encoded to AAC.
fn export_video_segment(input: &str, start: f64, end: f64, output: &str) -> std::io::Result<()> {
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
    let output = std::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-ss").arg(format!("{start:.3}"))
        .arg("-i").arg(input)
        .arg("-t").arg(format!("{duration:.3}"))
        .arg("-c:v").arg("libx264")
        .arg("-preset").arg("fast")
        .arg("-crf").arg("18")
        .arg("-c:a").arg("aac")
        .arg("-b:a").arg("192k")
        .arg(output)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = stderr.lines().last().unwrap_or("unknown ffmpeg error");
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("ffmpeg: {msg}"),
        ));
    }
    Ok(())
}

