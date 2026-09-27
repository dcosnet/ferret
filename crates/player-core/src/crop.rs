//! Map the on-screen zoom/pan view onto a source-space crop rectangle.
//!
//! mpv positions the video in the window with (per axis, see
//! `src_dst_split_scaling` in mpv's `video/out/aspect.c`):
//!
//! ```text
//! scaled_size = aspect_fit_size * 2^zoom
//! dst_start   = (window_size - scaled_size) / 2 + pan * scaled_size
//! ```
//!
//! i.e. the video is aspect-fit into the window (letterbox/pillarbox,
//! panscan 0, no margins), zoomed around the window center, and pan is
//! measured in fractions of the *scaled* video size.
//!
//! The functions here reproduce that transform to answer: "which rectangle
//! of source pixels is currently visible in the window?" — used by the
//! A-B clip export so a saved clip contains exactly the focus area the
//! user aligned, instead of the full frame.

/// A crop rectangle in source pixels: (x, y, width, height).
pub type CropRect = (u32, u32, u32, u32);

/// Compute the source-space rectangle visible in a `win_w` × `win_h`
/// window given the current zoom (log2 units) and pan (screen fractions).
///
/// Returns `None` when the whole frame is visible (no zoom in, no pan, or
/// the panned video still covers the window) — then no crop is needed.
/// Also `None` for degenerate inputs (unknown source or window size).
///
/// Dimensions are rounded to even numbers (yuv420p/libx264 friendly) by
/// shrinking, and coordinates are clamped inside the frame.
pub fn visible_crop(
    src_w: u32,
    src_h: u32,
    win_w: u32,
    win_h: u32,
    zoom: f32,
    pan_x: f32,
    pan_y: f32,
) -> Option<CropRect> {
    if src_w == 0 || src_h == 0 || win_w == 0 || win_h == 0 {
        return None;
    }
    let (sw, sh, ww, wh) = (src_w as f64, src_h as f64, win_w as f64, win_h as f64);
    if !zoom.is_finite() || !pan_x.is_finite() || !pan_y.is_finite() {
        return None;
    }

    // Aspect-fit scale (mpv `aspect_calc_panscan`, panscan 0, no margins).
    let fit = (ww / sw).min(wh / sh);
    let scale = fit * 2.0_f32.powf(zoom) as f64;

    // Scaled display size and top-left corner (centered + pan, pan in
    // units of the scaled size — exactly mpv's arithmetic).
    let dw = sw * scale;
    let dh = sh * scale;
    let x0 = (ww - dw) / 2.0 + (pan_x as f64) * dw;
    let y0 = (wh - dh) / 2.0 + (pan_y as f64) * dh;

    // Visible overlap of the video rect with the window, in window px.
    let vx0 = x0.max(0.0);
    let vx1 = (x0 + dw).min(ww);
    let vy0 = y0.max(0.0);
    let vy1 = (y0 + dh).min(wh);

    // Map the overlap back to source pixels.
    let sx = ((vx0 - x0) / dw * sw).floor().clamp(0.0, sw) as u32;
    let ex = ((vx1 - x0) / dw * sw).ceil().clamp(0.0, sw) as u32;
    let sy = ((vy0 - y0) / dh * sh).floor().clamp(0.0, sh) as u32;
    let ey = ((vy1 - y0) / dh * sh).ceil().clamp(0.0, sh) as u32;

    // Full frame visible → nothing to crop.
    let full = sx == 0 && sy == 0 && ex == src_w && ey == src_h;
    if full {
        return None;
    }

    // Even dimensions for yuv420p, kept inside the frame.
    let mut w = ex - sx;
    let mut h = ey - sy;
    w -= w % 2;
    h -= h % 2;
    if w == 0 || h == 0 {
        return None;
    }
    Some((sx, sy, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// src 100×100 in a 100×100 window: fit scale 1, no letterbox.
    #[test]
    fn no_zoom_no_pan_is_full_frame() {
        assert_eq!(visible_crop(100, 100, 100, 100, 0.0, 0.0, 0.0), None);
    }

    #[test]
    fn degenerate_inputs_are_none() {
        assert_eq!(visible_crop(0, 100, 100, 100, 1.0, 0.0, 0.0), None);
        assert_eq!(visible_crop(100, 100, 0, 100, 0.0, 0.5, 0.0), None);
    }

    /// 2× zoom, square video, square window: the centered half is visible.
    #[test]
    fn zoom_2x_shows_center_quarter() {
        let (x, y, w, h) = visible_crop(100, 100, 100, 100, 1.0, 0.0, 0.0).unwrap();
        assert_eq!((x, y, w, h), (25, 25, 50, 50));
    }

    /// Pan right by half the scaled size slides the video right, so the
    /// window ends up over the video's LEFT half (mpv: pan is relative to
    /// the scaled video size, not the window).
    #[test]
    fn pan_right_shows_left_half_of_source() {
        let (x, y, w, h) = visible_crop(100, 100, 100, 100, 0.0, 0.5, 0.0).unwrap();
        assert_eq!((x, y, w, h), (0, 0, 50, 100));
    }

    /// Pan beyond the window keeps the crop clamped to the frame edge: only
    /// the first 10 source columns remain visible.
    #[test]
    fn pan_off_screen_clamps() {
        let (x, _y, w, _h) = visible_crop(100, 100, 100, 100, 0.0, 0.9, 0.0).unwrap();
        assert_eq!((x, w), (0, 10));
    }

    /// Zoom *out* (0.5×) always leaves the whole frame visible — the
    /// letterbox bars belong to the window, not the source.
    #[test]
    fn zoom_out_needs_no_crop() {
        assert_eq!(visible_crop(100, 100, 100, 100, -1.0, 0.0, 0.0), None);
        // Panning a zoomed-out video can still push part of it off-screen.
        assert!(visible_crop(100, 100, 100, 100, -1.0, 0.9, 0.0).is_some());
    }

    /// Letterboxed fit: src 200×100 in a 400×100 window fits by height
    /// (scale 1), so 2× zoom makes x exactly fill the window while y
    /// overflows — the crop takes only the centered vertical half.
    #[test]
    fn letterboxed_fit_drives_scale() {
        let (x, y, w, h) = visible_crop(200, 100, 400, 100, 1.0, 0.0, 0.0).unwrap();
        assert_eq!((x, y, w, h), (0, 25, 200, 50));
    }

    /// Real-world-ish sizes stay even and in bounds. 2× zoom on 1920×1080
    /// shows source x 480..1440; panning right by a quarter of the scaled
    /// size slides the visible window back to x 0..960 (clamped at the
    /// frame edge), y stays the centered half 270..810.
    #[test]
    fn odd_sizes_round_to_even() {
        let (x, y, w, h) = visible_crop(1920, 1080, 1920, 1080, 1.0, 0.25, 0.0).unwrap();
        assert_eq!(w % 2, 0);
        assert_eq!(h % 2, 0);
        assert!(x + w <= 1920 && y + h <= 1080);
        assert_eq!((x, y, w, h), (0, 270, 960, 540));
    }
}
