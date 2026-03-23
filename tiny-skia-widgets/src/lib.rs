//! Minimal widget primitives for rendering a plugin GUI using tiny-skia and fontdue.
//!
//! All drawing targets a [`tiny_skia::Pixmap`]. Coordinates are in physical pixels;
//! the caller is responsible for DPI scaling. No event handling lives here — only
//! pure drawing functions.

pub mod primitives;
pub mod text;
pub mod controls;
pub mod param_dial;

pub use primitives::*;
pub use text::*;
pub use controls::*;
pub use param_dial::*;
