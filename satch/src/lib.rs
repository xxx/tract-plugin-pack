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

// ── Plugin impl ────────────────────────────────────────────────────────────────

impl Plugin for Satch {
    const NAME: &'static str = "satch";
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

        let detail = self.params.detail.value() / 100.0;
        let knee = self.params.knee.value() / 100.0;
        let mix = self.params.mix.value() / 100.0;
        let dry_mix = 1.0 - mix;

        // Skip FFT processing when the spectral result won't be used.
        // detail=0: spectral contribution is multiplied by 0 in the blend.
        // mix=0: entire wet path is unused (output = dry).
        let skip_fft = detail == 0.0 || mix == 0.0;

        for i in 0..num_samples {
            let gain = self.params.gain.smoothed.next();
            let threshold = self.params.threshold.smoothed.next();
            // Precompute reciprocal once per sample, reused across both
            // channels and the spectral path (saves 3 divides per sample).
            let inv_threshold = 1.0 / threshold;

            let in_l = left[i];
            let in_r = right[i];

            // Delay dry signal to align with spectral path latency
            let dry_l = self.dry_delay_l[self.dry_delay_pos];
            let dry_r = self.dry_delay_r[self.dry_delay_pos];
            self.dry_delay_l[self.dry_delay_pos] = in_l;
            self.dry_delay_r[self.dry_delay_pos] = in_r;
            self.dry_delay_pos = (self.dry_delay_pos + 1) % self.dry_delay_l.len();

            // Time-domain saturation path (on delayed dry signal).
            // Returns both the saturated output and tanh(gained/threshold)
            // for reuse in the clip mask.
            let (td_l, tanh_l) =
                spectral::saturate_td_with_tanh_fast(dry_l, gain, threshold, inv_threshold, knee);
            let (td_r, tanh_r) =
                spectral::saturate_td_with_tanh_fast(dry_r, gain, threshold, inv_threshold, knee);

            // Spectral path (has built-in latency from STFT).
            // When skip_fft=true, ring buffer state is maintained but FFT
            // frames are not computed (saves ~95% of CPU in this path).
            let sp_l = if skip_fft {
                self.spectral_l.process_sample_skip_fft_fast(
                    in_l,
                    gain,
                    threshold,
                    inv_threshold,
                    knee,
                )
            } else {
                self.spectral_l
                    .process_sample_fast(in_l, gain, threshold, inv_threshold, knee)
            };
            let sp_r = if skip_fft {
                self.spectral_r.process_sample_skip_fft_fast(
                    in_r,
                    gain,
                    threshold,
                    inv_threshold,
                    knee,
                )
            } else {
                self.spectral_r
                    .process_sample_fast(in_r, gain, threshold, inv_threshold, knee)
            };

            // Clip mask: tanh(gained/threshold)² is 0 when gained is small
            // (below threshold) and 1 when gained is at/above threshold.
            // Detail is only applied where clipping occurs.
            let clip_l = tanh_l * tanh_l;
            let clip_r = tanh_r * tanh_r;

            let lost_l = sp_l - td_l;
            let lost_r = sp_r - td_r;
            let wet_l = (td_l + detail * clip_l * lost_l).clamp(-threshold, threshold);
            let wet_r = (td_r + detail * clip_r * lost_r).clamp(-threshold, threshold);

            // Dry/wet mix (dry is delayed to align)
            left[i] = mix * wet_l + dry_mix * dry_l;
            right[i] = mix * wet_r + dry_mix * dry_r;
        }

        ProcessStatus::Normal
    }
}

// ── CLAP / VST3 ────────────────────────────────────────────────────────────────

impl ClapPlugin for Satch {
    const CLAP_ID: &'static str = "com.mpd.satch";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A detail-preserving spectral saturator");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::Distortion];
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
    use super::spectral;
    use std::f32::consts::PI;

    /// Replicate the per-sample inner loop from process() for testing without
    /// needing a full nih-plug Buffer/ProcessContext.
    ///
    /// Signal flow:
    /// 1. TD saturate at ±threshold (gain boosts, threshold clips)
    /// 2. Clip mask = tanh(gained/threshold)²
    /// 3. Detail blend using clip mask
    fn process_sample_e2e(
        dry: f32,
        spectral_out: f32,
        gain: f32,
        threshold: f32,
        detail: f32,
        knee: f32,
    ) -> (f32, f32) {
        let (td, tanh_val) = spectral::saturate_td_with_tanh(dry, gain, threshold, knee);
        let sp = spectral_out;

        let clip_mask = tanh_val * tanh_val;

        let lost = sp - td;
        let wet = td + detail * clip_mask * lost;

        (wet, clip_mask)
    }

    // ── Test 1: gain=1, threshold=1 passthrough (hard knee) ─────────────

    #[test]
    fn test_gain_zero_db_passthrough() {
        // gain=1 (0dB), threshold=1 (0dB), knee=0 (hard clip) -> output = input
        // (hard clip at ±1.0 doesn't touch signals below 1.0).
        let gain = 1.0_f32;
        let threshold = 1.0_f32;
        let knee = 0.0; // hard clip = exact passthrough for sub-threshold

        for &dry in &[0.0_f32, 0.1, 0.5, 0.8, -0.3, -0.9] {
            let td = spectral::saturate_td(dry, gain, threshold, knee);
            let (wet, _) = process_sample_e2e(dry, td, gain, threshold, 0.0, knee);
            assert!(
                (wet - dry).abs() < 1e-6,
                "gain=1, threshold=1, knee=0 should pass through: input {dry}, output {wet}"
            );
        }
    }

    // ── Test 2: gain boosts signal, clipped at threshold ───────────────

    #[test]
    fn test_gain_boosts_signal() {
        // gain=10 (+20dB), threshold=1 → output peak ≈ 1.0
        let gain = 10.0_f32;
        let threshold = 1.0_f32;
        let knee = 0.0; // hard clip for exact ceiling check

        let td = spectral::saturate_td(0.5, gain, threshold, knee);
        assert!(
            (td - threshold).abs() < 1e-6,
            "hard clip should produce exactly threshold: got {td}"
        );
    }

    // ── Test 3: threshold clips without gain ─────────────────────────────

    #[test]
    fn test_threshold_clips_without_gain() {
        // gain=1, threshold=0.5 → 0.8 input → output = 0.5 (hard knee)
        let td = spectral::saturate_td(0.8, 1.0, 0.5, 0.0);
        assert!(
            (td - 0.5).abs() < 1e-6,
            "hard clip at threshold=0.5 should clamp 0.8 to 0.5: got {td}"
        );
    }

    // ── Test 4: below threshold unchanged (hard knee) ────────────────────

    #[test]
    fn test_below_threshold_unchanged() {
        // gain=1, threshold=0.5, input=0.3 → output = 0.3 (below ceiling)
        let td = spectral::saturate_td(0.3, 1.0, 0.5, 0.0);
        assert!(
            (td - 0.3).abs() < 1e-6,
            "below-threshold signal should pass through at knee=0: got {td}"
        );
    }

    // ── Test 5: detail at threshold level ─────────────────────────────────

    #[test]
    fn test_detail_at_threshold_level() {
        // gain=1, threshold=0.5, detail=100%, composite signal
        // The clip mask (tanh(gained/threshold)²) activates when gained ≈ threshold.
        // For dry=0.8, gain=1: gained=0.8, gained/threshold=1.6, tanh²(1.6)≈0.91.
        // So clip mask is high, and detail should be blended in.
        let gain = 1.0_f32;
        let threshold = 0.5_f32;
        let knee = 1.0;
        let dry = 0.8_f32;

        let td = spectral::saturate_td(dry, gain, threshold, knee);
        let sp_with_ripple = td + 0.03;

        let (wet_no_detail, _) =
            process_sample_e2e(dry, sp_with_ripple, gain, threshold, 0.0, knee);
        let (wet_with_detail, clip_mask) =
            process_sample_e2e(dry, sp_with_ripple, gain, threshold, 1.0, knee);

        assert!(
            clip_mask > 0.5,
            "clip mask should be high for signal above threshold: {clip_mask}"
        );

        let diff = (wet_with_detail - wet_no_detail).abs();
        assert!(
            diff > 0.001,
            "detail=100% should differ from detail=0%: diff={diff}"
        );
    }

    // ── Test 6: detail at gain level ──────────────────────────────────────

    #[test]
    fn test_detail_at_gain_level() {
        // gain=10, threshold=1.0, detail=100%
        // High gain drives signal well past threshold, clip mask ≈ 1.0.
        let gain = 10.0_f32;
        let threshold = 1.0_f32;
        let knee = 1.0;
        let dry = 0.8_f32;

        let td = spectral::saturate_td(dry, gain, threshold, knee);
        let sp_with_ripple = td + 0.05;

        let (wet_no_detail, _) =
            process_sample_e2e(dry, sp_with_ripple, gain, threshold, 0.0, knee);
        let (wet_with_detail, clip_mask) =
            process_sample_e2e(dry, sp_with_ripple, gain, threshold, 1.0, knee);

        assert!(
            clip_mask > 0.9,
            "clip mask should be near 1.0 at high gain: {clip_mask}"
        );
        let diff = (wet_with_detail - wet_no_detail).abs();
        assert!(
            diff > 0.01,
            "detail=100% should differ from detail=0% at high gain: diff={diff}"
        );
    }

    // ── Test 7: gain + threshold ──────────────────────────────────────────

    #[test]
    fn test_gain_plus_threshold() {
        // gain=4, threshold=0.25 → output peak ≈ 0.25
        let td = spectral::saturate_td(0.5, 4.0, 0.25, 0.0);
        assert!(
            (td - 0.25).abs() < 1e-6,
            "gain=4, threshold=0.25 should clip 0.5 at 0.25: got {td}"
        );
    }

    // ── Test 8: negative signal symmetry ────────────────────────────────

    #[test]
    fn test_negative_signal_symmetry() {
        let gain = 10.0_f32;
        let threshold = 0.5_f32;
        let knee = 1.0;

        let pos = spectral::saturate_td(0.8, gain, threshold, knee);
        let neg = spectral::saturate_td(-0.8, gain, threshold, knee);

        assert!(
            (pos + neg).abs() < 1e-6,
            "saturation should be symmetric: pos={pos}, neg={neg}"
        );
    }

    // ── Test 9: full spectral pipeline — peak near threshold ────────────

    #[test]
    fn test_spectral_pipeline_peak_near_threshold() {
        // Run the full SpectralClipper at gain=8 (~18dB), threshold=0.5
        // and verify the output peak is near threshold.
        let gain = 8.0_f32;
        let threshold = 0.5_f32;
        let knee = 1.0;

        let num_samples = 32768;
        let freq = 440.0_f32;
        let sr = 48000.0_f32;
        let amplitude = 0.8_f32;

        let input: Vec<f32> = (0..num_samples)
            .map(|i| amplitude * (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();

        let mut sc = spectral::SpectralClipper::new(2048, 512);
        let spectral_out: Vec<f32> = input
            .iter()
            .map(|&s| sc.process_sample(s, gain, threshold, knee))
            .collect();

        let latency = 2048;
        let mut dry_delay = vec![0.0_f32; latency];
        let mut dry_pos = 0;
        let mut output = Vec::with_capacity(num_samples);

        for i in 0..num_samples {
            let dry = dry_delay[dry_pos];
            dry_delay[dry_pos] = input[i];
            dry_pos = (dry_pos + 1) % latency;

            let (td, tanh_val) = spectral::saturate_td_with_tanh(dry, gain, threshold, knee);
            let sp = spectral_out[i];

            let clip_mask = tanh_val * tanh_val;
            let lost = sp - td;
            let wet = td + 1.0 * clip_mask * lost;
            output.push(wet);
        }

        let skip = latency + 4096;
        let peak = output[skip..].iter().fold(0.0_f32, |m, &s| m.max(s.abs()));

        // TD clips at threshold, spectral detail can ride slightly above
        assert!(
            peak < threshold * 1.5 + 0.01,
            "output peak {peak} should not far exceed threshold {threshold}"
        );
        assert!(
            peak > threshold * 0.85,
            "output peak {peak} should reach near threshold {threshold}"
        );
    }

    // ── Test 10: full pipeline — detail variation with gain ─────────────

    #[test]
    fn test_spectral_pipeline_detail_variation_with_gain() {
        // At high gain (18dB), the clip mask is near 1.0 for loud signals,
        // so the spectral detail term adds variation in clipped regions.
        let gain = 8.0_f32;
        let threshold = 1.0_f32;
        let knee = 1.0;

        let num_samples = 32768;
        let freq = 440.0_f32;
        let sr = 48000.0_f32;
        let amplitude = 0.8_f32;

        let input: Vec<f32> = (0..num_samples)
            .map(|i| amplitude * (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();

        let mut sc = spectral::SpectralClipper::new(2048, 512);
        let spectral_out: Vec<f32> = input
            .iter()
            .map(|&s| sc.process_sample(s, gain, threshold, knee))
            .collect();

        let latency = 2048;
        let mut dry_delay = vec![0.0_f32; latency];
        let mut dry_pos = 0;

        let mut output_td = Vec::with_capacity(num_samples);
        let mut output_detail = Vec::with_capacity(num_samples);

        for i in 0..num_samples {
            let dry = dry_delay[dry_pos];
            dry_delay[dry_pos] = input[i];
            dry_pos = (dry_pos + 1) % latency;

            let (td, tanh_val) = spectral::saturate_td_with_tanh(dry, gain, threshold, knee);
            let sp = spectral_out[i];
            let clip_mask = tanh_val * tanh_val;

            let lost = sp - td;
            output_td.push(td);
            output_detail.push(td + 1.0 * clip_mask * lost);
        }

        let skip = latency + 4096;

        let mut td_var = 0.0_f64;
        let mut detail_var = 0.0_f64;
        let mut count = 0usize;
        for i in skip..num_samples.saturating_sub(1) {
            let orig = input[i.saturating_sub(latency)];
            if orig.abs() > 0.7 {
                let d_td = (output_td[i + 1] - output_td[i]).abs() as f64;
                let d_detail = (output_detail[i + 1] - output_detail[i]).abs() as f64;
                td_var += d_td;
                detail_var += d_detail;
                count += 1;
            }
        }
        assert!(
            count > 100,
            "should have enough clipped samples: count={count}"
        );
        assert!(
            detail_var > td_var * 1.05,
            "detail output should have more variation than TD in clipped regions: \
             detail_var={detail_var:.6}, td_var={td_var:.6}, count={count}"
        );
    }
}
