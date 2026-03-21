pub mod param_dial;

pub use param_dial::ParamDial;

use nih_plug_vizia::vizia::prelude::*;

/// Load the shared dark theme CSS into a vizia context.
/// Uses `include_str!` for compile-time embedding — no runtime file access needed.
pub fn load_style(cx: &mut Context) {
    cx.add_stylesheet(include_str!("style.css"))
        .expect("Failed to load nih-plug-widgets stylesheet");
}
