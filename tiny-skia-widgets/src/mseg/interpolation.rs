//! Shared, allocation-free interpolation for playback, drawing, and curve baking.

use super::{MsegNode, warp};

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SegmentShape {
    #[default]
    Curve,
    SCurve,
}

impl SegmentShape {
    pub fn is_curve(&self) -> bool {
        *self == Self::Curve
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PointWeight {
    #[default]
    Hard,
    Medium,
    Soft,
}

impl PointWeight {
    pub fn is_hard(&self) -> bool {
        *self == Self::Hard
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Hard => "Hard",
            Self::Medium => "Medium",
            Self::Soft => "Soft",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Hard => Self::Medium,
            Self::Medium => Self::Soft,
            Self::Soft => Self::Hard,
        }
    }
    fn slope(self) -> f32 {
        match self {
            Self::Hard => 1.0,
            Self::Medium => 0.5,
            Self::Soft => 0.0,
        }
    }
}

/// Sample an outgoing segment; callers select the next segment at a breakpoint.
#[inline]
pub fn segment_value(start: &MsegNode, end: &MsegNode, phase: f32) -> f32 {
    if start.stepped {
        return start.value;
    }
    let span = end.time - start.time;
    let t = if span > 1e-9 {
        (phase - start.time) / span
    } else {
        0.0
    };
    let progress = warp(t, start.tension);
    if start.shape == SegmentShape::Curve
        && start.weight == PointWeight::Hard
        && end.weight == PointWeight::Hard
    {
        return start.value + (end.value - start.value) * progress;
    }
    let u = progress.clamp(0.0, 1.0);
    let (a, b) = if start.shape == SegmentShape::SCurve {
        (0.0, 0.0)
    } else {
        (start.weight.slope(), end.weight.slope())
    };
    let complement = 1.0 - u;
    let eased = (a * u * complement * complement + (3.0 - b) * u * u * complement + u * u * u)
        .clamp(0.0, 1.0);
    (start.value + (end.value - start.value) * eased)
        .clamp(start.value.min(end.value), start.value.max(end.value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::{MsegData, value_at_phase};

    #[test]
    fn legacy_curve_path_preserves_sampled_bits() {
        for tension in [-1.0, -0.6, -1e-7, 0.0, 0.2, 1.0] {
            let mut data = MsegData::default();
            data.nodes[0].value = 0.2;
            data.nodes[1].value = 0.9;
            data.nodes[0].tension = tension;
            for i in 0..1000 {
                let phase = i as f32 / 1000.0;
                let old = 0.2 + (0.9 - 0.2) * warp(phase, tension);
                assert_eq!(value_at_phase(&data, phase).to_bits(), old.to_bits());
            }
        }
        assert_eq!(std::mem::size_of::<MsegNode>(), 16);
    }

    #[test]
    fn native_s_curve_matches_smoothstep_with_two_points() {
        let mut data = MsegData::default();
        data.nodes[0].shape = SegmentShape::SCurve;
        for i in 0..=1000 {
            let t = i as f32 / 1000.0;
            assert!((value_at_phase(&data, t) - t * t * (3.0 - 2.0 * t)).abs() < 3e-7);
        }
        assert_eq!(data.node_count, 2);
    }

    #[test]
    fn every_shape_weight_and_bend_is_monotone_bounded_and_endpoint_exact() {
        for shape in [SegmentShape::Curve, SegmentShape::SCurve] {
            for a in [PointWeight::Hard, PointWeight::Medium, PointWeight::Soft] {
                for b in [PointWeight::Hard, PointWeight::Medium, PointWeight::Soft] {
                    for tension in [-1.0, -0.7, 0.0, 0.7, 1.0] {
                        let mut data = MsegData::default();
                        data.nodes[0].shape = shape;
                        data.nodes[0].weight = a;
                        data.nodes[1].weight = b;
                        data.nodes[0].tension = tension;
                        let mut previous = 0.0;
                        for i in 0..=1000 {
                            let t = i as f32 / 1000.0;
                            let value = value_at_phase(&data, t);
                            assert!((0.0..=1.0).contains(&value));
                            assert!(
                                value + 1e-6 >= previous,
                                "{shape:?} {a:?} {b:?} {tension} at {t}"
                            );
                            previous = value;
                        }
                        assert_eq!(value_at_phase(&data, 0.0), 0.0);
                        assert_eq!(value_at_phase(&data, 1.0), 1.0);
                    }
                }
            }
        }
    }

    #[test]
    fn medium_halves_and_soft_flattens_endpoint_slopes_without_rounding_steps() {
        let mut data = MsegData::default();
        let epsilon = 1e-4;
        for (weight, slope) in [
            (PointWeight::Hard, 1.0),
            (PointWeight::Medium, 0.5),
            (PointWeight::Soft, 0.0),
        ] {
            data.nodes[0].weight = weight;
            data.nodes[1].weight = weight;
            assert!((value_at_phase(&data, epsilon) / epsilon - slope).abs() < 0.002);
            assert!(((1.0 - value_at_phase(&data, 1.0 - epsilon)) / epsilon - slope).abs() < 0.002);
        }
        data.nodes[0].stepped = true;
        assert_eq!(value_at_phase(&data, 0.9999), 0.0);
        assert_eq!(value_at_phase(&data, 1.0), 1.0);
    }

    #[test]
    fn new_shapes_remain_inside_arbitrary_rising_and_falling_endpoint_levels() {
        for shape in [SegmentShape::Curve, SegmentShape::SCurve] {
            for weight in [PointWeight::Medium, PointWeight::Soft] {
                for (start, end) in [(0.2_f32, 0.9_f32), (0.9, 0.2), (1.0, 0.0)] {
                    let mut data = MsegData::default();
                    data.nodes[0].value = start;
                    data.nodes[1].value = end;
                    data.nodes[0].shape = shape;
                    data.nodes[1].weight = weight;
                    data.nodes[0].tension = -0.4;
                    for i in 0..=4096 {
                        let value = value_at_phase(&data, i as f32 / 4096.0);
                        assert!((start.min(end)..=start.max(end)).contains(&value));
                    }
                }
            }
        }
    }
}
