//! VLC-style seek bar.
//!
//! Visual layout:
//!   [00:12]  ████████████░░░░░░░░░░░░  [01:30]
//!            ^progress    ^thumb (visible on hover/drag)
//!
//! - The track is ~6px tall, chunky and easy to click.
//! - The progress fill is VLC orange.
//! - The thumb is a 12px circle that appears on hover/drag.
//! - The whole widget has generous vertical padding so the hit area
//!   is ~24px even though the track is only 6px.

use egui::{Color32, Response, Sense, Ui, Vec2};

/// Draw a VLC-style seek bar.
///
/// Returns `(response, new_position_fraction)` where `new_position_fraction`
/// is `Some(0.0..=1.0)` if the user clicked or dragged this frame.
pub fn seek_bar(
    ui: &mut Ui,
    progress: Option<f32>,    // 0..=1, None if unknown
    buffered: Option<f32>,    // 0..=1, None if unknown
    height: f32,              // total widget height (hit area)
) -> (Response, Option<f32>) {
    let desired = Vec2::new(ui.available_width(), height);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());

    let progress = progress.unwrap_or(0.0).clamp(0.0, 1.0);
    let buffered = buffered.unwrap_or(0.0).clamp(0.0, 1.0);
    let track_h = 6.0;
    let track_y = rect.center().y;
    let track_rect = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, track_y),
        Vec2::new(rect.width() - 4.0, track_h),
    );

    let track_color = ui.style().visuals.widgets.inactive.bg_fill;
    let buffered_color = Color32::from_rgb(120, 64, 0);
    let progress_color = Color32::from_rgb(255, 136, 0);
    let thumb_color = Color32::from_rgb(255, 168, 40);

    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);

        // Track background (rounded).
        painter.rect_filled(track_rect, track_h * 0.5, track_color);

        // Buffered range (drawn behind progress).
        if buffered > 0.0 {
            let buf_w = track_rect.width() * buffered;
            let buf_rect = egui::Rect::from_min_size(
                track_rect.min,
                Vec2::new(buf_w, track_h),
            );
            painter.rect_filled(buf_rect, track_h * 0.5, buffered_color);
        }

        // Progress fill.
        if progress > 0.0 {
            let prog_w = track_rect.width() * progress;
            let prog_rect = egui::Rect::from_min_size(
                track_rect.min,
                Vec2::new(prog_w, track_h),
            );
            painter.rect_filled(prog_rect, track_h * 0.5, progress_color);
        }

        // Thumb — visible on hover, drag, or focus.
        let show_thumb = response.hovered() || response.dragged() || response.has_focus();
        if show_thumb && progress > 0.0 {
            let thumb_x = track_rect.min.x + track_rect.width() * progress;
            let thumb_y = track_y;
            // Outer ring.
            painter.circle_filled(egui::pos2(thumb_x, thumb_y), 7.0, thumb_color);
            // Inner dot (slightly darker).
            painter.circle_filled(egui::pos2(thumb_x, thumb_y), 3.0, Color32::from_rgb(80, 40, 0));
        }

        // Hover preview line (thin vertical line where the cursor is).
        if response.hovered() {
            if let Some(pos) = response.interact_pointer_pos() {
                if pos.x >= track_rect.min.x && pos.x <= track_rect.max.x {
                    painter.line_segment(
                        [
                            egui::pos2(pos.x, track_rect.min.y - 4.0),
                            egui::pos2(pos.x, track_rect.max.y + 4.0),
                        ],
                        egui::Stroke::new(1.0_f32, Color32::from_rgb(255, 168, 40)),
                    );
                }
            }
        }
    }

    // Compute new position if the user interacted.
    let new_pos = if response.dragged() || response.clicked() {
        let click_x = response.interact_pointer_pos().map(|p| p.x);
        click_x.map(|x| {
            let frac = ((x - track_rect.min.x) / track_rect.width()).clamp(0.0, 1.0);
            frac as f32
        })
    } else {
        None
    };

    (response, new_pos)
}
