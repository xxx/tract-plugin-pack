//! Transactional GUI-side point operations.

use super::shape::validate_nodes;
use super::{HoldMode, MsegData, MsegNode, PointWeight, SegmentShape, ShapeError};

pub(crate) fn all_nodes(count: usize) -> u128 {
    if count >= 128 {
        u128::MAX
    } else {
        (1_u128 << count) - 1
    }
}

pub(crate) fn effective_selection(data: &MsegData, selection: u128) -> u128 {
    let selected = selection & all_nodes(data.node_count);
    if selected == 0 {
        all_nodes(data.node_count)
    } else {
        selected
    }
}

pub(crate) fn commit(data: &mut MsegData, nodes: &[MsegNode]) -> Result<bool, ShapeError> {
    validate_nodes(nodes)?;
    let find = |index: usize| nodes.iter().position(|n| n.time == data.nodes[index].time);
    let hold = match data.hold {
        HoldMode::None => HoldMode::None,
        HoldMode::Sustain(i) => find(i).map_or(HoldMode::None, HoldMode::Sustain),
        HoldMode::Loop { start, end } => match (find(start), find(end)) {
            (Some(start), Some(end)) if start < end => HoldMode::Loop { start, end },
            _ => HoldMode::None,
        },
    };
    let changed = data.active() != nodes || data.hold != hold;
    data.nodes[..nodes.len()].copy_from_slice(nodes);
    data.node_count = nodes.len();
    data.hold = hold;
    Ok(changed)
}

pub(crate) fn reconcile_interval_metadata(data: &MsegData, points: &mut [MsegNode]) {
    if let Some(first) = points.first_mut()
        && first.time > 0.0
        && let Some(boundary) = data.active().iter().find(|n| n.time == first.time)
    {
        first.weight = boundary.weight;
    }
    if let Some(last) = points.last_mut()
        && let Some(boundary) = data.active().iter().find(|n| n.time == last.time)
    {
        last.tension = boundary.tension;
        last.stepped = boundary.stepped;
        last.shape = boundary.shape;
        if last.time < 1.0 {
            last.weight = boundary.weight;
        }
    }
}

pub(crate) fn replace_interval(
    data: &mut MsegData,
    points: &[MsegNode],
) -> Result<(bool, u128), ShapeError> {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Err(ShapeError::Invalid);
    };
    let mut nodes: Vec<_> = data
        .active()
        .iter()
        .filter(|n| n.time < first.time)
        .copied()
        .collect();
    let offset = nodes.len();
    nodes.extend_from_slice(points);
    reconcile_interval_metadata(data, &mut nodes[offset..offset + points.len()]);
    nodes.extend(data.active().iter().filter(|n| n.time > last.time).copied());
    let changed = commit(data, &nodes)?;
    Ok((changed, all_nodes(points.len()) << offset))
}

pub(crate) fn flip_values(data: &mut MsegData, selection: u128) -> bool {
    let selected = effective_selection(data, selection);
    let mut changed = false;
    for (i, n) in data.nodes[..data.node_count].iter_mut().enumerate() {
        if selected & (1 << i) != 0 {
            changed |= n.value != 0.5;
            n.value = 1.0 - n.value;
        }
    }
    changed
}

pub(crate) fn set_segment_shape(
    data: &mut MsegData,
    selection: u128,
    shape: SegmentShape,
    stepped: bool,
) -> bool {
    let selected = effective_selection(data, selection);
    let mut changed = false;
    for (i, node) in data.nodes[..data.node_count - 1].iter_mut().enumerate() {
        if selected & (1 << i) == 0 {
            continue;
        }
        changed |= node.stepped != stepped || (!stepped && node.shape != shape);
        node.stepped = stepped;
        if !stepped {
            node.shape = shape;
        }
    }
    changed
}

pub(crate) fn set_point_weight(data: &mut MsegData, selection: u128, weight: PointWeight) -> bool {
    let selected = effective_selection(data, selection);
    let mut changed = false;
    for (i, node) in data.nodes[..data.node_count].iter_mut().enumerate() {
        if selected & (1 << i) == 0 {
            continue;
        }
        changed |= node.weight != weight;
        node.weight = weight;
    }
    changed
}

pub(crate) fn selected_range(
    data: &MsegData,
    selection: u128,
) -> Result<(usize, usize), ShapeError> {
    let selected = effective_selection(data, selection);
    let first = selected.trailing_zeros() as usize;
    let last = 127 - selected.leading_zeros() as usize;
    if first == last || selected >> first != all_nodes(last - first + 1) {
        return Err(ShapeError::Selection);
    }
    Ok((first, last))
}

pub(crate) fn capture_selection(
    data: &MsegData,
    selection: u128,
) -> Result<super::CurveShape, ShapeError> {
    if selection == 0 {
        return Err(ShapeError::Selection);
    }
    let (first, last) = selected_range(data, selection)?;
    let lo = data.nodes[first].time;
    let span = data.nodes[last].time - lo;
    let mut nodes = data.active()[first..=last].to_vec();
    for node in &mut nodes {
        node.time = (node.time - lo) / span;
    }
    nodes[0].time = 0.0;
    if let Some(last) = nodes.last_mut() {
        last.time = 1.0;
    }
    validate_nodes(&nodes)?;
    Ok(super::CurveShape { nodes })
}

pub(crate) fn paste_selection(
    data: &mut MsegData,
    shape: &super::CurveShape,
    selection: u128,
    merge: bool,
) -> Result<(bool, u128), ShapeError> {
    if selection == 0 {
        return Err(ShapeError::Selection);
    }
    let (first, last) = selected_range(data, selection)?;
    let lo = data.nodes[first].time;
    let hi = data.nodes[last].time;
    let mut points = shape.nodes().to_vec();
    for point in &mut points {
        point.time = lo + (hi - lo) * point.time;
    }
    points[0].time = lo;
    if let Some(last) = points.last_mut() {
        last.time = hi;
    }
    if merge {
        let existing: Vec<_> = data.active()[first..=last]
            .iter()
            .filter(|node| !points.iter().any(|p| p.time == node.time))
            .copied()
            .collect();
        points.extend(existing);
        points.sort_by(|a, b| a.time.total_cmp(&b.time));
    }
    replace_interval(data, &points)
}

pub(crate) fn reverse(data: &mut MsegData, selection: u128) -> Result<(bool, u128), ShapeError> {
    let (first, last) = selected_range(data, selection)?;
    let source = &data.active()[first..=last];
    let sum = source[0].time + source[source.len() - 1].time;
    let mut points = vec![source[source.len() - 1]];
    points[0].time = source[0].time;
    for pair in source.windows(2).rev() {
        let index = points.len() - 1;
        points[index].tension = -pair[0].tension;
        points[index].stepped = false;
        points[index].shape = pair[0].shape;
        if pair[0].stepped && pair[0].value != pair[1].value {
            let gap = ((pair[1].time - pair[0].time) * 0.25).min(1e-4);
            points[index].tension = 0.0;
            points[index].shape = SegmentShape::Curve;
            points.push(MsegNode {
                time: points[index].time + gap,
                value: pair[0].value,
                tension: 0.0,
                stepped: false,
                ..MsegNode::default()
            });
        }
        points.push(MsegNode {
            time: sum - pair[0].time,
            ..pair[0]
        });
    }
    if let Some(end) = points.last_mut() {
        end.time = source[source.len() - 1].time;
    }
    replace_interval(data, &points)
}

pub(crate) fn repeat(
    data: &mut MsegData,
    selection: u128,
    count: usize,
) -> Result<(bool, u128), ShapeError> {
    if !(2..=3).contains(&count) {
        return Err(ShapeError::Invalid);
    }
    let (first, last) = selected_range(data, selection)?;
    let source = &data.active()[first..=last];
    let lo = source[0].time;
    let hi = source[source.len() - 1].time;
    let width = (hi - lo) / count as f32;
    let mut points: Vec<MsegNode> = Vec::new();
    for cycle in 0..count {
        for (i, n) in source.iter().enumerate() {
            let mut next = *n;
            next.time = lo + cycle as f32 * width + (n.time - lo) / count as f32;
            if cycle > 0 && i == 0 {
                let previous_time = points
                    .get(points.len().saturating_sub(2))
                    .map_or(lo, |n| n.time);
                if let Some(previous) = points.last_mut() {
                    if previous.value == next.value && previous.weight == next.weight {
                        *previous = next;
                        continue;
                    }
                    let gap = ((previous.time - previous_time) * 0.25).min(1e-4);
                    previous.time -= gap;
                    previous.tension = 0.0;
                    previous.stepped = false;
                    previous.shape = SegmentShape::Curve;
                }
            }
            points.push(next);
        }
    }
    if let Some(end) = points.last_mut() {
        end.time = hi;
    }
    replace_interval(data, &points)
}

pub(crate) fn nudge(data: &mut MsegData, selection: u128, delta: f32) -> Result<bool, ShapeError> {
    let selected = effective_selection(data, selection);
    let mut left = f32::NEG_INFINITY;
    let mut right = f32::INFINITY;
    for i in 1..data.node_count - 1 {
        if selected & (1 << i) == 0 {
            continue;
        }
        if i == 1 || selected & (1 << (i - 1)) == 0 {
            left = left.max(data.nodes[i - 1].time - data.nodes[i].time + 1e-4);
        }
        if i + 2 == data.node_count || selected & (1 << (i + 1)) == 0 {
            right = right.min(data.nodes[i + 1].time - data.nodes[i].time - 1e-4);
        }
    }
    let delta = delta.max(left.min(0.0)).min(right.max(0.0));
    let mut candidate = *data;
    for i in 1..data.node_count - 1 {
        if selected & (1 << i) != 0 {
            candidate.nodes[i].time = data.nodes[i].time + delta;
        }
    }
    validate_nodes(candidate.active())?;
    let changed = candidate != *data;
    *data = candidate;
    Ok(changed)
}

pub(crate) fn quantize(
    data: &mut MsegData,
    selection: u128,
    pitch: Option<&super::PitchGrid>,
) -> Result<bool, ShapeError> {
    if data.value_grid == super::ValueGrid::Semitones && pitch.is_none() {
        return Err(ShapeError::UnavailableGrid);
    }
    let selected = effective_selection(data, selection);
    let mut nodes = data.active().to_vec();
    for (i, node) in nodes.iter_mut().enumerate() {
        if selected & (1 << i) != 0 {
            let cols = data.time_divisions.max(1) as f32;
            node.time = (node.time * cols).round() / cols;
            node.value = super::grid::snap_value(data, pitch, node.value);
        }
    }
    let hold = data.hold;
    let changed = commit(data, &nodes)?;
    data.hold = hold;
    Ok(changed)
}

pub(crate) fn mutate(
    data: &mut MsegData,
    selection: u128,
    seed: u32,
    values_only: bool,
    pitch: Option<&super::PitchGrid>,
) -> bool {
    let selected = effective_selection(data, selection);
    let first = selected.trailing_zeros() as usize;
    let last = 127 - selected.leading_zeros() as usize;
    let mut rng = super::randomize::Rng::new(seed);
    let before = *data;
    for i in 0..data.node_count {
        if selected & (1 << i) == 0 {
            continue;
        }
        data.nodes[i].value = (data.nodes[i].value + rng.range(-0.25, 0.25)).clamp(0.0, 1.0);
        if data.snap {
            data.nodes[i].value = super::grid::snap_value(data, pitch, data.nodes[i].value);
        }
        if !values_only {
            data.nodes[i].tension =
                (data.nodes[i].tension + rng.range(-0.25, 0.25)).clamp(-1.0, 1.0);
            if i > first && i < last {
                let lo = (data.nodes[i - 1].time + before.nodes[i].time) * 0.5;
                let hi = (before.nodes[i].time + before.nodes[i + 1].time) * 0.5;
                let mut time = rng.range(lo, hi);
                if data.snap {
                    let cols = data.time_divisions.max(1) as f32;
                    time = (time * cols).round() / cols;
                }
                if time > data.nodes[i - 1].time && time < before.nodes[i + 1].time {
                    data.nodes[i].time = time;
                }
            }
        }
    }
    *data != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::{FactoryShape, factory_shape, value_at_phase};

    fn shape(kind: FactoryShape) -> MsegData {
        let mut data = MsegData::default();
        factory_shape(kind).apply(&mut data, false).unwrap();
        data
    }

    fn dense_curve() -> MsegData {
        let mut data = MsegData {
            node_count: 33,
            ..MsegData::default()
        };
        for (i, node) in data.nodes[..data.node_count].iter_mut().enumerate() {
            let phase = i as f32 / 32.0;
            node.time = phase;
            node.value = 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos();
        }
        data
    }

    #[test]
    fn weighted_native_reversal_matches_the_reflected_sampled_shape() {
        let mut data = shape(FactoryShape::Triangle);
        data.insert_node(0.25, 0.3).unwrap();
        data.nodes[0].shape = SegmentShape::SCurve;
        data.nodes[1].tension = -0.5;
        data.nodes[1].weight = PointWeight::Medium;
        data.nodes[2].weight = PointWeight::Soft;
        data.nodes[3].weight = PointWeight::Medium;
        let before = data;
        reverse(&mut data, 0).unwrap();
        for i in 1..1000 {
            let t = i as f32 / 1000.0;
            assert!(
                (value_at_phase(&data, t) - value_at_phase(&before, 1.0 - t)).abs() < 2e-5,
                "{t}"
            );
        }
        for i in 0..data.node_count {
            assert_eq!(
                data.nodes[i].weight,
                before.nodes[before.node_count - 1 - i].weight
            );
        }
    }

    #[test]
    fn repeat_retains_different_weights_at_equal_level_seams() {
        let mut data = shape(FactoryShape::Triangle);
        data.nodes[1].weight = PointWeight::Medium;
        data.nodes[2].weight = PointWeight::Soft;
        let before = data;
        repeat(&mut data, 0, 2).unwrap();
        let seam = data.active().iter().position(|n| n.time == 0.5).unwrap();
        assert_eq!(data.nodes[seam].weight, PointWeight::Hard);
        assert_eq!(data.nodes[seam - 1].weight, PointWeight::Soft);
        for i in 1..1000 {
            if i == 500 {
                continue;
            }
            let t = i as f32 / 1000.0;
            assert!(
                (value_at_phase(&data, t) - value_at_phase(&before, (t * 2.0).fract())).abs()
                    < 0.002,
                "{t}"
            );
        }
    }

    #[test]
    fn interval_paste_preserves_outside_weighted_segments_and_copies_interior_metadata() {
        let mut data = MsegData::default();
        for (t, v) in [(0.25, 0.2), (0.5, 0.4), (0.75, 0.8)] {
            data.insert_node(t, v).unwrap();
        }
        data.nodes[0].tension = 0.4;
        data.nodes[0].weight = PointWeight::Medium;
        data.nodes[1].weight = PointWeight::Soft;
        data.nodes[3].weight = PointWeight::Medium;
        data.nodes[3].tension = -0.3;
        data.nodes[4].weight = PointWeight::Soft;
        let before = data;
        let mut source = shape(FactoryShape::Triangle);
        source.nodes[0].value = 0.2;
        source.nodes[2].value = 0.8;
        source.nodes[0].shape = SegmentShape::SCurve;
        source.nodes[1].weight = PointWeight::Soft;
        paste_selection(
            &mut data,
            &super::super::CurveShape::capture(&source),
            0b1110,
            false,
        )
        .unwrap();
        assert_eq!(data.nodes[1].shape, SegmentShape::SCurve);
        assert_eq!(data.nodes[1].weight, PointWeight::Soft);
        assert_eq!(data.nodes[2].weight, PointWeight::Soft);
        assert_eq!(data.nodes[3].weight, PointWeight::Medium);
        for i in (1..250).chain(751..1000) {
            let t = i as f32 / 1000.0;
            assert_eq!(
                value_at_phase(&data, t).to_bits(),
                value_at_phase(&before, t).to_bits()
            );
        }
    }

    #[test]
    fn selection_clipboard_normalizes_only_time_and_preserves_segment_geometry() {
        let mut data = dense_curve();
        data.nodes[4].tension = 0.7;
        data.nodes[5].stepped = true;
        let clip = capture_selection(&data, 0b1111 << 4).unwrap();
        assert_eq!(clip.nodes().len(), 4);
        for (i, node) in clip.nodes().iter().enumerate() {
            assert_eq!(node.value, data.nodes[i + 4].value);
            assert_eq!(node.tension, data.nodes[i + 4].tension);
            assert_eq!(node.stepped, data.nodes[i + 4].stepped);
            assert!((node.time - i as f32 / 3.0).abs() < 1e-6);
        }
        assert_eq!(capture_selection(&data, 0b101), Err(ShapeError::Selection));
        assert_eq!(capture_selection(&data, 0), Err(ShapeError::Selection));
    }

    #[test]
    fn paste_into_selection_supports_replace_merge_and_preserves_the_tail() {
        let mut base = dense_curve();
        base.nodes[24].tension = 0.6;
        base.nodes[24].stepped = true;
        let clip = crate::mseg::factory_shape(FactoryShape::Triangle);
        let selection = ((1_u128 << 17) - 1) << 8;
        for merge in [false, true] {
            let mut data = base;
            let (_, selected) = paste_selection(&mut data, &clip, selection, merge).unwrap();
            assert_eq!(data.node_count, if merge { 33 } else { 19 });
            assert_eq!(selected.count_ones(), if merge { 17 } else { 3 });
            assert_eq!(&data.active()[..8], &base.active()[..8]);
            let end = data.active().iter().position(|n| n.time == 0.75).unwrap();
            assert_eq!(data.nodes[end].tension, 0.6);
            assert!(data.nodes[end].stepped);
            assert_eq!(&data.active()[end + 1..], &base.active()[25..]);
            assert_eq!(
                data.active().iter().find(|n| n.time == 0.5).unwrap().value,
                1.0
            );
            assert!(data.is_valid());
        }
    }

    #[test]
    fn selection_paste_overflow_is_atomic() {
        let mut data = dense_curve();
        repeat(&mut data, 0, 3).unwrap();
        data.insert_node(0.001, 0.5).unwrap();
        let before = data;
        let clip = crate::mseg::CurveShape::capture(&dense_curve());
        assert_eq!(
            paste_selection(&mut data, &clip, 0b11, true),
            Err(ShapeError::Capacity)
        );
        assert_eq!(data, before);
    }

    #[test]
    fn reversing_curved_and_stepped_shapes_mirrors_the_sampled_curve() {
        for kind in [FactoryShape::Duck, FactoryShape::Square] {
            let original = shape(kind);
            let mut reversed = original;
            reverse(&mut reversed, 0).unwrap();
            for i in 1..1000 {
                let t = i as f32 / 1000.0;
                if (t - 0.5).abs() < 1e-3 {
                    continue;
                }
                assert!(
                    (value_at_phase(&original, 1.0 - t) - value_at_phase(&reversed, t)).abs()
                        < 1e-4,
                    "{kind:?} {t}"
                );
            }
        }
    }

    #[test]
    fn repeats_preserve_both_continuous_and_discontinuous_seams() {
        for kind in [FactoryShape::Triangle, FactoryShape::RampUp] {
            let original = shape(kind);
            let mut repeated = original;
            repeat(&mut repeated, 0, 3).unwrap();
            for i in 1..300 {
                let t = i as f32 / 300.0;
                if i % 100 == 0 {
                    continue;
                }
                assert!(
                    (value_at_phase(&repeated, t) - value_at_phase(&original, (t * 3.0).fract()))
                        .abs()
                        < 0.002
                );
            }
            assert!(repeated.is_valid());
        }
    }

    #[test]
    fn rejection_never_partially_modifies_a_document() {
        let mut data = dense_curve();
        let original = data;
        assert_eq!(repeat(&mut data, 0, 3).unwrap().0, true);
        let full = data;
        assert_eq!(repeat(&mut data, 0, 3), Err(ShapeError::Capacity));
        assert_eq!(data, full);
        data = original;
        data.time_divisions = 2;
        let before = data;
        assert_eq!(quantize(&mut data, 0, None), Err(ShapeError::Collision));
        assert_eq!(data, before);
    }

    #[test]
    fn mutation_preserves_rhythm_count_and_unselected_points() {
        let mut data = dense_curve();
        let before = data;
        assert!(mutate(&mut data, 0b11100, 19, true, None));
        assert_eq!(data.node_count, before.node_count);
        for i in 0..data.node_count {
            assert_eq!(data.nodes[i].time, before.nodes[i].time);
            if !(2..=4).contains(&i) {
                assert_eq!(data.nodes[i], before.nodes[i]);
            }
        }
        for seed in 0..100 {
            mutate(&mut data, 0, seed, false, None);
            assert!(data.is_valid());
        }
    }

    #[test]
    fn interval_edits_preserve_the_outgoing_segment_after_their_boundary() {
        let mut data = shape(FactoryShape::Triangle);
        data.nodes[1].stepped = true;
        data.nodes[1].tension = 0.6;
        let tail = data.nodes[1];
        let points = [
            MsegNode {
                time: 0.2,
                value: 0.3,
                tension: 0.0,
                stepped: false,
                ..MsegNode::default()
            },
            MsegNode {
                time: 0.5,
                value: 1.0,
                tension: 0.0,
                stepped: false,
                ..MsegNode::default()
            },
        ];
        replace_interval(&mut data, &points).unwrap();
        let boundary = data.active().iter().find(|n| n.time == 0.5).unwrap();
        assert_eq!(*boundary, tail);
        assert_eq!(value_at_phase(&data, 0.75), 1.0);
    }

    #[test]
    fn quantize_keeps_hold_markers_attached_to_moved_nodes() {
        let mut data = shape(FactoryShape::Triangle);
        data.nodes[1].time = 0.47;
        data.hold = HoldMode::Sustain(1);
        quantize(&mut data, 1 << 1, None).unwrap();
        assert_eq!(data.nodes[1].time, 0.5);
        assert_eq!(data.hold, HoldMode::Sustain(1));
    }

    #[test]
    fn nudge_rejects_rounding_that_would_collapse_distinct_times() {
        let mut data = MsegData {
            node_count: 4,
            ..MsegData::default()
        };
        data.nodes[1].time = f32::from_bits(0x3effffff);
        data.nodes[2].time = 0.5;
        data.nodes[3].time = 1.0;
        assert!(data.is_valid());
        let before = data;
        assert_eq!(
            nudge(&mut data, 0b110, 1.0 / 16.0),
            Err(ShapeError::Collision)
        );
        assert_eq!(data, before);
    }
}
