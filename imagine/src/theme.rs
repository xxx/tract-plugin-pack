//! Pink/cyan duo-tone palette for Imagine.
//! Pink = L-dominant / decorrelated. Cyan = R-dominant / coherent.

use tiny_skia::Color;

// tiny-skia's Color::from_rgba8 is not const, so we use inline functions.

#[inline]
pub fn bg() -> Color {
    Color::from_rgba8(8, 10, 16, 255)
}

#[inline]
pub fn panel_bg() -> Color {
    Color::from_rgba8(14, 18, 26, 255)
}

#[inline]
pub fn border() -> Color {
    Color::from_rgba8(36, 44, 60, 255)
}

#[inline]
pub fn text() -> Color {
    Color::from_rgba8(190, 198, 214, 255)
}

#[inline]
pub fn text_dim() -> Color {
    Color::from_rgba8(110, 118, 134, 255)
}

#[inline]
pub fn accent() -> Color {
    Color::from_rgba8(214, 100, 168, 255)
}

#[inline]
pub fn pink() -> Color {
    Color::from_rgba8(228, 96, 168, 255)
}

#[inline]
pub fn cyan() -> Color {
    Color::from_rgba8(96, 200, 228, 255)
}

#[inline]
pub fn split_line() -> Color {
    Color::from_rgba8(170, 130, 198, 200)
}

#[inline]
pub fn spectrum_bg() -> Color {
    Color::from_rgba8(20, 26, 38, 255)
}

/// 0.0 = fully cyan, 1.0 = fully pink. Used for coherence display
/// (low coherence = decorrelated/wide = pink; high coherence = coherent = cyan).
pub fn cyan_to_pink(t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let cyan = (96.0, 200.0, 228.0);
    let pink = (228.0, 96.0, 168.0);
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
            tiny_skia::ColorU8::from_rgba(96, 200, 228, 255)
        );
        assert_eq!(
            pink.to_color_u8(),
            tiny_skia::ColorU8::from_rgba(228, 96, 168, 255)
        );
    }

    #[test]
    fn midpoint_blends() {
        let mid = cyan_to_pink(0.5);
        let u = mid.to_color_u8();
        assert!((u.red() as i32 - 162).abs() <= 2);
        assert!((u.green() as i32 - 148).abs() <= 2);
        assert!((u.blue() as i32 - 198).abs() <= 2);
    }

    #[test]
    fn clamp_out_of_range() {
        let lo = cyan_to_pink(-1.0);
        let hi = cyan_to_pink(2.0);
        assert_eq!(lo.to_color_u8(), cyan_to_pink(0.0).to_color_u8());
        assert_eq!(hi.to_color_u8(), cyan_to_pink(1.0).to_color_u8());
    }
}
