//! wgpu + egui_wgpu renderer for the overlay window.

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use crossbeam_channel::Receiver;
use egui_wgpu::{wgpu, Renderer as EguiRenderer, ScreenDescriptor};
use tracing::info;

use player_core::event::EngineEvent;
use player_core::state::PlaybackState;
use player_core::Cmd;

use crate::app::OverlayApp;

static START_TIME: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Padding (pixels) added around every painted rect before it becomes part
/// of the overlay's X11 bounding shape. Covers glyph antialiasing bleed and
/// sub-pixel rounding so no painted pixel falls outside the shape.
pub const SHAPE_PAD_PX: f32 = 2.0;

/// Upper bound on the number of rects sent to XShapeCombineRectangles per
/// frame. Above it we degrade to a single coarse union rect — the X server
/// unions the list anyway, so the only cost of coarseness is a slightly
/// larger see-through-blocking region for one frame.
const MAX_SHAPE_RECTS: usize = 64;

/// Owns the wgpu surface + egui_wgpu renderer for ONE overlay window.
pub struct OverlayRenderer {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub surface: Arc<wgpu::Surface<'static>>,
    pub surface_config: wgpu::SurfaceConfiguration,
    pub egui_renderer: EguiRenderer,
    pub egui_ctx: egui::Context,
    pub app: OverlayApp,
    pub viewport_size: [u32; 2],
    /// Pixel-space rects egui actually painted last frame (window-local).
    /// The main app mirrors these onto the overlay window as an X11
    /// *bounding shape* (XShape), so the overlay is visually present ONLY
    /// where UI chrome exists. The rest of the window is a literal hole in
    /// the X window — the video window underneath shows through with NO
    /// dependence on compositors, EGL/Vulkan alpha modes, or window
    /// visuals. This is what makes "no video output" structurally
    /// impossible: the overlay can no longer blanket the video with a
    /// possibly-opaque surface.
    pub painted_rects: Vec<egui::Rect>,
    /// Timestamp until which the overlay must clear opaque (dark grey)
    /// instead of transparent. Set by `resize()`, `suppress_transparency()`,
    /// and the surface-error recovery paths. Keeps the desktop from showing
    /// through the overlay during window moves/resizes while libmpv catches
    /// up to the new geometry.
    force_opaque_until: Option<Instant>,
}

impl OverlayRenderer {
    pub fn new(
        window: Arc<winit::window::Window>,
        state: PlaybackState,
        event_rx: Receiver<EngineEvent>,
        cmd_tx: crossbeam_channel::Sender<Cmd>,
    ) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            flags: wgpu::InstanceFlags::default(),
            dx12_shader_compiler: wgpu::Dx12Compiler::default(),
            gles_minor_version: wgpu::Gles3MinorVersion::default(),
        });

        let surface = instance.create_surface(window.clone())?;

        let adapter = pollster::block_on(async {
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                })
                .await
                .context("no suitable wgpu adapter")
        })?;

        let (device, queue) = pollster::block_on(async {
            adapter
                .request_device(
                    &wgpu::DeviceDescriptor {
                        label: Some("ferret-overlay"),
                        required_features: wgpu::Features::empty(),
                        required_limits: wgpu::Limits::downlevel_defaults(),
                        memory_hints: wgpu::MemoryHints::default(),
                    },
                    None,
                )
                .await
                .context("failed to acquire wgpu device")
        })?;

        let device = Arc::new(device);
        let queue = Arc::new(queue);
        let surface = Arc::new(surface);

        let caps = surface.get_capabilities(&adapter);
        // Prefer non-sRGB formats — egui warns about sRGB framebuffers
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm))
            .or_else(|| caps.formats.iter().copied().find(|f| matches!(f, wgpu::TextureFormat::Rgba8Unorm)))
            .or_else(|| caps.formats.iter().copied().find(|f| matches!(f, wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Rgba8UnormSrgb)))
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| anyhow::anyhow!("surface has no supported formats"))?;
        // Force PresentMode::Fifo (vsync). Mailbox causes "Unrecognized
        // present mode" warnings on some X11/EGL setups and produces
        // broken presentation during window moves/resizes. Fifo is the
        // most compatible mode and is required by the WebGPU spec.
        let present_mode = wgpu::PresentMode::Fifo;
        // Explicit alpha-mode selection. `CompositeAlphaMode::Auto` in wgpu
        // can only ever resolve to Opaque or Inherit (see wgpu-core
        // `device/global.rs`, the `Auto` fallback list) — it will NEVER pick
        // PreMultiplied/PostMultiplied, so on a Vulkan-backed surface (any
        // real GPU) an `Auto` overlay presents OPAQUE black over the video:
        // the recurring "app has no video output" bug. Prefer an actually
        // transparent composite mode when the surface reports one; fall
        // back to Opaque (harmless — the X11 bounding shape is what
        // guarantees the video is visible, and the bars look fine opaque).
        let alpha_mode = caps
            .alpha_modes
            .iter()
            .copied()
            .find(|m| {
                matches!(
                    m,
                    wgpu::CompositeAlphaMode::PreMultiplied
                        | wgpu::CompositeAlphaMode::PostMultiplied
                )
            })
            .unwrap_or(wgpu::CompositeAlphaMode::Opaque);
        info!(
            "overlay alpha modes: supported={:?} chosen={:?}",
            caps.alpha_modes, alpha_mode
        );

        let size = window.inner_size();
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        let egui_renderer = EguiRenderer::new(
            &device,
            format,
            None,  // no depth buffer
            1,     // msaa_samples
            false, // no dithering
        );

        let egui_ctx = egui::Context::default();
        let mut app = OverlayApp::new(state, event_rx, cmd_tx);
        app.window_size = egui::vec2(size.width as f32, size.height as f32);

        info!(
            "overlay renderer ready: {}x{} {:?} alpha={:?}",
            size.width, size.height, format, alpha_mode
        );

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            egui_renderer,
            egui_ctx,
            app,
            viewport_size: [size.width.max(1), size.height.max(1)],
            painted_rects: Vec::new(),
            force_opaque_until: None,
        })
    }

    /// Render one frame.
    pub fn render(&mut self, state: PlaybackState, mouse_pos: Option<egui::Pos2>) -> Result<()> {
        self.app.update_state(state);
        self.app.pointer_pos = mouse_pos;
        self.app.set_mouse_inside(mouse_pos.is_some());
        self.app.compute_visibility();
        self.app.poll_events();

        // Drain any egui input events (mouse moves, clicks) that the main
        // app pushed since the last frame. Without these, egui never sees
        // the mouse and no buttons/sliders/dropdowns respond.
        let events = self.app.drain_events();

        let pixels_per_point = 1.0;
        let screen_size = [self.surface_config.width, self.surface_config.height];

        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(screen_size[0] as f32, screen_size[1] as f32),
            )),
            time: Some(START_TIME.elapsed().as_secs_f64()),
            events,
            ..Default::default()
        };

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            self.app.draw(ctx);
        });

        // Remember whether egui now has a focused text field (e.g. the
        // save-dialog filename input). The main app consults this flag on
        // the next physical keypress to decide whether the keystroke goes
        // to the text field (translated to egui events) or to the global
        // hotkey map.
        self.app.ui_wants_keyboard = self.egui_ctx.wants_keyboard_input();

        // Record what was actually painted, in window-local pixel coords.
        // The main app applies this as the overlay's X11 bounding shape
        // (see `painted_pixel_rects`).
        self.painted_rects = painted_pixel_rects(&full_output.shapes, SHAPE_PAD_PX);

        // Sync textures (new/updated).
        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }

        // Acquire surface frame. During a window resize/move, the surface can
        // become outdated. We MUST reconfigure and retry — skipping the render
        // leaves the old (possibly transparent) frame visible, which is the
        // root cause of the "ghosting from what's behind it" artifact. By
        // reconfiguring and rendering immediately, we paint a fresh opaque
        // frame that covers the video window's resize gap.
        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Outdated) | Err(wgpu::SurfaceError::Lost) => {
                // Surface is stale — reconfigure with current config and retry.
                // This ensures we always paint a fresh frame instead of leaving
                // a stale transparent one visible.
                self.surface.configure(&self.device, &self.surface_config);
                self.suppress_transparency(Duration::from_millis(200));
                // Retry once. If it still fails, skip (rare — usually means
                // the GPU is truly unavailable).
                match self.surface.get_current_texture() {
                    Ok(f) => f,
                    Err(wgpu::SurfaceError::Timeout) => return Ok(()),
                    Err(e) => {
                        tracing::warn!("surface acquire failed after reconfigure: {e:?}");
                        return Ok(());
                    }
                }
            }
            Err(wgpu::SurfaceError::Timeout) => {
                // GPU is busy — skip this frame, try again next time.
                return Ok(());
            }
            Err(e) => Err(e)?,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ferret-overlay-encoder"),
        });

        let paint_jobs = self.egui_ctx.tessellate(full_output.shapes, pixels_per_point);

        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: screen_size,
            pixels_per_point,
        };

        // Upload vertex/index buffers.
        let extra_cmd_buffers = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        // Render pass: clear to dark grey when no file is loaded, or when
        // the overlay is in a transitional state (resize, move, surface
        // recovery). Clearing to transparent when a file is loaded lets
        // libmpv's video show through — but if libmpv hasn't repainted yet
        // (common during a resize/drag), transparent would reveal the desktop.
        // The `force_opaque_until` timestamp keeps the overlay opaque for a
        // grace period after every window operation, giving libmpv time to
        // catch up to the new geometry.
        let has_file = self.app.state.path.is_some();
        let force_opaque = self
            .force_opaque_until
            .map(|t| Instant::now() < t)
            .unwrap_or(false);
        let clear_color = if has_file && !force_opaque {
            wgpu::Color::TRANSPARENT
        } else {
            // #1a1a1d — matches the VLC dark theme bg.
            wgpu::Color { r: 0.10, g: 0.10, b: 0.114, a: 1.0 }
        };
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ferret-overlay-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // SAFETY: `egui_wgpu::Renderer::render` requires
            // `&mut RenderPass<'static>` even though it consumes the pass
            // synchronously and never escapes it. We transmute the lifetime
            // to `'static`; the borrow is sound because:
            //   - `rpass` borrows `encoder` for the duration of this block.
            //   - `encoder.finish()` is called only after `rpass` drops at
            //     the block's end, so no mutable aliasing of the encoder
            //     occurs while the pass is live.
            //   - `egui_wgpu::Renderer::render` does not retain the
            //     `RenderPass` reference past its own return.
            let rpass_static: &mut wgpu::RenderPass<'static> = unsafe {
                std::mem::transmute::<&mut wgpu::RenderPass<'_>, &mut wgpu::RenderPass<'static>>(&mut rpass)
            };
            self.egui_renderer.render(rpass_static, &paint_jobs, &screen_descriptor);
        }
        // rpass is dropped here; encoder is no longer borrowed.

        // Free dropped textures.
        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        // Submit encoder + any callback command buffers.
        let mut all_cmd_buffers: Vec<wgpu::CommandBuffer> = extra_cmd_buffers;
        all_cmd_buffers.push(encoder.finish());
        self.queue.submit(all_cmd_buffers);
        frame.present();

        Ok(())
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        // Only reconfigure the surface if the size actually changed.
        // During a window MOVE (not resize), this function gets called with
        // the same dimensions — reconfiguring would destroy the valid
        // framebuffer and replace it with an uninitialized one, causing
        // a flash of garbage on the next frame.
        let changed = self.surface_config.width != width || self.surface_config.height != height;
        if changed {
            self.surface_config.width = width;
            self.surface_config.height = height;
            self.surface.configure(&self.device, &self.surface_config);
            self.viewport_size = [width, height];
            self.app.window_size = egui::vec2(width as f32, height as f32);
        }
        // Always suppress transparency on resize/move, even if the size
        // didn't change (e.g., during a window move where only position
        // changes). This ensures the overlay clears opaque for a grace
        // period, hiding any lag in libmpv's repainting.
        self.suppress_transparency(Duration::from_millis(200));
    }

    /// Suppress transparency for the given duration. Call this on every
    /// window move/resize event and on surface-error recovery. The overlay
    /// will clear opaque (dark grey) until the timestamp expires, hiding
    /// any lag in libmpv's repainting of the video window underneath.
    pub fn suppress_transparency(&mut self, duration: Duration) {
        self.force_opaque_until = Some(Instant::now() + duration);
    }
}

// ---------------------------------------------------------------------------
// Painted-rect extraction (feeds the X11 bounding shape)
// ---------------------------------------------------------------------------

/// Compute the window-local pixel rects egui actually painted this frame.
///
/// For every clipped shape we take the shape's visual bounding box, clip it
/// to the shape's clip rect, pad it by `pad`, and then coalesce the list
/// into a small set of disjoint rects. Empty input (nothing painted —
/// controls auto-hidden, no dialogs) yields an empty Vec, which the caller
/// turns into an empty bounding region: the overlay becomes fully
/// see-through AND click-through, and the video window beneath receives
/// the input events.
pub fn painted_pixel_rects(shapes: &[egui::epaint::ClippedShape], pad: f32) -> Vec<egui::Rect> {
    let pad_v = egui::vec2(pad, pad);
    let mut rects: Vec<egui::Rect> = shapes
        .iter()
        .filter_map(|cs| {
            let bounds = cs.shape.visual_bounding_rect();
            // Rect::NOTHING (and any non-finite garbage) means "paints
            // nothing visible".
            if bounds.is_negative() || !bounds.min.is_finite() || !bounds.max.is_finite() {
                return None;
            }
            let clipped = bounds.intersect(cs.clip_rect);
            if clipped.is_negative() || clipped.width() <= 0.0 || clipped.height() <= 0.0 {
                return None;
            }
            Some(egui::Rect::from_min_max(
                clipped.min - pad_v,
                clipped.max + pad_v,
            ))
        })
        .collect();

    coalesce_rects(&mut rects, 4);

    if rects.len() > MAX_SHAPE_RECTS {
        // Too fragmented — collapse to the overall union. The X server
        // unions the rect list anyway, so this is purely a protocol-cost
        // guard.
        let mut iter = rects.into_iter();
        let Some(first) = iter.next() else {
            return Vec::new();
        };
        let union = iter.fold(first, |acc, r| acc.union(r));
        return vec![union];
    }
    rects
}

/// Merge overlapping rects until stable (bounded passes). Adjacent glyph
/// runs and widget clusters collapse into a handful of rects this way, so
/// the X11 shape request stays tiny.
fn coalesce_rects(rects: &mut Vec<egui::Rect>, max_passes: usize) {
    for _ in 0..max_passes {
        let mut out: Vec<egui::Rect> = Vec::with_capacity(rects.len());
        let mut merged_any = false;
        for r in rects.iter().copied() {
            match out.iter_mut().find(|o| o.intersects(r)) {
                Some(o) => {
                    *o = o.union(r);
                    merged_any = true;
                }
                None => out.push(r),
            }
        }
        *rects = out;
        if !merged_any {
            break;
        }
    }
}

#[cfg(test)]
mod shape_tests {
    use super::*;
    use egui::{Color32, Pos2, Rect, Shape, Vec2};

    fn clipped(clip: Rect, shape: Shape) -> egui::epaint::ClippedShape {
        egui::epaint::ClippedShape { clip_rect: clip, shape }
    }

    fn full_screen() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0))
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
    }

    #[test]
    fn nothing_painted_yields_empty() {
        // No shapes at all -> no rects.
        assert!(painted_pixel_rects(&[], 2.0).is_empty());
        // A Noop shape paints nothing.
        let shapes = vec![clipped(full_screen(), Shape::Noop)];
        assert!(painted_pixel_rects(&shapes, 2.0).is_empty());
    }

    #[test]
    fn single_painted_rect_is_padded() {
        let shapes = vec![clipped(
            full_screen(),
            Shape::rect_filled(rect(100.0, 200.0, 50.0, 20.0), 0.0, Color32::GRAY),
        )];
        let out = painted_pixel_rects(&shapes, 2.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], rect(98.0, 198.0, 54.0, 24.0));
    }

    #[test]
    fn overlapping_rects_coalesce() {
        let shapes = vec![
            clipped(full_screen(), Shape::rect_filled(rect(0.0, 0.0, 100.0, 30.0), 0.0, Color32::GRAY)),
            clipped(full_screen(), Shape::rect_filled(rect(50.0, 10.0, 100.0, 30.0), 0.0, Color32::GRAY)),
        ];
        let out = painted_pixel_rects(&shapes, 0.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], rect(0.0, 0.0, 150.0, 40.0));
    }

    #[test]
    fn disjoint_rects_stay_disjoint() {
        // Top menu strip and bottom control bar must not merge into a
        // full-window rect — that would re-create the "overlay covers the
        // video" bug the shape exists to prevent.
        let shapes = vec![
            clipped(full_screen(), Shape::rect_filled(rect(0.0, 0.0, 1280.0, 32.0), 0.0, Color32::GRAY)),
            clipped(full_screen(), Shape::rect_filled(rect(0.0, 602.0, 1280.0, 118.0), 0.0, Color32::GRAY)),
        ];
        let out = painted_pixel_rects(&shapes, 2.0);
        assert_eq!(out.len(), 2);
        assert!((out[0].center().y - 16.0).abs() < 0.5);
        assert!((out[1].center().y - 661.0).abs() < 0.5);
    }

    #[test]
    fn clip_rect_trims_shape_bounds() {
        // A shape whose bounds exceed its clip rect must be trimmed, so the
        // shape region never claims area egui was not allowed to paint.
        let shapes = vec![clipped(
            rect(0.0, 0.0, 100.0, 100.0),
            Shape::rect_filled(rect(0.0, 0.0, 1280.0, 720.0), 0.0, Color32::GRAY),
        )];
        let out = painted_pixel_rects(&shapes, 0.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], rect(0.0, 0.0, 100.0, 100.0));
    }

    #[test]
    fn rect_cap_collapses_to_union() {
        // >MAX_SHAPE_RECTS disjoint rects (all within the clip window!) ->
        // one coarse union rect.
        let mut shapes = Vec::new();
        for i in 0..(MAX_SHAPE_RECTS + 10) {
            let x = (i as f32) * 3.0;
            shapes.push(clipped(
                full_screen(),
                Shape::rect_filled(rect(x, 0.0, 1.0, 10.0), 0.0, Color32::GRAY),
            ));
        }
        let out = painted_pixel_rects(&shapes, 0.0);
        assert_eq!(out.len(), 1);
        // 74 rects (MAX + 10) at 3px pitch: last spans 219..220px.
        assert!((out[0].width() - 220.0).abs() < 0.5);
    }
}
