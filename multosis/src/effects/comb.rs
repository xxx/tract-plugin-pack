use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// A comb filter with three switchable modes and per-channel stereo offset.
///
/// **Modes:**
/// - **Resonant** = pure feedback comb (`y = x + α·y[n−D]`). Sharp
///   resonant peaks at multiples of `1/D`; sounds like a tuned tube
///   or plucked-string-ish ring. Negative α (signed Depth) shifts the
///   peaks to the half-integer multiples, giving a thinner/octave-up
///   timbre — one knob, two distinct tones.
/// - **Notch** = pure feedforward comb (`y = x + α·x[n−D]`). Notches
///   in the spectrum without resonance. Flatter character; useful for
///   subtle colour and flange-style effects.
/// - **Allpass** = Schroeder allpass (`y = −α·x + x[n−D] + α·y[n−D]`).
///   Magnitude-flat — sounds identical to dry on its own, but summing
///   with dry through the per-row Mix dial reveals the comb-spaced
///   phase pattern as audible filtering.
///
/// **Damping** runs the delay tap through a one-pole low-pass filter
/// in all modes (HF rolloff in the loop tightens FB ring, darkens
/// FF/Allpass timbre).
///
/// **Stereo** spreads the per-channel comb pitch by ±0.5 octaves at
/// 100 %, computed each sample so it tracks Pitch under MSEG
/// modulation. Fractional delay reads via linear interpolation.
pub struct CombEffect {
    pitch_hz: f32,
    mode_idx: f32,
    depth_pct: f32,
    damping_pct: f32,
    stereo_pct: f32,
    sample_rate: f32,
    /// Per-channel delay lines. Sized for the worst case (20 Hz lowest
    /// pitch at 192 kHz = 9600 samples). Indexed by `write_idx`.
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    write_idx: usize,
    /// One-pole LP state for the damping filter, one per channel.
    /// Persists across samples; `damping = 0` collapses the filter to a
    /// passthrough (state isn't read), so leaving it zero across all
    /// samples is safe.
    damp_state_l: f32,
    damp_state_r: f32,
}

impl CombEffect {
    /// 20 Hz lowest pitch → 50 ms longest delay → 9600 samples at
    /// 192 kHz worst case. Sized once in `new`; per-sample work only
    /// reads/writes existing slots.
    const DELAY_BUF_LEN: usize = (1.0 / 20.0 * 192_000.0) as usize + 1;
    /// Hard cap on the loop gain. The user can dial Depth ±100 %, but
    /// internally we clamp to ±0.95 to keep the feedback loop a safe
    /// margin from the unit circle.
    const DEPTH_GAIN_CAP: f32 = 0.95;
    /// Max per-channel pitch offset at Stereo = 100 %, in octaves.
    /// 0.5 octaves per side → up to a full octave of L/R spread.
    const STEREO_OCT_PER_PCT: f32 = 0.005;

    /// Pitch range (Hz). 20 Hz lowest = 50 ms longest delay (audibly
    /// like a short slap echo); 5 kHz highest = 0.2 ms delay (sharp
    /// pitched ring).
    const PITCH_MIN_HZ: f32 = 20.0;
    const PITCH_MAX_HZ: f32 = 5_000.0;

    const MODE_LABELS: &'static [&'static str] = &["Resonant", "Notch", "Allpass"];
    const MODE_RESONANT: usize = 0;
    const MODE_NOTCH: usize = 1;
    const MODE_ALLPASS: usize = 2;

    const PARAMS: [ParamSpec; 5] = [
        ParamSpec {
            name: "Pitch",
            min: Self::PITCH_MIN_HZ,
            max: Self::PITCH_MAX_HZ,
            default: 200.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Mode",
            min: 0.0,
            max: (Self::MODE_LABELS.len() - 1) as f32,
            default: 0.0, // Resonant
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: Self::MODE_LABELS,
            },
        },
        ParamSpec {
            name: "Depth",
            min: -100.0,
            max: 100.0,
            default: 70.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Damping",
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
            name: "Stereo",
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
        Self {
            pitch_hz: Self::PARAMS[0].default,
            mode_idx: Self::PARAMS[1].default,
            depth_pct: Self::PARAMS[2].default,
            damping_pct: Self::PARAMS[3].default,
            stereo_pct: Self::PARAMS[4].default,
            sample_rate: 48_000.0,
            delay_l: vec![0.0; Self::DELAY_BUF_LEN],
            delay_r: vec![0.0; Self::DELAY_BUF_LEN],
            write_idx: 0,
            damp_state_l: 0.0,
            damp_state_r: 0.0,
        }
    }

    /// Read the delay line `buf` at a (fractional) `delay_samples` back
    /// from `write_idx`. Linear interpolation between adjacent slots so
    /// MSEG-sweeping Pitch doesn't zipper.
    #[inline]
    fn read_tap(buf: &[f32], write_idx: usize, delay_samples: f32) -> f32 {
        let n = buf.len();
        let pos = write_idx as f32 + n as f32 - delay_samples;
        let i_floor = pos.floor();
        let frac = pos - i_floor;
        let i0 = (i_floor as usize) % n;
        let i1 = (i0 + 1) % n;
        buf[i0] * (1.0 - frac) + buf[i1] * frac
    }

    /// Compute the per-channel delay-in-samples from the current Pitch +
    /// Stereo settings. Stereo widens by giving the left channel a
    /// slightly LOWER pitch (longer delay) and the right channel a
    /// slightly HIGHER pitch (shorter delay), up to ±0.5 octaves at
    /// Stereo = 100 %.
    fn channel_delays(&self) -> (f32, f32) {
        let stereo_oct = self.stereo_pct * Self::STEREO_OCT_PER_PCT;
        let pitch_l =
            (self.pitch_hz * (-stereo_oct).exp2()).clamp(Self::PITCH_MIN_HZ, Self::PITCH_MAX_HZ);
        let pitch_r =
            (self.pitch_hz * stereo_oct.exp2()).clamp(Self::PITCH_MIN_HZ, Self::PITCH_MAX_HZ);
        let d_l = (self.sample_rate / pitch_l).clamp(2.0, (Self::DELAY_BUF_LEN - 2) as f32);
        let d_r = (self.sample_rate / pitch_r).clamp(2.0, (Self::DELAY_BUF_LEN - 2) as f32);
        (d_l, d_r)
    }
}

impl Default for CombEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for CombEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let (d_l, d_r) = self.channel_delays();
        // Depth in [-1, +1], capped to ±0.95 for loop stability in FB
        // and Allpass modes.
        let alpha = (self.depth_pct * 0.01).clamp(-Self::DEPTH_GAIN_CAP, Self::DEPTH_GAIN_CAP);
        // Damping coefficient for the one-pole LP that smooths the
        // delay tap: 0 = passthrough, 1 = output never moves
        // (everything HF removed). One-pole form `y = (1-d)·x + d·y`.
        let damping = (self.damping_pct * 0.01).clamp(0.0, 0.99);

        let tap_l_raw = Self::read_tap(&self.delay_l, self.write_idx, d_l);
        let tap_r_raw = Self::read_tap(&self.delay_r, self.write_idx, d_r);
        // Apply damping LP. `damp_state` carries the previous output.
        self.damp_state_l = (1.0 - damping) * tap_l_raw + damping * self.damp_state_l;
        self.damp_state_r = (1.0 - damping) * tap_r_raw + damping * self.damp_state_r;
        let tap_l = self.damp_state_l;
        let tap_r = self.damp_state_r;

        let mode = (self.mode_idx.round() as usize).min(Self::MODE_LABELS.len() - 1);
        let (out_l, out_r, write_l, write_r) = match mode {
            // Resonant: y = x + α·tap_damped; feed y back into the
            // delay so the loop resonates. Write y into delay.
            Self::MODE_RESONANT => {
                let y_l = left + alpha * tap_l;
                let y_r = right + alpha * tap_r;
                (y_l, y_r, y_l, y_r)
            }
            // Notch: y = x + α·tap_damped; feed only the dry x into
            // the delay so there's no resonance. Pure FIR.
            Self::MODE_NOTCH => {
                let y_l = left + alpha * tap_l;
                let y_r = right + alpha * tap_r;
                (y_l, y_r, left, right)
            }
            // Allpass (Schroeder): y = -α·x + tap; write x + α·y into
            // the delay. Magnitude is unity at all frequencies; phase
            // shifts at comb-spaced frequencies become audible when
            // the engine's Mix sums dry against this output.
            Self::MODE_ALLPASS => {
                let y_l = -alpha * left + tap_l;
                let y_r = -alpha * right + tap_r;
                (y_l, y_r, left + alpha * y_l, right + alpha * y_r)
            }
            // `set_param` already clamps `mode_idx` into the valid
            // range, so this arm is unreachable in normal operation.
            // Define it as a safe dry passthrough rather than panic so
            // a degenerate state (e.g. a corrupted preset) audibly
            // bypasses the effect rather than crashing the audio thread.
            _ => (left, right, left, right),
        };

        self.delay_l[self.write_idx] = write_l;
        self.delay_r[self.write_idx] = write_r;
        self.write_idx = (self.write_idx + 1) % Self::DELAY_BUF_LEN;

        (out_l, out_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn reset(&mut self) {
        for s in self.delay_l.iter_mut() {
            *s = 0.0;
        }
        for s in self.delay_r.iter_mut() {
            *s = 0.0;
        }
        self.write_idx = 0;
        self.damp_state_l = 0.0;
        self.damp_state_r = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.pitch_hz = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => {
                let max_idx = (Self::MODE_LABELS.len() - 1) as f32;
                self.mode_idx = value.round().clamp(0.0, max_idx);
            }
            2 => self.depth_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.damping_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            4 => self.stereo_pct = value.clamp(Self::PARAMS[4].min, Self::PARAMS[4].max),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    #[test]
    fn comb_lists_five_parameters_with_the_expected_specs() {
        let c = CombEffect::new();
        let specs = c.parameters();
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[0].name, "Pitch");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert_eq!(specs[0].min, 20.0);
        assert_eq!(specs[0].max, 5_000.0);
        assert_eq!(specs[1].name, "Mode");
        assert!(matches!(specs[1].format, ParamFormat::Enum { .. }));
        assert_eq!(specs[1].default, 0.0); // Resonant
        assert_eq!(specs[2].name, "Depth");
        assert_eq!(specs[2].min, -100.0);
        assert_eq!(specs[2].max, 100.0);
        assert_eq!(specs[3].name, "Damping");
        assert_eq!(specs[4].name, "Stereo");
    }

    #[test]
    fn comb_set_param_clamps_each_slot() {
        let mut c = CombEffect::new();
        c.set_param(0, 50_000.0);
        assert_eq!(c.pitch_hz, 5_000.0);
        c.set_param(0, 1.0);
        assert_eq!(c.pitch_hz, 20.0);
        // Mode clamps to [0, MODE_LABELS.len() - 1] = [0, 2].
        c.set_param(1, 99.0);
        assert_eq!(c.mode_idx, 2.0);
        c.set_param(1, -5.0);
        assert_eq!(c.mode_idx, 0.0);
        // Depth signed.
        c.set_param(2, 999.0);
        assert_eq!(c.depth_pct, 100.0);
        c.set_param(2, -999.0);
        assert_eq!(c.depth_pct, -100.0);
        c.set_param(3, 200.0);
        assert_eq!(c.damping_pct, 100.0);
        c.set_param(4, 200.0);
        assert_eq!(c.stereo_pct, 100.0);
    }

    #[test]
    fn comb_resonant_mode_rings_at_pitch_frequency() {
        // Feed a single impulse into the resonant comb and look for the
        // first echo at delay = sample_rate / pitch. The impulse should
        // recirculate through the feedback loop, giving a strong peak
        // ~D samples after the input.
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, 1_000.0); // Pitch = 1 kHz → D = 48 samples
        c.set_param(1, 0.0); // Mode = Resonant
        c.set_param(2, 80.0); // Depth = 80 % (alpha = 0.8)
        c.set_param(3, 0.0); // No damping
        c.set_param(4, 0.0); // No stereo offset
        let (l, _) = c.process_sample(1.0, 1.0);
        // First sample = input + alpha * 0 = 1.0.
        assert!((l - 1.0).abs() < 1e-5, "impulse sample should pass through");
        // Look for the recirculated peak around sample 48.
        let mut peak_idx = 0usize;
        let mut peak_val = 0.0_f32;
        for i in 1..200 {
            let (out, _) = c.process_sample(0.0, 0.0);
            if out.abs() > peak_val {
                peak_val = out.abs();
                peak_idx = i;
            }
        }
        assert!(
            peak_val > 0.5,
            "resonant feedback should produce a clear recirculated peak, got {peak_val}"
        );
        assert!(
            (40..=56).contains(&peak_idx),
            "peak should land near sample 48 (= 48000 / 1000), got {peak_idx}"
        );
    }

    #[test]
    fn comb_notch_mode_has_no_feedback_decay() {
        // Notch mode writes only dry to the delay → an impulse produces
        // exactly one delayed echo, no recirculation. Sum the absolute
        // output over a window much longer than the delay; should be
        // bounded near the input + one echo (~2.0) for any Depth.
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, 1_000.0); // Pitch = 1 kHz → D = 48 samples
        c.set_param(1, 1.0); // Mode = Notch
        c.set_param(2, 90.0); // Depth = 90 % (would resonate in FB mode)
                              // Drive impulse, sum |output| over a long tail.
        let (first, _) = c.process_sample(1.0, 1.0);
        let mut energy: f32 = first.abs();
        for _ in 0..2_000 {
            let (out, _) = c.process_sample(0.0, 0.0);
            energy += out.abs();
        }
        // ~1.0 from impulse + ~0.9 from one delayed echo = ~1.9; allow
        // headroom for fractional-delay interpolation spreading the
        // echo across adjacent samples.
        assert!(
            energy < 3.5,
            "Notch mode should not recirculate (sum |y| was {energy})"
        );
    }

    #[test]
    fn comb_allpass_mode_is_magnitude_flat_on_a_sustained_tone() {
        // Allpass mode preserves magnitude at every frequency. For a
        // sustained sine at any pitch, the wet output's RMS should
        // match the input's RMS after the loop settles. (Engine-level
        // Mix would reveal the phase shifts when summed with dry; we
        // test the wet output's RMS directly here.)
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, 800.0); // Pitch
        c.set_param(1, 2.0); // Mode = Allpass
        c.set_param(2, 70.0); // Depth = 70 %
        let test_hz = 440.0;
        let mut input_energy = 0.0_f32;
        let mut output_energy = 0.0_f32;
        // Warm-up: let the allpass loop settle.
        for i in 0..2_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * test_hz * t).sin();
            let _ = c.process_sample(dry, dry);
        }
        // Measure RMS in/out over a long window.
        for i in 0..48_000 {
            let t = (2_000 + i) as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * test_hz * t).sin();
            let (out, _) = c.process_sample(dry, dry);
            input_energy += dry * dry;
            output_energy += out * out;
        }
        let ratio = (output_energy / input_energy).sqrt();
        assert!(
            (0.9..=1.1).contains(&ratio),
            "Allpass RMS should match dry within 10 % (got {ratio})"
        );
    }

    #[test]
    fn comb_stereo_offset_separates_l_and_r() {
        // With Stereo > 0 the per-channel pitches differ → L ≠ R for
        // a mono-sum input through the comb.
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, 500.0);
        c.set_param(2, 60.0);
        c.set_param(4, 100.0); // Stereo = 100 %
        let mut diff_energy = 0.0_f32;
        for i in 0..4_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 700.0 * t).sin();
            let (l, r) = c.process_sample(dry, dry);
            diff_energy += (l - r) * (l - r);
        }
        assert!(
            diff_energy > 1.0,
            "Stereo = 100 must produce L \u{2260} R for a mono input (diff_energy = {diff_energy})"
        );
    }

    #[test]
    fn comb_stereo_zero_collapses_to_mono() {
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(4, 0.0); // Stereo = 0
        for i in 0..2_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 800.0 * t).sin();
            let (l, r) = c.process_sample(dry, dry);
            assert!(
                (l - r).abs() < 1e-5,
                "Stereo = 0 must be L == R, sample {i}: {l} vs {r}"
            );
        }
    }

    #[test]
    fn comb_stays_bounded_under_max_depth_and_stereo_sweep() {
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(1, 0.0); // Resonant mode (the one that can ring)
        c.set_param(2, 100.0); // Max positive depth (clamps internally to 0.95)
        c.set_param(4, 100.0); // Max stereo offset
        for i in 0..48_000 {
            // Sweep Pitch wildly via set_param every sample.
            let p = (i as f32 / 4096.0).fract();
            let pitch = 20.0 * (250.0_f32).powf(p); // 20..5000 Hz log
            c.set_param(0, pitch);
            let dry = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, r) = c.process_sample(dry, dry);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            assert!(
                l.abs() < 16.0 && r.abs() < 16.0,
                "sample {i} blew up: ({l}, {r})"
            );
        }
    }

    #[test]
    fn comb_reset_clears_delay_line_and_damping_state() {
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(2, 90.0);
        c.set_param(3, 50.0);
        for _ in 0..4_000 {
            let _ = c.process_sample(0.5, 0.5);
        }
        c.reset();
        assert!(c.delay_l.iter().all(|&v| v == 0.0));
        assert!(c.delay_r.iter().all(|&v| v == 0.0));
        assert_eq!(c.write_idx, 0);
        assert_eq!(c.damp_state_l, 0.0);
        assert_eq!(c.damp_state_r, 0.0);
    }
}
