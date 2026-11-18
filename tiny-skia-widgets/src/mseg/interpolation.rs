//! Shared, allocation-free interpolation for playback, drawing, and curve baking.

use super::{MsegData, MsegNode, warp};

const LEADING_SEGMENTS: usize = 16;

/// Sample the raw envelope shape at a phase clamped to 0..1.
pub fn value_at_phase(data: &MsegData, phase: f32) -> f32 {
    sample_curve(data.active(), phase, segment_index)
}

/// Allocation-free curve sampler that reuses the current segment as phases advance.
pub struct CurveSampler<'a> {
    nodes: &'a [MsegNode],
    segment: usize,
}

impl<'a> CurveSampler<'a> {
    /// Borrow a valid envelope document for the duration of the sampling pass.
    pub fn new(data: &'a MsegData) -> Self {
        Self::from_nodes(data.active())
    }

    pub(super) fn from_nodes(nodes: &'a [MsegNode]) -> Self {
        Self { nodes, segment: 0 }
    }

    /// Sample a clamped phase. Seeking backward also resolves the correct segment.
    pub fn sample(&mut self, phase: f32) -> f32 {
        sample_curve(self.nodes, phase, |nodes, phase| {
            if self.segment != 0 && (phase < nodes[self.segment].time || phase.is_nan()) {
                self.segment = segment_index(nodes, phase);
            } else {
                while self.segment + 2 < nodes.len() && nodes[self.segment + 1].time <= phase {
                    self.segment += 1;
                }
            }
            self.segment
        })
    }
}

#[inline]
fn sample_curve(
    nodes: &[MsegNode],
    phase: f32,
    locate_segment: impl FnOnce(&[MsegNode], f32) -> usize,
) -> f32 {
    let phase = phase.clamp(0.0, 1.0);
    let last = &nodes[nodes.len() - 1];
    if phase >= last.time {
        return last.value;
    }
    if let [start, end] = nodes {
        return segment_value(start, end, phase);
    }
    let i = locate_segment(nodes, phase);
    segment_value(&nodes[i], &nodes[i + 1], phase)
}

pub(super) fn segment_index(nodes: &[MsegNode], phase: f32) -> usize {
    let starts = &nodes[..nodes.len() - 1];
    // Frequent retriggers can keep a dense curve within its leading segments.
    if starts.len() > LEADING_SEGMENTS && starts[LEADING_SEGMENTS].time <= phase {
        return LEADING_SEGMENTS
            + starts[LEADING_SEGMENTS..].partition_point(|node| node.time <= phase)
            - 1;
    }
    let following = &starts[1..];
    for (i, node) in following.iter().enumerate() {
        if node.time <= phase {
            continue;
        }
        return i;
    }
    following.len()
}

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
    use crate::mseg::MAX_NODES;

    #[test]
    fn both_sampling_paths_match_at_and_around_breakpoints() {
        for count in [2, LEADING_SEGMENTS + 1, LEADING_SEGMENTS + 2, MAX_NODES] {
            let mut data = MsegData {
                node_count: count,
                ..MsegData::default()
            };
            for (i, node) in data.nodes[..count].iter_mut().enumerate() {
                node.time = i as f32 / (count - 1) as f32;
                node.value = ((i * 17) % 31) as f32 / 31.0;
                node.tension = if i % 2 == 0 { 0.6 } else { -0.4 };
                node.stepped = i % 4 == 1;
                node.shape = if i % 3 == 0 {
                    SegmentShape::SCurve
                } else {
                    SegmentShape::Curve
                };
                node.weight = [PointWeight::Hard, PointWeight::Medium, PointWeight::Soft][i % 3];
            }
            if count > 2 {
                data.nodes[2].time = data.nodes[1].time.next_up();
            }
            assert!(data.is_valid());
            let nodes = data.active();
            let mut phases = vec![-0.1, 1.1];
            phases.extend((0..=1024).map(|i| i as f32 / 1024.0));
            for node in nodes {
                phases.extend([node.time.next_down(), node.time, node.time.next_up()]);
            }
            phases.sort_unstable_by(f32::total_cmp);
            let mut sampler = CurveSampler::new(&data);
            for phase in phases.iter().copied().chain(phases.iter().rev().copied()) {
                let clamped = phase.clamp(0.0, 1.0);
                let expected = if clamped == 1.0 {
                    nodes[count - 1].value
                } else {
                    let i = nodes[..count - 1]
                        .iter()
                        .rposition(|node| node.time <= clamped)
                        .unwrap_or(0);
                    segment_value(&nodes[i], &nodes[i + 1], clamped)
                };
                assert_eq!(value_at_phase(&data, phase).to_bits(), expected.to_bits());
                assert_eq!(sampler.sample(phase).to_bits(), expected.to_bits());
            }
        }
    }

    #[test]
    fn cursor_uses_preview_endpoints_and_handles_nonfinite_seeks() {
        let mut data = MsegData::default();
        data.insert_node(0.25, 0.2).unwrap();
        data.insert_node(0.5, 0.8).unwrap();
        data.insert_node(0.75, 0.1).unwrap();
        data.nodes[1].stepped = true;
        let preview = &data.active()[1..4];
        let mut sampler = CurveSampler::from_nodes(preview);
        assert_eq!(sampler.sample(f32::NEG_INFINITY), 0.2);
        assert_eq!(sampler.sample(0.5), 0.8);
        assert_eq!(sampler.sample(f32::INFINITY), 0.1);
        assert_eq!(sampler.sample(f32::NAN), 0.2);
        assert_eq!(sampler.sample(0.25), 0.2);
    }

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
