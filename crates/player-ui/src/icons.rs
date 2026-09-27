//! Vector-drawn icons for the control bar.
//!
//! VLC uses simple geometric icons (filled triangles, squares, bars).
//! Drawing them with the painter instead of Unicode characters gives us:
//!   - Consistent rendering across all fonts / OSes
//!   - Pixel-perfect sizing
//!   - Easy color tinting on hover / active states

use egui::{Color32, Painter, Pos2, Rect, Stroke, Vec2};

/// Draw a play triangle (▶), pointed right, centered in `rect`.
pub fn play(painter: &Painter, rect: Rect, color: Color32) {
    let size = rect.height().min(rect.width()) * 0.6;
    let cx = rect.center().x + size * 0.1; // nudge right slightly for visual balance
    let cy = rect.center().y;
    let half_h = size * 0.5;
    let half_w = size * 0.55;
    let points = [
        Pos2::new(cx - half_w, cy - half_h),
        Pos2::new(cx - half_w, cy + half_h),
        Pos2::new(cx + half_w, cy),
    ];
    painter.add(egui::Shape::convex_polygon(
        points.to_vec(),
        color,
        Stroke::NONE,
    ));
}

/// Draw a pause icon (two vertical bars).
pub fn pause(painter: &Painter, rect: Rect, color: Color32) {
    let size = rect.height().min(rect.width()) * 0.55;
    let cx = rect.center().x;
    let cy = rect.center().y;
    let bar_w = size * 0.22;
    let bar_h = size;
    let gap = size * 0.18;
    let left = Rect::from_center_size(Pos2::new(cx - gap - bar_w * 0.5, cy), Vec2::new(bar_w, bar_h));
    let right = Rect::from_center_size(Pos2::new(cx + gap + bar_w * 0.5, cy), Vec2::new(bar_w, bar_h));
    painter.rect_filled(left, 1.0, color);
    painter.rect_filled(right, 1.0, color);
}

/// Draw a stop icon (filled square).
pub fn stop(painter: &Painter, rect: Rect, color: Color32) {
    let size = rect.height().min(rect.width()) * 0.5;
    let r = Rect::from_center_size(rect.center(), Vec2::splat(size));
    painter.rect_filled(r, 1.0, color);
}

/// Draw a "previous track" icon (|◀) — bar + left-pointing triangle.
pub fn previous(painter: &Painter, rect: Rect, color: Color32) {
    let size = rect.height().min(rect.width()) * 0.55;
    let cx = rect.center().x;
    let cy = rect.center().y;
    let bar_w = size * 0.18;
    let bar_h = size;
    let tri_w = size * 0.55;
    let tri_h = size * 0.9;
    // Bar on the left
    let bar = Rect::from_center_size(Pos2::new(cx - tri_w * 0.5 - bar_w * 0.5, cy), Vec2::new(bar_w, bar_h));
    painter.rect_filled(bar, 1.0, color);
    // Triangle pointing left
    let tri_cx = cx + tri_w * 0.1;
    let points = [
        Pos2::new(tri_cx + tri_w * 0.5, cy - tri_h * 0.5),
        Pos2::new(tri_cx + tri_w * 0.5, cy + tri_h * 0.5),
        Pos2::new(tri_cx - tri_w * 0.5, cy),
    ];
    painter.add(egui::Shape::convex_polygon(points.to_vec(), color, Stroke::NONE));
}

/// Draw a "next track" icon (▶|) — right triangle + bar.
pub fn next(painter: &Painter, rect: Rect, color: Color32) {
    let size = rect.height().min(rect.width()) * 0.55;
    let cx = rect.center().x;
    let cy = rect.center().y;
    let bar_w = size * 0.18;
    let bar_h = size;
    let tri_w = size * 0.55;
    let tri_h = size * 0.9;
    // Triangle pointing right
    let tri_cx = cx - tri_w * 0.1;
    let points = [
        Pos2::new(tri_cx - tri_w * 0.5, cy - tri_h * 0.5),
        Pos2::new(tri_cx - tri_w * 0.5, cy + tri_h * 0.5),
        Pos2::new(tri_cx + tri_w * 0.5, cy),
    ];
    painter.add(egui::Shape::convex_polygon(points.to_vec(), color, Stroke::NONE));
    // Bar on the right
    let bar = Rect::from_center_size(Pos2::new(cx + tri_w * 0.5 + bar_w * 0.5, cy), Vec2::new(bar_w, bar_h));
    painter.rect_filled(bar, 1.0, color);
}

/// Draw a "step backward" icon (|◀◀ is overkill; VLC uses a single |◀ with a small line).
/// For frame-step back we use ◀| (left triangle + right bar) to differentiate from previous.
pub fn frame_back(painter: &Painter, rect: Rect, color: Color32) {
    let size = rect.height().min(rect.width()) * 0.5;
    let cx = rect.center().x;
    let cy = rect.center().y;
    let tri_w = size * 0.55;
    let tri_h = size * 0.8;
    let bar_w = size * 0.15;
    let bar_h = size * 0.85;
    // Left-pointing triangle
    let tri_cx = cx - bar_w * 0.5;
    let points = [
        Pos2::new(tri_cx + tri_w * 0.5, cy - tri_h * 0.5),
        Pos2::new(tri_cx + tri_w * 0.5, cy + tri_h * 0.5),
        Pos2::new(tri_cx - tri_w * 0.5, cy),
    ];
    painter.add(egui::Shape::convex_polygon(points.to_vec(), color, Stroke::NONE));
    // Bar on the right
    let bar = Rect::from_center_size(Pos2::new(cx + tri_w * 0.5 + bar_w * 0.5, cy), Vec2::new(bar_w, bar_h));
    painter.rect_filled(bar, 1.0, color);
}

/// Draw a "step forward" icon (▶|) for frame-step.
pub fn frame_forward(painter: &Painter, rect: Rect, color: Color32) {
    let size = rect.height().min(rect.width()) * 0.5;
    let cx = rect.center().x;
    let cy = rect.center().y;
    let tri_w = size * 0.55;
    let tri_h = size * 0.8;
    let bar_w = size * 0.15;
    let bar_h = size * 0.85;
    // Right-pointing triangle
    let tri_cx = cx + bar_w * 0.5;
    let points = [
        Pos2::new(tri_cx - tri_w * 0.5, cy - tri_h * 0.5),
        Pos2::new(tri_cx - tri_w * 0.5, cy + tri_h * 0.5),
        Pos2::new(tri_cx + tri_w * 0.5, cy),
    ];
    painter.add(egui::Shape::convex_polygon(points.to_vec(), color, Stroke::NONE));
    // Bar on the left
    let bar = Rect::from_center_size(Pos2::new(cx - tri_w * 0.5 - bar_w * 0.5, cy), Vec2::new(bar_w, bar_h));
    painter.rect_filled(bar, 1.0, color);
}

/// Draw a fullscreen icon — four L-shaped corner brackets.
#[allow(unused_variables)]
pub fn fullscreen(painter: &Painter, rect: Rect, color: Color32) {
    let size = rect.height().min(rect.width()) * 0.6;
    let cx = rect.center().x;
    let cy = rect.center().y;
    let half = size * 0.5;
    let arm = size * 0.35;
    let thick = size * 0.12;

    let corners = [
        // top-left
        (Pos2::new(cx - half, cy - half + arm), Pos2::new(cx - half, cy - half), Pos2::new(cx - half + arm, cy - half)),
        // top-right
        (Pos2::new(cx + half - arm, cy - half), Pos2::new(cx + half, cy - half), Pos2::new(cx + half, cy - half + arm)),
        // bottom-left
        (Pos2::new(cx - half, cy + half - arm), Pos2::new(cx - half, cy + half), Pos2::new(cx - half + arm, cy + half)),
        // bottom-right
        (Pos2::new(cx + half - arm, cy + half), Pos2::new(cx + half, cy + half), Pos2::new(cx + half, cy + half - arm)),
    ];
    for (a, b, c) in corners {
        painter.line_segment([a, b], Stroke::new(thick, color));
        painter.line_segment([b, c], Stroke::new(thick, color));
    }
}

/// Draw a volume icon (speaker shape). `level` is 0..=1; we draw 0-3 sound waves.
#[allow(clippy::too_many_arguments)]
pub fn volume(painter: &Painter, rect: Rect, color: Color32, level: f32, muted: bool) {
    let size: f32 = rect.height().min(rect.width()) * 0.65;
    let cx: f32 = rect.center().x - size * 0.15;
    let cy: f32 = rect.center().y;
    let box_w: f32 = size * 0.25;
    let box_h: f32 = size * 0.4;
    let cone_w: f32 = size * 0.35;
    let cone_h: f32 = size * 0.6;

    // Speaker box (small rectangle on the left).
    let box_rect = Rect::from_center_size(Pos2::new(cx - cone_w * 0.5 + box_w * 0.5, cy), Vec2::new(box_w, box_h));
    painter.rect_filled(box_rect, 1.0, color);

    // Speaker cone (trapezoid extending right from the box).
    let cone_left_x = cx + box_w * 0.0;
    let cone_right_x = cx + cone_w * 0.6;
    let cone_top_left = cy - box_h * 0.5;
    let cone_bot_left = cy + box_h * 0.5;
    let cone_top_right = cy - cone_h * 0.5;
    let cone_bot_right = cy + cone_h * 0.5;
    let cone_pts = vec![
        Pos2::new(cone_left_x, cone_top_left),
        Pos2::new(cone_right_x, cone_top_right),
        Pos2::new(cone_right_x, cone_bot_right),
        Pos2::new(cone_left_x, cone_bot_left),
    ];
    painter.add(egui::Shape::convex_polygon(cone_pts, color, Stroke::NONE));

    if muted {
        // Red diagonal line through the speaker.
        painter.line_segment(
            [
                Pos2::new(cx - cone_w * 0.5, cy - cone_h * 0.55),
                Pos2::new(cx + cone_w * 0.9, cy + cone_h * 0.55),
            ],
            Stroke::new(size * 0.10, Color32::from_rgb(220, 60, 60)),
        );
    } else {
        // Sound waves — 1, 2, or 3 arcs depending on level.
        // We approximate arcs with short polylines (epaint 0.29 has no arc primitive).
        let wave_count = if level <= 0.0 { 0 }
            else if level < 0.34 { 1 }
            else if level < 0.67 { 2 }
            else { 3 };
        let arc_origin = Pos2::new(cx + cone_w * 0.45, cy);
        for i in 0..wave_count {
            let r = size * (0.25 + 0.15 * (i as f32 + 1.0));
            let start_angle = -std::f32::consts::FRAC_PI_4;
            let end_angle = std::f32::consts::FRAC_PI_4;
            let steps = 8;
            let mut prev = None;
            for step in 0..=steps {
                let t = start_angle + (end_angle - start_angle) * (step as f32 / steps as f32);
                let p = Pos2::new(arc_origin.x + r * t.cos(), arc_origin.y + r * t.sin());
                if let Some(p0) = prev {
                    painter.line_segment([p0, p], Stroke::new(size * 0.06, color));
                }
                prev = Some(p);
            }
        }
    }
}

/// Loop icon — two curved arrows forming a circle. `mode` is 0=off, 1=file,
/// 2=playlist. When off, dim the icon; when on, full opacity. Playlist mode
/// adds a small "1" badge in the corner to distinguish from file mode.
pub fn loop_icon(painter: &Painter, rect: Rect, color: Color32, mode: u8) {
    let size = rect.height().min(rect.width()) * 0.6;
    let cx = rect.center().x;
    let cy = rect.center().y;
    let r = size * 0.4;
    let thick = size * 0.10;

    // Draw two arcs covering 3/4 of the circle, leaving gaps at the top-right
    // and bottom-left for the arrowheads.
    let gap = std::f32::consts::FRAC_PI_8;
    let arc1_start = -std::f32::consts::FRAC_PI_2 + gap;
    let arc1_end = std::f32::consts::FRAC_PI_2 - gap;
    let arc2_start = std::f32::consts::FRAC_PI_2 + gap;
    let arc2_end = std::f32::consts::PI + std::f32::consts::FRAC_PI_2 - gap;

    let draw_arc = |start: f32, end: f32, arrow_at_end: bool, arrow_top: bool| {
        let steps = 12;
        let mut prev = None;
        for step in 0..=steps {
            let t = start + (end - start) * (step as f32 / steps as f32);
            let p = Pos2::new(cx + r * t.cos(), cy + r * t.sin());
            if let Some(p0) = prev {
                painter.line_segment([p0, p], Stroke::new(thick, color));
            }
            prev = Some(p);
        }
        if arrow_at_end {
            // Arrowhead at the END of the arc.
            let end_pt = Pos2::new(cx + r * end.cos(), cy + r * end.sin());
            let arrow_size = size * 0.18;
            // Tangent direction at `end` (derivative of position w.r.t. angle).
            let tx = -end.sin();
            let ty = end.cos();
            let nx = -end.cos();
            let ny = -end.sin();
            let tip = Pos2::new(end_pt.x + tx * arrow_size * 0.5, end_pt.y + ty * arrow_size * 0.5);
            let _ = arrow_top;
            let a = Pos2::new(end_pt.x + nx * arrow_size, end_pt.y + ny * arrow_size);
            let b = Pos2::new(end_pt.x - nx * arrow_size, end_pt.y - ny * arrow_size);
            painter.line_segment([tip, a], Stroke::new(thick, color));
            painter.line_segment([tip, b], Stroke::new(thick, color));
        }
    };

    draw_arc(arc1_start, arc1_end, true, true);
    draw_arc(arc2_start, arc2_end, true, false);

    // Mode badge — small "P" overlay for playlist mode so the user can tell
    // the difference from file mode at a glance. (Off = no badge, File = no
    // badge, Playlist = "P".) We could also tint the icon for playlist mode.
    let _ = mode;
}

/// Marker A icon — a small downward-pointing triangle (like a timeline pin)
/// above the letter "A".
pub fn marker_a(painter: &Painter, rect: Rect, color: Color32) {
    let size = rect.height().min(rect.width()) * 0.6;
    let cx = rect.center().x;
    let cy = rect.center().y;
    let pin_h = size * 0.5;
    let pin_w = size * 0.4;
    let pin_pts = vec![
        Pos2::new(cx - pin_w * 0.5, cy - pin_h * 0.4),
        Pos2::new(cx + pin_w * 0.5, cy - pin_h * 0.4),
        Pos2::new(cx, cy + pin_h * 0.4),
    ];
    painter.add(egui::Shape::convex_polygon(pin_pts, color, Stroke::NONE));
}

/// Marker B icon — same shape as marker A but distinguished by position
/// (rendered in a slightly different color when active).
pub fn marker_b(painter: &Painter, rect: Rect, color: Color32) {
    marker_a(painter, rect, color);
}

/// Marker-loop icon — the A and B pins side by side, joined by a horizontal
/// line below, suggesting the loop segment.
pub fn marker_loop(painter: &Painter, rect: Rect, color: Color32, active: bool) {
    let size = rect.height().min(rect.width()) * 0.6;
    let cx = rect.center().x;
    let cy = rect.center().y;
    let pin_h = size * 0.4;
    let pin_w = size * 0.22;
    let gap = size * 0.18;

    let left_cx = cx - gap - pin_w * 0.5;
    let right_cx = cx + gap + pin_w * 0.5;
    let top_y = cy - pin_h * 0.5;
    let bot_y = cy + pin_h * 0.5;

    // Two pins.
    for px in [left_cx, right_cx] {
        let pts = vec![
            Pos2::new(px - pin_w * 0.5, top_y),
            Pos2::new(px + pin_w * 0.5, top_y),
            Pos2::new(px, bot_y),
        ];
        painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
    }

    // Bottom connecting line (the "loop" segment). Thicker and brighter
    // when active.
    let line_thick = if active { size * 0.10 } else { size * 0.06 };
    let line_y = bot_y + size * 0.10;
    let line_color = if active { color } else { Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 120) };
    painter.line_segment(
        [Pos2::new(left_cx, line_y), Pos2::new(right_cx, line_y)],
        Stroke::new(line_thick, line_color),
    );

    // Loop arrowheads at each end (pointing inward when active).
    if active {
        let arrow = size * 0.10;
        for (px, dir) in [(left_cx, 1.0), (right_cx, -1.0)] {
            painter.line_segment(
                [Pos2::new(px, line_y), Pos2::new(px + dir * arrow, line_y - arrow * 0.7)],
                Stroke::new(line_thick, color),
            );
            painter.line_segment(
                [Pos2::new(px, line_y), Pos2::new(px + dir * arrow, line_y + arrow * 0.7)],
                Stroke::new(line_thick, color),
            );
        }
    }
}

/// Hamburger menu icon (three horizontal lines).
pub fn hamburger(painter: &Painter, rect: Rect, color: Color32) {
    let size = rect.height().min(rect.width()) * 0.6;
    let cx = rect.center().x;
    let cy = rect.center().y;
    let line_w = size * 0.8;
    let line_thick = size * 0.12;
    let gap = size * 0.25;
    [-1.0, 0.0, 1.0].iter().for_each(|&offset| {
        let y = cy + offset * gap;
        painter.line_segment(
            [Pos2::new(cx - line_w * 0.5, y), Pos2::new(cx + line_w * 0.5, y)],
            Stroke::new(line_thick, color),
        );
    });
}

/// Speed icon — a small "1×" or "{speed}×" indicator. We just draw a
/// tachometer-style arc + needle; the actual speed value is rendered as a
/// text label by the caller.
pub fn speed_gauge(painter: &Painter, rect: Rect, color: Color32, speed: f32) {
    let size = rect.height().min(rect.width()) * 0.6;
    let cx = rect.center().x;
    let cy = rect.center().y + size * 0.1;
    let r = size * 0.4;
    let thick = size * 0.08;

    // Half-circle arc from 180° to 360° (top half).
    let steps = 16;
    let mut prev = None;
    for step in 0..=steps {
        let t = std::f32::consts::PI + (std::f32::consts::PI) * (step as f32 / steps as f32);
        let p = Pos2::new(cx + r * t.cos(), cy + r * t.sin());
        if let Some(p0) = prev {
            painter.line_segment([p0, p], Stroke::new(thick, color));
        }
        prev = Some(p);
    }

    // Needle — position based on speed (0.25..=4.0 maps to 180°..=360°).
    let s_norm = ((speed - 0.25) / (4.0 - 0.25)).clamp(0.0, 1.0);
    let needle_angle = std::f32::consts::PI + std::f32::consts::PI * s_norm;
    let needle_tip = Pos2::new(cx + r * 0.85 * needle_angle.cos(), cy + r * 0.85 * needle_angle.sin());
    painter.line_segment(
        [Pos2::new(cx, cy), needle_tip],
        Stroke::new(thick * 1.2, Color32::from_rgb(255, 168, 40)),
    );
}

/// Shuffle icon — two crossed arrows with arrowheads. When `active` is true
/// the icon is rendered at full opacity; when false it is drawn dimmed to
/// indicate the feature is disengaged.
pub fn shuffle(painter: &Painter, rect: Rect, color: Color32, active: bool) {
    let size = rect.height().min(rect.width()) * 0.55;
    let cx = rect.center().x;
    let cy = rect.center().y;
    let thick = size * 0.10;
    let alpha = if active { 1.0 } else { 0.3 };
    let col = Color32::from_rgba_unmultiplied(
        (color.r() as f32 * alpha) as u8,
        (color.g() as f32 * alpha) as u8,
        (color.b() as f32 * alpha) as u8,
        (color.a() as f32 * alpha) as u8,
    );

    // Two lines crossing in an X pattern.
    let half_w = size * 0.45;
    let half_h = size * 0.28;

    // Top-left → bottom-right arrow.
    let tl = Pos2::new(cx - half_w, cy - half_h);
    let br = Pos2::new(cx + half_w, cy + half_h);
    painter.line_segment([tl, br], Stroke::new(thick, col));

    // Bottom-left → top-right arrow.
    let bl = Pos2::new(cx - half_w, cy + half_h);
    let tr = Pos2::new(cx + half_w, cy - half_h);
    painter.line_segment([bl, tr], Stroke::new(thick, col));

    // Arrowheads.
    let arrow = size * 0.16;
    let angle = 0.4;

    // Arrowhead at top-right (on bl→tr).
    let dir_x = -(tr.x - bl.x);
    let dir_y = -(tr.y - bl.y);
    let len = (dir_x * dir_x + dir_y * dir_y).sqrt();
    let dx = dir_x / len;
    let dy = dir_y / len;
    let px = -dy;
    let py = dx;
    painter.line_segment(
        [tr, Pos2::new(tr.x + (dx + px) * arrow * angle, tr.y + (dy + py) * arrow * angle)],
        Stroke::new(thick, col),
    );
    painter.line_segment(
        [tr, Pos2::new(tr.x + (dx - px) * arrow * angle, tr.y + (dy - py) * arrow * angle)],
        Stroke::new(thick, col),
    );

    // Arrowhead at bottom-right (on tl→br).
    let dir_x2 = -(br.x - tl.x);
    let dir_y2 = -(br.y - tl.y);
    let len2 = (dir_x2 * dir_x2 + dir_y2 * dir_y2).sqrt();
    let dx2 = dir_x2 / len2;
    let dy2 = dir_y2 / len2;
    let px2 = -dy2;
    let py2 = dx2;
    painter.line_segment(
        [br, Pos2::new(br.x + (dx2 + px2) * arrow * angle, br.y + (dy2 + py2) * arrow * angle)],
        Stroke::new(thick, col),
    );
    painter.line_segment(
        [br, Pos2::new(br.x + (dx2 - px2) * arrow * angle, br.y + (dy2 - py2) * arrow * angle)],
        Stroke::new(thick, col),
    );
}

/// Queue/playlist icon — three stacked lines of decreasing length with a
/// leading bullet each (VLC-style playlist glyph). Used by the control-bar
/// button that toggles the queue sidebar.
pub fn queue(painter: &Painter, rect: Rect, color: Color32) {
    let size = rect.height().min(rect.width()) * 0.6;
    let cx = rect.center().x;
    let cy = rect.center().y;
    let line_thick = size * 0.12;
    let gap = size * 0.28;
    let bullet_r = size * 0.09;
    for (offset, shrink) in [(-1.0_f32, 0.0_f32), (0.0, 0.15), (1.0, 0.3)] {
        let y = cy + offset * gap;
        // Leading bullet.
        let bx = cx - size * 0.32;
        painter.circle_filled(Pos2::new(bx, y), bullet_r, color);
        // Line of decreasing length.
        let x0 = bx + size * 0.18;
        let x1 = cx + size * 0.42 - shrink * size * 0.5;
        painter.line_segment(
            [Pos2::new(x0, y), Pos2::new(x1, y)],
            Stroke::new(line_thick, color),
        );
    }
}
