//! Amber phosphor terminal color palette.

#![allow(dead_code)]

/// ARGB color constants for the amber phosphor theme.
pub const BG: u32 = 0xFF0A_0600;
pub const FG: u32 = 0xFFFF_B833;
pub const PRIMARY_DIM: u32 = 0xFFAA_7700;
pub const GRID: u32 = 0xFF44_2E00;
pub const GRID_BRIGHT: u32 = 0xFF66_4400;
pub const BORDER: u32 = 0xFF1A_1400;
pub const BAR_LINE: u32 = 0xFFCC_6600;
pub const CYAN: u32 = 0xFF33_DDFF;
pub const ROSE: u32 = 0xFFFF_6699;
pub const YELLOW: u32 = 0xFFFF_DD33;
pub const RED: u32 = 0xFFFF_4444;
pub const PURPLE: u32 = 0xFFBB_66FF;
pub const ORANGE: u32 = 0xFFFF_9944;
pub const BLUE: u32 = 0xFF44_99FF;

/// 16-color channel palette indexed by slot number.
const CHANNEL_COLORS: [u32; 16] = [
    0xFFFF_B833, // 0: amber
    0xFF33_DDFF, // 1: cyan
    0xFFFF_6699, // 2: rose
    0xFFFF_DD33, // 3: yellow
    0xFFFF_9944, // 4: orange
    0xFFBB_66FF, // 5: purple
    0xFFFF_4444, // 6: red
    0xFF44_99FF, // 7: blue
    0xFFFF_D066, // 8: light amber
    0xFF66_EEFF, // 9: light cyan
    0xFFFF_99BB, // 10: light rose
    0xFFFF_EE66, // 11: light yellow
    0xFFFF_BB77, // 12: light orange
    0xFFCC_88FF, // 13: light purple
    0xFFFF_7777, // 14: light red
    0xFF77_BBFF, // 15: light blue
];

/// Get the channel color for a slot index (wraps at 16).
pub fn channel_color(slot: usize) -> u32 {
    CHANNEL_COLORS[slot % 16]
}

/// Convert an ARGB u32 to tiny-skia Color.
pub fn to_color(argb: u32) -> tiny_skia::Color {
    let a = ((argb >> 24) & 0xFF) as f32 / 255.0;
    let r = ((argb >> 16) & 0xFF) as f32 / 255.0;
    let g = ((argb >> 8) & 0xFF) as f32 / 255.0;
    let b = (argb & 0xFF) as f32 / 255.0;
    tiny_skia::Color::from_rgba(r, g, b, a).unwrap()
}

/// Convert an ARGB u32 to a tiny-skia Color with overridden alpha (0.0-1.0).
pub fn to_color_alpha(argb: u32, alpha: f32) -> tiny_skia::Color {
    let r = ((argb >> 16) & 0xFF) as f32 / 255.0;
    let g = ((argb >> 8) & 0xFF) as f32 / 255.0;
    let b = (argb & 0xFF) as f32 / 255.0;
    tiny_skia::Color::from_rgba(r, g, b, alpha).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_color_in_range() {
        for i in 0..16 {
            assert_eq!(channel_color(i), CHANNEL_COLORS[i]);
        }
    }

    #[test]
    fn test_channel_color_wraps() {
        assert_eq!(channel_color(16), channel_color(0));
        assert_eq!(channel_color(17), channel_color(1));
    }

    #[test]
    fn test_to_color_bg() {
        let c = to_color(BG);
        assert!((c.red() - 10.0 / 255.0).abs() < 0.01);
        assert!((c.green() - 6.0 / 255.0).abs() < 0.01);
        assert!((c.blue() - 0.0 / 255.0).abs() < 0.01);
        assert!((c.alpha() - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_to_color_alpha() {
        let c = to_color_alpha(FG, 0.5);
        assert!((c.alpha() - 0.5).abs() < 0.01);
        assert!((c.red() - 1.0).abs() < 0.01); // 0xFF
    }

    #[test]
    fn test_all_channel_colors_are_opaque() {
        for i in 0..16 {
            let c = channel_color(i);
            assert_eq!(c >> 24, 0xFF, "channel color {i} must be fully opaque");
        }
    }
}
