use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// A beat-repeat / stutter / pitched-buzz "loop" effect modelled on
/// Infiltrator's Loop mode. A capture clock fires per **Refresh** sync
/// subdivision; at each tick the loop's origin snaps to the most-recent
/// `loop_length` samples already in the ring buffer. Between ticks the
/// effect plays that captured slice on repeat — short loop lengths
/// audibly become pitched buzz (loop frequency = pitch), longer ones
/// stutter.
///
/// **Free vs Sync**: `Snap` is an enum of musical subdivisions plus a
/// trailing `Free` entry (mirrors Delay's Time control). When Snap is a
/// sync subdivision the loop length is beat-locked; when Snap = Free the
/// `Rate` dial (Hz) takes over. `Rate` is dimmed when Snap is sync.
///
/// **Length-vs-Refresh clamp**: if the configured loop length exceeds
/// the Refresh interval, it's silently clamped — the loop never spans
/// more than one capture window.
///
/// **Smooth**: linear crossfade at the loop seam, duration =
/// `(smooth_pct / 100) · (loop_length / 2)`. The last samples of each
/// iteration blend into the iteration's first samples, so re-looping
/// self-similar audio is smooth at every Smooth level.
pub struct RepeatEffect {
    rate_hz: f32,
    snap_idx: f32,
    refresh_idx: f32,
    smooth_pct: f32,
    sample_rate: f32,
    bpm: f32,
    /// Per-channel write-only ring buffer. Audio is always written here
    /// so the capture trigger has a back-window to pull `loop_length`
    /// samples from.
    ring_l: Vec<f32>,
    ring_r: Vec<f32>,
    write_idx: usize,
    /// Capture-clock phase in `[0, 1)`. Each sample advances by
    /// `1 / capture_period_samples`. When the phase crosses 1.0 the
    /// capture trigger fires (after priming).
    capture_phase: f32,
    /// Index of the first sample of the active loop in the ring buffer.
    /// Updated on each capture trigger to `write_idx − loop_length`.
    capture_origin: usize,
    /// Length of the active loop in samples. Held stable across one
    /// loop iteration; refreshed on the next capture trigger from the
    /// current `Snap` / `Rate` / Refresh settings.
    loop_length: usize,
    /// Playback position within the current loop, `0..loop_length`.
    loop_pos: usize,
    /// True once the ring has been filled with at least one loop
    /// length's worth of audio. Output is dry passthrough until then.
    primed: bool,
    /// Sample counter since the last `reset()`, used to detect priming.
    samples_since_reset: u64,
}

impl RepeatEffect {
    /// Worst-case buffer length: 4 s × 192 kHz. Covers the longest sync
    /// subdivision the Refresh enum can ask for at the slowest practical
    /// host tempo. Buffer is allocated once in `new`; per-sample work
    /// only reads/writes existing slots.
    const BUF_LEN: usize = (4.0 * 192_000.0) as usize;
    /// Floor on the active loop length. Anything shorter degenerates to
    /// noise; `Rate` already maxes at 1 kHz (≈ 48 samples at 48 kHz) so
    /// this is just a safety net.
    const MIN_LOOP_SAMPLES: usize = 16;
    /// Free-mode Rate range (Hz). 0.5 Hz → 2 s loop (long stutter);
    /// 1 kHz → 1 ms loop (high-pitched buzz).
    const RATE_MIN_HZ: f32 = 0.5;
    const RATE_MAX_HZ: f32 = 1_000.0;

    /// Snap subdivisions, in dropdown order. The trailing `Free` entry
    /// makes the Rate dial active.
    const SNAP_LABELS: &'static [&'static str] = &[
        "1/64", "1/64.", "1/32", "1/32.", "1/16", "1/16.", "1/8", "1/8.", "1/4", "1/4.", "1/2",
        "1/2.", "1/1", "1/1.", "Free",
    ];
    /// Index of the `Free` entry in `SNAP_LABELS`.
    const SNAP_FREE_IDX: usize = 14;

    /// Refresh subdivisions, in dropdown order. Sync-only — no Free entry.
    const REFRESH_LABELS: &'static [&'static str] = &[
        "1/64", "1/64.", "1/32", "1/32.", "1/16", "1/16.", "1/8", "1/8.", "1/4", "1/4.", "1/2",
        "1/2.", "1/1", "1/1.",
    ];

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Rate",
            min: Self::RATE_MIN_HZ,
            max: Self::RATE_MAX_HZ,
            default: 30.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Snap",
            min: 0.0,
            max: (Self::SNAP_LABELS.len() - 1) as f32,
            default: 6.0, // 1/8
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: Self::SNAP_LABELS,
            },
        },
        ParamSpec {
            name: "Refresh",
            min: 0.0,
            max: (Self::REFRESH_LABELS.len() - 1) as f32,
            default: 8.0, // 1/4
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: Self::REFRESH_LABELS,
            },
        },
        ParamSpec {
            name: "Smooth",
            min: 0.0,
            max: 100.0,
            default: 10.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        Self {
            rate_hz: Self::PARAMS[0].default,
            snap_idx: Self::PARAMS[1].default,
            refresh_idx: Self::PARAMS[2].default,
            smooth_pct: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            bpm: 120.0,
            ring_l: vec![0.0; Self::BUF_LEN],
            ring_r: vec![0.0; Self::BUF_LEN],
            write_idx: 0,
            capture_phase: 1.0,
            capture_origin: 0,
            loop_length: 0,
            loop_pos: 0,
            primed: false,
            samples_since_reset: 0,
        }
    }

    /// Beats per Snap-enum index. Returns `None` for the trailing `Free`
    /// slot (caller switches to the Rate dial in that case). Shared with
    /// `StretchEffect::refresh_beats` so both effects use the same
    /// subdivision lookup.
    pub(crate) fn snap_beats(idx: usize) -> Option<f32> {
        match idx {
            0 => Some(4.0 / 64.0),
            1 => Some(1.5 * 4.0 / 64.0),
            2 => Some(4.0 / 32.0),
            3 => Some(1.5 * 4.0 / 32.0),
            4 => Some(4.0 / 16.0),
            5 => Some(1.5 * 4.0 / 16.0),
            6 => Some(0.5),
            7 => Some(0.75),
            8 => Some(1.0),
            9 => Some(1.5),
            10 => Some(2.0),
            11 => Some(3.0),
            12 => Some(4.0),
            13 => Some(6.0),
            _ => None,
        }
    }

    /// Beats per Refresh-enum index. Always returns a finite value
    /// (sync-only). Out-of-range defaults to 1/4 so a stray modulation
    /// can't freeze the capture clock.
    fn refresh_beats(idx: usize) -> f32 {
        Self::snap_beats(idx).unwrap_or(1.0)
    }

    /// Loop length in samples — what the user dialled in via Snap or
    /// Rate, before the Refresh-interval clamp.
    fn loop_length_samples_raw(&self) -> f32 {
        let idx = self.snap_idx.round() as usize;
        match Self::snap_beats(idx) {
            Some(beats) => {
                let sec_per_beat = 60.0 / self.bpm.max(1.0);
                beats * sec_per_beat * self.sample_rate
            }
            None => self.sample_rate / self.rate_hz.clamp(Self::RATE_MIN_HZ, Self::RATE_MAX_HZ),
        }
    }

    /// Capture interval in samples — how often a fresh slice is grabbed.
    fn capture_period_samples(&self) -> f32 {
        let beats = Self::refresh_beats(self.refresh_idx.round() as usize);
        let sec_per_beat = 60.0 / self.bpm.max(1.0);
        (beats * sec_per_beat * self.sample_rate).max(1.0)
    }
}

impl Default for RepeatEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for RepeatEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // Always write incoming audio into the ring. This is the
        // back-window the capture trigger pulls from.
        self.ring_l[self.write_idx] = left;
        self.ring_r[self.write_idx] = right;
        self.write_idx = (self.write_idx + 1) % Self::BUF_LEN;
        self.samples_since_reset = self.samples_since_reset.saturating_add(1);

        // Loop length the user asked for, clamped to the Refresh window
        // so the loop never overruns its capture interval — no mid-loop
        // interruption ever.
        let capture_period = self.capture_period_samples();
        let raw = self.loop_length_samples_raw();
        let clamped = raw.min(capture_period).max(Self::MIN_LOOP_SAMPLES as f32) as usize;

        // Advance the capture-clock phase. A wrap past 1.0 means it's
        // time to grab a fresh slice.
        self.capture_phase += 1.0 / capture_period;
        let phase_fired = if self.capture_phase >= 1.0 {
            self.capture_phase -= 1.0;
            true
        } else {
            false
        };

        // Priming: until enough audio has been written into the ring
        // for at least one (clamped) loop_length, fall back to dry
        // passthrough so the user hears something immediately when they
        // enable the effect. Use the clamped value (not raw) because
        // that's the loop we'll actually play — gating on raw would
        // hold off priming for `min(raw, capture_period)` samples even
        // when the Refresh window is much shorter than the user's
        // Snap setting.
        let want_primed = self.samples_since_reset >= clamped as u64;
        let just_primed = want_primed && !self.primed;
        if just_primed {
            self.primed = true;
        }
        let trigger = self.primed && (phase_fired || just_primed);

        // On trigger: snap capture_origin to the most-recent slice in
        // the ring and restart the loop playhead.
        if trigger {
            self.loop_length = clamped;
            self.capture_origin =
                (self.write_idx + Self::BUF_LEN - self.loop_length) % Self::BUF_LEN;
            self.loop_pos = 0;
        }

        // Output: dry passthrough until primed (so the user always
        // hears something), then loop playback with optional
        // crossfade at the seam.
        if !self.primed || self.loop_length == 0 {
            return (left, right);
        }

        let offset = self.loop_pos;
        let main_idx = (self.capture_origin + offset) % Self::BUF_LEN;
        let main_l = self.ring_l[main_idx];
        let main_r = self.ring_r[main_idx];

        // Crossfade region: the last `crossfade` samples of each loop
        // iteration blend with the iteration's own first `crossfade`
        // samples. Linear sum-of-weights = 1 so self-similar audio
        // overlays cleanly.
        let crossfade =
            ((self.smooth_pct.clamp(0.0, 100.0) * 0.01) * (self.loop_length as f32 * 0.5)) as usize;
        let crossfade = crossfade.min(self.loop_length / 2);
        let (out_l, out_r) = if crossfade > 0 && offset + crossfade >= self.loop_length {
            let r = offset + crossfade - self.loop_length;
            let w_start = r as f32 / crossfade as f32;
            let w_end = 1.0 - w_start;
            let head_idx = (self.capture_origin + r) % Self::BUF_LEN;
            let head_l = self.ring_l[head_idx];
            let head_r = self.ring_r[head_idx];
            (
                w_end * main_l + w_start * head_l,
                w_end * main_r + w_start * head_r,
            )
        } else {
            (main_l, main_r)
        };

        self.loop_pos += 1;
        if self.loop_pos >= self.loop_length {
            self.loop_pos = 0;
        }

        (out_l, out_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn set_bpm(&mut self, bpm: f32) {
        self.bpm = bpm.max(1.0);
    }

    fn reset(&mut self) {
        for s in self.ring_l.iter_mut() {
            *s = 0.0;
        }
        for s in self.ring_r.iter_mut() {
            *s = 0.0;
        }
        self.write_idx = 0;
        // Phase at 1.0 so the very first sample after priming fires the
        // first capture (no awkward initial silent loop).
        self.capture_phase = 1.0;
        self.capture_origin = 0;
        self.loop_length = 0;
        self.loop_pos = 0;
        self.primed = false;
        self.samples_since_reset = 0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.rate_hz = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => {
                let max_idx = (Self::SNAP_LABELS.len() - 1) as f32;
                self.snap_idx = value.round().clamp(0.0, max_idx);
            }
            2 => {
                let max_idx = (Self::REFRESH_LABELS.len() - 1) as f32;
                self.refresh_idx = value.round().clamp(0.0, max_idx);
            }
            3 => self.smooth_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => {}
        }
    }

    /// The Rate dial (slot 0) is dimmed whenever Snap points at a sync
    /// subdivision — sync drives the loop length directly and the Rate
    /// value is unused. Mirrors Delay's Free/Time dim behaviour.
    fn param_dimmed(&self, index: usize) -> bool {
        if index != 0 {
            return false;
        }
        let idx = self.snap_idx.round() as usize;
        idx != Self::SNAP_FREE_IDX
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    #[test]
    fn repeat_lists_four_parameters_with_the_expected_specs() {
        let r = RepeatEffect::new();
        let specs = r.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Rate");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert_eq!(specs[0].min, 0.5);
        assert_eq!(specs[0].max, 1000.0);
        assert_eq!(specs[1].name, "Snap");
        assert!(matches!(specs[1].format, ParamFormat::Enum { .. }));
        // Snap default = 1/8 (idx 6).
        assert_eq!(specs[1].default, 6.0);
        assert_eq!(specs[2].name, "Refresh");
        assert!(matches!(specs[2].format, ParamFormat::Enum { .. }));
        // Refresh default = 1/4 (idx 8).
        assert_eq!(specs[2].default, 8.0);
        assert_eq!(specs[3].name, "Smooth");
        assert_eq!(specs[3].max, 100.0);
    }

    #[test]
    fn repeat_set_param_clamps_each_slot() {
        let mut r = RepeatEffect::new();
        r.set_param(0, 9_999.0);
        assert_eq!(r.rate_hz, 1_000.0);
        r.set_param(0, 0.0);
        assert_eq!(r.rate_hz, 0.5);
        // Snap clamps to [0, SNAP_LABELS.len() - 1] = [0, 14].
        r.set_param(1, 99.0);
        assert_eq!(r.snap_idx, 14.0);
        r.set_param(1, -5.0);
        assert_eq!(r.snap_idx, 0.0);
        // Refresh clamps to [0, REFRESH_LABELS.len() - 1] = [0, 13].
        r.set_param(2, 99.0);
        assert_eq!(r.refresh_idx, 13.0);
        r.set_param(3, 200.0);
        assert_eq!(r.smooth_pct, 100.0);
    }

    #[test]
    fn repeat_outputs_dry_passthrough_until_primed() {
        // Before the ring has filled with at least one loop_length of
        // audio, output mirrors input so the user always hears
        // something. Use Free mode with a long loop (0.5 Hz → 2 s loop)
        // so the priming window is much longer than the test span.
        let mut r = RepeatEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(1, 14.0); // Snap → Free
        r.set_param(0, 0.5); // Rate → 0.5 Hz (2 s loop)
                             // First 100 samples should pass dry through verbatim.
        for i in 0..100 {
            let dry = 0.1 + i as f32 * 0.001;
            let (l, ri) = r.process_sample(dry, -dry);
            assert!((l - dry).abs() < 1e-6, "sample {i} L not dry: {l} vs {dry}");
            assert!(
                (ri + dry).abs() < 1e-6,
                "sample {i} R not dry: {ri} vs {}",
                -dry
            );
        }
    }

    #[test]
    fn repeat_loops_a_captured_slice_in_free_mode() {
        // Free-mode short loop. After priming, the output sample at
        // playback position p should equal the input sample captured
        // (write_pos − loop_length + p) samples ago. Verify by feeding
        // a counter signal and checking the loop wraps at the right
        // boundary.
        let mut r = RepeatEffect::new();
        r.set_sample_rate(48_000.0);
        // Snap = Free, Rate = 1 kHz → 48-sample loop at 48 kHz.
        r.set_param(1, 14.0);
        r.set_param(0, 1_000.0);
        // Refresh interval long enough that no early refresh interferes
        // (1/1. at 30 BPM ≈ 12 s).
        r.set_param(2, 13.0); // 1/1.
        r.set_bpm(30.0);
        r.set_param(3, 0.0); // Smooth = 0 so we can compare exact samples
                             // Feed a recognisable sequence (sample i = i as f32 * 0.001).
        let mut last_output = 0.0;
        let mut wrapped_at_least_once = false;
        let mut samples_at_wrap = 0usize;
        for i in 0..1_000 {
            let dry = i as f32 * 0.001;
            let (out, _) = r.process_sample(dry, dry);
            // Once we've definitely primed (after 48 samples), the
            // output should NOT keep growing with the input — it should
            // loop. Detect the first time output is less than the
            // previous output (loop wrap).
            if i > 100 && out < last_output - 0.01 {
                wrapped_at_least_once = true;
                samples_at_wrap = i;
                break;
            }
            last_output = out;
        }
        assert!(
            wrapped_at_least_once,
            "Repeat in Free mode at 1 kHz should wrap its loop visibly"
        );
        // The wrap happens within ~loop_length samples of priming.
        assert!(
            samples_at_wrap < 200,
            "Wrap should land soon after priming, got at sample {samples_at_wrap}"
        );
    }

    #[test]
    fn repeat_loop_length_clamps_to_refresh_interval() {
        // If Snap asks for a longer loop than Refresh, the effective
        // loop_length must clamp to the Refresh interval — otherwise
        // captures would interrupt mid-loop.
        let mut r = RepeatEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_bpm(120.0);
        // Snap = 1/2 (large), Refresh = 1/16 (small).
        r.set_param(1, 10.0); // Snap → 1/2 note
        r.set_param(2, 4.0); // Refresh → 1/16 note
                             // After enough samples to prime, the loop_length must be ≤
                             // the 1/16-note window (= 60/120/4 × 48000 ≈ 6000 samples) not
                             // the 1/2-note value (24000 samples).
        for _ in 0..24_000 {
            let _ = r.process_sample(0.5, 0.5);
        }
        assert!(
            r.loop_length <= 6_010,
            "loop_length should clamp to ~1/16-note (6000 samples), got {}",
            r.loop_length
        );
        assert!(
            r.loop_length >= 5_990,
            "loop_length should be ~1/16-note (6000 samples), got {}",
            r.loop_length
        );
    }

    #[test]
    fn repeat_reset_clears_state_and_returns_to_passthrough() {
        let mut r = RepeatEffect::new();
        r.set_sample_rate(48_000.0);
        // Drive enough audio for priming, then reset.
        for _ in 0..96_000 {
            let _ = r.process_sample(0.5, 0.5);
        }
        assert!(r.primed);
        r.reset();
        assert!(!r.primed);
        assert_eq!(r.samples_since_reset, 0);
        assert_eq!(r.write_idx, 0);
        assert_eq!(r.loop_pos, 0);
        assert!(r.ring_l.iter().all(|&s| s == 0.0));
        assert!(r.ring_r.iter().all(|&s| s == 0.0));
        // The very first post-reset sample is dry passthrough.
        let (l, ri) = r.process_sample(0.42, -0.42);
        assert!((l - 0.42).abs() < 1e-6);
        assert!((ri + 0.42).abs() < 1e-6);
    }

    #[test]
    fn repeat_reports_zero_pdc_latency() {
        // The loop effect inherently delays the audio it plays back
        // (the loop is the most-recent N samples), but that's the
        // *point* of the effect — not host-PDC-reportable latency
        // (PDC compensates fixed plugin delay; the loop delay is
        // semantically the user's effect). Should report zero.
        let r = RepeatEffect::new();
        assert_eq!(r.latency_samples(), 0);
    }
}
