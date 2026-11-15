//! Signed audio ranges and pure backdrop drawing in envelope coordinates.

use super::{MsegLayout, phase_to_x, value_to_y};
use crate::primitives::{color_accent, draw_rect};
use tiny_skia::{Color, Pixmap};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioRange {
    pub min: f32,
    pub max: f32,
}

impl Default for AudioRange {
    fn default() -> Self {
        Self {
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
        }
    }
}

impl AudioRange {
    pub fn include(&mut self, sample: f32) {
        if sample.is_finite() {
            self.min = self.min.min(sample);
            self.max = self.max.max(sample);
        }
    }

    pub fn is_empty(self) -> bool {
        !self.min.is_finite() || !self.max.is_finite() || self.min > self.max
    }

    pub fn peak(self) -> f32 {
        if self.is_empty() {
            0.0
        } else {
            self.min.abs().max(self.max.abs())
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioBin {
    pub input: [AudioRange; 2],
    pub output: [AudioRange; 2],
}

impl AudioBin {
    pub fn include(&mut self, input: [f32; 2], output: [f32; 2]) {
        for channel in 0..2 {
            self.input[channel].include(input[channel]);
            self.output[channel].include(output[channel]);
        }
    }

    pub fn input_peak(self) -> f32 {
        self.input[0].peak().max(self.input[1].peak())
    }
    pub fn output_peak(self) -> f32 {
        self.output[0].peak().max(self.output[1].peak())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioDisplay {
    #[default]
    Magnitude,
    Combined,
    Stereo,
}

#[derive(Clone, Copy, Debug)]
pub struct AudioBackdropStyle {
    pub input: bool,
    pub output: bool,
    pub display: AudioDisplay,
    pub gain: f32,
    pub output_color: Color,
}

impl Default for AudioBackdropStyle {
    fn default() -> Self {
        Self {
            input: true,
            output: true,
            display: AudioDisplay::Magnitude,
            gain: 1.0,
            output_color: color_accent(),
        }
    }
}

struct Lane {
    top: f32,
    height: f32,
    raster_top: f32,
    raster_bottom: f32,
}
impl Lane {
    fn new(layout: &MsegLayout, display: AudioDisplay, channel: usize) -> Self {
        let (_, py, _, ph) = layout.plot;
        let divider = (py + ph * 0.5).floor();
        if display == AudioDisplay::Stereo {
            Self {
                top: py + channel as f32 * ph * 0.5,
                height: ph * 0.5,
                raster_top: if channel == 0 {
                    py.ceil()
                } else {
                    divider.max(py.ceil())
                },
                raster_bottom: if channel == 0 {
                    divider.min((py + ph).floor())
                } else {
                    (py + ph).floor()
                },
            }
        } else {
            Self {
                top: py,
                height: ph,
                raster_top: py.ceil(),
                raster_bottom: (py + ph).floor(),
            }
        }
    }
    fn stroke(&self, y: f32) -> Option<(f32, f32)> {
        if y < self.top || y > self.top + self.height || self.raster_bottom <= self.raster_top {
            return None;
        }
        let row = y.floor().clamp(self.raster_top, self.raster_bottom - 1.0);
        Some((row, row + 1.0))
    }
    fn range(&self, top: f32, bottom: f32) -> Option<(f32, f32)> {
        if top == bottom {
            return self.stroke(top);
        }
        let visible_top = top.max(self.top);
        let visible_bottom = bottom.min(self.top + self.height);
        if visible_bottom <= visible_top {
            return None;
        }
        let first = visible_top.floor().max(self.raster_top);
        let last = visible_bottom.ceil().min(self.raster_bottom);
        if last > first {
            Some((first, last))
        } else {
            self.stroke((visible_top + visible_bottom) * 0.5)
        }
    }
}

/// Paint magnitudes or signed channel ranges beneath the MSEG grid/curve.
pub fn draw_mseg_audio(
    pixmap: &mut Pixmap,
    layout: &MsegLayout,
    bins: &[Option<AudioBin>],
    style: AudioBackdropStyle,
) {
    let (px, py, pw, ph) = layout.plot;
    if pw <= 0.0 || ph <= 0.0 || (!style.input && !style.output) {
        return;
    }
    let gain = if style.gain.is_finite() && style.gain > 0.0 {
        style.gain
    } else {
        1.0
    };
    if style.display != AudioDisplay::Magnitude && bins.iter().any(Option::is_some) {
        let (lo, hi) = layout.viewport.value_range();
        for channel in 0..if style.display == AudioDisplay::Stereo {
            2
        } else {
            1
        } {
            let lane = Lane::new(layout, style.display, channel);
            let y = lane.top + (hi - 0.5) / (hi - lo) * lane.height;
            let (left, right) = (px.ceil(), (px + pw).floor());
            if let Some((top, bottom)) = lane.stroke(y)
                && right > left
            {
                draw_rect(
                    pixmap,
                    left,
                    top,
                    right - left,
                    bottom - top,
                    Color::from_rgba8(65, 69, 76, 160),
                );
            }
        }
    }
    let mut output_color = style.output_color;
    output_color.set_alpha(0.20);
    for (index, bin) in bins.iter().enumerate() {
        let Some(bin) = *bin else {
            continue;
        };
        let left = phase_to_x(layout, index as f32 / bins.len() as f32).max(px);
        let right = phase_to_x(layout, (index + 1) as f32 / bins.len() as f32).min(px + pw);
        if right <= left {
            continue;
        }
        for (show, ranges, color) in [
            (style.input, bin.input, Color::from_rgba8(180, 185, 195, 55)),
            (style.output, bin.output, output_color),
        ] {
            if !show {
                continue;
            }
            if style.display == AudioDisplay::Magnitude {
                let peak = ranges[0].peak().max(ranges[1].peak()) * gain;
                let top = value_to_y(layout, peak).max(py);
                let bottom = value_to_y(layout, 0.0).min(py + ph);
                if bottom > top {
                    draw_rect(pixmap, left, top, right - left, bottom - top, color);
                }
                continue;
            }
            for (channel, range) in ranges.iter().enumerate() {
                if range.is_empty() {
                    continue;
                }
                let lane = Lane::new(layout, style.display, channel);
                let (lo, hi) = layout.viewport.value_range();
                let to_y = |sample: f32| {
                    lane.top
                        + (hi - (0.5 + 0.5 * (sample * gain).clamp(-1.0, 1.0))) / (hi - lo)
                            * lane.height
                };
                let Some((top, bottom)) = lane.range(to_y(range.max), to_y(range.min)) else {
                    continue;
                };
                let mut color = color;
                if style.display == AudioDisplay::Combined && channel == 1 {
                    color.set_alpha(color.alpha() * 0.65);
                }
                let (left, right) = (
                    left.floor().max(px.ceil()),
                    right.ceil().min((px + pw).floor()),
                );
                if right > left {
                    draw_rect(pixmap, left, top, right - left, bottom - top, color);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_preserve_signed_dc_and_ignore_non_finite_samples() {
        let mut bin = AudioBin::default();
        bin.include([f32::NAN, f32::INFINITY], [f32::NEG_INFINITY, f32::NAN]);
        assert_eq!(bin, AudioBin::default());
        bin.include([0.8, -0.6], [-0.3, 0.2]);
        bin.include([0.4, -0.9], [-0.1, 0.7]);
        assert_eq!(
            bin.input,
            [
                AudioRange { min: 0.4, max: 0.8 },
                AudioRange {
                    min: -0.9,
                    max: -0.6
                }
            ]
        );
        assert_eq!(
            bin.output,
            [
                AudioRange {
                    min: -0.3,
                    max: -0.1
                },
                AudioRange { min: 0.2, max: 0.7 }
            ]
        );
        assert_eq!(bin.input_peak(), 0.9);
        assert_eq!(bin.output_peak(), 0.7);
    }

    fn layout() -> MsegLayout {
        let mut layout = super::super::mseg_layout((0.0, 0.0, 160.0, 140.0), true, 1.0);
        layout.plot = (10.0, 10.0, 120.0, 100.0);
        layout
    }

    #[test]
    fn combined_keeps_opposite_dc_channels_separate_and_visible() {
        let mut pm = Pixmap::new(160, 140).unwrap();
        let mut bin = AudioBin::default();
        bin.include([0.8, -0.8], [0.0; 2]);
        draw_mseg_audio(
            &mut pm,
            &layout(),
            &[Some(bin)],
            AudioBackdropStyle {
                output: false,
                display: AudioDisplay::Combined,
                ..AudioBackdropStyle::default()
            },
        );
        let alpha = |y: usize| pm.pixels()[y * 160 + 40].alpha();
        assert!((19..22).any(|y| alpha(y) > 0), "positive DC missing");
        assert!((99..102).any(|y| alpha(y) > 0), "negative DC missing");
        assert_eq!(alpha(35), 0, "channel union invented positive signal");
        assert_eq!(alpha(85), 0, "channel union invented negative signal");
    }

    #[test]
    fn stereo_zoom_pan_and_gain_keep_each_channel_inside_its_physical_lane() {
        for value in [(0.0, 1.0), (0.1, 0.7), (0.4, 0.6), (0.0, 0.3)] {
            for channel in 0..2 {
                let mut layout = layout();
                layout.viewport.value = value;
                layout.viewport.time = (0.2, 0.8);
                let style = AudioBackdropStyle {
                    output: false,
                    display: AudioDisplay::Stereo,
                    gain: 3.9810717,
                    ..AudioBackdropStyle::default()
                };
                let mut empty = Pixmap::new(160, 140).unwrap();
                let mut actual = Pixmap::new(160, 140).unwrap();
                let mut bin = AudioBin::default();
                bin.input[channel] = AudioRange {
                    min: -2.0,
                    max: 2.0,
                };
                draw_mseg_audio(&mut empty, &layout, &[Some(AudioBin::default())], style);
                draw_mseg_audio(&mut actual, &layout, &[Some(bin)], style);
                let mut changed = 0;
                for (index, (old, new)) in empty.pixels().iter().zip(actual.pixels()).enumerate() {
                    if old == new {
                        continue;
                    }
                    changed += 1;
                    let (x, y) = (index % 160, index / 160);
                    let lane = if channel == 0 { 10..60 } else { 60..110 };
                    assert!(
                        (10..130).contains(&x) && lane.contains(&y),
                        "channel {channel} escaped at ({x},{y}), viewport {value:?}"
                    );
                }
                assert!(changed > 100);
            }
        }
    }

    #[test]
    fn magnitude_uses_the_greater_absolute_channel_and_display_gain_only() {
        let mut bin = AudioBin::default();
        bin.include([0.1, -0.2], [0.0; 2]);
        for gain in [1.0, 3.9810717] {
            let mut pm = Pixmap::new(160, 140).unwrap();
            draw_mseg_audio(
                &mut pm,
                &layout(),
                &[Some(bin)],
                AudioBackdropStyle {
                    output: false,
                    gain,
                    ..AudioBackdropStyle::default()
                },
            );
            let first = pm
                .pixels()
                .iter()
                .enumerate()
                .filter(|(_, pixel)| pixel.alpha() > 0)
                .map(|(index, _)| index / 160)
                .min()
                .unwrap();
            assert!(first.abs_diff((110.0 - 20.0 * gain) as usize) <= 1);
            assert_eq!(pm.pixels()[110 * 160 + 40].alpha(), 0);
        }
        assert_eq!(
            bin.input,
            [
                AudioRange { min: 0.1, max: 0.1 },
                AudioRange {
                    min: -0.2,
                    max: -0.2
                }
            ]
        );
    }

    #[test]
    fn fractional_dividers_and_plot_edges_keep_boundary_dc_visible_inside_its_lane() {
        for plot in [
            (10.0, 10.0, 120.0, 99.0),
            (10.0, 10.625, 120.0, 100.875),
            (10.25, 10.625, 120.5, 100.875),
        ] {
            for display in [AudioDisplay::Stereo, AudioDisplay::Combined] {
                for channel in 0..2 {
                    for (dc, spread) in [
                        (-1.0, 0.0),
                        (0.0, 0.0),
                        (1.0, 0.0),
                        (-1.0, 0.0001),
                        (0.0, 0.0001),
                        (1.0, 0.0001),
                    ] {
                        let mut layout = layout();
                        layout.plot = plot;
                        let style = AudioBackdropStyle {
                            output: false,
                            display,
                            ..AudioBackdropStyle::default()
                        };
                        let mut empty = Pixmap::new(160, 140).unwrap();
                        let mut actual = empty.clone();
                        let mut bin = AudioBin::default();
                        bin.input[channel] = AudioRange {
                            min: dc - spread,
                            max: dc + spread,
                        };
                        draw_mseg_audio(&mut empty, &layout, &[Some(AudioBin::default())], style);
                        draw_mseg_audio(&mut actual, &layout, &[Some(bin)], style);
                        let lane = Lane::new(&layout, display, channel);
                        let mut changed = 0;
                        for (index, (old, new)) in
                            empty.pixels().iter().zip(actual.pixels()).enumerate()
                        {
                            if old == new {
                                continue;
                            }
                            changed += 1;
                            let (x, y) = ((index % 160) as f32, (index / 160) as f32);
                            assert!(
                                x >= plot.0.ceil()
                                    && x < (plot.0 + plot.2).floor()
                                    && y >= lane.raster_top
                                    && y < lane.raster_bottom,
                                "DC {dc}, channel {channel}, display {display:?}, plot {plot:?}: ({x},{y}) escaped"
                            );
                        }
                        assert!(
                            changed > 0,
                            "DC {dc}, channel {channel}, {display:?}, plot {plot:?} disappeared"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn signed_ranges_outside_the_value_viewport_do_not_paint_edge_pixels() {
        for display in [AudioDisplay::Combined, AudioDisplay::Stereo] {
            for channel in 0..2 {
                for (viewport, range) in [
                    (
                        (0.0, 0.5),
                        AudioRange {
                            min: 0.01,
                            max: 0.02,
                        },
                    ),
                    (
                        (0.5, 1.0),
                        AudioRange {
                            min: -0.02,
                            max: -0.01,
                        },
                    ),
                ] {
                    let mut layout = layout();
                    layout.plot = (10.0, 10.0, 120.0, 99.0);
                    layout.viewport.value = viewport;
                    let style = AudioBackdropStyle {
                        output: false,
                        display,
                        ..AudioBackdropStyle::default()
                    };
                    let mut empty = Pixmap::new(160, 140).unwrap();
                    let mut actual = empty.clone();
                    let mut bin = AudioBin::default();
                    bin.input[channel] = range;
                    draw_mseg_audio(&mut empty, &layout, &[Some(AudioBin::default())], style);
                    draw_mseg_audio(&mut actual, &layout, &[Some(bin)], style);
                    assert_eq!(
                        actual.data(),
                        empty.data(),
                        "channel {channel}, display {display:?}, viewport {viewport:?}"
                    );
                }
            }
        }
    }
}
