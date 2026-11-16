//! GUI-side loop analysis and authored seam repairs.

use super::{
    HoldMode, MAX_NODES, MsegData, PlayMode, PointWeight, SegmentShape, ShapeError, tools,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct LoopSeam {
    pub start: usize,
    pub end: usize,
    pub before: f32,
    pub after: f32,
}

impl LoopSeam {
    pub fn new(data: &MsegData, curve_only: bool) -> Self {
        let (start, end) = match data.hold {
            HoldMode::Loop { start, end } if !curve_only => (start, end),
            _ => (0, data.node_count - 1),
        };
        let incoming = data.nodes[end - 1];
        let terminal = data.nodes[end];
        let before = if incoming.stepped || terminal.time - incoming.time <= 1e-9 {
            incoming.value
        } else {
            terminal.value
        };
        Self {
            start,
            end,
            before,
            after: data.nodes[start].value,
        }
    }

    pub fn has_gap(self) -> bool {
        (self.before - self.after).abs() > 1e-5
    }

    pub fn visible(data: &MsegData, curve_only: bool) -> Option<Self> {
        (!curve_only
            && (data.play_mode == PlayMode::Cyclic || matches!(data.hold, HoldMode::Loop { .. })))
        .then(|| Self::new(data, curve_only))
        .filter(|seam| seam.has_gap())
    }

    fn is_flat(self, data: &MsegData) -> bool {
        let first = data.nodes[self.start];
        let next = data.nodes[self.start + 1];
        let incoming = data.nodes[self.end - 1];
        let terminal = data.nodes[self.end];
        let flat_start = first.stepped
            || first.value == next.value
            || first.shape == SegmentShape::SCurve
            || first.weight == PointWeight::Soft
            || next.time - first.time <= 1e-9;
        let flat_end = incoming.stepped
            || incoming.value == terminal.value
            || incoming.shape == SegmentShape::SCurve
            || terminal.weight == PointWeight::Soft
            || terminal.time - incoming.time <= 1e-9;
        self.before == self.after && terminal.value == self.after && flat_start && flat_end
    }
}

pub(crate) fn match_endpoint(data: &mut MsegData, curve_only: bool, end_to_start: bool) -> bool {
    let seam = LoopSeam::new(data, curve_only);
    let (source, target) = if end_to_start {
        (seam.start, seam.end)
    } else {
        (seam.end, seam.start)
    };
    let value = data.nodes[source].value;
    let changed = data.nodes[target].value != value;
    data.nodes[target].value = value;
    changed
}

pub(crate) fn smooth(
    data: &mut MsegData,
    curve_only: bool,
    selection: u128,
) -> Result<(bool, u128), ShapeError> {
    let seam = LoopSeam::new(data, curve_only);
    if seam.is_flat(data) {
        return Ok((false, selection));
    }
    if data.node_count == MAX_NODES {
        return Err(ShapeError::Capacity);
    }
    let start = data.nodes[seam.start];
    let end = data.nodes[seam.end];
    let previous = data.nodes[seam.end - 1];
    let requested = end.time - (end.time - start.time) * 0.1;
    let anchor = if requested > previous.time {
        requested
    } else {
        previous.time + (end.time - previous.time) * 0.5
    };
    if anchor <= previous.time
        || end.time - anchor <= 1e-9
        || (!previous.stepped && seam.before != previous.value && anchor - previous.time <= 1e-9)
    {
        return Err(ShapeError::Collision);
    }
    let mut nodes = data.active().to_vec();
    let mut bridge = end;
    bridge.time = anchor;
    bridge.value = seam.before;
    bridge.tension = 0.0;
    bridge.stepped = false;
    bridge.shape = SegmentShape::SCurve;
    bridge.weight = PointWeight::Soft;
    nodes.insert(seam.end, bridge);
    nodes[seam.start].weight = PointWeight::Soft;
    nodes[seam.end + 1].value = seam.after;
    let changed = tools::commit(data, &nodes)?;
    let lower = tools::all_nodes(seam.end);
    let selected = (selection & lower) | ((selection & !lower) << 1);
    Ok((changed, selected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::{CurveShape, MsegNode, value_at_phase};

    #[test]
    fn analysis_uses_playback_limits_and_distinguishes_static_and_one_shot_curves() {
        let mut data = MsegData::default();
        assert!(LoopSeam::visible(&data, false).is_none());
        data.play_mode = PlayMode::Cyclic;
        assert!(LoopSeam::visible(&data, false).unwrap().has_gap());
        assert!(LoopSeam::visible(&data, true).is_none());
        data.nodes[0].stepped = true;
        assert!(LoopSeam::visible(&data, false).is_none());
        data.nodes[0].stepped = false;
        data.insert_node(0.6, 0.4).unwrap();
        data.nodes[1].stepped = true;
        data.nodes[2].value = data.nodes[0].value;
        assert_eq!(LoopSeam::visible(&data, false).unwrap().before, 0.4);
        data.nodes[1].stepped = false;
        data.nodes[1].time = 0.0_f32.next_up();
        data.hold = HoldMode::Loop { start: 0, end: 1 };
        assert_eq!(LoopSeam::new(&data, false).before, data.nodes[0].value);
    }

    #[test]
    fn matching_only_changes_the_chosen_endpoint_level() {
        for end_to_start in [false, true] {
            let mut data = MsegData::default();
            data.insert_node(0.2, 0.25).unwrap();
            data.insert_node(0.7, 0.75).unwrap();
            data.nodes[1].tension = 0.8;
            data.nodes[2].weight = PointWeight::Medium;
            data.nodes[1].stepped = true;
            data.hold = HoldMode::Loop { start: 1, end: 2 };
            let mut expected = data;
            if end_to_start {
                expected.nodes[2].value = expected.nodes[1].value;
            } else {
                expected.nodes[1].value = expected.nodes[2].value;
            }
            assert!(match_endpoint(&mut data, false, end_to_start));
            assert_eq!(data, expected);
            assert!(!match_endpoint(&mut data, false, end_to_start));
        }
    }

    #[test]
    fn smooth_preserves_a_ramps_peak_and_gives_flat_matched_limits() {
        for shape in [SegmentShape::Curve, SegmentShape::SCurve] {
            for tension in [-1.0, 0.0, 1.0] {
                let mut data = MsegData::default();
                data.nodes[0].shape = shape;
                data.nodes[0].tension = tension;
                data.hold = HoldMode::Sustain(1);
                assert_eq!(smooth(&mut data, false, 3), Ok((true, 5)));
                assert_eq!(data.node_count, 3);
                assert_eq!(data.nodes[1].time, 0.9);
                assert_eq!(value_at_phase(&data, 0.9), 1.0);
                assert_eq!(data.hold, HoldMode::Sustain(2));
                assert!(data.is_valid());
                let epsilon = 1e-5;
                assert!(value_at_phase(&data, epsilon) / epsilon < 0.01);
                assert!(value_at_phase(&data, 1.0 - epsilon) / epsilon < 0.01);
                for i in 0..=1000 {
                    assert!((0.0..=1.0).contains(&value_at_phase(&data, i as f32 / 1000.0)));
                }
                let after = data;
                assert_eq!(smooth(&mut data, false, 5), Ok((false, 5)));
                assert_eq!(data, after);
                let shape = CurveShape::capture(&data);
                assert_eq!(
                    CurveShape::from_json(&shape.to_json().unwrap()).unwrap(),
                    shape
                );
                let json = serde_json::to_string(&data).unwrap();
                assert_eq!(serde_json::from_str::<MsegData>(&json).unwrap(), data);
            }
        }
    }

    #[test]
    fn smooth_remaps_hold_and_selection_while_preserving_existing_interior_and_outside_nodes() {
        let mut data = MsegData::default();
        for (time, value) in [(0.2, 0.2), (0.5, 0.9), (0.78, 0.6), (0.8, 0.7)] {
            data.insert_node(time, value).unwrap();
        }
        data.hold = HoldMode::Loop { start: 1, end: 4 };
        data.nodes[4].tension = -0.4;
        data.nodes[4].shape = SegmentShape::SCurve;
        let before = data;
        let selected = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 5);
        assert_eq!(
            smooth(&mut data, false, selected),
            Ok((true, 1 | 4 | 32 | 64))
        );
        assert_eq!(data.hold, HoldMode::Loop { start: 1, end: 5 });
        assert_eq!(data.nodes[0], before.nodes[0]);
        assert_eq!(data.nodes[2..4], before.nodes[2..4]);
        assert_eq!(data.nodes[6], before.nodes[5]);
        assert!((data.nodes[4].time - 0.79).abs() < 1e-7);
        assert_eq!(
            data.nodes[5],
            MsegNode {
                value: 0.2,
                ..before.nodes[4]
            }
        );
        assert!(LoopSeam::new(&data, false).is_flat(&data));
        assert!(data.is_valid());
    }

    #[test]
    fn static_commands_ignore_hidden_hold_loops() {
        let mut data = MsegData::default();
        data.insert_node(0.2, 0.2).unwrap();
        data.insert_node(0.8, 0.7).unwrap();
        data.hold = HoldMode::Loop { start: 1, end: 2 };
        let before = data;
        assert!(match_endpoint(&mut data, true, true));
        assert_eq!(data.nodes[3].value, before.nodes[0].value);
        assert_eq!(data.nodes[1..3], before.nodes[1..3]);
        data = before;
        smooth(&mut data, true, 0).unwrap();
        assert_eq!(data.hold, before.hold);
        assert_eq!(data.nodes[4].value, before.nodes[0].value);
    }

    #[test]
    fn smooth_does_not_invent_a_peak_from_a_stepped_terminal_node() {
        let mut data = MsegData::default();
        data.nodes[0].value = 0.2;
        data.nodes[0].stepped = true;
        data.nodes[1].value = 0.9;
        smooth(&mut data, false, 0).unwrap();
        for i in 0..=1000 {
            assert_eq!(value_at_phase(&data, i as f32 / 1000.0), 0.2);
        }
    }

    #[test]
    fn return_uses_ten_percent_unless_an_existing_point_occupies_that_window() {
        for (previous, expected) in [(0.85, 0.9), (0.96, 0.98)] {
            let mut data = MsegData::default();
            data.insert_node(previous, 0.7).unwrap();
            smooth(&mut data, false, 0).unwrap();
            assert_eq!(data.nodes[1].time, previous);
            assert!((data.nodes[2].time - expected).abs() < 1e-7);
        }
    }

    #[test]
    fn smooth_errors_are_atomic_at_capacity_and_unrepresentable_insertion_times() {
        let mut full = MsegData::default();
        full.node_count = MAX_NODES;
        for (i, node) in full.nodes.iter_mut().enumerate() {
            *node = MsegNode {
                time: i as f32 / 127.0,
                value: i as f32 / 127.0,
                ..MsegNode::default()
            };
        }
        let before = full;
        assert_eq!(
            smooth(&mut full, false, u128::MAX),
            Err(ShapeError::Capacity)
        );
        assert_eq!(full, before);
        let mut dense = MsegData::default();
        dense.insert_node(0.5, 0.9).unwrap();
        dense.nodes[1].time = 1.0_f32.next_down();
        let before = dense;
        assert_eq!(smooth(&mut dense, false, 3), Err(ShapeError::Collision));
        assert_eq!(dense, before);
    }
}
