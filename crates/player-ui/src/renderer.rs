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
        // ("Detected a linear (sRGBA aware) framebuffer Bgra8UnormSrgb.
        // egui prefers Rgba8Unorm or Bgra8Unorm"). Non-sRGB avoids color
        // management issues during window operations.
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
        // Use Auto alpha mode — let the surface pick the best-supported
        // compositing mode. PreMultiplied can cause artifacts on compositors
        // that don't fully support it (common on Xfwm4).
        let alpha_mode = wgpu::CompositeAlphaMode::Auto;

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
            force_opaque_until: None,
        })
    }

    /// Render one frame.
    pub fn render(&mut self, state: PlaybackState, mouse_pos: Option<egui::Pos2>) -> Result<()> {
        self.app.update_state(state);
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
