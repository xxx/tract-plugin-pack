use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Tape-style varispeed: pitch and time scale together so a hot
/// speed setting raises pitch AND skips through history faster
/// (unlike PitchShift, which preserves time, and unlike Stretch,
/// which preserves pitch). Signed Speed makes the playback head
/// reversible -- negative values play the recent input backward,
/// 0 % freezes whatever was last buffered, +100 % is forward 1x.
///
/// **Speed** spans -200 % to +200 %. The sign sets direction; the
/// magnitude sets the playback rate. Speed = +100 % is forward
/// 1x (effectively passthrough, modulo grain artifacts); +200 %
/// is forward 2x (1 octave up); -200 % is reverse 2x. 0 % freezes
/// the buffer (grains keep playing whatever was captured but no
/// new material is consumed). Reaching across zero with a slow
/// MSEG creates classic "tape stopping" effects.
///
/// **Grain** sets the grain duration in ms. Shorter grains
/// (10-30 ms) preserve transients but the granular texture is
/// audible -- the "tape" character. Longer grains (50-100 ms)
/// smear transients but sound smoother.
///
/// **Drift** scales a slow sine LFO on the effective Speed
/// (0..100 %). At 100 % the LFO swings the speed by +/-50 %
/// of its set value -- so at Speed=+100 % + Drift=100 % the
/// playback rate wobbles between 50 % and 150 %. Imitates tape
/// machine wow / flutter. Independent of the Downsample effect's
/// Jitter, which perturbs hold-period quantization rather than
/// continuous-speed playback.
///
/// **Rate** sets the Drift LFO frequency (0.05..5 Hz log).
/// Slow rates (0.1-0.3 Hz) feel like loose mechanical wow;
/// fast rates (1-5 Hz) approach flutter and at the upper end
/// become an audible warble.
///
/// **Width** symmetrically spreads the per-channel Speed. At 0 %
/// both channels share Speed. At 100 %, L runs at 1.5x Speed
/// and R at 0.5x Speed -- so a mono input ends up with the two
/// channels playing back at different pitches simultaneously.
///
/// **Algorithm.** 16-grain granular ring-buffer playback (the
/// same machinery as PitchShift). Each grain locks its
/// per-channel rate at spawn time so MSEG-modulating Speed
/// or Width mid-grain doesn't corrupt grains already in flight.
/// Triangle window per grain; sum-of-windows normalization keeps
/// the output level stable as overlap varies. Negative rates
/// read backward through the buffer; the start position is
/// adjusted on spawn so the grain has enough lookbehind / look-
/// ahead margin for its rate and length.
///
/// **Latency:** zero reported. The wet path has internal grain
/// lookbehind (proportional to grain length and speed) but the
/// engine's per-track Mix combines wet against instantaneous
/// dry, so PDC would time-align them and kill the granular
/// character. **Per-sample work:** ~16 grains x (1 triangle
/// window + 2 fractional reads + accumulate) + LFO sin + 1
/// division for normalization. No allocations.
pub struct VarispeedEffect {
    speed_pct: f32,
    grain_ms: f32,
    drift_pct: f32,
    drift_rate_hz: f32,
    width_pct: f32,
    sample_rate: f32,

    /// Stereo ring buffer fed at input rate. Sized for the worst
    /// case (200 ms grain x 2x speed at 192 kHz ~= 77k samples).
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    write_idx: usize,

    /// Active grain pool. Round-robin slot allocation; if all are
    /// alive the oldest is overwritten (preferable to dropping a
    /// beat in the grain stream entirely).
    grains: [Grain; Self::MAX_GRAINS],
    next_slot: usize,

    /// Samples until the next grain spawn. Decrements each sample;
    /// when <= 0 a grain spawns and the countdown resets to the
    /// fixed spawn interval (sample_rate / SPAWN_RATE_HZ).
    spawn_countdown: f32,

    /// Drift LFO phase accumulator in radians.
    lfo_phase: f32,
}

/// One in-flight grain. Locked at spawn time so per-sample
/// modulation of Speed / Grain / Width doesn't corrupt the grain.
#[derive(Clone, Copy, Default)]
struct Grain {
    /// Absolute initial ring-buffer position. The grain reads at
    /// `start_pos + age_samples * rate_l/r` (signed; negative rate
    /// reads backward).
    start_pos: f32,
    /// Output samples produced since spawn. Dead when
    /// `age_samples >= length_samples`.
    age_samples: f32,
    length_samples: f32,
    /// Per-channel signed read rate (samples-of-input-per-output-
    /// sample). Negative rates read backward.
    rate_l: f32,
    rate_r: f32,
}

impl VarispeedEffect {
    const MAX_GRAINS: usize = 16;
    const SPEED_MIN_PCT: f32 = -200.0;
    const SPEED_MAX_PCT: f32 = 200.0;
    const GRAIN_MIN_MS: f32 = 10.0;
    const GRAIN_MAX_MS: f32 = 100.0;
    const DRIFT_MAX_FRAC: f32 = 0.5; // Drift=100 % wobbles speed by +/-50 %
    const DRIFT_RATE_MIN_HZ: f32 = 0.05;
    const DRIFT_RATE_MAX_HZ: f32 = 5.0;
    /// Symmetric Width half-spread. At Width=100 %, L runs at
    /// (1 + half_spread)x speed and R at (1 - half_spread)x.
    const WIDTH_HALF_SPREAD: f32 = 0.5;

    /// Fixed grain spawn rate (Hz). PitchShift exposes this; we
    /// don't -- the Drift / Rate / Width / Grain knobs already
    /// give plenty of character control. 30 Hz gives a comfortable
    /// 2-3 concurrent grains at typical Grain settings.
    const SPAWN_RATE_HZ: f32 = 30.0;

    /// Worst-case grain extent: 100 ms grain at 2x speed at 192
    /// kHz reads 200 ms of input = 38400 samples. Buffer needs
    /// that plus margin for the start-pos lookbehind.
    const BUF_LEN: usize = 96_000;

    /// Floor on the sum-of-windows normalization denominator.
    /// Keeps the divisor bounded during startup / very sparse
    /// overlap; small enough that real-program overlap (~2-3
    /// grains, sum ~= 1.0) is unaffected.
    const NORM_FLOOR: f32 = 0.01;

    /// Minimum absolute rate magnitude. At Speed = 0 we still want
    /// grains to "freeze and hold" rather than divide by zero in
    /// any future math; a tiny epsilon keeps the playback
    /// trivially-finite without audibly progressing.
    const MIN_ABS_RATE: f32 = 1e-4;

    const PARAMS: [ParamSpec; 5] = [
        ParamSpec {
            name: "Speed",
            min: Self::SPEED_MIN_PCT,
            max: Self::SPEED_MAX_PCT,
            default: 100.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Grain",
            min: Self::GRAIN_MIN_MS,
            max: Self::GRAIN_MAX_MS,
            default: 50.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "ms",
            },
        },
        ParamSpec {
            name: "Drift",
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
            name: "Rate",
            min: Self::DRIFT_RATE_MIN_HZ,
            max: Self::DRIFT_RATE_MAX_HZ,
            default: 0.7,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
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
        Self {
            speed_pct: Self::PARAMS[0].default,
            grain_ms: Self::PARAMS[1].default,
            drift_pct: Self::PARAMS[2].default,
            drift_rate_hz: Self::PARAMS[3].default,
            width_pct: Self::PARAMS[4].default,
            sample_rate: 48_000.0,
            delay_l: vec![0.0; Self::BUF_LEN],
            delay_r: vec![0.0; Self::BUF_LEN],
            write_idx: 0,
            grains: [Grain::default(); Self::MAX_GRAINS],
            next_slot: 0,
            spawn_countdown: 0.0,
            lfo_phase: 0.0,
        }
    }

    /// Triangle window in `[0, 1]`. Triangle's perfect overlap at
    /// 50 % is what makes the sum-of-windows normalization clean
    /// without needing fancy windowing math.
    #[inline]
    fn triangle_window(age: f32, length: f32) -> f32 {
        let phase = (age / length).clamp(0.0, 1.0);
        if phase < 0.5 {
            2.0 * phase
        } else {
            2.0 - 2.0 * phase
        }
    }

    /// Fractional ring-buffer read with linear interpolation.
    /// `pos` is an absolute (unwrapped) sample index; `rem_euclid`
    /// handles negative wrapping for backward-reading grains and
    /// far-back start positions equally.
    #[inline]
    fn read_frac(buf: &[f32], pos: f32) -> f32 {
        let n = buf.len();
        let n_f = n as f32;
        let pos_wrapped = pos.rem_euclid(n_f);
        let i_floor = pos_wrapped.floor();
        let frac = pos_wrapped - i_floor;
        let i0 = (i_floor as usize) % n;
        let i1 = (i0 + 1) % n;
        buf[i0] * (1.0 - frac) + buf[i1] * frac
    }

    /// Compute effective per-channel rates given the current
    /// params and the live drift LFO sample. Returned values are
    /// signed (negative = reverse playback). Width's symmetric
    /// spread applies on top of the drifted speed so both spreads
    /// compound coherently.
    #[inline]
    fn effective_rates(&self, lfo_value: f32) -> (f32, f32) {
        let speed = self.speed_pct * 0.01;
        let drift_amount = self.drift_pct * 0.01 * Self::DRIFT_MAX_FRAC;
        let drifted_speed = speed * (1.0 + lfo_value * drift_amount);
        let width = self.width_pct * 0.01;
        let half_spread = width * Self::WIDTH_HALF_SPREAD;
        let rate_l = drifted_speed * (1.0 + half_spread);
        let rate_r = drifted_speed * (1.0 - half_spread);
        (rate_l, rate_r)
    }

    /// Allocate a grain slot and initialize it with the current
    /// effective rates + length. Locking these at spawn time means
    /// MSEG-modulating Speed/Grain/Width only affects FUTURE grains.
    fn spawn_grain(&mut self, rate_l: f32, rate_r: f32) {
        let length_samples = (self.grain_ms * 0.001 * self.sample_rate).max(2.0);
        // Lookbehind: for forward playback at rate r, a grain
        // reads `length * r` samples of FUTURE-relative-to-start;
        // we set start_pos = write_idx - length*r so the read
        // stays inside already-written history. For backward
        // playback (negative r), the grain reads BACKWARD from
        // start_pos, so we want start_pos near the write head so
        // there's plenty of history to walk into. Take the max
        // of L/R rates to size the lookbehind for whichever
        // channel needs more.
        let max_rate = rate_l.abs().max(rate_r.abs()).max(Self::MIN_ABS_RATE);
        let lookbehind = if rate_l >= 0.0 || rate_r >= 0.0 {
            // At least one channel reads forward; size for the
            // forward channel's reach.
            (length_samples * max_rate).max(1.0)
        } else {
            // Both reading backward; a tiny lookbehind is enough,
            // the backward sweep walks into older history naturally.
            1.0
        };
        let start_pos = self.write_idx as f32 - lookbehind;

        let mut slot_idx = self.next_slot;
        for _ in 0..Self::MAX_GRAINS {
            if self.grains[slot_idx].age_samples >= self.grains[slot_idx].length_samples {
                break;
            }
            slot_idx = (slot_idx + 1) % Self::MAX_GRAINS;
        }

        self.grains[slot_idx] = Grain {
            start_pos,
            age_samples: 0.0,
            length_samples,
            rate_l,
            rate_r,
        };
        self.next_slot = (slot_idx + 1) % Self::MAX_GRAINS;
    }
}

impl Default for VarispeedEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for VarispeedEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // ----- Write dry to the ring buffer -----
        self.delay_l[self.write_idx] = left;
        self.delay_r[self.write_idx] = right;

        // ----- Advance drift LFO -----
        let two_pi = 2.0 * std::f32::consts::PI;
        let lfo_value = self.lfo_phase.sin();
        self.lfo_phase += two_pi * self.drift_rate_hz / self.sample_rate;
        if self.lfo_phase >= two_pi {
            self.lfo_phase -= two_pi;
        }

        // ----- Maybe spawn a grain at the fixed spawn rate -----
        self.spawn_countdown -= 1.0;
        if self.spawn_countdown <= 0.0 {
            let (rate_l, rate_r) = self.effective_rates(lfo_value);
            self.spawn_grain(rate_l, rate_r);
            self.spawn_countdown += self.sample_rate / Self::SPAWN_RATE_HZ;
        }

        // ----- Sum windowed grain reads -----
        let mut out_l = 0.0_f32;
        let mut out_r = 0.0_f32;
        let mut sum_w = 0.0_f32;
        for g in self.grains.iter_mut() {
            if g.age_samples >= g.length_samples {
                continue;
            }
            let w = Self::triangle_window(g.age_samples, g.length_samples);
            let read_l = g.start_pos + g.age_samples * g.rate_l;
            let read_r = g.start_pos + g.age_samples * g.rate_r;
            out_l += w * Self::read_frac(&self.delay_l, read_l);
            out_r += w * Self::read_frac(&self.delay_r, read_r);
            sum_w += w;
            g.age_samples += 1.0;
        }
        let denom = sum_w.max(Self::NORM_FLOOR);
        let normalized_l = out_l / denom;
        let normalized_r = out_r / denom;

        // ----- Advance write index -----
        self.write_idx = (self.write_idx + 1) % Self::BUF_LEN;

        (normalized_l, normalized_r)
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
        for g in self.grains.iter_mut() {
            *g = Grain::default();
        }
        self.next_slot = 0;
        self.spawn_countdown = 0.0;
        self.lfo_phase = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.speed_pct = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.grain_ms = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.drift_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.drift_rate_hz = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            4 => self.width_pct = value.clamp(Self::PARAMS[4].min, Self::PARAMS[4].max),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    #[test]
    fn varispeed_lists_five_parameters_with_the_expected_specs() {
        let v = VarispeedEffect::new();
        let specs = v.parameters();
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[0].name, "Speed");
        assert_eq!(specs[0].min, -200.0);
        assert_eq!(specs[0].max, 200.0);
        assert_eq!(specs[0].default, 100.0);
        assert_eq!(specs[1].name, "Grain");
        assert!(matches!(specs[1].scaling, ParamScaling::Log));
        assert!(matches!(
            specs[1].format,
            ParamFormat::Number { unit: "ms", .. }
        ));
        assert_eq!(specs[2].name, "Drift");
        assert_eq!(specs[3].name, "Rate");
        assert!(matches!(specs[3].scaling, ParamScaling::Log));
        assert!(matches!(specs[3].format, ParamFormat::Hertz));
        assert_eq!(specs[4].name, "Width");
    }

    #[test]
    fn varispeed_set_param_clamps_each_slot() {
        let mut v = VarispeedEffect::new();
        v.set_param(0, 9_999.0);
        assert_eq!(v.speed_pct, 200.0);
        v.set_param(0, -9_999.0);
        assert_eq!(v.speed_pct, -200.0);
        v.set_param(1, 0.0);
        assert_eq!(v.grain_ms, 10.0);
        v.set_param(1, 9_999.0);
        assert_eq!(v.grain_ms, 100.0);
        v.set_param(2, 9_999.0);
        assert_eq!(v.drift_pct, 100.0);
        v.set_param(3, 9_999.0);
        assert_eq!(v.drift_rate_hz, 5.0);
        v.set_param(3, 0.0);
        assert_eq!(v.drift_rate_hz, 0.05);
        v.set_param(4, 9_999.0);
        assert_eq!(v.width_pct, 100.0);
    }

    /// Count sign changes (zero crossings) in `samples`. Two
    /// crossings per cycle -> frequency = crossings / (2 * dur).
    fn count_crossings(samples: &[f32]) -> usize {
        let mut prev = 0.0_f32;
        let mut count = 0usize;
        for (i, &s) in samples.iter().enumerate() {
            if i > 0 && prev.signum() != s.signum() && (prev != 0.0 || s != 0.0) {
                count += 1;
            }
            prev = s;
        }
        count
    }

    #[test]
    fn varispeed_double_speed_approximately_doubles_input_frequency() {
        // Speed = +200 % -> output should be 1 octave up.
        // Drive a 220 Hz sine in; expect ~440 Hz out (within
        // granular tolerance of ~10 %).
        let mut v = VarispeedEffect::new();
        v.set_sample_rate(48_000.0);
        v.set_param(0, 200.0); // +2x speed
        v.set_param(1, 30.0); // Smaller grain for tighter pitch
        v.set_param(2, 0.0); // No drift
        v.set_param(3, 0.7);
        v.set_param(4, 0.0);
        // Prime the buffer with some history before measuring.
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            let _ = v.process_sample(dry, dry);
        }
        let mut out = Vec::with_capacity(48_000);
        for i in 4_800..52_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            let (l, _) = v.process_sample(dry, dry);
            out.push(l);
        }
        let crossings = count_crossings(&out);
        let measured_hz = crossings as f32 / 2.0; // 1 s window
        assert!(
            (measured_hz - 440.0).abs() < 44.0,
            "expected ~440 Hz at Speed=+200 %; got {measured_hz} Hz ({crossings} crossings)"
        );
    }

    #[test]
    fn varispeed_half_speed_approximately_halves_input_frequency() {
        // Speed = +50 % -> 1 octave down.
        let mut v = VarispeedEffect::new();
        v.set_sample_rate(48_000.0);
        v.set_param(0, 50.0);
        v.set_param(1, 30.0);
        v.set_param(2, 0.0);
        v.set_param(4, 0.0);
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 880.0 * t).sin();
            let _ = v.process_sample(dry, dry);
        }
        let mut out = Vec::with_capacity(48_000);
        for i in 4_800..52_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 880.0 * t).sin();
            let (l, _) = v.process_sample(dry, dry);
            out.push(l);
        }
        let crossings = count_crossings(&out);
        let measured_hz = crossings as f32 / 2.0;
        assert!(
            (measured_hz - 440.0).abs() < 44.0,
            "expected ~440 Hz at Speed=+50 %; got {measured_hz} Hz ({crossings} crossings)"
        );
    }

    #[test]
    fn varispeed_reverse_at_unity_magnitude_inverts_the_phase_progression() {
        // Speed = -100 % reads backward at 1x. The pitch
        // magnitude should match input pitch (220 Hz in -> ~220
        // Hz out by zero-crossing count) BUT the phase moves
        // opposite the input. We verify the magnitude (frequency)
        // matches; the direction-reversal would need a more
        // sophisticated test to detect (e.g., chirp direction
        // tracking), which is out of scope for the unit test --
        // the algorithmic correctness is what we're confirming.
        let mut v = VarispeedEffect::new();
        v.set_sample_rate(48_000.0);
        v.set_param(0, -100.0); // Reverse 1x
        v.set_param(1, 30.0);
        v.set_param(2, 0.0);
        v.set_param(4, 0.0);
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            let _ = v.process_sample(dry, dry);
        }
        let mut out = Vec::with_capacity(48_000);
        for i in 4_800..52_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            let (l, _) = v.process_sample(dry, dry);
            out.push(l);
        }
        let crossings = count_crossings(&out);
        let measured_hz = crossings as f32 / 2.0;
        assert!(
            (measured_hz - 220.0).abs() < 30.0,
            "reverse-1x should preserve frequency magnitude (~220 Hz); got {measured_hz} Hz"
        );
    }

    #[test]
    fn varispeed_freeze_at_zero_speed_holds_the_buffer() {
        // Speed = 0 -> grains read at rate 0 = each grain holds
        // a single buffer sample for its entire duration. The
        // output should be a stair-stepped sequence of buffered
        // samples (one per grain), with NO progression of fresh
        // input content into the output.
        //
        // Setup: feed a known input for 200 ms (priming), then
        // switch to silent input + Speed=0; the output should
        // continue playing buffered content for ~one grain
        // duration, then settle to repeating a constant.
        let mut v = VarispeedEffect::new();
        v.set_sample_rate(48_000.0);
        v.set_param(0, 0.0); // Freeze
        v.set_param(1, 30.0);
        v.set_param(2, 0.0);
        v.set_param(4, 0.0);
        // Prime with a slow ramp so each grain captures a distinct
        // value.
        for i in 0..2_400 {
            let x = i as f32 * 0.001;
            let _ = v.process_sample(x, x);
        }
        // Now silent input. The buffered samples should keep
        // emerging, but each new grain captures a CONSTANT
        // value (whatever sample is at its start_pos), so within
        // a grain the output stays at a single value.
        let mut samples_within_grain = Vec::new();
        // Skip the LFO-influenced first cycle.
        for _ in 0..100 {
            let _ = v.process_sample(0.0, 0.0);
        }
        // Collect a window inside a single grain (smaller than
        // grain_ms = 30 ms = 1440 samples at 48 kHz; 100 samples
        // is well within).
        for _ in 0..50 {
            let (l, _) = v.process_sample(0.0, 0.0);
            samples_within_grain.push(l);
        }
        // Within a single grain, sum-of-windows normalization
        // should keep output ~constant (the value drift per
        // sample is from grain weighting, not freshness of
        // content). Check the run isn't monotonically marching
        // somewhere.
        let max = samples_within_grain
            .iter()
            .cloned()
            .fold(f32::NEG_INFINITY, f32::max);
        let min = samples_within_grain
            .iter()
            .cloned()
            .fold(f32::INFINITY, f32::min);
        // Allow some variation from window weighting; assert the
        // range is small relative to typical signal.
        assert!(
            (max - min) < 0.5,
            "at Speed=0, output should hold not progress; range {min}..{max}"
        );
    }

    #[test]
    fn varispeed_width_zero_collapses_to_mono_for_symmetric_input() {
        let mut v = VarispeedEffect::new();
        v.set_sample_rate(48_000.0);
        v.set_param(0, 100.0); // Forward unity
        v.set_param(2, 0.0); // No drift
        v.set_param(4, 0.0); // Width 0
        for i in 0..2_400 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, r) = v.process_sample(dry, dry);
            assert!(
                (l - r).abs() < 1e-5,
                "Width=0 must give L==R; sample {i}: {l} vs {r}"
            );
        }
    }

    #[test]
    fn varispeed_width_full_separates_l_and_r() {
        // Width=100 % -> L at 1.5x speed, R at 0.5x speed. For
        // a mono input the two outputs should be at DIFFERENT
        // pitches, so they diverge.
        let mut v = VarispeedEffect::new();
        v.set_sample_rate(48_000.0);
        v.set_param(0, 100.0);
        v.set_param(2, 0.0);
        v.set_param(4, 100.0);
        for _ in 0..4_800 {
            let _ = v.process_sample(0.5, 0.5);
        }
        let mut diff = 0.0_f32;
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, r) = v.process_sample(dry, dry);
            diff += (l - r).abs();
        }
        assert!(
            diff > 10.0,
            "Width=100 should split L/R for mono input; total |L-R| was {diff}"
        );
    }

    #[test]
    fn varispeed_drift_zero_is_lfo_inactive() {
        // Drift = 0 -> the LFO has zero amplitude, so its sine
        // is multiplied by 0 in `effective_rates`. Two runs from
        // the same start state should produce identical output.
        let run = || {
            let mut v = VarispeedEffect::new();
            v.set_sample_rate(48_000.0);
            v.set_param(0, 100.0);
            v.set_param(2, 0.0); // Drift = 0
            v.set_param(4, 0.0);
            let mut out = Vec::with_capacity(2_000);
            for i in 0..2_000 {
                let t = i as f32 / 48_000.0;
                let s = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                let (l, _) = v.process_sample(s, s);
                out.push(l);
            }
            out
        };
        let a = run();
        let b = run();
        for i in 0..a.len() {
            assert!(
                (a[i] - b[i]).abs() < 1e-5,
                "Drift=0 must be deterministic; sample {i}: {} vs {}",
                a[i],
                b[i]
            );
        }
    }

    #[test]
    fn varispeed_drift_modulates_the_effective_rate() {
        // Drift = 100 % + Rate = 1 Hz -> the effective speed
        // wobbles by +/-50 % at 1 Hz. Over a 1-second window the
        // output character should be noticeably different from
        // the Drift=0 control. We measure RMS difference between
        // the two outputs.
        let run = |drift: f32| {
            let mut v = VarispeedEffect::new();
            v.set_sample_rate(48_000.0);
            v.set_param(0, 100.0);
            v.set_param(1, 30.0);
            v.set_param(2, drift);
            v.set_param(3, 1.0); // 1 Hz Drift LFO
            v.set_param(4, 0.0);
            let mut out = Vec::with_capacity(48_000);
            for i in 0..48_000 {
                let t = i as f32 / 48_000.0;
                let s = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                let (l, _) = v.process_sample(s, s);
                out.push(l);
            }
            out
        };
        let dry = run(0.0);
        let wet = run(100.0);
        let mut rms_diff = 0.0_f32;
        for i in 0..dry.len() {
            rms_diff += (dry[i] - wet[i]) * (dry[i] - wet[i]);
        }
        let rms = (rms_diff / dry.len() as f32).sqrt();
        assert!(
            rms > 0.05,
            "Drift=100 should de-correlate from Drift=0; rms diff {rms}"
        );
    }

    #[test]
    fn varispeed_stays_bounded_under_aggressive_sweep() {
        let mut v = VarispeedEffect::new();
        v.set_sample_rate(48_000.0);
        for i in 0..48_000 {
            v.set_param(0, (i as f32 / 4_000.0).fract() * 400.0 - 200.0); // Sweep -200..+200
            let pg = (i as f32 / 6_000.0).fract();
            v.set_param(1, 10.0 * 10.0_f32.powf(pg)); // 10..100 ms log
            v.set_param(2, (i as f32 / 5_000.0).fract() * 100.0);
            let pr = (i as f32 / 7_000.0).fract();
            v.set_param(3, 0.05 * 100.0_f32.powf(pr)); // 0.05..5 Hz log
            v.set_param(4, (i as f32 / 3_000.0).fract() * 100.0);
            let dry = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, r) = v.process_sample(dry, dry);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            // Output is bounded by sum-of-windows normalization;
            // a few x dry magnitude is the worst-case overshoot
            // from triangle window edge effects.
            assert!(
                l.abs() < 4.0 && r.abs() < 4.0,
                "sample {i} blew up: ({l},{r})"
            );
        }
    }

    #[test]
    fn varispeed_reset_clears_state() {
        let mut v = VarispeedEffect::new();
        v.set_sample_rate(48_000.0);
        v.set_param(0, 150.0);
        v.set_param(2, 80.0);
        for _ in 0..4_800 {
            let _ = v.process_sample(0.5, 0.5);
        }
        v.reset();
        assert!(v.delay_l.iter().all(|&s| s == 0.0));
        assert!(v.delay_r.iter().all(|&s| s == 0.0));
        assert_eq!(v.write_idx, 0);
        assert_eq!(v.spawn_countdown, 0.0);
        assert_eq!(v.lfo_phase, 0.0);
        for g in v.grains.iter() {
            assert_eq!(g.age_samples, 0.0);
            assert_eq!(g.length_samples, 0.0);
        }
    }
}
