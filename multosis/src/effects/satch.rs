use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// A detail-preserving spectral saturator, ported from the `satch` plugin.
/// Wraps two `SpectralClipper`s (one per channel) plus per-channel dry-delay
/// buffers (matching the spectral path's 2048-sample latency), exposing
/// four modulatable params (Gain, Threshold, Detail, Knee).
///
/// **Algorithm** (per sample, per channel):
/// 1. Pull the delayed dry from the per-channel dry-delay buffer.
/// 2. Time-domain saturate the delayed dry → `td` (returns tanh value for
///    the clip mask).
/// 3. Spectral path on undelayed input → `sp` (the FFT-based detail-
///    preserving clip with a built-in 2048-sample delay).
/// 4. Detail rescue: `wet = (td + detail * clip² * (sp − td)).clamp(±threshold)`
///    where `clip = tanh²(gain·input/threshold)` only fires where the
///    waveshaper is actively clipping.
///
/// **Latency**: 2048 samples (= FFT size) ≈ 43 ms at 48 kHz. multosis
/// reports this to the host via `Effect::latency_samples`, so PDC keeps
/// the chain aligned automatically. The per-row Mix dial blends in-time
/// dry against this delayed wet — for clean saturation, run Mix at 100 %.
pub struct SatchEffect {
    gain_db: f32,
    threshold_db: f32,
    detail_pct: f32,
    knee_pct: f32,
    sample_rate: f32,
    spectral_l: tract_dsp::spectral_clipper::SpectralClipper,
    spectral_r: tract_dsp::spectral_clipper::SpectralClipper,
    /// Dry delay buffers — must be FFT_SIZE long so the time-domain
    /// waveshaper sees the same sample the spectral path was fed
    /// FFT_SIZE samples ago. Required by the algorithm itself
    /// (`td` and `sp` need to operate on time-aligned signals for
    /// `lost = sp − td` to be the spectral path's detail bonus).
    dry_delay_l: Vec<f32>,
    dry_delay_r: Vec<f32>,
    dry_delay_pos: usize,
}

impl SatchEffect {
    /// FFT frame length — matches the satch plugin.
    const FFT_SIZE: usize = 2048;
    /// Hop size — 75 % overlap = 4× redundancy with Hann window.
    const HOP_SIZE: usize = 512;

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Gain",
            min: 0.0,
            max: 24.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: " dB",
            },
        },
        ParamSpec {
            name: "Threshold",
            min: -24.0,
            max: 0.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: " dB",
            },
        },
        ParamSpec {
            name: "Detail",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Knee",
            min: 0.0,
            max: 100.0,
            default: 100.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        Self {
            gain_db: Self::PARAMS[0].default,
            threshold_db: Self::PARAMS[1].default,
            detail_pct: Self::PARAMS[2].default,
            knee_pct: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            spectral_l: tract_dsp::spectral_clipper::SpectralClipper::new(
                Self::FFT_SIZE,
                Self::HOP_SIZE,
            ),
            spectral_r: tract_dsp::spectral_clipper::SpectralClipper::new(
                Self::FFT_SIZE,
                Self::HOP_SIZE,
            ),
            dry_delay_l: vec![0.0; Self::FFT_SIZE],
            dry_delay_r: vec![0.0; Self::FFT_SIZE],
            dry_delay_pos: 0,
        }
    }

    /// Convert dB to a linear amplitude factor.
    #[inline]
    fn db_to_gain(db: f32) -> f32 {
        10.0_f32.powf(db / 20.0)
    }
}

impl Default for SatchEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for SatchEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let gain = Self::db_to_gain(self.gain_db);
        let threshold = Self::db_to_gain(self.threshold_db);
        let inv_threshold = 1.0 / threshold;
        let detail = self.detail_pct * 0.01;
        let knee = self.knee_pct * 0.01;
        // Skip-FFT optimisation: when Detail is zero, the spectral term
        // is multiplied by zero in the blend, so the FFT pipeline can
        // skip the expensive frame work (ring state still advances so
        // re-enabling Detail doesn't glitch).
        let skip_fft = detail <= 0.0;

        // Pull the delayed dry; the spectral path operates on undelayed
        // input but its output emerges 2048 samples late, so we
        // time-align by waveshaping the same-old dry sample.
        let dry_l = self.dry_delay_l[self.dry_delay_pos];
        let dry_r = self.dry_delay_r[self.dry_delay_pos];
        self.dry_delay_l[self.dry_delay_pos] = left;
        self.dry_delay_r[self.dry_delay_pos] = right;
        self.dry_delay_pos = (self.dry_delay_pos + 1) % self.dry_delay_l.len();

        // Time-domain waveshaper on the delayed dry (returns tanh so we
        // can build the clip mask without recomputing it).
        let (td_l, tanh_l) = tract_dsp::spectral_clipper::saturate_td_with_tanh_fast(
            dry_l,
            gain,
            threshold,
            inv_threshold,
            knee,
        );
        let (td_r, tanh_r) = tract_dsp::spectral_clipper::saturate_td_with_tanh_fast(
            dry_r,
            gain,
            threshold,
            inv_threshold,
            knee,
        );

        // Spectral path: gives the detail-preserved reconstruction. With
        // skip_fft we still advance ring/counter state (the clipper
        // handles that internally) but bypass the FFT frame work.
        let sp_l = if skip_fft {
            self.spectral_l
                .process_sample_skip_fft_fast(left, gain, threshold, inv_threshold, knee)
        } else {
            self.spectral_l
                .process_sample_fast(left, gain, threshold, inv_threshold, knee)
        };
        let sp_r = if skip_fft {
            self.spectral_r.process_sample_skip_fft_fast(
                right,
                gain,
                threshold,
                inv_threshold,
                knee,
            )
        } else {
            self.spectral_r
                .process_sample_fast(right, gain, threshold, inv_threshold, knee)
        };

        // Clip mask: tanh²(gained/threshold) is ~0 below the knee and ~1
        // above, so detail is only added where the waveshaper is
        // actively clipping. Without this gate the detail term would
        // colour sub-threshold material that the user expects to pass
        // through clean.
        let clip_l = tanh_l * tanh_l;
        let clip_r = tanh_r * tanh_r;
        let lost_l = sp_l - td_l;
        let lost_r = sp_r - td_r;
        let wet_l = (td_l + detail * clip_l * lost_l).clamp(-threshold, threshold);
        let wet_r = (td_r + detail * clip_r * lost_r).clamp(-threshold, threshold);
        (wet_l, wet_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn reset(&mut self) {
        self.spectral_l.reset();
        self.spectral_r.reset();
        for s in self.dry_delay_l.iter_mut() {
            *s = 0.0;
        }
        for s in self.dry_delay_r.iter_mut() {
            *s = 0.0;
        }
        self.dry_delay_pos = 0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.gain_db = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.threshold_db = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.detail_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.knee_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => {}
        }
    }

    /// 2048-sample FFT delay through the spectral clipper. Reported to
    /// the host via the engine's chain-latency sum so PDC keeps the
    /// multosis output aligned with the rest of the project.
    fn latency_samples(&self) -> usize {
        Self::FFT_SIZE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::Effect;

    #[test]
    fn satch_lists_four_parameters_with_the_expected_specs() {
        let s = SatchEffect::new();
        let specs = s.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Gain");
        assert_eq!(specs[0].min, 0.0);
        assert_eq!(specs[0].max, 24.0);
        assert_eq!(specs[1].name, "Threshold");
        assert_eq!(specs[1].min, -24.0);
        assert_eq!(specs[1].max, 0.0);
        assert_eq!(specs[2].name, "Detail");
        assert_eq!(specs[2].max, 100.0);
        assert_eq!(specs[3].name, "Knee");
        assert_eq!(specs[3].max, 100.0);
    }

    #[test]
    fn satch_reports_fft_size_latency() {
        let s = SatchEffect::new();
        assert_eq!(s.latency_samples(), 2048);
    }

    #[test]
    fn satch_set_param_clamps_to_each_spec_range() {
        let mut s = SatchEffect::new();
        s.set_param(0, 100.0);
        assert_eq!(s.gain_db, 24.0);
        s.set_param(0, -5.0);
        assert_eq!(s.gain_db, 0.0);
        s.set_param(1, 10.0);
        assert_eq!(s.threshold_db, 0.0);
        s.set_param(1, -100.0);
        assert_eq!(s.threshold_db, -24.0);
        s.set_param(2, 999.0);
        assert_eq!(s.detail_pct, 100.0);
        s.set_param(3, -50.0);
        assert_eq!(s.knee_pct, 0.0);
    }

    #[test]
    fn satch_output_stays_bounded_under_aggressive_gain_and_detail() {
        // Max gain + min threshold + full detail + soft knee — every per-
        // sample dial is at its limit. The wet output must stay within the
        // ±threshold clamp the algorithm enforces (≈ -24 dB ≈ 0.063 linear)
        // for every sample.
        let mut s = SatchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_param(0, 24.0); // +24 dB gain
        s.set_param(1, -24.0); // -24 dB threshold
        s.set_param(2, 100.0); // full Detail
        s.set_param(3, 100.0); // full Knee (tanh soft clip)
        let threshold_linear = 10.0_f32.powf(-24.0 / 20.0);
        for i in 0..96_000 {
            // 2 s of program
            let t = i as f32 / 48_000.0;
            let dry = 0.7 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                + 0.3 * (2.0 * std::f32::consts::PI * 880.0 * t).sin();
            let (l, r) = s.process_sample(dry, dry);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            // Algorithm clamps the wet to ±threshold; ride a tiny epsilon
            // above to absorb f32 round-trip rounding.
            let cap = threshold_linear + 1e-4;
            assert!(
                l.abs() <= cap && r.abs() <= cap,
                "sample {i} ({l}, {r}) exceeds threshold {threshold_linear}"
            );
        }
    }

    #[test]
    fn satch_reset_zeroes_dry_delay_and_clipper_state() {
        let mut s = SatchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_param(0, 12.0);
        s.set_param(2, 50.0);
        // Pump signal through to fill the delay + STFT rings.
        for i in 0..4_096 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 1_000.0 * t).sin();
            let _ = s.process_sample(dry, dry);
        }
        s.reset();
        assert!(s.dry_delay_l.iter().all(|&v| v == 0.0));
        assert!(s.dry_delay_r.iter().all(|&v| v == 0.0));
        assert_eq!(s.dry_delay_pos, 0);
        // First post-reset sample is finite + L==R symmetric for mono input.
        let (l, r) = s.process_sample(0.5, 0.5);
        assert!(l.is_finite() && r.is_finite());
        assert!((l - r).abs() < 1e-6);
    }
}
