//! Cassiopeia A duo-tone palette for Imagine.
//!
//! Inspired by Hubble false-colour imagery of Cassiopeia A: deep
//! teal-cyan jets through warm gold dust on a near-black space
//! background.
//!
//! Channel mapping (variable names retained from the previous
//! pink/cyan palette to avoid churning every call site):
//!   `pink()`  = L-dominant / decorrelated  → warm gold
//!   `cyan()`  = R-dominant / coherent      → deep teal-cyan
//!   `accent()` ≈ `pink()` for active controls

use tiny_skia::Color;

// tiny-skia's Color::from_rgba8 is not const, so we use inline functions.

#[inline]
pub fn bg() -> Color {
    Color::from_rgba8(6, 18, 26, 255)
}

#[inline]
pub fn panel_bg() -> Color {
    Color::from_rgba8(14, 28, 36, 255)
}

#[inline]
pub fn border() -> Color {
    Color::from_rgba8(30, 56, 72, 255)
}

#[inline]
pub fn text() -> Color {
    Color::from_rgba8(212, 208, 196, 255)
}

#[inline]
pub fn text_dim() -> Color {
    Color::from_rgba8(112, 128, 152, 255)
}

#[inline]
pub fn accent() -> Color {
    Color::from_rgba8(232, 184, 80, 255)
}

#[inline]
pub fn pink() -> Color {
    Color::from_rgba8(240, 192, 96, 255)
}

#[inline]
pub fn cyan() -> Color {
    Color::from_rgba8(32, 168, 200, 255)
}

/// Saturation/clip warning. Used by the dot-cloud vectorscope modes
/// (HalfPolar, Polar/Goniometer, Lissajous) to flag samples that
/// exceed 0 dBFS without colliding with the in-range gold/teal
/// palette.
#[inline]
pub fn warn() -> Color {
    Color::from_rgba8(255, 112, 80, 255)
}

/// Foreground colour for text drawn on top of `accent()` (or any
/// similarly-bright fill like `cyan_to_pink(0.5)`). The default `text()`
/// cream has too little contrast against the warm gold accent —
/// active toggle/segment labels use this near-black tint instead.
#[inline]
pub fn on_accent() -> Color {
    Color::from_rgba8(8, 16, 22, 255)
}

#[inline]
pub fn split_line() -> Color {
    Color::from_rgba8(160, 168, 176, 200)
}

#[inline]
pub fn spectrum_bg() -> Color {
    Color::from_rgba8(20, 36, 52, 255)
}

/// 0.0 = fully cyan (teal), 1.0 = fully pink (gold). Used for coherence
/// display (low coherence = decorrelated/wide = gold; high coherence =
/// coherent = teal).
pub fn cyan_to_pink(t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let cyan = (32.0, 168.0, 200.0);
    let pink = (240.0, 192.0, 96.0);
    Color::from_rgba8(
        ((1.0 - t) * cyan.0 + t * pink.0) as u8,
        ((1.0 - t) * cyan.1 + t * pink.1) as u8,
        ((1.0 - t) * cyan.2 + t * pink.2) as u8,
        255,
    )
}

/// Pre-mix a cyan_to_pink color with the BG into an opaque u32 (RGBA).
/// Used by direct-pixel-write fast paths.
pub fn cyan_to_pink_u32(t: f32) -> u32 {
    let c = cyan_to_pink(t);
    let r = (c.red() * 255.0) as u32;
    let g = (c.green() * 255.0) as u32;
    let b = (c.blue() * 255.0) as u32;
    (r << 16) | (g << 8) | b | 0xff000000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        let cyan = cyan_to_pink(0.0);
        let pink = cyan_to_pink(1.0);
        assert_eq!(
            cyan.to_color_u8(),
            tiny_skia::ColorU8::from_rgba(32, 168, 200, 255)
        );
        assert_eq!(
            pink.to_color_u8(),
            tiny_skia::ColorU8::from_rgba(240, 192, 96, 255)
        );
    }

    #[test]
    fn midpoint_blends() {
        // Midpoint of (32, 168, 200) and (240, 192, 96).
        let mid = cyan_to_pink(0.5);
        let u = mid.to_color_u8();
        assert!((u.red() as i32 - 136).abs() <= 2);
        assert!((u.green() as i32 - 180).abs() <= 2);
        assert!((u.blue() as i32 - 148).abs() <= 2);
    }

    #[test]
    fn clamp_out_of_range() {
        let lo = cyan_to_pink(-1.0);
        let hi = cyan_to_pink(2.0);
        assert_eq!(lo.to_color_u8(), cyan_to_pink(0.0).to_color_u8());
        assert_eq!(hi.to_color_u8(), cyan_to_pink(1.0).to_color_u8());
    }
}
