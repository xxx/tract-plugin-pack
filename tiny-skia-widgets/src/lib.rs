//! Minimal widget primitives for rendering a plugin GUI using tiny-skia and fontdue.
//!
//! All drawing targets a [`tiny_skia::Pixmap`]. Coordinates are in physical pixels;
//! the caller is responsible for DPI scaling. No event handling lives here — only
//! pure drawing functions.

pub mod controls;
pub mod drag;
pub mod editor_base;
pub mod grid_selector;
pub mod param_dial;
pub mod primitives;
pub mod text;
pub mod text_edit;

#[cfg(test)]
mod test_font;

pub use controls::*;
pub use drag::*;
pub use editor_base::*;
pub use grid_selector::*;
pub use param_dial::*;
pub use primitives::*;
pub use text::*;
pub use text_edit::*;
