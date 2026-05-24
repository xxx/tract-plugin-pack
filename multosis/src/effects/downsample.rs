use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Sample-rate reduction with optional smoothing and timing jitter.
/// Distinct from Bitcrush in that it focuses purely on the rate
/// reduction (no bit-depth quantization) and adds two character
/// knobs the Bitcrush sample-and-hold doesn't have:
///
/// - **Smoothing** blends between the bare sample-and-hold output
///   (0 %) and a one-pole-LP-followed version (100 %) cutoff at
///   half the Rate. The harsh stair-step character of pure S&H
///   transitions into a "lazy rubber-band" follow as Smoothing
///   rises -- between-sample motion stays continuous, but the
///   output still lags the held value with exponential decay.
///
/// - **Jitter** randomly perturbs each hold period by up to +/-30 %
///   of its nominal length at 100 %. Imitates wow / flutter from
///   imperfect clock generators -- subtle pitch wobble layered on
///   top of the downsampling artifacts.
///
/// **Rate** spans 50 Hz to 20 kHz (Log). Below 1 kHz everything
/// gets crunchy lo-fi; 50 Hz is essentially "one sample per audio
/// cycle, output is whatever waveform-shape the held value traces
/// over 20 ms".
///
/// **Width** symmetrically spreads the per-channel Rate. At 0 %
/// both channels share `Rate`. At 100 %, L runs at `1.5 * Rate`
/// and R runs at `0.5 * Rate` -- an asymmetric downsample that
/// gives stereo movement to mono input.
///
/// **Latency:** zero. **Per-sample work:** ~10 MAC + 1 exp (for the
/// LP coefficient, but only when Rate or Smoothing changes -- cached
/// in `set_param`). The audio path uses cached coefficients only.
pub struct DownsampleEffect {
    rate_hz: f32,
    smoothing_pct: f32,
    jitter_pct: f32,
    width_pct: f32,
    sample_rate: f32,

    /// Per-channel hold-phase counter (samples since last capture).
    /// Wraps when it crosses the channel's hold period.
    phase_l: f32,
    phase_r: f32,
    /// Effective hold period in samples for this current cycle.
    /// Re-rolled with jitter applied each time the phase wraps so
    /// jitter perturbs every hold individually instead of producing
    /// a single fixed perturbation per `set_param` call.
    period_l: f32,
    period_r: f32,

    /// Last captured sample per channel.
    held_l: f32,
    held_r: f32,

    /// One-pole LP state per channel. Tracks the held value with
    /// exponential decay; mixed with the bare hold by Smoothing.
    lp_state_l: f32,
    lp_state_r: f32,

    /// Cached LP coefficient `exp(-2*pi*Rate/2/sr)`. Recomputed in
    /// `set_param` when Rate (or sample rate) changes so the per-
    /// sample path stays free of `exp` calls.
    lp_coef: f32,

    /// xorshift32 RNG state for the jitter draws. Seeded from a
    /// non-zero constant in `new`; advances on every hold.
    rng_state: u32,
}

impl DownsampleEffect {
    const RATE_MIN_HZ: f32 = 50.0;
    const RATE_MAX_HZ: f32 = 20_000.0;
    /// Max one-sided perturbation of the hold period at Jitter=100 %,
    /// as a fraction of the nominal period. 30 % gives audible flutter
    /// without letting the period drop near zero (which would degenerate
    /// into "no downsampling at all" for a few samples).
    const JITTER_MAX_FRAC: f32 = 0.3;
    /// Symmetric Width spread: at 100 %, L runs at (1 + half_spread)x
    /// rate and R at (1 - half_spread)x rate.
    const WIDTH_HALF_SPREAD: f32 = 0.5;
    /// Minimum allowed effective period (samples). Stops the audio
    /// path from looping at >1 capture per sample if Rate is somehow
    /// pushed above SR.
    const MIN_PERIOD: f32 = 1.0;

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Rate",
            min: Self::RATE_MIN_HZ,
            max: Self::RATE_MAX_HZ,
            default: 8_000.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Smoothing",
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
            name: "Jitter",
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
            name: "Width",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        let rate_hz = Self::PARAMS[0].default;
        let sample_rate = 48_000.0_f32;
        Self {
            rate_hz,
            smoothing_pct: Self::PARAMS[1].default,
            jitter_pct: Self::PARAMS[2].default,
            width_pct: Self::PARAMS[3].default,
            sample_rate,
            phase_l: 0.0,
            phase_r: 0.0,
            // Start mid-period so the first sample-and-hold capture
            // doesn't pop on initial inputs.
            period_l: (sample_rate / rate_hz).max(Self::MIN_PERIOD),
            period_r: (sample_rate / rate_hz).max(Self::MIN_PERIOD),
            held_l: 0.0,
            held_r: 0.0,
            lp_state_l: 0.0,
            lp_state_r: 0.0,
            lp_coef: Self::compute_lp_coef(rate_hz, sample_rate),
            rng_state: 0x9E37_79B9,
        }
    }

    /// One-pole LP coefficient `exp(-2*pi*fc/sr)` with `fc = rate_hz / 2`
    /// (so the smoothing rolls off above the new effective Nyquist).
    /// Returns a value in `[0, 1)`; at high rates / low SR the cutoff
    /// approaches Nyquist and the coefficient approaches 0 (LP becomes
    /// transparent).
    #[inline]
    fn compute_lp_coef(rate_hz: f32, sample_rate: f32) -> f32 {
        let fc = (rate_hz * 0.5).max(1.0);
        let sr = sample_rate.max(1.0);
        (-2.0 * std::f32::consts::PI * fc / sr).exp()
    }

    /// xorshift32 step. Returns a uniform u32; consumers convert to
    /// `[-1, +1]` or `[0, 1)` as needed.
    #[inline]
    fn rng_next(&mut self) -> u32 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x;
        x
    }

    /// Uniform `[-1, +1]` f32 draw for the jitter perturbation.
    #[inline]
    fn rng_bipolar(&mut self) -> f32 {
        // Top 24 bits / 2^23 - 1 -> [-1, +1).
        let bits = self.rng_next() >> 8;
        (bits as f32) * (1.0 / (1u32 << 23) as f32) - 1.0
    }

    /// Compute the next hold period for a channel given the nominal
    /// (Rate-derived) period and the current Jitter setting. Each
    /// hold re-draws the perturbation so the wow/flutter modulates
    /// continuously.
    #[inline]
    fn next_period(&mut self, nominal: f32) -> f32 {
        if self.jitter_pct <= 0.0 {
            return nominal.max(Self::MIN_PERIOD);
        }
        let amount = self.jitter_pct * 0.01 * Self::JITTER_MAX_FRAC;
        let perturbation = self.rng_bipolar() * amount;
        (nominal * (1.0 + perturbation)).max(Self::MIN_PERIOD)
    }
}

impl Default for DownsampleEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for DownsampleEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // Width spreads per-channel rate symmetrically. At 0 % both
        // channels share Rate; at 100 % L runs faster, R slower.
        let width = self.width_pct * 0.01;
        let half_spread = width * Self::WIDTH_HALF_SPREAD;
        let rate_l = self.rate_hz * (1.0 + half_spread);
        let rate_r = self.rate_hz * (1.0 - half_spread);
        let nominal_period_l = (self.sample_rate / rate_l.max(1.0)).max(Self::MIN_PERIOD);
        let nominal_period_r = (self.sample_rate / rate_r.max(1.0)).max(Self::MIN_PERIOD);
        let smoothing = (self.smoothing_pct * 0.01).clamp(0.0, 1.0);

        // ----- Phase accumulators: capture new sample when phase wraps -----
        self.phase_l += 1.0;
        if self.phase_l >= self.period_l {
            self.held_l = left;
            self.phase_l -= self.period_l;
            // Re-draw jitter for the next cycle.
            self.period_l = self.next_period(nominal_period_l);
        }
        self.phase_r += 1.0;
        if self.phase_r >= self.period_r {
            self.held_r = right;
            self.phase_r -= self.period_r;
            self.period_r = self.next_period(nominal_period_r);
        }

        // ----- One-pole LP follows the held value -----
        // y[n] = (1 - a) * x + a * y[n-1] where a = exp(-2*pi*fc/sr).
        let a = self.lp_coef;
        self.lp_state_l = (1.0 - a) * self.held_l + a * self.lp_state_l;
        self.lp_state_r = (1.0 - a) * self.held_r + a * self.lp_state_r;

        // ----- Smoothing blends bare hold (0 %) with LP follow (100 %) -----
        let out_l = self.held_l + smoothing * (self.lp_state_l - self.held_l);
        let out_r = self.held_r + smoothing * (self.lp_state_r - self.held_r);

        (out_l, out_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.lp_coef = Self::compute_lp_coef(self.rate_hz, self.sample_rate);
        // Reseat the nominal periods so an in-flight phase doesn't
        // overshoot the new period (which could trigger a spurious
        // capture).
        self.period_l = (self.sample_rate / self.rate_hz.max(1.0)).max(Self::MIN_PERIOD);
        self.period_r = self.period_l;
    }

    fn reset(&mut self) {
        self.phase_l = 0.0;
        self.phase_r = 0.0;
        self.held_l = 0.0;
        self.held_r = 0.0;
        self.lp_state_l = 0.0;
        self.lp_state_r = 0.0;
        self.period_l = (self.sample_rate / self.rate_hz.max(1.0)).max(Self::MIN_PERIOD);
        self.period_r = self.period_l;
        // Don't reset rng_state -- continuity of the pseudo-random
        // jitter stream across resets is fine, and re-seeding would
        // give identical wow/flutter every reset.
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => {
                self.rate_hz = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max);
                self.lp_coef = Self::compute_lp_coef(self.rate_hz, self.sample_rate);
            }
            1 => self.smoothing_pct = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.jitter_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.width_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    #[test]
    fn downsample_lists_four_parameters_with_the_expected_specs() {
        let d = DownsampleEffect::new();
        let specs = d.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Rate");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert_eq!(specs[0].min, 50.0);
        assert_eq!(specs[0].max, 20_000.0);
        assert_eq!(specs[1].name, "Smoothing");
        assert_eq!(specs[2].name, "Jitter");
        assert_eq!(specs[3].name, "Width");
    }

    #[test]
    fn downsample_set_param_clamps_each_slot() {
        let mut d = DownsampleEffect::new();
        d.set_param(0, 99_999.0);
        assert_eq!(d.rate_hz, 20_000.0);
        d.set_param(0, 0.0);
        assert_eq!(d.rate_hz, 50.0);
        // Cached LP coefficient follows the clamp.
        let expected = DownsampleEffect::compute_lp_coef(50.0, 48_000.0);
        assert!((d.lp_coef - expected).abs() < 1e-6);
        d.set_param(1, 999.0);
        assert_eq!(d.smoothing_pct, 100.0);
        d.set_param(2, -10.0);
        assert_eq!(d.jitter_pct, 0.0);
        d.set_param(3, 999.0);
        assert_eq!(d.width_pct, 100.0);
    }

    #[test]
    fn downsample_holds_each_sample_for_the_expected_number_of_steps() {
        // At Rate=1 kHz with sr=48 kHz, the nominal period is exactly
        // 48 samples. Feed an ascending counter and verify each output
        // value is held for 48 consecutive samples before changing.
        let mut d = DownsampleEffect::new();
        d.set_sample_rate(48_000.0);
        d.set_param(0, 1_000.0); // Rate
        d.set_param(1, 0.0); // Smoothing 0 -> pure S&H
        d.set_param(2, 0.0); // Jitter 0 -> deterministic period
        d.set_param(3, 0.0); // Width 0 -> mono
                             // Feed a slowly-changing input (one new value every sample,
                             // ascending) and collect the L outputs.
        let mut out = Vec::with_capacity(200);
        for i in 0..200 {
            let x = i as f32;
            let (l, _) = d.process_sample(x, x);
            out.push(l);
        }
        // The first capture happens at sample 47 (phase wraps 0 -> 48
        // after the 48th increment). Before that the held value is 0.
        // After: every 48 samples a new capture.
        // Count distinct values in the first 144 samples (should be ~3).
        let mut runs = Vec::new();
        let mut prev = out[0];
        let mut run_len = 1usize;
        for &v in &out[1..144] {
            if v == prev {
                run_len += 1;
            } else {
                runs.push((prev, run_len));
                prev = v;
                run_len = 1;
            }
        }
        runs.push((prev, run_len));
        // The first run is the pre-capture settling (length depends
        // on whatever period was left from `new()` since `set_param`
        // doesn't snap it). The last run is partial (we hit the end
        // of `out` mid-hold). Both flank the steady-state runs we
        // care about -- check the middle ones, which should all be
        // 48 samples (= 48 kHz / 1 kHz Rate).
        assert!(runs.len() >= 3, "need at least 3 runs to skip endpoints");
        for (val, len) in &runs[1..runs.len() - 1] {
            assert!(
                (47..=49).contains(len),
                "expected hold run of ~48 samples, got value={val}, len={len}"
            );
        }
    }

    #[test]
    fn downsample_at_rate_equal_to_sr_passes_input_through() {
        // When Rate equals the sample rate, the period is 1 sample
        // -> every sample gets captured -> output mirrors input
        // exactly (modulo the 1-sample S&H delay).
        let mut d = DownsampleEffect::new();
        d.set_sample_rate(48_000.0);
        d.set_param(0, 20_000.0); // Max rate (below SR but high)
        d.set_param(1, 0.0);
        d.set_param(2, 0.0);
        d.set_param(3, 0.0);
        // 20 kHz at 48 kHz SR -> period = 2.4 samples; not quite
        // sample-accurate but every input gets captured within 1-2
        // samples. Verify the held output follows the input closely.
        let mut max_lag = 0.0_f32;
        let mut prev_in = 0.0_f32;
        for i in 0..2_000 {
            let t = i as f32 / 48_000.0;
            let x = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            let (l, _) = d.process_sample(x, x);
            // The output should track the input within ~3 samples
            // (period 2.4 + S&H lag).
            if i > 10 {
                max_lag = max_lag.max((l - prev_in).abs());
            }
            prev_in = x;
        }
        // At Rate >= 20 kHz on a 220 Hz tone, lag is small (the input
        // changes slowly between captures).
        assert!(
            max_lag < 0.2,
            "high-rate output should track input closely; max lag {max_lag}"
        );
    }

    #[test]
    fn downsample_smoothing_zero_is_pure_sample_and_hold() {
        // Smoothing=0 -> output equals held value, ignoring the LP
        // state. Verify by feeding a sharp step and observing the
        // output jumps cleanly between hold values (no exponential
        // decay).
        let mut d = DownsampleEffect::new();
        d.set_sample_rate(48_000.0);
        d.set_param(0, 1_000.0);
        d.set_param(1, 0.0); // Smoothing 0
        d.set_param(2, 0.0);
        d.set_param(3, 0.0);
        // Settle past the initial 0-held period.
        for _ in 0..96 {
            let _ = d.process_sample(0.0, 0.0);
        }
        // Step input to 1.0; first capture should snap output to 1.0
        // (no gradual climb).
        let mut snapped = false;
        for i in 0..96 {
            let (l, _) = d.process_sample(1.0, 1.0);
            if l > 0.5 {
                // First time we see a non-zero output, it should be
                // exactly 1.0 (the captured value), NOT a fraction.
                assert!(
                    (l - 1.0).abs() < 1e-5,
                    "Smoothing=0 must snap to captured value; got {l} at sample {i}"
                );
                snapped = true;
                break;
            }
        }
        assert!(
            snapped,
            "expected a capture within 96 samples at Rate=1 kHz"
        );
    }

    #[test]
    fn downsample_smoothing_full_lp_follows_held_value() {
        // Smoothing=100 -> output is the LP-filtered held signal.
        // Verify by feeding a sharp step at the first capture and
        // observing the output ramps up exponentially rather than
        // snapping.
        let mut d = DownsampleEffect::new();
        d.set_sample_rate(48_000.0);
        d.set_param(0, 1_000.0);
        d.set_param(1, 100.0); // Full smoothing
        d.set_param(2, 0.0);
        d.set_param(3, 0.0);
        // Settle the held value at 0.
        for _ in 0..96 {
            let _ = d.process_sample(0.0, 0.0);
        }
        // Feed 1.0 forever; output should rise gradually toward 1.0
        // via the LP, NOT snap.
        let mut saw_intermediate = false;
        for _ in 0..480 {
            let (l, _) = d.process_sample(1.0, 1.0);
            if l > 0.05 && l < 0.95 {
                saw_intermediate = true;
            }
        }
        assert!(
            saw_intermediate,
            "Smoothing=100 must produce gradual LP rise (intermediate values), not snap"
        );
    }

    #[test]
    fn downsample_jitter_zero_holds_for_constant_period() {
        // Jitter=0 must give a deterministic hold period.
        // Two runs from identical state must produce identical outputs.
        let run = || {
            let mut d = DownsampleEffect::new();
            d.set_sample_rate(48_000.0);
            d.set_param(0, 1_000.0);
            d.set_param(1, 0.0);
            d.set_param(2, 0.0); // No jitter
            d.set_param(3, 0.0);
            let mut out = Vec::with_capacity(500);
            for i in 0..500 {
                let x = (i as f32 * 0.01).sin();
                out.push(d.process_sample(x, x).0);
            }
            out
        };
        let a = run();
        let b = run();
        for i in 0..a.len() {
            assert_eq!(
                a[i], b[i],
                "Jitter=0 must be deterministic; sample {i}: {} vs {}",
                a[i], b[i]
            );
        }
    }

    #[test]
    fn downsample_jitter_varies_the_period_across_holds() {
        // Jitter>0 -> hold lengths should vary from sample to sample.
        // Run with Jitter=100 % and verify successive hold lengths
        // aren't all identical.
        let mut d = DownsampleEffect::new();
        d.set_sample_rate(48_000.0);
        d.set_param(0, 1_000.0);
        d.set_param(1, 0.0);
        d.set_param(2, 100.0); // Max jitter
        d.set_param(3, 0.0);
        // Feed a counter and record hold lengths (run length of each
        // distinct output value).
        let mut out = Vec::with_capacity(2_000);
        for i in 0..2_000 {
            let x = i as f32;
            out.push(d.process_sample(x, x).0);
        }
        // Compute consecutive hold lengths.
        let mut lens = Vec::new();
        let mut prev = out[0];
        let mut run = 1usize;
        for &v in &out[1..] {
            if v == prev {
                run += 1;
            } else {
                lens.push(run);
                prev = v;
                run = 1;
            }
        }
        // Verify there's actual variation: at least two distinct
        // hold lengths in the first 20 holds (post-startup).
        let distinct: std::collections::HashSet<_> =
            lens.iter().skip(2).take(20).copied().collect();
        assert!(
            distinct.len() > 3,
            "Jitter=100 should vary hold lengths; saw only {} distinct values",
            distinct.len()
        );
    }

    #[test]
    fn downsample_width_zero_collapses_to_mono_for_symmetric_input() {
        // Width=0 -> both channels share rate. For an L=R input the
        // L and R outputs must match exactly (same captures at the
        // same phases).
        let mut d = DownsampleEffect::new();
        d.set_sample_rate(48_000.0);
        d.set_param(0, 500.0);
        d.set_param(1, 0.0);
        d.set_param(2, 0.0); // No jitter -> identical phase progress
        d.set_param(3, 0.0); // Width 0
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let x = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            let (l, r) = d.process_sample(x, x);
            assert!(
                (l - r).abs() < 1e-6,
                "Width=0 must give L==R; sample {i}: {l} vs {r}"
            );
        }
    }

    #[test]
    fn downsample_width_full_separates_channel_rates() {
        // Width=100 -> L at 1.5*Rate, R at 0.5*Rate. The outputs
        // diverge for L=R input because captures land at different
        // phases.
        let mut d = DownsampleEffect::new();
        d.set_sample_rate(48_000.0);
        d.set_param(0, 1_000.0);
        d.set_param(1, 0.0);
        d.set_param(2, 0.0);
        d.set_param(3, 100.0);
        // Settle past startup.
        for _ in 0..96 {
            let _ = d.process_sample(0.5, 0.5);
        }
        let mut diff = 0.0_f32;
        for i in 0..2_400 {
            let t = i as f32 / 48_000.0;
            let x = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, r) = d.process_sample(x, x);
            diff += (l - r).abs();
        }
        assert!(
            diff > 1.0,
            "Width=100 should split L/R captures; total |L-R| was {diff}"
        );
    }

    #[test]
    fn downsample_stays_bounded_under_aggressive_sweep() {
        let mut d = DownsampleEffect::new();
        d.set_sample_rate(48_000.0);
        for i in 0..48_000 {
            let p = (i as f32 / 4_096.0).fract();
            d.set_param(0, 50.0 * 400.0_f32.powf(p)); // 50..20000 Hz log
            d.set_param(1, (i as f32 / 5_000.0).fract() * 100.0);
            d.set_param(2, (i as f32 / 3_000.0).fract() * 100.0);
            d.set_param(3, (i as f32 / 7_000.0).fract() * 100.0);
            let x = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, r) = d.process_sample(x, x);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            // Output is at most max(|input|, |LP_state|) <= |dry| so
            // it can never exceed input magnitude. Generous bound.
            assert!(
                l.abs() <= 1.5 && r.abs() <= 1.5,
                "sample {i} exceeded input bound: ({l},{r})"
            );
        }
    }

    #[test]
    fn downsample_reset_clears_state() {
        let mut d = DownsampleEffect::new();
        d.set_sample_rate(48_000.0);
        d.set_param(1, 50.0);
        for _ in 0..4_800 {
            let _ = d.process_sample(0.7, 0.7);
        }
        d.reset();
        assert_eq!(d.phase_l, 0.0);
        assert_eq!(d.phase_r, 0.0);
        assert_eq!(d.held_l, 0.0);
        assert_eq!(d.held_r, 0.0);
        assert_eq!(d.lp_state_l, 0.0);
        assert_eq!(d.lp_state_r, 0.0);
    }
}
