use super::*;
use effects::{ParamFormat, ParamScaling};
use widgets::mseg::PitchGrid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PitchGridBinding {
    kind: EffectKind,
    slot: usize,
    target: Option<ModulationTarget>,
    base: u32,
    depth: u32,
    polarity: widgets::Polarity,
}

impl EffectEditor {
    pub(super) fn refresh_pitch_grid(&mut self, effect: &TrackEffect, config: &TrackModulation) {
        let slot = self.selected_mseg.min(3);
        let target = (slot > 0).then(|| config.targets[slot - 1]).flatten();
        let depth = if slot > 0 {
            config.depths[slot - 1]
        } else {
            0.0
        };
        let base = target.map_or(0.0, |target| target.base_value(effect));
        let polarity = config.msegs[slot].polarity;
        let binding = PitchGridBinding {
            kind: effect.kind,
            slot,
            target,
            base: base.to_bits(),
            depth: depth.to_bits(),
            polarity,
        };
        if self.pitch_grid_binding == Some(binding) {
            return;
        }
        let grid = target
            .and_then(|target| target.spec(effect.kind))
            .and_then(|spec| mapped_grid(spec, base, depth, polarity));
        self.mseg_edit.set_pitch_grid(grid, &config.msegs[slot]);
        self.pitch_grid_binding = Some(binding);
    }
}

fn mapped_grid(
    spec: ParamSpec,
    base: f32,
    depth: f32,
    polarity: widgets::Polarity,
) -> Option<PitchGrid> {
    if !base.is_finite()
        || !depth.is_finite()
        || depth == 0.0
        || !spec.min.is_finite()
        || !spec.max.is_finite()
        || spec.max <= spec.min
    {
        return None;
    }
    let frequency = match (spec.format, spec.scaling) {
        (ParamFormat::Hertz, ParamScaling::Log)
            if spec.min > 0.0 && (spec.max / spec.min).is_finite() =>
        {
            true
        }
        (ParamFormat::Number { unit, .. }, ParamScaling::Linear) if unit.trim() == "st" => false,
        _ => return None,
    };
    let pitch = |value: f32| {
        if frequency {
            69.0 + 12.0 * (value as f64 / 440.0).log2()
        } else {
            value as f64
        }
    };
    let first = pitch(spec.min).ceil();
    let last = pitch(spec.max).floor();
    if last < first || last - first + 1.0 > PitchGrid::MAX_LEVELS as f64 {
        return None;
    }
    let norm_base = if frequency {
        effects::value_to_norm(base, spec.min, spec.max, ParamScaling::Log)
    } else {
        base
    };
    let span = if frequency { 1.0 } else { spec.max - spec.min };
    if !norm_base.is_finite() || !span.is_finite() {
        return None;
    }
    let mut levels = Vec::new();
    for note in first as i32..=last as i32 {
        let value = if frequency {
            (440.0 * 2.0_f64.powf((note as f64 - 69.0) / 12.0)) as f32
        } else {
            note as f32
        };
        let norm = if frequency {
            effects::value_to_norm(value, spec.min, spec.max, ParamScaling::Log)
        } else {
            value
        };
        let factor = (norm as f64 - norm_base as f64) / (depth as f64 * span as f64);
        let level = match polarity {
            widgets::Polarity::Unipolar => factor,
            widgets::Polarity::Bipolar => (factor + 1.0) * 0.5,
        };
        if (0.0..=1.0).contains(&level) {
            levels.push(level as f32);
        }
    }
    PitchGrid::new(levels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modulation::assignable_value;

    #[test]
    fn routed_pitch_and_frequency_levels_are_real_semitones_for_both_depth_signs_and_polarities() {
        for kind in [
            EffectKind::PitchShift,
            EffectKind::WarpZone,
            EffectKind::Svf,
        ] {
            let spec = effects::parameter_specs(kind)[0];
            let frequency = spec.format == ParamFormat::Hertz;
            for base in if frequency {
                [220.0, 443.0, 15000.0]
            } else {
                [-7.25, 0.0, 13.4]
            } {
                for depth in [-1.0, -0.17, 0.17, 1.0] {
                    for polarity in [widgets::Polarity::Unipolar, widgets::Polarity::Bipolar] {
                        let grid = mapped_grid(spec, base, depth, polarity).unwrap();
                        for &level in grid.levels() {
                            let actual = assignable_value(level, base, depth, spec, polarity);
                            let note = if frequency {
                                69.0 + 12.0 * (actual as f64 / 440.0).log2()
                            } else {
                                actual as f64
                            };
                            assert!(
                                (note - note.round()).abs() < 2e-4,
                                "{kind:?}, base={base}, depth={depth}, {polarity:?}: {actual} -> {note}"
                            );
                            assert_eq!(grid.snap(level), level);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn grid_includes_a440_and_handles_clipped_and_unreachable_routes() {
        let frequency = effects::parameter_specs(EffectKind::Svf)[0];
        let grid = mapped_grid(frequency, 440.0, 0.5, widgets::Polarity::Bipolar).unwrap();
        assert!(grid.levels().contains(&0.5));
        let pitch = effects::parameter_specs(EffectKind::PitchShift)[0];
        let clipped = mapped_grid(pitch, 0.0, 1.0, widgets::Polarity::Unipolar).unwrap();
        assert_eq!(clipped.levels().first(), Some(&0.0));
        assert_eq!(clipped.levels().last(), Some(&0.5));
        assert!(mapped_grid(pitch, 0.1, 0.001, widgets::Polarity::Unipolar).is_none());
        for depth in [0.0, -0.0, f32::NAN, f32::INFINITY] {
            assert!(mapped_grid(pitch, 0.0, depth, widgets::Polarity::Bipolar).is_none());
        }
        assert!(mapped_grid(pitch, f32::NAN, 1.0, widgets::Polarity::Bipolar).is_none());
    }

    #[test]
    fn unsupported_targets_and_mapping_changes_clear_old_pitch_positions_without_editing_curves() {
        let mut editor = EffectEditor::default();
        let mut effect = TrackEffect {
            kind: EffectKind::PitchShift,
            params: effects::default_params_for_kind(EffectKind::PitchShift),
            ..TrackEffect::default()
        };
        let mut config = TrackModulation::default();
        config.targets[0] = Some(ModulationTarget::EffectParameter(0));
        config.depths[0] = 0.5;
        let before = config.clone();
        editor.refresh_pitch_grid(&effect, &config);
        let levels = editor.mseg_edit.pitch_grid().unwrap().levels().to_vec();
        effect.params[0] = 0.3;
        editor.refresh_pitch_grid(&effect, &config);
        assert_ne!(editor.mseg_edit.pitch_grid().unwrap().levels(), levels);
        config.targets[0] = Some(ModulationTarget::Mix);
        editor.refresh_pitch_grid(&effect, &config);
        assert!(editor.mseg_edit.pitch_grid().is_none());
        config.targets[0] = before.targets[0];
        config.depths[0] = 0.0;
        editor.refresh_pitch_grid(&effect, &config);
        assert!(editor.mseg_edit.pitch_grid().is_none());
        assert_eq!(config.msegs, before.msegs);
    }
}
