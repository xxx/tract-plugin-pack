//! Vectorscope view: half-polar Ozone-style disc, full-square polar dot cloud,
//! Lissajous trace, or polar level (per-pan-angle level histogram).
//! Below the scope: correlation bar + balance bar.

use crate::polar_rays::{Ray, RING_CAPACITY as POLAR_RING_CAPACITY};
use crate::theme;
use crate::vectorscope::VectorConsumer;
use crate::ImagineParams;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, PixmapMut, Rect, Transform};
use tiny_skia_widgets::TextRenderer;

/// Per-frame audio history snapshot fed to the dot-cloud modes
/// (HalfPolar / Polar / Lissajous). Sized for ~800 ms at 48 kHz so the
/// rendered history matches Ozone's slow-decay phosphor look. Each dot
/// is alpha-blended with a per-sample age fade, so older samples within
/// this window render dimmer than newer ones.
const SAMPLE_BUDGET: usize = 38_400;

/// Reserved footer height beneath the scope dot cloud for the mode label
/// ("Polar" / "Goniometer" / "Lissajous"). Sits between the scope and the
/// correlation/balance bars (which themselves occupy the bottom 36 px).
const MODE_LABEL_FOOTER_H: i32 = 16;

#[inline]
fn scaled(v: i32, s: f32) -> i32 {
    ((v as f32) * s).round().max(1.0) as i32
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VectorMode {
    /// Ozone-style half-disc polar scope. Mono → straight up; hard-L /
    /// hard-R in-phase → upper-left / upper-right 45° spokes (per the
    /// Ozone manual's "safe lines"); anti-phase content → baseline
    /// corners. Default.
    HalfPolar = 0,
    /// Full-square 45°-rotated dot cloud, dual-tone (pink = L, cyan = R).
    Polar = 1,
    /// Ozone-style polar level: periodic peak-pick emit ring rendered as
    /// triangular fans on the same half-disc geometry as `HalfPolar`.
    PolarLevel = 2,
    /// L on X, R on Y (no rotation).
    Lissajous = 3,
}

impl VectorMode {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => VectorMode::Polar,
            2 => VectorMode::PolarLevel,
            3 => VectorMode::Lissajous,
            _ => VectorMode::HalfPolar,
        }
    }

    pub fn as_u32(self) -> u32 {
        self as u32
    }

    pub fn next(self) -> Self {
        match self {
            VectorMode::HalfPolar => VectorMode::Polar,
            VectorMode::Polar => VectorMode::PolarLevel,
            VectorMode::PolarLevel => VectorMode::Lissajous,
            VectorMode::Lissajous => VectorMode::HalfPolar,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VectorMode::HalfPolar => "Polar",
            VectorMode::Polar => "Goniometer",
            VectorMode::PolarLevel => "Polar Level",
            VectorMode::Lissajous => "Lissajous",
        }
    }
}

/// Local fill helper using PixmapMut::fill_rect with opaque BlendMode::Source.
fn fill_rect_i(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = false;
    paint.blend_mode = if color.is_opaque() {
        tiny_skia::BlendMode::Source
    } else {
        tiny_skia::BlendMode::SourceOver
    };
    if let Some(rect) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) {
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

fn stroke_rect_i(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    fill_rect_i(pixmap, x, y, w, 1, color);
    fill_rect_i(pixmap, x, y + h - 1, w, 1, color);
    fill_rect_i(pixmap, x, y, 1, h, color);
    fill_rect_i(pixmap, x + w - 1, y, 1, h, color);
}

/// Direct-pixel fill for a single 1×1 dot. Bounds-checked.
fn put_pixel(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, color: Color) {
    let w = pixmap.width() as i32;
    let h = pixmap.height() as i32;
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let pixels = pixmap.pixels_mut();
    let idx = (y as usize) * (w as usize) + (x as usize);
    let cu = color.to_color_u8();
    pixels[idx] =
        tiny_skia::PremultipliedColorU8::from_rgba(cu.red(), cu.green(), cu.blue(), cu.alpha())
            .unwrap_or(pixels[idx]);
}

/// Source-over alpha blend of `color` onto a single pixel. Used for the
/// phosphor-decay dot-cloud renderers — each dot writes with low alpha,
/// so dense regions accumulate toward full brightness while sparse
/// regions stay dim. Output stays opaque (alpha=255).
fn blend_pixel(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, color: Color, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let w = pixmap.width() as i32;
    let h = pixmap.height() as i32;
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let alpha = alpha.min(1.0);
    let inv = 1.0 - alpha;
    let pixels = pixmap.pixels_mut();
    let idx = (y as usize) * (w as usize) + (x as usize);
    let dst = pixels[idx];
    let cu = color.to_color_u8();
    let r = ((cu.red() as f32) * alpha + (dst.red() as f32) * inv).round() as u8;
    let g = ((cu.green() as f32) * alpha + (dst.green() as f32) * inv).round() as u8;
    let b = ((cu.blue() as f32) * alpha + (dst.blue() as f32) * inv).round() as u8;
    pixels[idx] =
        tiny_skia::PremultipliedColorU8::from_rgba(r, g, b, 255).unwrap_or(dst);
}

/// 2×2 dot variant of `blend_pixel`. Writes the same colour with the
/// same alpha to four adjacent pixels (`(x, y)` plus right, down, and
/// down-right neighbours). Each dot covers 4× the area, so single
/// stray samples produce visible "haze" instead of being lost as
/// sub-pixel-dim flecks. Used by the dot-cloud vectorscope modes.
fn blend_dot_2x2(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, color: Color, alpha: f32) {
    blend_pixel(pixmap, x, y, color, alpha);
    blend_pixel(pixmap, x + 1, y, color, alpha);
    blend_pixel(pixmap, x, y + 1, color, alpha);
    blend_pixel(pixmap, x + 1, y + 1, color, alpha);
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    pixmap: &mut Pixmap,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    params: &Arc<ImagineParams>,
    vec: &Arc<VectorConsumer>,
    vec_l: &mut Vec<f32>,
    vec_r: &mut Vec<f32>,
    text_renderer: &mut TextRenderer,
    scale_factor: f32,
) {
    let s = scale_factor.max(0.1);
    let mode = VectorMode::from_u32(params.vector_mode.load(Ordering::Relaxed));
    let corr = f32::from_bits(params.correlation.load(Ordering::Relaxed));
    let bal = f32::from_bits(params.balance.load(Ordering::Relaxed));

    let pad = scaled(6, s);
    let bar_h = scaled(8, s);
    let bar_gap = scaled(4, s);
    let bottom_reserve = scaled(36, s);
    let footer_h = scaled(MODE_LABEL_FOOTER_H, s);

    {
        let mut pm = pixmap.as_mut();
        fill_rect_i(&mut pm, x, y, w, h, theme::panel_bg());
        stroke_rect_i(&mut pm, x, y, w, h, theme::border());

        // Reserve bottom area for correlation + balance bars, plus an
        // additional footer above them for the mode label so it
        // doesn't overlap the dot cloud.
        let scope_h = h - bottom_reserve - footer_h;
        let scope_x = x + pad;
        let scope_y = y + pad;
        let scope_w = w - 2 * pad;

        // Snapshot up to SAMPLE_BUDGET samples.
        if vec_l.len() < SAMPLE_BUDGET {
            vec_l.resize(SAMPLE_BUDGET, 0.0);
            vec_r.resize(SAMPLE_BUDGET, 0.0);
        }
        let n = vec.snapshot(SAMPLE_BUDGET, vec_l, vec_r);

        let cx = scope_x + scope_w / 2;
        let cy = scope_y + scope_h / 2;
        let r = scope_w.min(scope_h) / 2 - pad;

        match mode {
            VectorMode::HalfPolar | VectorMode::PolarLevel => {
                // Half-disc geometry has its own grid (half-circle outline +
                // spokes + baseline) drawn inside the per-mode renderer.
            }
            VectorMode::Polar | VectorMode::Lissajous => {
                // Background grid (a faint cross) for the square modes.
                fill_rect_i(
                    &mut pm,
                    scope_x,
                    scope_y + scope_h / 2,
                    scope_w,
                    1,
                    theme::text_dim(),
                );
                fill_rect_i(
                    &mut pm,
                    scope_x + scope_w / 2,
                    scope_y,
                    1,
                    scope_h,
                    theme::text_dim(),
                );
            }
        }

        match mode {
            VectorMode::HalfPolar => draw_half_polar(
                &mut pm,
                scope_x,
                scope_y,
                scope_w,
                scope_h,
                &vec_l[..n],
                &vec_r[..n],
                theme::cyan(),
                s,
            ),
            VectorMode::Polar => draw_polar(&mut pm, cx, cy, r, &vec_l[..n], &vec_r[..n]),
            VectorMode::Lissajous => draw_lissajous(&mut pm, cx, cy, r, &vec_l[..n], &vec_r[..n]),
            VectorMode::PolarLevel => draw_polar_level(
                &mut pm,
                scope_x,
                scope_y,
                scope_w,
                scope_h,
                params,
                s,
            ),
        }

        // Correlation bar at the bottom. Bar layout (from bottom up):
        //   pad-from-bottom (bar_gap), bal bar (bar_h), gap, corr bar (bar_h)
        let bal_y = y + h - bar_h - bar_gap;
        let corr_y = bal_y - bar_h - bar_gap;
        let corr_color = theme::cyan_to_pink(if corr > 0.0 { 0.0 } else { 1.0 });
        draw_meter_bar(
            &mut pm,
            x + pad,
            corr_y,
            w - 2 * pad,
            bar_h,
            corr,
            corr_color,
        );

        // Balance bar
        draw_meter_bar(
            &mut pm,
            x + pad,
            bal_y,
            w - 2 * pad,
            bar_h,
            bal,
            theme::accent(),
        );
    }

    // Text labels.
    let label_size = (10.0_f32 * s).max(6.0);
    let small_size = (9.0_f32 * s).max(6.0);
    let mode_label = mode.label();

    // For half-disc modes (HalfPolar, PolarLevel), render the L/R corner
    // labels and the +1/0 amplitude markers. The disc geometry must mirror
    // draw_half_polar / draw_polar_level exactly.
    if mode == VectorMode::HalfPolar || mode == VectorMode::PolarLevel {
        let scope_h = h - bottom_reserve - footer_h;
        let scope_x = x + pad;
        let scope_y = y + pad;
        let scope_w = w - 2 * pad;
        let (cx, base_y, disc_radius) = half_disc_geometry(scope_x, scope_y, scope_w, scope_h, s);
        let r_f = disc_radius as f32;
        let cx_f = cx as f32;
        let base_yf = base_y as f32;
        let pad_f = (4.0_f32 * s).max(2.0);

        // L label (bottom-left corner, just outside the disc baseline).
        text_renderer.draw_text(
            pixmap,
            cx_f - r_f - small_size * 0.6 - pad_f,
            base_yf + small_size * 0.4,
            "L",
            small_size,
            theme::text_dim(),
        );
        // R label (bottom-right corner, just outside the disc baseline).
        text_renderer.draw_text(
            pixmap,
            cx_f + r_f + pad_f,
            base_yf + small_size * 0.4,
            "R",
            small_size,
            theme::text_dim(),
        );
        // Amplitude markers along the right edge: +1 at top, 0 at base.
        text_renderer.draw_text(
            pixmap,
            cx_f + r_f + pad_f,
            base_yf - r_f + small_size * 0.4,
            "+1",
            small_size,
            theme::text_dim(),
        );
        text_renderer.draw_text(
            pixmap,
            cx_f + pad_f,
            base_yf - small_size * 0.2,
            "0",
            small_size,
            theme::text_dim(),
        );
    }
    // Mode toggle area is the bottom-left corner above the meter bars
    // (matches the hit region in the editor: 80×16 px starting at +pad from
    // the panel's left edge, ending bottom_reserve px above the bottom).
    let toggle_y1 = (y + h) as f32 - bottom_reserve as f32;
    let toggle_y0 = toggle_y1 - footer_h as f32;
    text_renderer.draw_text(
        pixmap,
        x as f32 + pad as f32,
        toggle_y0 + (toggle_y1 - toggle_y0) * 0.5 + label_size * 0.35,
        mode_label,
        label_size,
        theme::text(),
    );

    // Correlation / Balance captions and values. Recompute the bar positions
    // (mirrors the shape pass above) so captions sit just above each bar.
    let bal_y = y + h - bar_h - bar_gap;
    let corr_y = bal_y - bar_h - bar_gap;
    let corr_text = format!("Corr {:+.2}", corr);
    text_renderer.draw_text(
        pixmap,
        x as f32 + pad as f32,
        corr_y as f32 - 2.0,
        &corr_text,
        small_size,
        theme::text_dim(),
    );
    let bal_text = format!("Bal {:+.2}", bal);
    text_renderer.draw_text(
        pixmap,
        x as f32 + pad as f32,
        bal_y as f32 - 2.0,
        &bal_text,
        small_size,
        theme::text_dim(),
    );
}

/// Polar-sample dot cloud, 45°-rotated so mono = vertical axis.
/// Pink for L-leaning, cyan for R-leaning.
fn draw_polar(pixmap: &mut PixmapMut<'_>, cx: i32, cy: i32, radius: i32, l: &[f32], r: &[f32]) {
    let r_f = radius as f32;
    let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
    let n = l.len().min(r.len());
    if n == 0 {
        return;
    }
    let inv_n_minus_1 = if n > 1 { 1.0 / (n - 1) as f32 } else { 0.0 };
    for i in 0..n {
        let xn = (l[i] - r[i]) * inv_sqrt2;
        let yn = -((l[i] + r[i]) * inv_sqrt2);
        let px = cx + (xn.clamp(-1.0, 1.0) * r_f) as i32;
        let py = cy + (yn.clamp(-1.0, 1.0) * r_f) as i32;
        // Color by L-vs-R dominance.
        let denom = l[i].abs() + r[i].abs() + 1e-9;
        let bias = if l[i].abs() > r[i].abs() {
            ((l[i].abs() - r[i].abs()) / denom).clamp(0.0, 1.0)
        } else {
            -((r[i].abs() - l[i].abs()) / denom).clamp(0.0, 1.0)
        };
        // bias > 0 → pink (L); bias < 0 → cyan (R)
        let t = (bias + 1.0) * 0.5;
        let color = theme::cyan_to_pink(t);
        // Snapshot returns oldest at i=0, newest at i=n-1. Freshness
        // ramps 0 (oldest) → 1 (newest), so newest dots are bright and
        // oldest fade to background — matches Ozone's phosphor decay.
        let freshness = (i as f32) * inv_n_minus_1;
        let alpha = DOT_BASE_ALPHA * freshness;
        blend_dot_2x2(pixmap, px, py, color, alpha);
    }
}

/// Lissajous: L on X, R on Y, no rotation.
fn draw_lissajous(pixmap: &mut PixmapMut<'_>, cx: i32, cy: i32, radius: i32, l: &[f32], r: &[f32]) {
    let r_f = radius as f32;
    let n = l.len().min(r.len());
    if n == 0 {
        return;
    }
    let inv_n_minus_1 = if n > 1 { 1.0 / (n - 1) as f32 } else { 0.0 };
    let color = theme::accent();
    for i in 0..n {
        let px = cx + (l[i].clamp(-1.0, 1.0) * r_f) as i32;
        let py = cy - (r[i].clamp(-1.0, 1.0) * r_f) as i32;
        let freshness = (i as f32) * inv_n_minus_1;
        let alpha = DOT_BASE_ALPHA * freshness;
        blend_dot_2x2(pixmap, px, py, color, alpha);
    }
}

/// Compute the inscribed half-disc geometry for a given panel rect.
/// Returns `(cx, base_y, disc_radius)`. The half-disc is the upper hemisphere
/// of a circle whose flat baseline sits at `base_y` and whose center is at
/// `cx`. Radius is bounded by panel width / 2 and panel height (with margin).
fn half_disc_geometry(
    panel_x: i32,
    panel_y: i32,
    panel_w: i32,
    panel_h: i32,
    s: f32,
) -> (i32, i32, i32) {
    let margin = scaled(8, s);
    let max_r_x = panel_w / 2 - margin;
    let max_r_y = panel_h - margin * 2;
    let disc_radius = max_r_x.min(max_r_y).max(1);
    let cx = panel_x + panel_w / 2;
    // Anchor the baseline near the bottom of the panel, leaving a small
    // margin so the L/R labels (drawn just below the baseline) have room.
    let base_y = panel_y + panel_h - margin;
    (cx, base_y, disc_radius)
}

/// Ozone-style half-disc polar scope.
/// Origin at bottom-center. Mono → straight up (θ=π/2), hard-L in-phase →
/// upper-left 45° spoke (θ=3π/4), hard-R in-phase → upper-right 45° spoke
/// (θ=π/4), anti-phase L-dom → left baseline corner (θ=π), anti-phase R-dom →
/// right baseline corner (θ=0).
#[allow(clippy::too_many_arguments)]
fn draw_half_polar(
    pixmap: &mut PixmapMut<'_>,
    panel_x: i32,
    panel_y: i32,
    panel_w: i32,
    panel_h: i32,
    samples_l: &[f32],
    samples_r: &[f32],
    color: Color,
    scale: f32,
) {
    let (cx, base_y, disc_radius) = half_disc_geometry(panel_x, panel_y, panel_w, panel_h, scale);

    // Faint half-circle outline + spokes + baseline.
    draw_half_disc_grid(pixmap, cx, base_y, disc_radius, theme::text_dim());

    // Plot each sample as a 1×1 dot. Geometry:
    //
    //   M = (L+R)/2, S = (L-R)/2,  sgn = sign(M)  (sign(0) := +1)
    //   x_disc = -sgn · S,  y_disc = |M|
    //   θ = atan2(y_disc, x_disc) ∈ [0, π]
    //   r = (|L| + |R|), clamped to 1
    //
    // The radial distance is the L1 stereo norm so a mono sample at peak
    // amplitude `a` reaches `2a` on the disc (clamped at the rim) and a
    // hard-panned sample at peak `a` reaches `a`. This matches Ozone's
    // dot spread on real material — using `max(|L|, |R|)` produces a
    // visibly tighter cluster (~⅓ the radial extent) for stereo content.
    //
    // Mappings (angle):
    //   - mono in-phase            → θ = π/2 (top)
    //   - hard-L in-phase (1, 0)   → θ = 3π/4 (upper-left 45° spoke)
    //   - hard-R in-phase (0, 1)   → θ = π/4 (upper-right 45° spoke)
    //   - anti-phase L-dom (1,-1)  → θ = π (left baseline corner)
    //   - anti-phase R-dom (-1, 1) → θ = 0 (right baseline corner)
    //
    // The mapping is polarity-invariant: (L,R) and (-L,-R) land on the
    // same disc point, which folds anti-phase samples onto the baseline
    // corners directly without a `dy < 0` skip.
    let r_f = disc_radius as f32;
    let n = samples_l.len().min(samples_r.len());
    if n == 0 {
        return;
    }
    // Per-sample age fade: samples are chronological (oldest first, newest
    // last in the snapshot). Newest sample gets full DOT_BASE_ALPHA; oldest
    // fades to zero. Source-over blending means dense pixel regions
    // accumulate toward the cyan target while sparse single hits stay
    // visibly dim — Ozone's phosphor-decay look without a persistent
    // pixmap.
    let inv_n_minus_1 = if n > 1 { 1.0 / (n - 1) as f32 } else { 0.0 };
    for i in 0..n {
        let l = samples_l[i].clamp(-1.0, 1.0);
        let rr = samples_r[i].clamp(-1.0, 1.0);
        let amplitude = (l.abs() + rr.abs()).min(1.0);
        if amplitude < 1e-6 {
            continue;
        }
        let m = 0.5 * (l + rr);
        let s = 0.5 * (l - rr);
        let sgn_m = if m >= 0.0 { 1.0 } else { -1.0 };
        let x_disc = -sgn_m * s;
        let y_disc = m.abs();
        let theta = y_disc.atan2(x_disc);
        let dx = theta.cos();
        let dy = theta.sin();
        let px = cx + (amplitude * dx * r_f) as i32;
        let py = base_y - (amplitude * dy * r_f) as i32;
        // Snapshot returns oldest at i=0, newest at i=n-1. Freshness
        // ramps 0 (oldest) → 1 (newest), so newest dots are bright and
        // oldest fade to background — matches Ozone's phosphor decay.
        let freshness = (i as f32) * inv_n_minus_1;
        let alpha = DOT_BASE_ALPHA * freshness;
        blend_dot_2x2(pixmap, px, py, color, alpha);
    }
}

/// Per-dot base opacity for the dot-cloud renderers (HalfPolar, Polar,
/// Lissajous). Low so individual dots are dim and dense pixel regions
/// build up toward full cyan via source-over blending — matches Ozone's
/// phosphor display where rare transients stay sparse and common
/// stereo positions accumulate to a bright cluster.
const DOT_BASE_ALPHA: f32 = 0.18;

/// Polar Level: per-pan-angle level "rays" on the half-disc. Each of the
/// `NUM_POLAR_BINS` pan bins is rendered as ONE independent radial line
/// from the origin to a length proportional to that bin's level. The
/// iZotope manual is explicit: "The length of the rays represents
/// amplitude. The angle of the rays represents their position in the
/// stereo image." Densely-packed long rays in the centre visually merge
/// into a wedge; sparse out-of-phase content shows as individual rays
/// near the L/R baseline. The angular space between adjacent rays is
/// NOT filled — that fill is precisely what produced the "blob"
/// regression in the polygon-based renderer.
#[allow(clippy::too_many_arguments)]
fn draw_polar_level(
    pixmap: &mut PixmapMut<'_>,
    panel_x: i32,
    panel_y: i32,
    panel_w: i32,
    panel_h: i32,
    params: &Arc<ImagineParams>,
    _scale: f32,
) {
    let (cx, base_y, disc_radius) =
        half_disc_geometry(panel_x, panel_y, panel_w, panel_h, _scale);
    if disc_radius <= 0 {
        return;
    }

    // Faint half-circle outline + spokes + baseline. Same grid as the
    // dot-cloud HalfPolar mode so the user can switch between them with no
    // visual reframing.
    draw_half_disc_grid(pixmap, cx, base_y, disc_radius, theme::text_dim());

    let r_f = disc_radius as f32;
    let consumer = &params.polar_consumer;
    let mut rays = [Ray {
        angle: 0.0,
        amp: 0.0,
        age_normalised: 0.0,
    }; POLAR_RING_CAPACITY];
    let n = consumer.snapshot(&mut rays);
    if n == 0 {
        return;
    }
    let cyan = theme::cyan();
    let bg = theme::panel_bg();
    let cx_f = cx as f32;
    let base_yf = base_y as f32;
    // Per-ray triangular fan: each ray spans ±POLAR_RAY_HALF_WIDTH_RAD from
    // its emit angle, tapered to a point at origin and broadening to a
    // visible width at the ray's endpoint. Renders via tiny-skia
    // anti-aliased fill so the fan reads as a "lobe" rather than a 1px
    // line — matches Ozone's visible ray thickness.
    for ray in rays.iter().take(n) {
        if ray.amp <= POLAR_LEVEL_GATE {
            continue;
        }
        let alpha = (1.0 - ray.age_normalised).clamp(0.0, 1.0);
        let color = lerp_color(bg, cyan, alpha);
        let r = ray.amp.clamp(0.0, 1.0) * r_f;
        let theta = ray.angle;
        let theta_l = theta + POLAR_RAY_HALF_WIDTH_RAD;
        let theta_r = theta - POLAR_RAY_HALF_WIDTH_RAD;
        let p1 = (cx_f + theta_l.cos() * r, base_yf - theta_l.sin() * r);
        let p2 = (cx_f + theta_r.cos() * r, base_yf - theta_r.sin() * r);
        let mut pb = PathBuilder::new();
        pb.move_to(cx_f, base_yf);
        pb.line_to(p1.0, p1.1);
        pb.line_to(p2.0, p2.1);
        pb.close();
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(color);
            paint.anti_alias = true;
            pixmap.fill_path(
                &path,
                &paint,
                tiny_skia::FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

/// Half-width of each polar-level ray's triangular fan, in radians.
/// 1.5° per side → 3° total fan, narrow enough that adjacent rays at
/// distinct emit angles read as separate lobes, wide enough that a
/// single ray is visible as a thick triangle rather than a 1 px line.
const POLAR_RAY_HALF_WIDTH_RAD: f32 = 0.026; // ≈ 1.5°

/// Threshold below which a ray is not drawn at all. Filters out the
/// silent-emit sentinel (amp = 0) plus any near-zero amplitude noise.
const POLAR_LEVEL_GATE: f32 = 0.005;

/// Linear interpolation between two RGB colors. Used to pre-mix faded
/// rays toward the background so the direct-pixel-write `draw_line`
/// helper can paint them without any alpha-blending pass.
fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let r = a.red() * (1.0 - t) + b.red() * t;
    let g = a.green() * (1.0 - t) + b.green() * t;
    let b_ = a.blue() * (1.0 - t) + b.blue() * t;
    Color::from_rgba(r, g, b_, 1.0).unwrap_or(b)
}

/// Faint half-circle outline + 3 angular spokes + baseline at the bottom.
/// Uses a midpoint-circle algorithm for the arc (direct pixel writes —
/// matches the existing dot-cloud rendering style). The spokes are drawn at
/// 3π/4, π/2, and π/4 (upper-left 45°, vertical, upper-right 45°) and use a
/// pre-mixed dimmed color so they don't visually divide the dot cloud.
fn draw_half_disc_grid(
    pixmap: &mut PixmapMut<'_>,
    cx: i32,
    base_y: i32,
    radius: i32,
    color: Color,
) {
    if radius <= 0 {
        return;
    }
    // Baseline (horizontal at base_y).
    fill_rect_i(pixmap, cx - radius, base_y, 2 * radius + 1, 1, color);

    // Half-circle outline (upper half only). Midpoint circle algorithm,
    // emitting only the four octants whose Y is above (or on) the baseline.
    let mut x = radius;
    let mut y = 0;
    let mut err = 1 - x;
    while x >= y {
        // Upper half: use base_y - y and base_y - x (since screen-y grows down).
        // Right half (octants 1,2):
        put_pixel(pixmap, cx + x, base_y - y, color);
        put_pixel(pixmap, cx + y, base_y - x, color);
        // Left half (octants 3,4):
        put_pixel(pixmap, cx - x, base_y - y, color);
        put_pixel(pixmap, cx - y, base_y - x, color);

        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }

    // Three internal spokes at 3π/4 (upper-left 45°), π/2 (vertical), and
    // π/4 (upper-right 45°). Drawn with a dimmed pre-mixed color so they
    // recede behind the dot cloud rather than slicing it.
    let spoke_color = dimmed_spoke_color();
    let r_f = radius as f32;
    let angles = [
        3.0 * std::f32::consts::FRAC_PI_4,
        std::f32::consts::FRAC_PI_2,
        std::f32::consts::FRAC_PI_4,
    ];
    for angle in angles {
        let ex = cx + (r_f * angle.cos()) as i32;
        let ey = base_y - (r_f * angle.sin()) as i32;
        draw_line(pixmap, cx, base_y, ex, ey, spoke_color);
    }
}

/// Pre-mix the dim text color with the panel background at 30% opacity. The
/// `put_pixel` / Bresenham renderer writes pixels directly without
/// source-over blending, so we have to do the alpha compositing ourselves and
/// hand the result back as an opaque color.
fn dimmed_spoke_color() -> Color {
    let bg = theme::panel_bg();
    let fg = theme::text_dim();
    let a = 0.30_f32;
    Color::from_rgba(
        bg.red() * (1.0 - a) + fg.red() * a,
        bg.green() * (1.0 - a) + fg.green() * a,
        bg.blue() * (1.0 - a) + fg.blue() * a,
        1.0,
    )
    .unwrap_or_else(theme::text_dim)
}

/// Bresenham line for the spokes. Direct pixel writes, no anti-aliasing —
/// consistent with the rest of this module.
fn draw_line(pixmap: &mut PixmapMut<'_>, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;
    loop {
        put_pixel(pixmap, x, y, color);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Horizontal bar with a marker at `value` ∈ [-1, 1].
fn draw_meter_bar(
    pixmap: &mut PixmapMut<'_>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    value: f32,
    color: Color,
) {
    fill_rect_i(pixmap, x, y, w, h, theme::panel_bg());
    stroke_rect_i(pixmap, x, y, w, h, theme::border());
    let cx = x + w / 2;
    fill_rect_i(pixmap, cx, y + 1, 1, h - 2, theme::text_dim());
    let v = value.clamp(-1.0, 1.0);
    let marker_x = x + ((v + 1.0) * 0.5 * w as f32) as i32;
    fill_rect_i(pixmap, marker_x - 1, y + 1, 3, h - 2, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vectorscope::ring_pair;

    #[test]
    fn vector_mode_from_u32() {
        assert_eq!(VectorMode::from_u32(0), VectorMode::HalfPolar);
        assert_eq!(VectorMode::from_u32(1), VectorMode::Polar);
        assert_eq!(VectorMode::from_u32(2), VectorMode::PolarLevel);
        assert_eq!(VectorMode::from_u32(3), VectorMode::Lissajous);
        assert_eq!(VectorMode::from_u32(99), VectorMode::HalfPolar);
    }

    #[test]
    fn vector_mode_default_is_half_polar() {
        // ImagineParams initializes vector_mode to AtomicU32::new(0) and
        // HalfPolar must be the 0-discriminant so the default is half-polar.
        let params = ImagineParams::default();
        assert_eq!(
            VectorMode::from_u32(params.vector_mode.load(Ordering::Relaxed)),
            VectorMode::HalfPolar,
        );
    }

    #[test]
    fn vector_mode_next_cycles() {
        assert_eq!(VectorMode::HalfPolar.next(), VectorMode::Polar);
        assert_eq!(VectorMode::Polar.next(), VectorMode::PolarLevel);
        assert_eq!(VectorMode::PolarLevel.next(), VectorMode::Lissajous);
        assert_eq!(VectorMode::Lissajous.next(), VectorMode::HalfPolar);
    }

    #[test]
    fn vector_mode_labels() {
        assert_eq!(VectorMode::HalfPolar.label(), "Polar");
        assert_eq!(VectorMode::Polar.label(), "Goniometer");
        assert_eq!(VectorMode::Lissajous.label(), "Lissajous");
        assert_eq!(VectorMode::PolarLevel.label(), "Polar Level");
    }

    #[test]
    fn vector_mode_as_u32_roundtrip() {
        for m in [
            VectorMode::HalfPolar,
            VectorMode::Polar,
            VectorMode::Lissajous,
            VectorMode::PolarLevel,
        ] {
            assert_eq!(VectorMode::from_u32(m.as_u32()), m);
        }
    }

    #[test]
    fn render_half_polar_with_data() {
        let params = Arc::new(ImagineParams::default());
        params
            .vector_mode
            .store(VectorMode::HalfPolar.as_u32(), Ordering::Relaxed);
        let (prod, cons) = ring_pair();
        let cons = Arc::new(cons);
        // Push a mix of mono / hard-left / hard-right / anti-phase samples.
        for i in 0..512 {
            let t = i as f32 * 0.05;
            prod.push(t.sin() * 0.5, t.sin() * 0.5); // mono
            prod.push(0.5, 0.0); // hard-left
            prod.push(0.0, 0.5); // hard-right
            prod.push(0.5, -0.5); // anti-phase
        }
        let mut pixmap = tiny_skia::Pixmap::new(300, 400).unwrap();
        let mut vl = vec![0.0; SAMPLE_BUDGET];
        let mut vr = vec![0.0; SAMPLE_BUDGET];
        let font_data = include_bytes!("../fonts/DejaVuSans.ttf");
        let mut tr = TextRenderer::new(font_data);
        draw(
            &mut pixmap,
            0,
            0,
            300,
            400,
            &params,
            &cons,
            &mut vl,
            &mut vr,
            &mut tr,
            1.0,
        );
    }

    #[test]
    fn render_with_empty_ring() {
        let params = Arc::new(ImagineParams::default());
        let (_, cons) = ring_pair();
        let cons = Arc::new(cons);
        let mut pixmap = tiny_skia::Pixmap::new(300, 400).unwrap();
        let mut vl = vec![0.0; SAMPLE_BUDGET];
        let mut vr = vec![0.0; SAMPLE_BUDGET];
        let font_data = include_bytes!("../fonts/DejaVuSans.ttf");
        let mut tr = TextRenderer::new(font_data);
        draw(
            &mut pixmap,
            0,
            0,
            300,
            400,
            &params,
            &cons,
            &mut vl,
            &mut vr,
            &mut tr,
            1.0,
        );
    }

    #[test]
    fn render_with_data() {
        let params = Arc::new(ImagineParams::default());
        let (prod, cons) = ring_pair();
        let cons = Arc::new(cons);
        for i in 0..1000 {
            let phase = (i as f32 * 0.1).sin();
            prod.push(phase * 0.5, phase * 0.6);
        }
        let mut pixmap = tiny_skia::Pixmap::new(300, 400).unwrap();
        let mut vl = vec![0.0; SAMPLE_BUDGET];
        let mut vr = vec![0.0; SAMPLE_BUDGET];
        let font_data = include_bytes!("../fonts/DejaVuSans.ttf");
        let mut tr = TextRenderer::new(font_data);
        draw(
            &mut pixmap,
            0,
            0,
            300,
            400,
            &params,
            &cons,
            &mut vl,
            &mut vr,
            &mut tr,
            1.0,
        );
    }

    /// Locate the brightest cyan pixel in the pixmap. Returns disc-local
    /// (dx, dy) where dx is signed (− = left of cx, + = right) and dy is
    /// positive going up from baseline.
    ///
    /// Cyan = RGB(96, 200, 228); bg/grid are gray (R≈G≈B in the 14..134
    /// range). The signature "B > G + 20" filters cyan pixels from
    /// baseline/spoke pixels (which are grayish, R≈G≈B).
    fn find_dot(pixmap: &tiny_skia::Pixmap, cx: i32, base_y: i32) -> Option<(i32, i32)> {
        let w = pixmap.width() as i32;
        let h = pixmap.height() as i32;
        let pixels = pixmap.pixels();
        for y in 0..h {
            for x in 0..w {
                let p = pixels[(y as usize) * (w as usize) + (x as usize)];
                let b = p.blue() as i32;
                let g = p.green() as i32;
                let r = p.red() as i32;
                // Cyan: B clearly > G > R (blue-green leaning).
                if b > 200 && g > 150 && b > g + 20 && g > r + 50 {
                    return Some((x - cx, base_y - y));
                }
            }
        }
        None
    }

    /// Helper: render `draw_half_polar` into a fresh pixmap with constant
    /// (l, r) samples and return (pixmap, cx, base_y, disc_radius).
    fn render_half_polar_constant(
        l: f32,
        r: f32,
        n_samples: usize,
    ) -> (tiny_skia::Pixmap, i32, i32, i32) {
        let mut pixmap = tiny_skia::Pixmap::new(300, 300).unwrap();
        let panel_x = 0;
        let panel_y = 0;
        let panel_w = 300;
        let panel_h = 300;
        let scale = 1.0;
        let (cx, base_y, disc_radius) =
            half_disc_geometry(panel_x, panel_y, panel_w, panel_h, scale);
        let samples_l = vec![l; n_samples];
        let samples_r = vec![r; n_samples];
        {
            let mut pm = pixmap.as_mut();
            // Pre-fill panel bg so spoke colour isn't confused with cyan dots.
            fill_rect_i(&mut pm, panel_x, panel_y, panel_w, panel_h, theme::panel_bg());
            draw_half_polar(
                &mut pm,
                panel_x,
                panel_y,
                panel_w,
                panel_h,
                &samples_l,
                &samples_r,
                theme::cyan(),
                scale,
            );
        }
        (pixmap, cx, base_y, disc_radius)
    }

    /// Mono in-phase (L=R=0.4) lands on the central vertical column.
    /// θ = π/2 → (cos, sin) = (0, 1) → dx = 0, dy = +radius · (|L|+|R|).
    /// Uses 0.4 so the L1 amplitude (0.8) doesn't clamp at the rim.
    #[test]
    fn half_polar_mono_dot_lands_on_vertical_axis() {
        let (pixmap, cx, base_y, disc_radius) = render_half_polar_constant(0.4, 0.4, 256);
        let (dx, dy) = find_dot(&pixmap, cx, base_y).expect("expected a cyan dot");
        assert!(
            dx.abs() <= 1,
            "mono dot should sit on cx (dx≈0), got dx={dx}"
        );
        assert!(dy > 0, "mono dot should be above baseline, got dy={dy}");
        let expected = (0.8 * disc_radius as f32) as i32;
        assert!(
            (dy - expected).abs() <= 2,
            "mono dot dy={dy}, expected ~{expected} ((|L|+|R|)·radius)"
        );
    }

    /// Hard-L in-phase (L=1.0, R=0.0) lands on the upper-left 45° spoke.
    /// θ = 3π/4 → (cos, sin) = (-√2/2, +√2/2) → dx ≈ -r/√2, dy ≈ +r/√2.
    #[test]
    fn half_polar_hard_l_in_phase_lands_on_upper_left_spoke() {
        let (pixmap, cx, base_y, disc_radius) = render_half_polar_constant(1.0, 0.0, 256);
        let (dx, dy) = find_dot(&pixmap, cx, base_y).expect("expected a cyan dot");
        let expected = (disc_radius as f32 * std::f32::consts::FRAC_1_SQRT_2) as i32;
        assert!(
            dx < -2,
            "hard-L in-phase dot should be left of cx, got dx={dx}"
        );
        assert!(
            dy > 2,
            "hard-L in-phase dot should be above baseline, got dy={dy}"
        );
        assert!(
            (dx + expected).abs() <= 3 && (dy - expected).abs() <= 3,
            "hard-L in-phase dot at (dx={dx}, dy={dy}), expected near (-{expected}, +{expected})"
        );
    }

    /// Hard-R in-phase (L=0.0, R=1.0) lands on the upper-right 45° spoke.
    /// θ = π/4 → (cos, sin) = (+√2/2, +√2/2) → dx ≈ +r/√2, dy ≈ +r/√2.
    #[test]
    fn half_polar_hard_r_in_phase_lands_on_upper_right_spoke() {
        let (pixmap, cx, base_y, disc_radius) = render_half_polar_constant(0.0, 1.0, 256);
        let (dx, dy) = find_dot(&pixmap, cx, base_y).expect("expected a cyan dot");
        let expected = (disc_radius as f32 * std::f32::consts::FRAC_1_SQRT_2) as i32;
        assert!(
            dx > 2,
            "hard-R in-phase dot should be right of cx, got dx={dx}"
        );
        assert!(
            dy > 2,
            "hard-R in-phase dot should be above baseline, got dy={dy}"
        );
        assert!(
            (dx - expected).abs() <= 3 && (dy - expected).abs() <= 3,
            "hard-R in-phase dot at (dx={dx}, dy={dy}), expected near (+{expected}, +{expected})"
        );
    }

    /// Anti-phase L-dom (L=+0.35, R=-0.35) lands at the left baseline corner.
    /// M=0 (sgn=+1), S=+0.35 → x_disc=-0.35, y_disc=0 → θ=π → dx=-r·(|L|+|R|), dy=0.
    /// Uses 0.35 so the L1 amplitude (0.7) doesn't clamp at the rim.
    #[test]
    fn half_polar_anti_phase_l_dom_lands_at_left_baseline_corner() {
        let (pixmap, cx, base_y, disc_radius) = render_half_polar_constant(0.35, -0.35, 256);
        let (dx, dy) = find_dot(&pixmap, cx, base_y).expect("expected a cyan dot");
        let expected = (0.7 * disc_radius as f32) as i32;
        assert!(
            dx < -2,
            "anti-phase L-dom dot should be left of cx, got dx={dx}"
        );
        assert!(
            dy.abs() <= 2,
            "anti-phase L-dom dot should sit on baseline (dy≈0), got dy={dy}"
        );
        assert!(
            (dx + expected).abs() <= 3,
            "anti-phase L-dom dot at dx={dx}, expected near -{expected} (left baseline corner)"
        );
    }

    /// Anti-phase R-dom (L=-0.35, R=+0.35) lands at the right baseline corner.
    /// M=0 (sgn=+1), S=-0.35 → x_disc=+0.35, y_disc=0 → θ=0 → dx=+r·(|L|+|R|), dy=0.
    /// Uses 0.35 so the L1 amplitude (0.7) doesn't clamp at the rim.
    #[test]
    fn half_polar_anti_phase_r_dom_lands_at_right_baseline_corner() {
        let (pixmap, cx, base_y, disc_radius) = render_half_polar_constant(-0.35, 0.35, 256);
        let (dx, dy) = find_dot(&pixmap, cx, base_y).expect("expected a cyan dot");
        let expected = (0.7 * disc_radius as f32) as i32;
        assert!(
            dx > 2,
            "anti-phase R-dom dot should be right of cx, got dx={dx}"
        );
        assert!(
            dy.abs() <= 2,
            "anti-phase R-dom dot should sit on baseline (dy≈0), got dy={dy}"
        );
        assert!(
            (dx - expected).abs() <= 3,
            "anti-phase R-dom dot at dx={dx}, expected near +{expected} (right baseline corner)"
        );
    }

    /// Polarity-invariance: (L, R) and (-L, -R) must map to the same disc
    /// pixel — audio is real-valued, a polarity flip on both channels is
    /// the same acoustic signal. This is the property that lets
    /// anti-phase samples populate the corners directly without the
    /// previous `dy < 0` skip.
    #[test]
    fn half_polar_polarity_invariance() {
        let (pix_a, cx_a, base_y_a, _) = render_half_polar_constant(1.0, 0.0, 256);
        let (pix_b, cx_b, base_y_b, _) = render_half_polar_constant(-1.0, 0.0, 256);
        let a = find_dot(&pix_a, cx_a, base_y_a).expect("(+1, 0) dot");
        let b = find_dot(&pix_b, cx_b, base_y_b).expect("(-1, 0) dot");
        assert_eq!(
            a, b,
            "(L,R) and (-L,-R) must map to the same disc point, got {a:?} vs {b:?}"
        );
    }
}
