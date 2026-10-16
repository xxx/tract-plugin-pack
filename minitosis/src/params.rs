//! Static, effect-specific host automation banks.

use crate::state::{ModulationBank, bank_index, bank_kinds};
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tract_effects::effects::{self, EffectKind, MAX_EFFECT_PARAMS, ParamFormat, TrackEffect};

pub fn host_effect_name(kind: EffectKind) -> String {
    match kind.family() {
        Some(family) => format!("{family} / {}", kind.name()),
        None => kind.name().to_string(),
    }
}

#[derive(Params)]
pub struct EffectControls {
    #[id = "p0"]
    pub p0: FloatParam,
    #[id = "p1"]
    pub p1: FloatParam,
    #[id = "p2"]
    pub p2: FloatParam,
    #[id = "p3"]
    pub p3: FloatParam,
    #[id = "p4"]
    pub p4: FloatParam,
    pub kind: EffectKind,
}

impl EffectControls {
    fn new(kind: EffectKind) -> Self {
        let make = |index: usize| {
            let Some(&spec) = effects::parameter_specs(kind).get(index) else {
                return FloatParam::new(
                    format!("{} Unused {}", host_effect_name(kind), index + 1),
                    0.0,
                    FloatRange::Linear { min: 0.0, max: 1.0 },
                )
                .hide();
            };
            let mut param = FloatParam::new(
                format!("{} {}", host_effect_name(kind), spec.name),
                effects::value_to_norm(spec.default, spec.min, spec.max, spec.scaling),
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_value_to_string(Arc::new(move |norm| {
                effects::format_value(
                    effects::norm_to_value(norm, spec.min, spec.max, spec.scaling),
                    spec.format,
                )
            }))
            .with_string_to_value(Arc::new(move |text| {
                effects::parse_value(text, spec.format)
                    .filter(|v| v.is_finite())
                    .map(|v| {
                        effects::value_to_norm(
                            v.clamp(spec.min, spec.max),
                            spec.min,
                            spec.max,
                            spec.scaling,
                        )
                    })
            }));
            if let ParamFormat::Enum { labels } = spec.format {
                param = param.with_step_size(1.0 / labels.len().saturating_sub(1).max(1) as f32);
            }
            param
        };
        Self {
            p0: make(0),
            p1: make(1),
            p2: make(2),
            p3: make(3),
            p4: make(4),
            kind,
        }
    }

    pub fn controls(&self) -> [&FloatParam; MAX_EFFECT_PARAMS] {
        [&self.p0, &self.p1, &self.p2, &self.p3, &self.p4]
    }

    pub fn track(&self, mix: f32, unmodulated: bool) -> TrackEffect {
        let mut params = [0.0; MAX_EFFECT_PARAMS];
        for ((dst, param), spec) in params
            .iter_mut()
            .zip(self.controls())
            .zip(effects::parameter_specs(self.kind))
        {
            let norm = if unmodulated {
                param.unmodulated_normalized_value()
            } else {
                param.value()
            };
            *dst = effects::norm_to_value(norm, spec.min, spec.max, spec.scaling);
        }
        TrackEffect {
            kind: self.kind,
            params,
            mix,
            muted: false,
            soloed: false,
        }
    }
}

#[derive(Params)]
pub struct MinitosisParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<tiny_skia_widgets::EditorState>,
    #[persist = "modulation-bank"]
    pub modulation: Arc<Mutex<ModulationBank>>,
    #[id = "effect"]
    pub effect: IntParam,
    #[nested(array)]
    pub effects: Vec<EffectControls>,
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "output"]
    pub output: FloatParam,
    #[id = "bypass"]
    pub bypass: BoolParam,
    #[id = "retrig"]
    pub retrig_on_play: BoolParam,
    pub selection_generation: Arc<AtomicU64>,
}

impl Default for MinitosisParams {
    fn default() -> Self {
        let generation = Arc::new(AtomicU64::new(1));
        let changed = generation.clone();
        let kinds = bank_kinds();
        let display_kinds = kinds.clone();
        let parse_kinds = kinds.clone();
        let parse_mix = formatters::s2v_f32_percentage();
        Self {
            editor_state: tiny_skia_widgets::EditorState::from_size(
                crate::editor::WINDOW_WIDTH,
                crate::editor::WINDOW_HEIGHT,
            ),
            modulation: Arc::new(Mutex::new(ModulationBank::default())),
            effect: IntParam::new(
                "Effect",
                0,
                IntRange::Linear {
                    min: 0,
                    max: kinds.len() as i32 - 1,
                },
            )
            .non_automatable()
            .with_value_to_string(Arc::new(move |i| {
                host_effect_name(display_kinds[i as usize])
            }))
            .with_string_to_value(Arc::new(move |text| {
                parse_kinds
                    .iter()
                    .position(|k| host_effect_name(*k).eq_ignore_ascii_case(text.trim()))
                    .map(|i| i as i32)
            }))
            .with_callback(Arc::new(move |_| {
                changed.fetch_add(1, Ordering::AcqRel);
            })),
            effects: kinds.into_iter().map(EffectControls::new).collect(),
            mix: FloatParam::new("Mix", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(Arc::new(move |text| {
                    parse_mix(text).filter(|value| value.is_finite())
                })),
            output: FloatParam::new(
                "Output",
                1.0,
                FloatRange::Skewed {
                    min: util::db_to_gain(-30.0),
                    max: util::db_to_gain(12.0),
                    factor: FloatRange::gain_skew_factor(-30.0, 12.0),
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(Arc::new(|text| {
                effects::parse_value(
                    text,
                    ParamFormat::Number {
                        decimals: 1,
                        unit: "dB",
                    },
                )
                .map(|db| util::db_to_gain(db.clamp(-30.0, 12.0)))
            }))
            .with_smoother(SmoothingStyle::Linear(20.0)),
            bypass: BoolParam::new("Bypass", false).make_bypass(),
            retrig_on_play: BoolParam::new("Retrig on play", false),
            selection_generation: generation,
        }
    }
}

impl MinitosisParams {
    pub fn selected_kind(&self) -> EffectKind {
        self.effects[self.effect.value() as usize].kind
    }
    pub fn controls_for(&self, kind: EffectKind) -> &EffectControls {
        &self.effects[bank_index(kind)]
    }
    pub fn selected_track(&self) -> TrackEffect {
        self.controls_for(self.selected_kind())
            .track(self.mix.unmodulated_normalized_value(), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_effect_has_a_unique_stable_host_parameter_bank() {
        let params = MinitosisParams::default();
        let map = params.param_map();
        let unique: HashSet<_> = map.iter().map(|(id, _, _)| id).collect();
        assert_eq!(unique.len(), map.len());
        let actual: Vec<_> = params
            .effects
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let ids = &map[1 + i * MAX_EFFECT_PARAMS..1 + (i + 1) * MAX_EFFECT_PARAMS];
                format!(
                    "{i}: {:?}: {}",
                    p.kind,
                    ids.iter()
                        .map(|(id, _, _)| id.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .collect();
        assert_eq!(
            actual.join("\n") + "\n",
            include_str!("../test-data/automation-ids.txt")
        );
        for kind in EffectKind::ALL {
            assert_eq!(params.controls_for(kind).kind, kind);
        }
    }

    #[test]
    fn defaults_match_shared_effects_without_automatically_modulating_them() {
        let params = MinitosisParams::default();
        for kind in EffectKind::ALL {
            let track = params.controls_for(kind).track(1.0, true);
            for (value, spec) in track.params.iter().zip(effects::parameter_specs(kind)) {
                assert!((value - spec.default).abs() <= spec.default.abs().max(1.0) * 1e-5);
            }
        }
        assert_eq!(params.selected_kind(), EffectKind::None);
        assert_eq!(params.mix.value(), 1.0);
        assert_eq!(params.output.value(), 1.0);
        assert!(!params.bypass.value());
        assert!(!params.retrig_on_play.value());
    }

    #[test]
    fn log_parameter_host_format_and_parse_use_shared_scaling() {
        let params = MinitosisParams::default();
        let cutoff = &params.controls_for(EffectKind::Svf).p0;
        let norm = cutoff.string_to_normalized_value("800 Hz").unwrap();
        assert_eq!(cutoff.normalized_value_to_string(norm, true), "800 Hz");
        assert!(cutoff.string_to_normalized_value("not a value").is_none());
        assert_eq!(cutoff.string_to_normalized_value("-1 Hz"), Some(0.0));
        assert_eq!(cutoff.string_to_normalized_value("-1 kHz"), Some(0.0));
        for text in ["NaN", "inf", "1e100"] {
            assert!(cutoff.string_to_normalized_value(text).is_none());
            assert!(params.mix.string_to_normalized_value(text).is_none());
            assert!(params.output.string_to_normalized_value(text).is_none());
        }
        assert!(params.effect.flags().contains(ParamFlags::NON_AUTOMATABLE));
    }
    #[test]
    fn every_generic_host_effect_label_round_trips_without_ambiguity() {
        let params = MinitosisParams::default();
        let mut labels = HashSet::new();
        for (i, kind) in bank_kinds().into_iter().enumerate() {
            let norm = params.effect.preview_normalized(i as i32);
            let label = params.effect.normalized_value_to_string(norm, false);
            assert!(
                labels.insert(label.clone()),
                "duplicate host label for {kind:?}"
            );
            assert_eq!(params.effect.string_to_normalized_value(&label), Some(norm));
        }
    }
}
