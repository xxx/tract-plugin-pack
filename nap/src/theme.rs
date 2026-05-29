//! Per-pane accent colours for the three stacked curve editors.
//!
//! Each curve gets a distinct hue so the Decay / Width / Tone panes read
//! apart at a glance. Values are warm-to-cool, top-to-bottom.

use tiny_skia::Color;

/// Decay pane accent — warm amber (loudness/energy).
pub fn color_decay() -> Color {
    Color::from_rgba8(0xff, 0xa6, 0x3a, 0xff)
}

/// Width pane accent — teal (stereo spread).
pub fn color_width() -> Color {
    Color::from_rgba8(0x4a, 0xd6, 0xc8, 0xff)
}

/// Tone pane accent — violet (brightness/colour).
pub fn color_tone() -> Color {
    Color::from_rgba8(0xb4, 0x8a, 0xff, 0xff)
}

/// The three pane accents, indexed by pane (0 = Decay, 1 = Width, 2 = Tone).
pub fn pane_colors() -> [Color; 3] {
    [color_decay(), color_width(), color_tone()]
}

/// Accent for the bottom-strip dials (value arc + modulation arc). A warm
/// amber in the Decay family; defined here so the strip's accent is
/// single-sourced rather than a magic literal repeated in the editor.
pub fn color_strip_accent() -> Color {
    Color::from_rgba8(255, 160, 50, 255)
}
