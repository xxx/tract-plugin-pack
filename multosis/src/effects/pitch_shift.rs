use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Granular pitch shifter (PSOLA-style). Input feeds a stereo ring
/// buffer; up to `MAX_GRAINS` concurrent grains read from it at the
/// pitch-shifted resampling rate, windowed with a triangle envelope
/// and summed with per-sample sum-of-windows normalization so the
/// output level stays stable as grain overlap changes.
///
/// Each grain locks its `rate` and `length` at spawn time so a
/// per-sample MSEG modulation of Pitch / Size doesn't corrupt
/// grains already in flight -- the modulation only affects future
/// spawns. Result: pitch sweeps stay glitch-free, and the sound
/// gracefully transitions across parameter changes.
///
/// **Pitch** spans -24 to +24 semitones (4 octaves total). Continuous
/// so cent-precision detune is reachable through the same dial.
///
/// **Frequency** sets how often new grains spawn (5..100 Hz). Higher
/// = denser, smoother; lower = sparser, more individual grains
/// audible.
///
/// **Size** sets each grain's duration (10..100 ms). Short grains
/// preserve transients but introduce coarser pitch; long grains
/// give smoother pitch but smear transients.
///
/// **Feedback** routes the wet output back into the ring buffer
/// (capped at +/-95 % for loop stability). At positive Pitch this
/// produces Shepard-style cascading shifts (each recirculation adds
/// another shift); negative Feedback inverts the recirculated phase
/// for sharper comb-style cancellations.
///
/// **Detune** spreads L and R pitch by +/-half the value in cents
/// (so at 50 cents the L/R total is a 50-cent gap). Unison chorus
/// character without needing a second effect slot.
///
/// **No PDC.** The wet path has internal latency from the lookbehind
/// (up to ~400 ms at +24 semi / 100 ms size) but each grain emerges
/// in real time; the engine's per-track Mix combines wet against
/// instantaneous dry, so PDC would time-shift dry against wet and
/// kill the effect. **Per-sample work:** ~16 grains x (1 triangle
/// window + 2 fractional reads + accumulate) + 1 division for the
/// normalization. Bounded; no transcendentals.
pub struct PitchShiftEffect {
    pitch_semi: f32,
    grain_freq_hz: f32,
    grain_size_ms: f32,
    feedback_pct: f32,
    detune_cents: f32,
    sample_rate: f32,

    /// Stereo ring buffer fed with `dry + feedback*wet_prev` every
    /// sample. Sized for the worst-case grain demand at 192 kHz.
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    write_idx: usize,

    /// Active grain pool. New spawns hunt for a dead slot
    /// (round-robin from `next_slot`); if all 16 are alive the
    /// new grain overwrites the oldest. With Frequency capped at
    /// 100 Hz x Size capped at 100 ms = 10 max concurrent grains,
    /// 16 slots leave headroom.
    grains: [Grain; Self::MAX_GRAINS],
    next_slot: usize,

    /// Samples until the next grain spawn. Decremented per sample;
    /// when <= 0 a grain spawns and the countdown resets to the
    /// next interval (sample_rate / grain_freq_hz).
    spawn_countdown: f32,

    /// Previous sample's wet output, recirculated as feedback.
    /// The 1-sample delay keeps the feedback loop strictly causal.
    fb_l: f32,
    fb_r: f32,

    /// Cached `2^(pitch_semi / 12)` -- the resampling rate ratio.
    /// Recomputed in `set_param` when Pitch changes, so the per-
    /// sample path is free of `exp2` calls (similar to the gain
    /// caching pattern in Distortion / Satch / Bitcrush).
    pitch_rate: f32,
    /// Cached half-detune ratio `2^(detune_cents / 2 / 1200)` --
    /// applied as `pitch_rate / detune_ratio` to L and
    /// `pitch_rate * detune_ratio` to R.
    detune_ratio: f32,
}

/// One in-flight grain. Locked at spawn time -- changing Pitch /
/// Size mid-grain doesn't disturb this grain's rate or length.
#[derive(Clone, Copy, Default)]
struct Grain {
    /// Absolute initial ring-buffer position (in samples, not yet
    /// taken modulo the buffer length). The grain reads at
    /// `start_pos + age_samples * rate_l/r`.
    start_pos: f32,
    /// Output samples produced since spawn. The grain is "dead"
    /// when `age_samples >= length_samples`.
    age_samples: f32,
    length_samples: f32,
    rate_l: f32,
    rate_r: f32,
}

impl PitchShiftEffect {
    const MAX_GRAINS: usize = 16;
    const PITCH_MIN_SEMI: f32 = -24.0;
    const PITCH_MAX_SEMI: f32 = 24.0;
    const GRAIN_FREQ_MIN_HZ: f32 = 5.0;
    const GRAIN_FREQ_MAX_HZ: f32 = 100.0;
    const GRAIN_SIZE_MIN_MS: f32 = 10.0;
    const GRAIN_SIZE_MAX_MS: f32 = 100.0;
    const DETUNE_MAX_CENTS: f32 = 50.0;
    const FB_CAP: f32 = 0.95;

    /// Ring-buffer length. Worst case: +24 semi (rate = 4) over a
    /// 100 ms grain reads 400 ms of input; at 192 kHz that's
    /// 76 800 samples. Round up for clean modulo math.
    const BUF_LEN: usize = 96_000;

    /// Floor on the normalization sum-of-windows. Keeps the divisor
    /// from going to zero during startup (1-2 grains active) or
    /// extreme silence; the floor is small enough that real-program
    /// overlap (~2 grains at 50 % each = sum 1.0) is unaffected.
    const NORM_FLOOR: f32 = 0.01;

    const PARAMS: [ParamSpec; 5] = [
        ParamSpec {
            name: "Pitch",
            min: Self::PITCH_MIN_SEMI,
            max: Self::PITCH_MAX_SEMI,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "st",
            },
        },
        ParamSpec {
            name: "Frequency",
            min: Self::GRAIN_FREQ_MIN_HZ,
            max: Self::GRAIN_FREQ_MAX_HZ,
            default: 30.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Size",
            min: Self::GRAIN_SIZE_MIN_MS,
            max: Self::GRAIN_SIZE_MAX_MS,
            default: 50.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "ms",
            },
        },
        ParamSpec {
            name: "Feedback",
            min: -100.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Detune",
            min: 0.0,
            max: Self::DETUNE_MAX_CENTS,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: " ct",
            },
        },
    ];

    pub fn new() -> Self {
        let pitch_semi = Self::PARAMS[0].default;
        let detune_cents = Self::PARAMS[4].default;
        Self {
            pitch_semi,
            grain_freq_hz: Self::PARAMS[1].default,
            grain_size_ms: Self::PARAMS[2].default,
            feedback_pct: Self::PARAMS[3].default,
            detune_cents,
            sample_rate: 48_000.0,
            delay_l: vec![0.0; Self::BUF_LEN],
            delay_r: vec![0.0; Self::BUF_LEN],
            write_idx: 0,
            grains: [Grain::default(); Self::MAX_GRAINS],
            next_slot: 0,
            spawn_countdown: 0.0,
            fb_l: 0.0,
            fb_r: 0.0,
            pitch_rate: (pitch_semi / 12.0).exp2(),
            detune_ratio: (detune_cents * 0.5 / 1200.0).exp2(),
        }
    }

    /// Triangle window: 0 at age = 0, 1 at age = length/2, 0 at
    /// age = length. Two cheap multiplies + a min, no transcendental.
    /// Practically indistinguishable from Hann for granular pitch
    /// shifting and noticeably faster across 16 concurrent grains.
    #[inline]
    fn triangle_window(age: f32, length: f32) -> f32 {
        let phase = (age / length).clamp(0.0, 1.0);
        if phase < 0.5 {
            2.0 * phase
        } else {
            2.0 - 2.0 * phase
        }
    }

    /// Fractional ring-buffer read with linear interpolation. `pos`
    /// is an absolute (unwrapped) sample index; `rem_euclid` wraps
    /// it into `[0, n)` for correct indexing under both positive
    /// and negative `pos` (start_pos can go negative during
    /// lookbehind calculation).
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

    /// Allocate a slot from the grain pool and initialize it with
    /// the current pitch/size/detune values. Locking the rate and
    /// length at spawn time means MSEG-modulating Pitch or Size
    /// only affects FUTURE grains, not the in-flight ones --
    /// crucial for glitch-free modulation.
    fn spawn_grain(&mut self) {
        let length_samples = (self.grain_size_ms * 0.001 * self.sample_rate).max(2.0);
        // Lookbehind: at upward shifts the grain reads faster than
        // the write head, so we need to start `length * rate`
        // samples behind it. At downward shifts a small lookbehind
        // is enough -- the read trails the write naturally.
        let lookbehind = (length_samples * self.pitch_rate).max(1.0);
        let start_pos = self.write_idx as f32 - lookbehind;

        // Find a dead slot, round-robin from next_slot. If all 16
        // are alive (extreme density), the oldest slot at next_slot
        // gets overwritten -- preferable to dropping a beat in the
        // grain stream entirely.
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
            rate_l: self.pitch_rate / self.detune_ratio,
            rate_r: self.pitch_rate * self.detune_ratio,
        };
        self.next_slot = (slot_idx + 1) % Self::MAX_GRAINS;
    }
}

impl Default for PitchShiftEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for PitchShiftEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let feedback = (self.feedback_pct * 0.01).clamp(-Self::FB_CAP, Self::FB_CAP);

        // ----- Stage 1: write dry + feedback into the ring -----
        self.delay_l[self.write_idx] = left + feedback * self.fb_l;
        self.delay_r[self.write_idx] = right + feedback * self.fb_r;

        // ----- Stage 2: maybe spawn a new grain -----
        self.spawn_countdown -= 1.0;
        if self.spawn_countdown <= 0.0 {
            self.spawn_grain();
            // Reset countdown to the inverse of the grain frequency
            // (samples-per-grain). Add to current countdown rather
            // than assigning so a slight over-countdown carries over.
            self.spawn_countdown += self.sample_rate / self.grain_freq_hz;
        }

        // ----- Stage 3: sum windowed grain reads -----
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

        // Sum-of-windows normalization. The floor keeps the divisor
        // bounded during startup or when grains lapse momentarily.
        let denom = sum_w.max(Self::NORM_FLOOR);
        let normalized_l = out_l / denom;
        let normalized_r = out_r / denom;

        // ----- Stage 4: advance write index, store feedback -----
        self.write_idx = (self.write_idx + 1) % Self::BUF_LEN;
        self.fb_l = normalized_l;
        self.fb_r = normalized_r;

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
        self.fb_l = 0.0;
        self.fb_r = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => {
                self.pitch_semi = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max);
                self.pitch_rate = (self.pitch_semi / 12.0).exp2();
            }
            1 => self.grain_freq_hz = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.grain_size_ms = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.feedback_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            4 => {
                self.detune_cents = value.clamp(Self::PARAMS[4].min, Self::PARAMS[4].max);
                self.detune_ratio = (self.detune_cents * 0.5 / 1200.0).exp2();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    #[test]
    fn pitch_shift_lists_five_parameters_with_the_expected_specs() {
        let p = PitchShiftEffect::new();
        let specs = p.parameters();
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[0].name, "Pitch");
        assert_eq!(specs[0].min, -24.0);
        assert_eq!(specs[0].max, 24.0);
        assert_eq!(specs[1].name, "Frequency");
        assert!(matches!(specs[1].scaling, ParamScaling::Log));
        assert!(matches!(specs[1].format, ParamFormat::Hertz));
        assert_eq!(specs[2].name, "Size");
        assert!(matches!(specs[2].scaling, ParamScaling::Log));
        assert!(matches!(
            specs[2].format,
            ParamFormat::Number { unit: "ms", .. }
        ));
        assert_eq!(specs[3].name, "Feedback");
        assert_eq!(specs[3].min, -100.0);
        assert_eq!(specs[3].max, 100.0);
        assert_eq!(specs[4].name, "Detune");
        assert_eq!(specs[4].min, 0.0);
        assert_eq!(specs[4].max, 50.0);
    }

    #[test]
    fn pitch_shift_set_param_clamps_each_slot() {
        let mut p = PitchShiftEffect::new();
        p.set_param(0, 999.0);
        assert_eq!(p.pitch_semi, 24.0);
        p.set_param(0, -999.0);
        assert_eq!(p.pitch_semi, -24.0);
        // Cached rate should track the clamped value.
        let expected_rate = (-24.0_f32 / 12.0).exp2();
        assert!((p.pitch_rate - expected_rate).abs() < 1e-5);
        p.set_param(1, 999.0);
        assert_eq!(p.grain_freq_hz, 100.0);
        p.set_param(1, 0.0);
        assert_eq!(p.grain_freq_hz, 5.0);
        p.set_param(2, 0.0);
        assert_eq!(p.grain_size_ms, 10.0);
        p.set_param(2, 999.0);
        assert_eq!(p.grain_size_ms, 100.0);
        p.set_param(3, 999.0);
        assert_eq!(p.feedback_pct, 100.0);
        p.set_param(4, 999.0);
        assert_eq!(p.detune_cents, 50.0);
        p.set_param(4, -10.0);
        assert_eq!(p.detune_cents, 0.0);
    }

    /// Count sign changes (zero crossings) in `samples`. Two zero
    /// crossings per cycle, so frequency_hz = crossings / (2 * dur).
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
    fn pitch_shift_up_octave_approximately_doubles_input_frequency() {
        // Drive a 220 Hz sine in, pitch up 12 semitones, and verify
        // the output's zero-crossing density matches ~440 Hz to
        // within a granular shifter's tolerance (~10 %).
        let mut p = PitchShiftEffect::new();
        p.set_sample_rate(48_000.0);
        p.set_param(0, 12.0); // +1 octave
        p.set_param(1, 50.0); // 50 Hz grain rate
        p.set_param(2, 40.0); // 40 ms grains
        p.set_param(3, 0.0);
        p.set_param(4, 0.0);
        // Prime: let the first few grains start emitting.
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            let _ = p.process_sample(dry, dry);
        }
        // Measure 1 s of output.
        let mut out = Vec::with_capacity(48_000);
        for i in 4_800..52_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            let (l, _) = p.process_sample(dry, dry);
            out.push(l);
        }
        let crossings = count_crossings(&out);
        let measured_hz = crossings as f32 / 2.0; // 1 s window
                                                  // Expected 440 Hz +/- 10 % for granular artifacts at the
                                                  // grain boundaries.
        assert!(
            (measured_hz - 440.0).abs() < 44.0,
            "expected ~440 Hz, got {measured_hz} Hz ({crossings} crossings/s)"
        );
    }

    #[test]
    fn pitch_shift_down_octave_approximately_halves_input_frequency() {
        // Same idea, pitch down 12 semitones -> 220 Hz becomes 110 Hz.
        let mut p = PitchShiftEffect::new();
        p.set_sample_rate(48_000.0);
        p.set_param(0, -12.0);
        p.set_param(1, 50.0);
        p.set_param(2, 40.0);
        p.set_param(3, 0.0);
        p.set_param(4, 0.0);
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let _ = p.process_sample(dry, dry);
        }
        let mut out = Vec::with_capacity(48_000);
        for i in 4_800..52_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, _) = p.process_sample(dry, dry);
            out.push(l);
        }
        let crossings = count_crossings(&out);
        let measured_hz = crossings as f32 / 2.0;
        // Expected 220 Hz +/- 10 %.
        assert!(
            (measured_hz - 220.0).abs() < 22.0,
            "expected ~220 Hz, got {measured_hz} Hz ({crossings} crossings/s)"
        );
    }

    #[test]
    fn pitch_shift_zero_passes_audio_through_at_input_frequency() {
        // Pitch=0 (rate=1) is identity up to grain-window artifacts.
        // Verify output frequency matches input.
        let mut p = PitchShiftEffect::new();
        p.set_sample_rate(48_000.0);
        p.set_param(0, 0.0);
        p.set_param(1, 50.0);
        p.set_param(2, 40.0);
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let _ = p.process_sample(dry, dry);
        }
        let mut out = Vec::with_capacity(48_000);
        for i in 4_800..52_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, _) = p.process_sample(dry, dry);
            out.push(l);
        }
        let crossings = count_crossings(&out);
        let measured_hz = crossings as f32 / 2.0;
        assert!(
            (measured_hz - 440.0).abs() < 44.0,
            "Pitch=0 should produce ~440 Hz, got {measured_hz} Hz"
        );
    }

    #[test]
    fn pitch_shift_silent_input_eventually_decays_to_silence() {
        // With feedback at the cap, silent input must still decay
        // -- the loop is bounded by FB_CAP. The decay rate here is
        // governed by the GRAIN cycle, not per-sample: each grain
        // length (default 50 ms) the feedback multiplies once, so
        // RT60 ~ (-3 / log(0.95)) * 50 ms ~ 2.9 s. At 10 s we expect
        // ~3.5 RT60s of decay -> residual << 1e-3.
        let mut p = PitchShiftEffect::new();
        p.set_sample_rate(48_000.0);
        p.set_param(0, 0.0);
        p.set_param(3, 95.0); // Feedback at the cap
        let _ = p.process_sample(1.0, 1.0);
        for _ in 0..480_000 {
            let _ = p.process_sample(0.0, 0.0);
        }
        let mut peak = 0.0_f32;
        for _ in 0..1_024 {
            let (l, r) = p.process_sample(0.0, 0.0);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(
            peak < 1e-3,
            "feedback loop must decay below 1e-3 at 10 s; peak={peak}"
        );
    }

    #[test]
    fn pitch_shift_detune_separates_l_and_r_on_mono_input() {
        // Detune != 0 means L and R rates differ -> their outputs
        // drift in phase relative to each other over time -> the
        // L/R difference is no longer zero for a mono input.
        let mut p = PitchShiftEffect::new();
        p.set_sample_rate(48_000.0);
        p.set_param(0, 0.0);
        p.set_param(1, 50.0);
        p.set_param(2, 40.0);
        p.set_param(4, 50.0); // Max detune
        for _ in 0..4_800 {
            let _ = p.process_sample(0.5, 0.5);
        }
        let mut diff = 0.0_f32;
        for i in 0..48_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, r) = p.process_sample(dry, dry);
            diff += (l - r).abs();
        }
        assert!(
            diff > 10.0,
            "Detune=50 ct should separate L/R on a mono input; total |L-R| was {diff}"
        );
    }

    #[test]
    fn pitch_shift_detune_zero_collapses_to_mono() {
        // Detune=0 -> rate_l == rate_r -> L == R for an L=R input.
        let mut p = PitchShiftEffect::new();
        p.set_sample_rate(48_000.0);
        p.set_param(0, 7.0); // Some pitch shift to exercise grains
        p.set_param(1, 50.0);
        p.set_param(2, 40.0);
        p.set_param(4, 0.0);
        for i in 0..2_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, r) = p.process_sample(dry, dry);
            assert!(
                (l - r).abs() < 1e-5,
                "Detune=0 with L=R input must give L==R; sample {i}: {l} vs {r}"
            );
        }
    }

    #[test]
    fn pitch_shift_feedback_amplifies_wet_energy() {
        // Compare wet energy decay after an impulse: feedback=0 vs
        // feedback=70. The high-feedback case should sustain more.
        let measure = |fb: f32| {
            let mut p = PitchShiftEffect::new();
            p.set_sample_rate(48_000.0);
            p.set_param(0, 5.0); // Some pitch shift to mark feedback re-entries
            p.set_param(1, 50.0);
            p.set_param(2, 40.0);
            p.set_param(3, fb);
            let _ = p.process_sample(1.0, 1.0);
            let mut e = 0.0_f32;
            // Skip the first 50 ms (grain priming) then measure tail.
            for _ in 0..2_400 {
                let _ = p.process_sample(0.0, 0.0);
            }
            for _ in 0..9_600 {
                let (l, _) = p.process_sample(0.0, 0.0);
                e += l * l;
            }
            e
        };
        let e0 = measure(0.0);
        let e70 = measure(70.0);
        assert!(
            e70 > e0 * 1.5,
            "high feedback should sustain wet; fb=0:{e0}, fb=70:{e70}"
        );
    }

    #[test]
    fn pitch_shift_stays_bounded_under_aggressive_sweep() {
        let mut p = PitchShiftEffect::new();
        p.set_sample_rate(48_000.0);
        for i in 0..48_000 {
            // Sweep every parameter
            p.set_param(0, (i as f32 / 2_000.0).fract() * 48.0 - 24.0);
            let pf = (i as f32 / 4_096.0).fract();
            p.set_param(1, 5.0 * 20.0_f32.powf(pf)); // 5..100 Hz log
            let ps = (i as f32 / 6_000.0).fract();
            p.set_param(2, 10.0 * 10.0_f32.powf(ps)); // 10..100 ms log
            p.set_param(3, (i as f32 / 3_000.0).fract() * 200.0 - 100.0);
            p.set_param(4, (i as f32 / 5_000.0).fract() * 50.0);
            let x = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, r) = p.process_sample(x, x);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            assert!(
                l.abs() < 4.0 && r.abs() < 4.0,
                "sample {i} blew up: ({l},{r})"
            );
        }
    }

    #[test]
    fn pitch_shift_reset_clears_state() {
        let mut p = PitchShiftEffect::new();
        p.set_sample_rate(48_000.0);
        p.set_param(0, 12.0);
        p.set_param(3, 70.0);
        for _ in 0..4_800 {
            let _ = p.process_sample(0.5, 0.5);
        }
        p.reset();
        assert!(p.delay_l.iter().all(|&v| v == 0.0));
        assert!(p.delay_r.iter().all(|&v| v == 0.0));
        assert_eq!(p.write_idx, 0);
        assert_eq!(p.fb_l, 0.0);
        assert_eq!(p.fb_r, 0.0);
        for g in p.grains.iter() {
            assert_eq!(g.age_samples, 0.0);
            assert_eq!(g.length_samples, 0.0);
        }
    }
}
