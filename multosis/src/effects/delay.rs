use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Tempo-syncable delay with feedback and ducking.
///
/// **Time** is a single Enum-format dropdown spanning every musical
/// subdivision (with dotted variants) plus a `Free` slot. When `Free` is
/// selected, the `Free` ms dial sets the delay time directly; otherwise
/// the subdivision is converted to samples using the host's current BPM
/// (cached via `set_bpm`).
///
/// **Feedback** scales the delay-line output back into its input
/// (chorus-style feedback, capped at 0.95 to prevent runaway).
///
/// **Duck** envelope-follows the input level and attenuates the delayed
/// output proportionally — at full Duck, loud input fully silences the
/// delay, leaving the dry signal clear; the delay swells in as the
/// input drops. Same mechanism Bitwig Delay+ uses.
///
/// Output is **additive**: `out = dry + delayed · duck_factor`. The
/// per-row Mix then controls how much of the delayed signal mixes in,
/// matching standard delay-plugin semantics where the dry signal
/// always passes through.
pub struct DelayEffect {
    // Stored parameters.
    time_idx: f32,     // 0..14: subdivisions 0..13 + Free at 14.
    free_ms: f32,      // Used only when time_idx selects Free.
    feedback_pct: f32, // 0..100.
    duck_pct: f32,     // 0..100; 0 = no ducking, 100 = aggressive duck.
    sample_rate: f32,
    bpm: f32,
    /// Per-channel circular delay buffers, sized for the worst-case
    /// 2-second delay at 192 kHz. Allocated once in `new`; reads and
    /// writes are wrap-around with linear-interp fractional reads.
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    write_idx: usize,
    /// Duck envelope follower over the input (rectified-stereo). Cached
    /// coefficients are recomputed in `set_sample_rate`.
    duck_env: f32,
    duck_attack_coef: f32,
    duck_release_coef: f32,
}

/// Time-dropdown labels — order matches `delay_time_beats()`. The last
/// entry, "Free", is special: it tells `process_sample` to use the
/// `free_ms` dial instead of computing samples from BPM.
const DELAY_TIME_LABELS: &[&str] = &[
    "1/64", "1/64.", "1/32", "1/32.", "1/16", "1/16.", "1/8", "1/8.", "1/4", "1/4.", "1/2", "1/2.",
    "1/1", "1/1.", "Free",
];

/// Beat count for time-dropdown index `idx` in the sync slots (0..14).
/// Sub-1 beat for shorter subdivisions, > 1 beat for half/whole notes.
/// Returns `None` for the Free slot (index 14) — the caller switches
/// over to the `Free` dial in that case.
fn delay_time_beats(idx: usize) -> Option<f32> {
    // Whole note = 4 beats in 4/4 time; each smaller subdivision halves.
    // Dotted = 1.5 ×. Index pairs are (straight, dotted) per row.
    let beats = match idx {
        0 => 4.0 / 64.0,       // 1/64
        1 => 1.5 * 4.0 / 64.0, // 1/64.
        2 => 4.0 / 32.0,       // 1/32
        3 => 1.5 * 4.0 / 32.0, // 1/32.
        4 => 4.0 / 16.0,       // 1/16
        5 => 1.5 * 4.0 / 16.0, // 1/16.
        6 => 0.5,              // 1/8
        7 => 0.75,             // 1/8.
        8 => 1.0,              // 1/4
        9 => 1.5,              // 1/4.
        10 => 2.0,             // 1/2
        11 => 3.0,             // 1/2.
        12 => 4.0,             // 1/1 (whole)
        13 => 6.0,             // 1/1.
        _ => return None,      // Free
    };
    Some(beats)
}

impl DelayEffect {
    /// Worst-case sample count = 2 seconds × 192 kHz.
    const BUF_LEN: usize = (2.0 * 192_000.0) as usize;
    const MAX_DELAY_MS: f32 = 2_000.0;
    const MIN_DELAY_MS: f32 = 1.0;
    /// Duck envelope follower time constants — same shape as the FM
    /// effect's input gate: fast attack so transients duck the delay
    /// immediately, slow release so the delay swells back smoothly.
    const DUCK_ATTACK_MS: f32 = 5.0;
    const DUCK_RELEASE_MS: f32 = 150.0;
    const FEEDBACK_CAP: f32 = 0.95;

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Free",
            min: Self::MIN_DELAY_MS,
            max: Self::MAX_DELAY_MS,
            default: 250.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "ms",
            },
        },
        ParamSpec {
            name: "Time",
            min: 0.0,
            max: (DELAY_TIME_LABELS.len() - 1) as f32,
            // Default to the trailing `Free` slot so a fresh delay
            // uses the (continuous) Free dial directly; the user can
            // switch to a tempo-synced subdivision via the dropdown.
            // This also makes the default MSEG target (`Some(0)` →
            // slot 0 = Free) modulate a useful continuous parameter
            // rather than rhythmically switching subdivisions.
            default: (DELAY_TIME_LABELS.len() - 1) as f32,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: DELAY_TIME_LABELS,
            },
        },
        ParamSpec {
            name: "Feedback",
            min: 0.0,
            max: 100.0,
            default: 30.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Duck",
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
        let mut d = Self {
            free_ms: Self::PARAMS[0].default,
            time_idx: Self::PARAMS[1].default,
            feedback_pct: Self::PARAMS[2].default,
            duck_pct: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            bpm: 120.0,
            delay_l: vec![0.0; Self::BUF_LEN],
            delay_r: vec![0.0; Self::BUF_LEN],
            write_idx: 0,
            duck_env: 0.0,
            duck_attack_coef: 0.0,
            duck_release_coef: 0.0,
        };
        d.recompute_duck_coefs();
        d
    }

    fn recompute_duck_coefs(&mut self) {
        let sr = self.sample_rate.max(1.0);
        self.duck_attack_coef = (-1.0 / (Self::DUCK_ATTACK_MS * 0.001 * sr)).exp();
        self.duck_release_coef = (-1.0 / (Self::DUCK_RELEASE_MS * 0.001 * sr)).exp();
    }

    /// Current delay time in samples, given the cached BPM + sample rate.
    /// Sync subdivisions consult `delay_time_beats`; the Free slot uses
    /// the `Free` ms dial. Clamped to `[1, BUF_LEN − 2]` so the read tap
    /// never wraps onto the write head (which would feed back the
    /// just-written sample = unstable).
    fn delay_samples(&self) -> f32 {
        let idx = self.time_idx.round() as usize;
        let raw_samples = if let Some(beats) = delay_time_beats(idx) {
            let sec_per_beat = 60.0 / self.bpm.max(1.0);
            beats * sec_per_beat * self.sample_rate
        } else {
            self.free_ms * 0.001 * self.sample_rate
        };
        raw_samples.clamp(1.0, (Self::BUF_LEN - 2) as f32)
    }

    /// Linear-interp delay read: returns the sample `delay_samples`
    /// behind the current write head from `buf`.
    fn read_delay(buf: &[f32], write_idx: usize, delay_samples: f32) -> f32 {
        let n = buf.len();
        let read = (write_idx as f32 + n as f32 - delay_samples).rem_euclid(n as f32);
        let i0 = (read.floor() as usize) % n;
        let i1 = (i0 + 1) % n;
        let frac = read - read.floor();
        buf[i0] * (1.0 - frac) + buf[i1] * frac
    }
}

impl Default for DelayEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for DelayEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let delay = self.delay_samples();
        let fb = (self.feedback_pct * 0.01).clamp(0.0, Self::FEEDBACK_CAP);
        let duck_amount = (self.duck_pct * 0.01).clamp(0.0, 1.0);

        // Read the delay tap BEFORE writing, so feedback uses the past
        // value (not the one we're about to write).
        let tap_l = Self::read_delay(&self.delay_l, self.write_idx, delay);
        let tap_r = Self::read_delay(&self.delay_r, self.write_idx, delay);

        // Update the duck envelope from the current input level.
        let target_env = (left.abs() + right.abs()) * 0.5;
        let coef = if target_env > self.duck_env {
            self.duck_attack_coef
        } else {
            self.duck_release_coef
        };
        self.duck_env = target_env + (self.duck_env - target_env) * coef;
        // `1 - duck_amount · env` keeps the delay at full level when env
        // is silent, attenuates by up to `duck_amount` when env is at
        // peak. Clamped to ≥ 0 so a hot input doesn't invert.
        let duck_factor = (1.0 - duck_amount * self.duck_env).max(0.0);

        // Write input + feedback × tap to the buffer; the next call's
        // tap reads from here.
        self.delay_l[self.write_idx] = left + fb * tap_l;
        self.delay_r[self.write_idx] = right + fb * tap_r;
        self.write_idx = (self.write_idx + 1) % self.delay_l.len();

        // Additive output: dry + ducked delay tap.
        let out_l = left + duck_factor * tap_l;
        let out_r = right + duck_factor * tap_r;
        (out_l, out_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute_duck_coefs();
    }

    fn reset(&mut self) {
        for s in self.delay_l.iter_mut() {
            *s = 0.0;
        }
        for s in self.delay_r.iter_mut() {
            *s = 0.0;
        }
        self.write_idx = 0;
        self.duck_env = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.free_ms = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => {
                let max_idx = (DELAY_TIME_LABELS.len() - 1) as f32;
                self.time_idx = value.round().clamp(0.0, max_idx);
            }
            2 => self.feedback_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.duck_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => {}
        }
    }

    fn set_bpm(&mut self, bpm: f32) {
        self.bpm = bpm.max(1.0);
    }

    /// The Free dial (slot 0) is dimmed whenever the Time dropdown points
    /// at a tempo-synced subdivision — Time then drives `delay_samples()`
    /// directly and the Free value is unused. The Free slot itself (the
    /// last `time_idx` value) re-enables it. Every other slot is always
    /// active.
    fn param_dimmed(&self, index: usize) -> bool {
        if index != 0 {
            return false;
        }
        let idx = self.time_idx.round() as usize;
        delay_time_beats(idx).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    #[test]
    fn delay_effect_lists_four_parameters_with_the_expected_specs() {
        let d = DelayEffect::new();
        let specs = d.parameters();
        assert_eq!(specs.len(), 4);
        // Free at slot 0 so a fresh delay uses the continuous ms knob,
        // and the default MSEG target (Some(0)) modulates a useful
        // continuous param rather than rhythmically switching sync
        // subdivisions.
        assert_eq!(specs[0].name, "Free");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert_eq!(specs[1].name, "Time");
        assert!(matches!(specs[1].format, ParamFormat::Enum { .. }));
        // Default Time = the trailing "Free" slot, so the dropdown
        // out of the box defers to the Free ms dial.
        let max_time_idx = (DELAY_TIME_LABELS.len() - 1) as f32;
        assert_eq!(specs[1].default, max_time_idx);
        assert_eq!(specs[2].name, "Feedback");
        assert_eq!(specs[3].name, "Duck");
    }

    #[test]
    fn delay_free_time_produces_a_delayed_echo_of_the_input() {
        // Free-mode at 100 ms, feedback = 0, duck = 0. Drive an impulse,
        // then expect a single echo ~100 ms (= 4800 samples at 48 kHz)
        // later as `output - input`. Additive output: out = dry + delayed.
        let mut d = DelayEffect::new();
        d.set_sample_rate(48_000.0);
        d.set_param(0, 100.0); // Free = 100 ms
        d.set_param(1, 14.0); // Time → Free
        d.set_param(2, 0.0); // Feedback = 0
        d.set_param(3, 0.0); // Duck = 0
                             // Impulse at sample 0, then silence — count where the echo lands.
        let (l, _r) = d.process_sample(1.0, 1.0);
        // At t=0 the input is dry + (whatever the delay tap reads from
        // the still-empty buffer), so just the dry pass-through.
        assert!(
            (l - 1.0).abs() < 1e-6,
            "t=0 output should be the dry impulse"
        );
        let mut echo_peak_idx = 0usize;
        let mut echo_peak_val = 0.0_f32;
        for i in 1..12_000 {
            let (l, _r) = d.process_sample(0.0, 0.0);
            if l.abs() > echo_peak_val {
                echo_peak_val = l.abs();
                echo_peak_idx = i;
            }
        }
        // 100 ms @ 48 kHz = 4 800 samples; with linear-interp reads
        // the peak lands within a sample of that.
        assert!(
            echo_peak_idx >= 4_795 && echo_peak_idx <= 4_805,
            "echo peak should land near sample 4800, got {echo_peak_idx}"
        );
        assert!(
            echo_peak_val > 0.95,
            "echo should preserve the impulse level at fb=0, got {echo_peak_val}"
        );
    }

    #[test]
    fn delay_feedback_extends_the_echo_train() {
        // Higher feedback = more decaying repeats. Drive an impulse and
        // sum |output| over a long window; total energy must grow
        // monotonically with feedback and stay bounded at the cap.
        let render_energy = |fb_pct: f32| -> f32 {
            let mut d = DelayEffect::new();
            d.set_sample_rate(48_000.0);
            d.set_param(0, 50.0); // Free = 50 ms — quick echoes for a short test
            d.set_param(1, 14.0); // Time → Free
            d.set_param(2, fb_pct);
            d.set_param(3, 0.0);
            let mut energy = 0.0_f32;
            let (l, _r) = d.process_sample(1.0, 1.0);
            energy += l.abs();
            for _ in 0..48_000 {
                // 1 second window
                let (l, _r) = d.process_sample(0.0, 0.0);
                energy += l.abs();
            }
            energy
        };
        let e0 = render_energy(0.0);
        let e50 = render_energy(50.0);
        let e90 = render_energy(90.0);
        assert!(
            e50 > e0 * 1.3,
            "50 % feedback should add tail energy beyond fb=0 (e0={e0}, e50={e50})"
        );
        assert!(
            e90 > e50 * 1.3,
            "90 % feedback should add yet more (e50={e50}, e90={e90})"
        );
        assert!(
            e90.is_finite() && e90 < 1e6,
            "feedback at the cap stays bounded (e90={e90})"
        );
    }

    #[test]
    fn delay_duck_attenuates_the_echo_while_input_is_loud() {
        // With Duck = 100 % and continuous loud input, the duck envelope
        // saturates near 1.0 and the echo factor collapses near 0 — so
        // the audible "wet" component is negligible. Comparing total
        // off-dry energy between Duck=0 and Duck=100 over the same
        // long-input window gives a clear monotone decrease.
        let measure_wet_energy = |duck_pct: f32| -> f32 {
            let mut d = DelayEffect::new();
            d.set_sample_rate(48_000.0);
            d.set_param(0, 100.0); // Free = 100 ms
            d.set_param(1, 14.0); // Time → Free
            d.set_param(2, 80.0); // Healthy feedback so an echo is audible
            d.set_param(3, duck_pct);
            // Drive 0.5-amplitude DC for 1 s so the duck envelope settles
            // at 0.5 (vs 0 with Duck=0). The Free-mode 100 ms tap will
            // produce an "echo" that's also at 0.5 — output = dry + echo
            // = 1.0 with Duck=0. With Duck=100 the echo's factor is
            // (1 - 1.0·0.5) = 0.5 — output = 0.5 + 0.5·0.5 = 0.75.
            // Sum (output - dry) so we isolate the wet contribution.
            let mut wet = 0.0_f32;
            // Warm-up: let the delay buffer fill and the duck envelope
            // settle.
            for _ in 0..12_000 {
                d.process_sample(0.5, 0.5);
            }
            for _ in 0..48_000 {
                let (l, _r) = d.process_sample(0.5, 0.5);
                wet += (l - 0.5).abs();
            }
            wet
        };
        let no_duck = measure_wet_energy(0.0);
        let full_duck = measure_wet_energy(100.0);
        assert!(
            full_duck < no_duck * 0.7,
            "full duck should reduce wet content well below Duck=0 \
             (no_duck={no_duck}, full_duck={full_duck})"
        );
    }

    #[test]
    fn delay_sync_time_tracks_bpm() {
        // Sync mode at 1/4 note (idx 8): at 120 BPM, a quarter is 500 ms;
        // at 60 BPM, 1000 ms. Drive an impulse, then look for the echo
        // peak in each case. The peak index = subdivision-in-samples
        // (within a sample of jitter from linear interp).
        let echo_idx_at_bpm = |bpm: f32| -> usize {
            let mut d = DelayEffect::new();
            d.set_sample_rate(48_000.0);
            d.set_bpm(bpm);
            d.set_param(1, 8.0); // Time → 1/4 note (Time is at slot 1)
            d.set_param(2, 0.0); // Feedback = 0
            d.set_param(3, 0.0); // Duck = 0
            d.process_sample(1.0, 1.0);
            let mut peak_idx = 0usize;
            let mut peak_val = 0.0_f32;
            for i in 1..60_000 {
                let (l, _r) = d.process_sample(0.0, 0.0);
                if l.abs() > peak_val {
                    peak_val = l.abs();
                    peak_idx = i;
                }
            }
            peak_idx
        };
        let at_120 = echo_idx_at_bpm(120.0);
        let at_60 = echo_idx_at_bpm(60.0);
        // 120 BPM: quarter = 500 ms = 24 000 samples at 48 kHz.
        assert!(
            (23_990..=24_010).contains(&at_120),
            "1/4 note at 120 BPM should echo at ~24000 samples, got {at_120}"
        );
        // 60 BPM: quarter = 1000 ms = 48 000 samples.
        assert!(
            (47_990..=48_010).contains(&at_60),
            "1/4 note at 60 BPM should echo at ~48000 samples, got {at_60}"
        );
    }

    #[test]
    fn delay_dims_the_free_dial_only_when_time_points_at_a_sync_subdivision() {
        // Default Time is the trailing "Free" slot → Free is active, no dim.
        let mut d = DelayEffect::new();
        assert!(
            !d.param_dimmed(0),
            "Free dial active at default (Time → Free)"
        );
        // Pick any sync subdivision (e.g. 1/4 note at idx 8) → Free is unused,
        // so the editor should dim its dial.
        d.set_param(1, 8.0);
        assert!(d.param_dimmed(0), "Free dial dimmed when Time = 1/4");
        // Switch back to the Free slot → un-dim.
        let free_slot = (DELAY_TIME_LABELS.len() - 1) as f32;
        d.set_param(1, free_slot);
        assert!(!d.param_dimmed(0), "Free dial active again at Time → Free");
        // Other parameter slots are never dimmed.
        d.set_param(1, 8.0);
        for i in 1..d.parameters().len() {
            assert!(!d.param_dimmed(i), "slot {i} should never dim");
        }
        // Out-of-range index: harmless.
        assert!(!d.param_dimmed(99));
    }
}
