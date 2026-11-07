use nih_plug::prelude::*;
use std::sync::Arc;

pub mod editor;
pub mod spectral;

// ── Plugin struct ──────────────────────────────────────────────────────────────

pub struct Satch {
    params: Arc<SatchParams>,
    spectral_l: spectral::SpectralClipper,
    spectral_r: spectral::SpectralClipper,
    // Dry signal delay to align with spectral latency
    dry_delay_l: Vec<f32>,
    dry_delay_r: Vec<f32>,
    dry_delay_pos: usize,
    /// Counts silent input samples for conditional ProcessStatus::Tail.
    silent_input_samples: u32,
}

// ── Params ─────────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct SatchParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<editor::EditorState>,

    /// Gain: input level boost. Stored as linear gain, displayed/edited in dB.
    #[id = "gain"]
    pub gain: FloatParam,

    /// Detail: 0–100%, controls spectral detail preservation.
    #[id = "detail"]
    pub detail: FloatParam,

    /// Threshold: clip ceiling in dB (0 dB = ±1.0, -24 dB = ±0.063).
    /// Stored as linear gain.
    #[id = "threshold"]
    pub threshold: FloatParam,

    /// Knee: 0–100%, crossfades between hard clip (0%) and soft tanh (100%).
    #[id = "knee"]
    pub knee: FloatParam,

    /// Mix: 0–100%, dry/wet blend.
    #[id = "mix"]
    pub mix: FloatParam,
}

impl Default for Satch {
    fn default() -> Self {
        Self {
            params: Arc::new(SatchParams::new()),
            spectral_l: spectral::SpectralClipper::new(2048, 512),
            spectral_r: spectral::SpectralClipper::new(2048, 512),
            dry_delay_l: vec![0.0; 2048],
            dry_delay_r: vec![0.0; 2048],
            dry_delay_pos: 0,
            silent_input_samples: 0,
        }
    }
}

impl SatchParams {
    fn new() -> Self {
        Self {
            editor_state: editor::default_editor_state(),
            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(0.0),
                    max: util::db_to_gain(24.0),
                    factor: FloatRange::gain_skew_factor(0.0, 24.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            threshold: FloatParam::new(
                "Threshold",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(0.0),
                    factor: FloatRange::gain_skew_factor(-24.0, 0.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            detail: FloatParam::new(
                "Detail",
                0.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            knee: FloatParam::new(
                "Knee",
                100.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            mix: FloatParam::new(
                "Mix",
                100.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
        }
    }
}

impl Satch {
    fn process_audio(&mut self, left: &mut [f32], right: &mut [f32]) {
        let num_samples = left.len();
        let detail = self.params.detail.value() / 100.0;
        let knee = self.params.knee.value() / 100.0;
        let mix = self.params.mix.value() / 100.0;
        let dry_mix = 1.0 - mix;

        let effective_detail = if mix == 0.0 { 0.0 } else { detail };

        for i in 0..num_samples {
            let gain = self.params.gain.smoothed.next();
            let threshold = self.params.threshold.smoothed.next();
            // Both channels share the smoothed ceiling and its reciprocal.
            let inv_threshold = 1.0 / threshold;

            let in_l = left[i];
            let in_r = right[i];

            // Delay dry signal to align with spectral path latency
            let dry_l = self.dry_delay_l[self.dry_delay_pos];
            let dry_r = self.dry_delay_r[self.dry_delay_pos];
            self.dry_delay_l[self.dry_delay_pos] = in_l;
            self.dry_delay_r[self.dry_delay_pos] = in_r;
            self.dry_delay_pos = (self.dry_delay_pos + 1) % self.dry_delay_l.len();

            let wet_l = self.spectral_l.process_sample_with_detail_fast(
                in_l,
                gain,
                threshold,
                inv_threshold,
                effective_detail,
                knee,
            );
            let wet_r = self.spectral_r.process_sample_with_detail_fast(
                in_r,
                gain,
                threshold,
                inv_threshold,
                effective_detail,
                knee,
            );

            // Dry/wet mix (dry is delayed to align)
            left[i] = mix * wet_l + dry_mix * dry_l;
            right[i] = mix * wet_r + dry_mix * dry_r;
        }
    }
}

// ── Plugin impl ────────────────────────────────────────────────────────────────

impl Plugin for Satch {
    const NAME: &'static str = "Satch";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone())
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        _buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.spectral_l.reset();
        self.spectral_r.reset();
        self.dry_delay_l.fill(0.0);
        self.dry_delay_r.fill(0.0);
        self.dry_delay_pos = 0;
        context.set_latency_samples(self.spectral_l.latency_samples() as u32);
        true
    }

    fn reset(&mut self) {
        self.spectral_l.reset();
        self.spectral_r.reset();
        self.dry_delay_l.fill(0.0);
        self.dry_delay_r.fill(0.0);
        self.dry_delay_pos = 0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let num_samples = buffer.samples();
        if num_samples == 0 {
            return ProcessStatus::Normal;
        }

        let channel_slices = buffer.as_slice();
        if channel_slices.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (first, rest) = channel_slices.split_at_mut(1);
        let left = &mut first[0][..num_samples];
        let right = &mut rest[0][..num_samples];

        self.process_audio(left, right);

        const TAIL_LEN: u32 = 2048;
        let input_peak = left
            .iter()
            .chain(right.iter())
            .fold(0.0_f32, |a, &s| a.max(s.abs()));
        if input_peak < 1e-6 {
            self.silent_input_samples += num_samples as u32;
        } else {
            self.silent_input_samples = 0;
        }
        if self.silent_input_samples > 0 && self.silent_input_samples <= TAIL_LEN {
            ProcessStatus::Tail(TAIL_LEN)
        } else {
            ProcessStatus::Normal
        }
    }
}

// ── CLAP / VST3 ────────────────────────────────────────────────────────────────

impl ClapPlugin for Satch {
    const CLAP_ID: &'static str = "com.mpd.satch";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A detail-preserving spectral saturator");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Distortion,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Satch {
    const VST3_CLASS_ID: [u8; 16] = *b"SatchMpdPlugin\0\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

nih_export_clap!(Satch);
nih_export_vst3!(Satch);

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(gain: f32, threshold: f32, detail: f32, knee: f32, mix: f32) -> Satch {
        let mut params = SatchParams::new();
        params.detail = FloatParam::new(
            "Detail",
            detail,
            FloatRange::Linear {
                min: 0.0,
                max: 100.0,
            },
        );
        params.knee = FloatParam::new(
            "Knee",
            knee,
            FloatRange::Linear {
                min: 0.0,
                max: 100.0,
            },
        );
        params.mix = FloatParam::new(
            "Mix",
            mix,
            FloatRange::Linear {
                min: 0.0,
                max: 100.0,
            },
        );
        params.gain.smoothed.reset(gain);
        params.threshold.smoothed.reset(threshold);
        Satch {
            params: Arc::new(params),
            ..Satch::default()
        }
    }

    fn tone(sample: usize) -> f32 {
        (std::f64::consts::TAU * 215.0 * sample as f64 / 2048.0).sin() as f32
    }

    #[test]
    fn production_audio_loop_preserves_complete_detail_beneath_the_ceiling() {
        for detail in [0.0, 25.0, 100.0] {
            let mut plugin = configured(4.0, 1.0, detail, 0.0, 100.0);
            let mut left: Vec<_> = (0..16_384).map(|i| 0.8 + 0.02 * tone(i)).collect();
            let mut right: Vec<_> = (0..16_384).map(|i| -0.8 + 0.02 * tone(i)).collect();
            for (l, r) in left.chunks_mut(127).zip(right.chunks_mut(127)) {
                plugin.process_audio(l, r);
            }
            assert!(left[..2048].iter().chain(&right[..2048]).all(|&x| x == 0.0));
            let amplitude = 4.0 * 0.02 * detail / 100.0;
            for sample in 8192..left.len() {
                let quiet = amplitude * tone(sample - 2048);
                for (actual, sign) in [(left[sample], 1.0), (right[sample], -1.0)] {
                    assert!((actual - (sign * (1.0 - amplitude) + quiet)).abs() < 1e-5);
                    assert!(actual.abs() <= 1.0);
                }
            }
        }
    }

    #[test]
    fn production_audio_loop_leaves_subthreshold_material_identical_at_any_detail() {
        for knee in [0.0, 37.5, 100.0] {
            let mut standard = configured(1.0, 1.0, 0.0, knee, 100.0);
            let mut preserved = configured(1.0, 1.0, 100.0, knee, 100.0);
            let input: Vec<_> = (0..8192)
                .map(|i| {
                    0.8 * (std::f64::consts::TAU * 8.0 * i as f64 / 2048.0).sin() as f32
                        + 0.02 * tone(i)
                })
                .collect();
            let mut a_l = input.clone();
            let mut a_r = input.clone();
            let mut b_l = input.clone();
            let mut b_r = input;
            standard.process_audio(&mut a_l, &mut a_r);
            preserved.process_audio(&mut b_l, &mut b_r);
            assert_eq!(a_l, b_l);
            assert_eq!(a_r, b_r);
        }
    }

    #[test]
    fn dry_and_parallel_mix_are_aligned_with_the_headroom_processed_wet_signal() {
        for mix in [0.0, 35.0, 100.0] {
            let mut wet_plugin = configured(4.0, 0.5, 100.0, 37.5, 100.0);
            let mut mixed_plugin = configured(4.0, 0.5, 100.0, 37.5, mix);
            let input: Vec<_> = (0..8192).map(|i| 0.8 + 0.02 * tone(i)).collect();
            let mut wet_l = input.clone();
            let mut wet_r = input.clone();
            let mut mixed_l = input.clone();
            let mut mixed_r = input.clone();
            wet_plugin.process_audio(&mut wet_l, &mut wet_r);
            mixed_plugin.process_audio(&mut mixed_l, &mut mixed_r);
            for sample in 0..input.len() {
                let dry = if sample < 2048 {
                    0.0
                } else {
                    input[sample - 2048]
                };
                let expected = mix / 100.0 * wet_l[sample] + (1.0 - mix / 100.0) * dry;
                assert!((mixed_l[sample] - expected).abs() < 1e-6);
                assert_eq!(mixed_l[sample], mixed_r[sample]);
            }
        }
    }

    #[test]
    fn reset_clears_dry_delay_and_analytic_detail_without_changing_parameters() {
        let mut used = configured(4.0, 0.5, 100.0, 37.5, 35.0);
        let mut fresh = configured(4.0, 0.5, 100.0, 37.5, 35.0);
        let mut warm_l: Vec<_> = (0..5789).map(|i| 0.8 + 0.02 * tone(i)).collect();
        let mut warm_r = warm_l.clone();
        used.process_audio(&mut warm_l, &mut warm_r);
        used.reset();
        assert!(
            used.dry_delay_l
                .iter()
                .chain(&used.dry_delay_r)
                .all(|&x| x == 0.0)
        );
        let mut a_l: Vec<_> = (0..8192).map(|i| 0.8 + 0.02 * tone(i)).collect();
        let mut a_r = a_l.clone();
        let mut b_l = a_l.clone();
        let mut b_r = a_l.clone();
        used.process_audio(&mut a_l, &mut a_r);
        fresh.process_audio(&mut b_l, &mut b_r);
        assert_eq!(a_l, b_l);
        assert_eq!(a_r, b_r);
    }
}
