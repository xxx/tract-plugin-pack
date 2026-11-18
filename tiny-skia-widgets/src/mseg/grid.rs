//! GUI-only musical snap positions supplied by an animated host.

use super::MsegData;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ValueGrid {
    #[default]
    Divisions,
    Semitones,
}

impl ValueGrid {
    pub fn is_divisions(&self) -> bool {
        *self == Self::Divisions
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PitchGrid {
    levels: Vec<f32>,
}

impl PitchGrid {
    pub const MAX_LEVELS: usize = 512;

    pub fn new(mut levels: Vec<f32>) -> Option<Self> {
        if levels.is_empty()
            || levels.len() > Self::MAX_LEVELS
            || levels.iter().any(|value| !(0.0..=1.0).contains(value))
        {
            return None;
        }
        levels.sort_by(f32::total_cmp);
        levels.dedup();
        Some(Self { levels })
    }

    pub fn levels(&self) -> &[f32] {
        &self.levels
    }

    pub fn snap(&self, value: f32) -> f32 {
        let upper = self.levels.partition_point(|level| *level < value);
        if upper == 0 {
            self.levels[0]
        } else if upper == self.levels.len() {
            self.levels[upper - 1]
        } else {
            let lo = self.levels[upper - 1];
            let hi = self.levels[upper];
            if value - lo < hi - value { lo } else { hi }
        }
    }
}

pub(crate) fn snap_value(data: &MsegData, pitch: Option<&PitchGrid>, value: f32) -> f32 {
    match data.value_grid {
        ValueGrid::Divisions => {
            let rows = data.value_steps.max(1) as f32;
            ((value * rows).round() / rows).clamp(0.0, 1.0)
        }
        ValueGrid::Semitones => pitch.map_or(value.clamp(0.0, 1.0), |grid| grid.snap(value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_bounded_sorted_unique_and_snap_to_nearest_with_upper_ties() {
        let grid = PitchGrid::new(vec![0.75, 0.0, 0.5, 0.25, 0.5, 1.0]).unwrap();
        assert_eq!(grid.levels(), &[0.0, 0.25, 0.5, 0.75, 1.0]);
        for (input, expected) in [
            (-1.0, 0.0),
            (0.12, 0.0),
            (0.125, 0.25),
            (0.26, 0.25),
            (0.9, 1.0),
            (2.0, 1.0),
        ] {
            assert_eq!(grid.snap(input), expected);
        }
        for level in grid.levels() {
            assert_eq!(grid.snap(*level), *level);
        }
        let single = PitchGrid::new(vec![0.4]).unwrap();
        assert_eq!(single.snap(0.0), 0.4);
        assert_eq!(single.snap(1.0), 0.4);
        for values in [
            vec![],
            vec![f32::NAN],
            vec![f32::INFINITY],
            vec![-0.1],
            vec![1.1],
            vec![0.5; PitchGrid::MAX_LEVELS + 1],
        ] {
            assert!(PitchGrid::new(values).is_none());
        }
    }

    #[test]
    fn old_documents_omit_the_preference_and_sampling_ignores_the_new_grid() {
        let original = MsegData::default();
        let legacy = serde_json::to_string(&original).unwrap();
        assert!(!legacy.contains("value_grid"));
        assert_eq!(serde_json::from_str::<MsegData>(&legacy).unwrap(), original);
        let mut musical = original;
        musical.value_grid = ValueGrid::Semitones;
        assert_eq!(
            serde_json::from_str::<MsegData>(&serde_json::to_string(&musical).unwrap()).unwrap(),
            musical
        );
        for i in 0..=1000 {
            let phase = i as f32 / 1000.0;
            assert_eq!(
                super::super::value_at_phase(&original, phase).to_bits(),
                super::super::value_at_phase(&musical, phase).to_bits()
            );
        }
    }

    #[test]
    fn shape_files_transfer_geometry_without_replacing_grid_preferences() {
        let source = MsegData {
            value_grid: ValueGrid::Semitones,
            ..MsegData::default()
        };
        let shape = super::super::CurveShape::capture(&source);
        let json = shape.to_json().unwrap();
        assert!(!json.contains("value_grid"));
        let mut destination = MsegData::default();
        shape.apply(&mut destination, false).unwrap();
        assert_eq!(destination.value_grid, ValueGrid::Divisions);
        assert_eq!(destination.active(), source.active());
    }
}
