//! Vectorscope view (polar + Lissajous). Filled in by Task 14.

use crate::vectorscope::VectorConsumer;
use crate::ImagineParams;
use std::sync::Arc;
use tiny_skia::PixmapMut;

#[allow(clippy::too_many_arguments)]
pub fn draw(
    _pixmap: &mut PixmapMut<'_>,
    _x: i32,
    _y: i32,
    _w: i32,
    _h: i32,
    _params: &Arc<ImagineParams>,
    _vec: &Arc<VectorConsumer>,
    _vec_l: &mut Vec<f32>,
    _vec_r: &mut Vec<f32>,
) {
}
