use super::{Effect, ParamFormat, ParamScaling, ParamSpec, RepeatEffect};

/// A granular time-stretch effect — slows down captured audio without
/// changing pitch, modelled on Infiltrator's Stretch.
///
/// **How it works:** every Refresh tick captures a slice of incoming
/// audio whose length is `Refresh interval × Pace` — that's the trick
/// that keeps the stretch synchronised. A read pointer crawls through
/// that capture window at `Pace` samples per output sample, so the
/// window is fully traversed in exactly one Refresh interval. While
/// the read pointer crawls, a grain scheduler spawns small windowed
/// snippets that each play at original speed (pitch preserved). The
/// snippets overlap-add into the output.
///
/// `Smooth` lerps each grain's envelope from boxcar (0 %) to Hann
/// (100 %); the adjacent-grain sum is exactly `2 − smooth` at every
/// position, so a matching scale (`1 / (2 − smooth)`) keeps the
/// summed amplitude unit across the whole Smooth range.
/// Grain durations round to even sample counts and remain fixed while a grain plays.
///
/// **Trade-offs:**
/// - Output is dry passthrough until the ring has filled with at least
///   one capture window of audio, so the user always hears something
///   when the effect first engages.
/// - Refresh is capped at half the ring capacity to retain the active
///   capture while recording the next one, including during automation.
/// - Latency reported as 0: the granulation delay (= one Refresh
///   interval) is *the effect*, not a fixed plugin delay PDC should
///   compensate for. Same call as Repeat.
pub struct StretchEffect {
    pace: f32,
    refresh_idx: f32,
    grain_hz: f32,
    smooth_pct: f32,
    sample_rate: f32,
    bpm: f32,
    /// Per-channel input ring. Audio is written here unconditionally so
    /// the capture-and-granulate logic has a back-window to pull from.
    ring_l: Vec<f32>,
    ring_r: Vec<f32>,
    write_idx: usize,
    /// Capture-clock phase in `[0, 1)`. Wrap → fire a Refresh tick.
    capture_phase: f64,
    /// Index into the ring where the active capture window starts.
    /// Updated on each Refresh tick.
    capture_origin: usize,
    /// Length of the active capture window in samples
    /// (= `capture_period × pace`). Held stable between Refresh ticks.
    capture_window: f32,
    /// Read pointer within the capture window, `0..capture_window`.
    /// Advances at `pace` samples per output sample so it traverses the
    /// window in exactly `capture_period` output samples.
    read_pos: f64,
    /// Samples until the next grain. Each spawn latches an integer
    /// half-grain interval so adjacent windows remain complementary.
    grain_samples_until_spawn: usize,
    /// Active grain pool. 4 slots — at 50 % overlap, never more than 2
    /// are simultaneously active in steady state; the extra two cover
    /// the brief overlap during grain hand-off.
    grains: [StretchGrain; 4],
    /// True once the ring has at least one capture window's worth of
    /// audio. Output is dry passthrough until then.
    primed: bool,
    samples_since_reset: u64,
}

/// One windowed playback voice for `StretchEffect`. `active = false`
/// slots are skipped on the audio loop. Spawned by the grain scheduler;
/// retires itself when `elapsed >= length`.
#[derive(Clone, Copy)]
struct StretchGrain {
    active: bool,
    /// Absolute ring index this grain reads from, set at spawn time.
    start_idx: usize,
    /// Samples played since spawn (fractional for sub-sample accuracy
    /// even though we step by 1.0 per output sample).
    elapsed: f32,
    /// Total grain length in samples.
    length: f32,
}

impl Default for StretchGrain {
    fn default() -> Self {
        Self {
            active: false,
            start_idx: 0,
            elapsed: 0.0,
            length: 0.0,
        }
    }
}

impl StretchEffect {
    /// Worst-case ring buffer length: 4 s × 192 kHz, same as Repeat.
    const BUF_LEN: usize = (4.0 * 192_000.0) as usize;
    /// Floor on the capture window so degenerate (very low Pace × very
    /// short Refresh) settings still produce something playable.
    const MIN_WINDOW_SAMPLES: usize = 16;
    /// Pace range. 0.05 = 20× stretch (extreme but stable); 1.0 = no
    /// stretch (granulator runs at real-time speed).
    const PACE_MIN: f32 = 0.05;
    const PACE_MAX: f32 = 1.0;
    /// Grain Hz range. 5 Hz → 200 ms grains (long stutter), 200 Hz →
    /// 5 ms grains (pitched/timbral). Matches Repeat's Rate envelope.
    const GRAIN_MIN_HZ: f32 = 5.0;
    const GRAIN_MAX_HZ: f32 = 200.0;

    /// Refresh subdivisions, in dropdown order. Sync-only.
    const REFRESH_LABELS: &'static [&'static str] = &[
        "1/64", "1/64.", "1/32", "1/32.", "1/16", "1/16.", "1/8", "1/8.", "1/4", "1/4.", "1/2",
        "1/2.", "1/1", "1/1.",
    ];

    pub(super) const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Pace",
            min: Self::PACE_MIN,
            max: Self::PACE_MAX,
            default: 0.5,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "x",
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
            name: "Grain",
            min: Self::GRAIN_MIN_HZ,
            max: Self::GRAIN_MAX_HZ,
            default: 30.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Smooth",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        Self {
            pace: Self::PARAMS[0].default,
            refresh_idx: Self::PARAMS[1].default,
            grain_hz: Self::PARAMS[2].default,
            smooth_pct: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            bpm: 120.0,
            ring_l: vec![0.0; Self::BUF_LEN],
            ring_r: vec![0.0; Self::BUF_LEN],
            write_idx: 0,
            capture_phase: 1.0,
            capture_origin: 0,
            capture_window: 0.0,
            read_pos: 0.0,
            grain_samples_until_spawn: 0,
            grains: [StretchGrain::default(); 4],
            primed: false,
            samples_since_reset: 0,
        }
    }

    /// Beats per Refresh-enum index. Reuses Repeat's `snap_beats` table
    /// (same subdivision values, same indexing). Defaults to 1/4 on an
    /// out-of-range index so a stray modulation can't freeze the
    /// capture clock.
    fn refresh_beats(idx: usize) -> f32 {
        RepeatEffect::snap_beats(idx).unwrap_or(1.0)
    }

    /// Capture interval in samples — how often a fresh slice is grabbed.
    fn capture_period_samples(&self) -> f32 {
        let beats = Self::refresh_beats(self.refresh_idx.round() as usize);
        let sec_per_beat = 60.0 / self.bpm.max(1.0);
        (beats * sec_per_beat * self.sample_rate)
            .max(1.0)
            .min((Self::BUF_LEN / 2) as f32)
    }

    /// Grain length in samples — what each individual snippet plays for
    /// at original speed. Even lengths give exact half-grain hops.
    fn grain_length_samples(&self) -> f32 {
        let hz = self.grain_hz.clamp(Self::GRAIN_MIN_HZ, Self::GRAIN_MAX_HZ);
        let samples = (self.sample_rate / hz).max(2.0);
        (samples * 0.5).round() * 2.0
    }
}

impl Default for StretchEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for StretchEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // Always write input to the ring; the capture-and-granulate
        // logic pulls from here on every Refresh tick.
        self.ring_l[self.write_idx] = left;
        self.ring_r[self.write_idx] = right;
        self.write_idx = (self.write_idx + 1) % Self::BUF_LEN;
        self.samples_since_reset = self.samples_since_reset.saturating_add(1);

        let pace = self.pace.clamp(Self::PACE_MIN, Self::PACE_MAX);
        let capture_period = self.capture_period_samples();
        // capture_window = capture_period × pace. Sizes the window so
        // the read pointer (advancing at pace per sample) traverses it
        // in exactly one Refresh interval — no mid-stretch interruption.
        let next_capture_window = (capture_period * pace).max(Self::MIN_WINDOW_SAMPLES as f32);

        // Capture clock.
        self.capture_phase += 1.0 / f64::from(capture_period);
        let phase_fired = if self.capture_phase >= 1.0 {
            self.capture_phase -= 1.0;
            true
        } else {
            false
        };

        // Priming: until enough audio has been written for one capture
        // window's worth of input, pass the dry signal through so the
        // user always hears something when they enable the effect.
        let want_primed = self.samples_since_reset >= next_capture_window as u64;
        let just_primed = want_primed && !self.primed;
        if just_primed {
            self.primed = true;
        }
        let trigger = self.primed && (phase_fired || just_primed);

        // On trigger: snap capture_origin to the most-recent window in
        // the ring and reset the read pointer.
        if trigger {
            self.capture_window = next_capture_window;
            self.capture_origin =
                (self.write_idx + Self::BUF_LEN - self.capture_window as usize) % Self::BUF_LEN;
            self.read_pos = 0.0;
        }

        // Output: dry passthrough until primed (so the user never hears
        // silence when they first engage the effect).
        if !self.primed || self.capture_window <= 0.0 {
            return (left, right);
        }

        // Grain scheduler: spawn rate = 2 / grain_length so adjacent
        // grains hit 50 % overlap, the standard granular setting.
        let grain_length = self.grain_length_samples();
        let spawn_period = (grain_length as usize) / 2;
        if self.grain_samples_until_spawn == 0 {
            self.grain_samples_until_spawn = spawn_period;
            // Grain start lives at (capture_origin + read_pos) — the
            // crawling read pointer determines what audio each grain
            // reads. Find the first inactive slot; if all slots are
            // active, evict the oldest (longest-elapsed).
            let start = (self.capture_origin + self.read_pos as usize) % Self::BUF_LEN;
            let slot = if let Some(i) = self.grains.iter().position(|g| !g.active) {
                i
            } else {
                self.grains
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| {
                        a.elapsed
                            .partial_cmp(&b.elapsed)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            };
            self.grains[slot] = StretchGrain {
                active: true,
                start_idx: start,
                elapsed: 0.0,
                length: grain_length,
            };
        }
        self.grain_samples_until_spawn -= 1;

        // Advance the read pointer at the stretch rate. Saturate at
        // capture_window so degenerate state can't index past it; the
        // capture-clock math keeps this in sync in normal operation.
        self.read_pos += f64::from(pace);
        if self.read_pos >= f64::from(self.capture_window) {
            self.read_pos = f64::from(self.capture_window);
        }

        // Sum active grains with the Smooth-blended window. The window
        // lerps from boxcar (Smooth=0) to Hann (Smooth=100); the
        // adjacent-grain sum is exactly `2 − smooth` everywhere, so a
        // matching scale keeps the output unit-amplitude across the
        // whole Smooth range.
        let smooth = (self.smooth_pct * 0.01).clamp(0.0, 1.0);
        let scale = 1.0 / (2.0 - smooth);
        let two_pi = std::f32::consts::PI * 2.0;
        let mut out_l = 0.0;
        let mut out_r = 0.0;
        for grain in self.grains.iter_mut() {
            if !grain.active {
                continue;
            }
            if grain.elapsed >= grain.length {
                grain.active = false;
                continue;
            }
            let t = grain.elapsed / grain.length;
            // window = (1−smooth)·boxcar + smooth·Hann
            let hann = 0.5 * (1.0 - (two_pi * t).cos());
            let window = (1.0 - smooth) + smooth * hann;
            let idx = (grain.start_idx + grain.elapsed as usize) % Self::BUF_LEN;
            out_l += window * self.ring_l[idx];
            out_r += window * self.ring_r[idx];
            grain.elapsed += 1.0;
        }
        (out_l * scale, out_r * scale)
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
        self.capture_phase = 1.0;
        self.capture_origin = 0;
        self.capture_window = 0.0;
        self.read_pos = 0.0;
        self.grain_samples_until_spawn = 0;
        for g in self.grains.iter_mut() {
            *g = StretchGrain::default();
        }
        self.primed = false;
        self.samples_since_reset = 0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.pace = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => {
                let max_idx = (Self::REFRESH_LABELS.len() - 1) as f32;
                self.refresh_idx = value.round().clamp(0.0, max_idx);
            }
            2 => self.grain_hz = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.smooth_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    fn configured_stretch(pace: f32, refresh: f32, grain_hz: f32, smooth: f32) -> StretchEffect {
        let mut stretch = StretchEffect::new();
        stretch.set_sample_rate(1_024.0);
        stretch.set_bpm(240.0);
        for (index, value) in [pace, refresh, grain_hz, smooth].into_iter().enumerate() {
            stretch.set_param(index, value);
        }
        stretch
    }

    #[test]
    fn half_overlapping_grains_preserve_constant_stereo_gain_after_startup() {
        for smooth in [0.0, 50.0, 100.0] {
            let mut stretch = configured_stretch(1.0, 0.0, 8.0, smooth);
            for sample in 0..512 {
                let actual = stretch.process_sample(1.0, -0.25);
                if sample >= 15 + 64 {
                    assert!(
                        (actual.0 - 1.0).abs() < 3e-6,
                        "smooth {smooth}, sample {sample}: {actual:?}"
                    );
                    assert!((actual.1 + 0.25).abs() < 3e-6);
                }
            }
        }
    }

    #[test]
    fn fractional_grain_rates_preserve_steady_stereo_gain() {
        for (sample_rate, count, settled) in [(1_024.0, 4_096, 511), (48_000.0, 20_000, 14_000)] {
            for smooth in [0.0, 50.0, 100.0] {
                let mut stretch = configured_stretch(0.5, 8.0, 31.0, smooth);
                stretch.set_sample_rate(sample_rate);
                stretch.set_bpm(120.0);
                for sample in 0..count {
                    let actual = stretch.process_sample(1.0, -0.25);
                    if sample >= settled {
                        assert!(
                            (actual.0 - 1.0).abs() < 3e-6,
                            "rate {sample_rate}, smooth {smooth}, sample {sample}: {actual:?}"
                        );
                        assert!((actual.1 + 0.25).abs() < 3e-6);
                    }
                }
            }
        }
    }

    #[test]
    fn pace_automation_uses_the_existing_window_until_the_next_refresh() {
        let mut stretch = configured_stretch(0.5, 8.0, 8.0, 0.0);
        for sample in 0..320 {
            if sample == 160 {
                stretch.set_param(0, 0.25);
            }
            let input = (sample as f32 / 1_024.0, (256.0 - sample as f32) / 512.0);
            let actual = stretch.process_sample(input.0, input.1);
            if (160..255).contains(&sample) {
                assert_eq!(stretch.capture_window, 128.0);
            }
            if sample >= 255 {
                assert_eq!(stretch.capture_window, 64.0);
            }
            let expected = match sample {
                191 => Some((0.04296875, 0.4140625)),
                223 => Some((0.07421875, 0.3515625)),
                255 => Some((0.13671875, 0.2265625)),
                287 => Some((0.16796875, 0.1640625)),
                _ => None,
            };
            if let Some(expected) = expected {
                assert_eq!(actual, expected, "sample {sample}");
            }
        }
    }

    #[test]
    fn long_refresh_windows_fit_while_recording_the_next_capture() {
        let mut stretch = configured_stretch(1.0, 13.0, 200.0, 0.0);
        stretch.set_sample_rate(192_000.0);
        stretch.set_bpm(1.0);
        assert_eq!(stretch.capture_period_samples(), 384_000.0);
        for sample in 0..384_000 {
            let actual = stretch.process_sample(0.25, -0.5);
            assert!(actual.0.is_finite() && actual.1.is_finite());
            assert_eq!(stretch.primed, sample == 383_999);
        }
        assert_eq!(stretch.capture_window, 384_000.0);
        assert_eq!(stretch.capture_origin, 0);
    }

    #[test]
    fn long_capture_clocks_stay_within_one_sample_of_the_host_tempo() {
        let mut stretch = configured_stretch(0.25, 8.0, 200.0, 0.0);
        stretch.set_sample_rate(100_000.0);
        stretch.set_bpm(60.0);
        let mut first_refresh = None;
        for sample in 0..100_002 {
            stretch.process_sample(0.25, -0.5);
            if stretch.capture_origin != 0 {
                first_refresh = Some(sample);
                break;
            }
        }
        assert!(
            matches!(first_refresh, Some(99_999..=100_000)),
            "first refresh: {first_refresh:?}"
        );
    }

    #[test]
    fn the_read_cursor_keeps_the_selected_pace_over_a_long_capture() {
        let mut stretch = configured_stretch(0.3, 8.0, 200.0, 0.0);
        stretch.set_sample_rate(100_000.0);
        stretch.set_bpm(60.0);
        for _ in 0..80_000 {
            stretch.process_sample(0.25, -0.5);
        }
        assert_eq!(stretch.capture_origin, 0);
        assert!(
            (stretch.read_pos - 50_001.0 * 0.3).abs() < 0.003,
            "read position {}",
            stretch.read_pos
        );
    }

    #[test]
    fn stereo_ramp_anchors_match_analytic_hann_overlap_and_capture_timing() {
        for (smooth, at_64, at_128) in [
            (0.0, (0.046875, 0.40625), (0.109375, 0.28125)),
            (
                50.0,
                (0.052083333333, 0.395833333333),
                (0.104166666667, 0.291666666667),
            ),
            (100.0, (0.0625, 0.375), (0.09375, 0.3125)),
        ] {
            let mut stretch = configured_stretch(0.5, 8.0, 8.0, smooth);
            stretch.write_idx = StretchEffect::BUF_LEN - 64;
            for sample in 0..320 {
                let input = (sample as f32 / 1_024.0, (256.0 - sample as f32) / 512.0);
                let actual = stretch.process_sample(input.0, input.1);
                if sample < 127 {
                    assert_eq!(actual, input);
                }
                let expected = match sample {
                    159 => Some((0.015625, 0.21875)),
                    191 => Some(at_64),
                    223 => Some((0.078125, 0.34375)),
                    255 => Some(at_128),
                    287 => Some((0.140625, 0.21875)),
                    _ => None,
                };
                if let Some(expected) = expected {
                    assert!(
                        (actual.0 as f64 - expected.0).abs() < 3e-6,
                        "smooth {smooth}, sample {sample}: {actual:?} vs {expected:?}"
                    );
                    assert!((actual.1 as f64 - expected.1).abs() < 3e-6);
                }
            }
        }
    }

    fn seeded_grain_processor(smooth: f32) -> StretchEffect {
        let mut stretch = configured_stretch(0.5, 8.0, 8.0, smooth);
        stretch.primed = true;
        stretch.capture_window = 128.0;
        stretch.capture_phase = 0.0;
        stretch.grain_samples_until_spawn = 64;
        stretch.ring_l.fill(0.75);
        stretch.ring_r.fill(-0.375);
        stretch
    }

    #[test]
    fn a_grain_follows_the_analytic_blended_hann_window_and_retires() {
        for smooth in [0.0, 25.0, 50.0, 100.0] {
            let mut stretch = seeded_grain_processor(smooth);
            stretch.grains[0] = StretchGrain {
                active: true,
                start_idx: StretchEffect::BUF_LEN - 4,
                elapsed: 0.0,
                length: 8.0,
            };
            for elapsed in 0..9 {
                let actual = stretch.process_sample(0.75, -0.375);
                let expected = if elapsed == 8 {
                    0.0
                } else {
                    let hann = (std::f64::consts::PI * elapsed as f64 / 8.0).sin().powi(2);
                    let amount = smooth as f64 / 100.0;
                    ((1.0 - amount) + amount * hann) / (2.0 - amount)
                };
                assert!((actual.0 as f64 - 0.75 * expected).abs() < 3e-7);
                assert!((actual.1 as f64 + 0.375 * expected).abs() < 3e-7);
            }
            assert!(!stretch.grains[0].active);
        }
    }

    #[test]
    fn paired_hann_windows_sum_to_unity_at_every_smooth_setting() {
        for smooth in [0.0, 25.0, 50.0, 100.0] {
            let mut stretch = seeded_grain_processor(smooth);
            for (slot, elapsed) in [(0, 0.0), (1, 4.0)] {
                stretch.grains[slot] = StretchGrain {
                    active: true,
                    start_idx: slot * 16,
                    elapsed,
                    length: 8.0,
                };
            }
            for _ in 0..4 {
                let actual = stretch.process_sample(0.75, -0.375);
                assert!((actual.0 - 0.75).abs() < 3e-7);
                assert!((actual.1 + 0.375).abs() < 3e-7);
            }
        }
    }

    #[test]
    fn new_grains_use_a_free_slot_before_evicting_the_oldest() {
        for free_slot in [None, Some(0), Some(2)] {
            let mut stretch = seeded_grain_processor(50.0);
            stretch.capture_origin = 50;
            stretch.read_pos = 3.5;
            stretch.grain_samples_until_spawn = 0;
            for (slot, elapsed) in [4.0, 40.0, 12.0, 24.0].into_iter().enumerate() {
                stretch.grains[slot] = StretchGrain {
                    active: true,
                    start_idx: 100 * (slot + 1),
                    elapsed,
                    length: 128.0,
                };
            }
            if let Some(slot) = free_slot {
                stretch.grains[slot].active = false;
            }
            let previous = stretch.grains;
            stretch.process_sample(0.75, -0.375);
            let selected = free_slot.unwrap_or(1);
            for (slot, old) in previous.into_iter().enumerate() {
                let grain = &stretch.grains[slot];
                assert!(grain.active);
                assert_eq!(grain.length, 128.0);
                if slot == selected {
                    assert_eq!(grain.start_idx, 53);
                    assert_eq!(grain.elapsed, 1.0);
                } else {
                    assert_eq!(grain.start_idx, old.start_idx);
                    assert_eq!(grain.elapsed, old.elapsed + 1.0);
                }
            }
        }
    }

    #[test]
    fn grain_clocks_preserve_long_half_grain_intervals() {
        let mut stretch = configured_stretch(1.0, 0.0, 5.0, 0.0);
        stretch.set_sample_rate(100_000.0);
        stretch.set_bpm(23_437.5);
        let mut starts = Vec::new();
        for sample in 0..30_020 {
            stretch.process_sample(0.25, -0.5);
            if stretch
                .grains
                .iter()
                .any(|grain| grain.active && grain.elapsed == 1.0)
            {
                starts.push(sample);
            }
        }
        assert_eq!(starts, [15, 10_015, 20_015, 30_015]);
    }

    #[test]
    fn grain_duration_changes_apply_at_the_next_scheduled_spawn() {
        let mut stretch = configured_stretch(1.0, 0.0, 8.0, 0.0);
        for _ in 0..20 {
            stretch.process_sample(0.25, -0.5);
        }
        stretch.set_param(2, 31.0);
        for sample in 20..80 {
            stretch.process_sample(0.25, -0.5);
            assert_eq!(stretch.grains[0].length, 128.0);
            assert_eq!(stretch.grains[0].elapsed, (sample - 14) as f32);
            if sample < 79 {
                assert!(stretch.grains[1..].iter().all(|grain| !grain.active));
            }
        }
        assert_eq!(stretch.grains[1].length, 34.0);
        assert_eq!(stretch.grains[1].elapsed, 1.0);
        assert_eq!(stretch.grain_samples_until_spawn, 16);
    }

    #[test]
    fn refresh_and_grain_lengths_follow_tempo_sample_rate_and_enum_rounding() {
        let beats = [
            0.0625, 0.09375, 0.125, 0.1875, 0.25, 0.375, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0,
        ];
        let mut stretch = configured_stretch(0.5, 8.0, 8.0, 0.0);
        for (index, beats) in beats.into_iter().enumerate() {
            assert_eq!(StretchEffect::refresh_beats(index), beats);
            stretch.set_param(1, index as f32);
            for (sample_rate, bpm, samples_per_beat) in
                [(1_024.0, 240.0, 256.0), (48_000.0, 120.0, 24_000.0)]
            {
                stretch.set_sample_rate(sample_rate);
                stretch.set_bpm(bpm);
                assert_eq!(stretch.capture_period_samples(), beats * samples_per_beat);
                assert_eq!(stretch.grain_length_samples(), sample_rate / 8.0);
            }
        }
        for index in [14, usize::MAX] {
            assert_eq!(StretchEffect::refresh_beats(index), 1.0);
        }
        for (sample_rate, hz, samples) in [
            (48_000.0, 31.0, 1_548.0),
            (44_100.0, 200.0, 220.0),
            (1_024.0, 31.0, 34.0),
        ] {
            stretch.set_sample_rate(sample_rate);
            stretch.set_param(2, hz);
            assert_eq!(stretch.grain_length_samples(), samples);
        }
        stretch.set_param(1, -1.0);
        assert_eq!(stretch.refresh_idx, 0.0);
        stretch.set_param(1, 6.49);
        assert_eq!(stretch.refresh_idx, 6.0);
        stretch.set_param(1, 6.5);
        assert_eq!(stretch.refresh_idx, 7.0);
        stretch.set_param(3, -1.0);
        assert_eq!(stretch.smooth_pct, 0.0);
        stretch.set_sample_rate(0.0);
        stretch.set_bpm(-120.0);
        assert_eq!(stretch.sample_rate, 1.0);
        assert_eq!(stretch.bpm, 1.0);
        assert_eq!(stretch.grain_length_samples(), 2.0);
        stretch.set_bpm(1_000_000.0);
        assert_eq!(stretch.capture_period_samples(), 1.0);
        stretch.set_sample_rate(f32::INFINITY);
        stretch.set_bpm(f32::INFINITY);
        assert_eq!(stretch.capture_period_samples(), 1.0);
    }

    #[test]
    fn parameter_changes_preserve_active_grains_capture_and_clocks() {
        let mut changed = configured_stretch(0.5, 8.0, 8.0, 50.0);
        let mut reference = configured_stretch(0.5, 8.0, 8.0, 50.0);
        for sample in 0..320 {
            let input = ((sample as f32 * 0.17).sin(), (sample as f32 * 0.31).cos());
            changed.process_sample(input.0, input.1);
            reference.process_sample(input.0, input.1);
        }
        changed.set_sample_rate(48_000.0);
        changed.set_bpm(120.0);
        for (index, value) in [0.25, 13.0, 200.0, 100.0].into_iter().enumerate() {
            changed.set_param(index, value);
        }
        changed.set_sample_rate(1_024.0);
        changed.set_bpm(240.0);
        for (index, value) in [0.5, 8.0, 8.0, 50.0].into_iter().enumerate() {
            changed.set_param(index, value);
        }
        changed.set_param(99, 123.0);
        for sample in 320..720 {
            let input = ((sample as f32 * 0.13).cos(), (sample as f32 * 0.23).sin());
            assert_eq!(
                changed.process_sample(input.0, input.1),
                reference.process_sample(input.0, input.1)
            );
        }
    }

    #[test]
    fn reset_replays_a_fresh_granulator_with_the_current_settings() {
        let mut used = configured_stretch(0.25, 8.0, 16.0, 25.0);
        let mut fresh = configured_stretch(0.25, 8.0, 16.0, 25.0);
        for sample in 0..333 {
            used.process_sample((sample as f32 * 0.17).sin(), 0.5);
        }
        used.reset();
        for sample in 0..512 {
            let input = ((sample as f32 * 0.13).cos(), -0.25);
            assert_eq!(
                used.process_sample(input.0, input.1),
                fresh.process_sample(input.0, input.1)
            );
        }
    }

    #[test]
    fn stretch_lists_four_parameters_with_the_expected_specs() {
        let s = StretchEffect::new();
        let specs = s.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Pace");
        assert_eq!(specs[0].min, 0.05);
        assert_eq!(specs[0].max, 1.0);
        assert_eq!(specs[1].name, "Refresh");
        assert!(matches!(specs[1].format, ParamFormat::Enum { .. }));
        assert_eq!(specs[2].name, "Grain");
        assert!(matches!(specs[2].scaling, ParamScaling::Log));
        assert!(matches!(specs[2].format, ParamFormat::Hertz));
        assert_eq!(specs[2].min, 5.0);
        assert_eq!(specs[2].max, 200.0);
        assert_eq!(specs[3].name, "Smooth");
        assert_eq!(specs[3].max, 100.0);
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.min, spec.max, spec.default))
                .collect::<Vec<_>>(),
            [
                (0.05, 1.0, 0.5),
                (0.0, 13.0, 8.0),
                (5.0, 200.0, 30.0),
                (0.0, 100.0, 50.0)
            ]
        );
        assert!(matches!(specs[0].scaling, ParamScaling::Linear));
        assert!(matches!(
            specs[0].format,
            ParamFormat::Number {
                decimals: 2,
                unit: "x"
            }
        ));
        assert!(matches!(specs[1].scaling, ParamScaling::Linear));
        assert!(matches!(
            specs[1].format,
            ParamFormat::Enum {
                labels: [
                    "1/64", "1/64.", "1/32", "1/32.", "1/16", "1/16.", "1/8", "1/8.", "1/4",
                    "1/4.", "1/2", "1/2.", "1/1", "1/1."
                ]
            }
        ));
        assert!(matches!(specs[3].scaling, ParamScaling::Linear));
        assert!(matches!(
            specs[3].format,
            ParamFormat::Number {
                decimals: 0,
                unit: "%"
            }
        ));
    }

    #[test]
    fn stretch_set_param_clamps_each_slot() {
        let mut s = StretchEffect::new();
        s.set_param(0, 5.0);
        assert_eq!(s.pace, 1.0);
        s.set_param(0, -1.0);
        assert_eq!(s.pace, 0.05);
        // Refresh clamps to [0, REFRESH_LABELS.len() - 1] = [0, 13].
        s.set_param(1, 99.0);
        assert_eq!(s.refresh_idx, 13.0);
        s.set_param(2, 9_999.0);
        assert_eq!(s.grain_hz, 200.0);
        s.set_param(2, 0.0);
        assert_eq!(s.grain_hz, 5.0);
        s.set_param(3, 999.0);
        assert_eq!(s.smooth_pct, 100.0);
    }

    #[test]
    fn stretch_outputs_dry_passthrough_until_primed() {
        // The capture window = capture_period × Pace. At 30 BPM,
        // Refresh = 1/1. (idx 13 = 6 beats), Pace = 1.0 → capture
        // window ≈ 12 s × 48 kHz = 576k samples. Test span of 100
        // samples is dwarfed by that priming threshold; output must
        // be exactly dry over the whole span.
        let mut s = StretchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_bpm(30.0);
        s.set_param(1, 13.0); // Refresh → 1/1.
        s.set_param(0, 1.0); // Pace → 1.0
        for i in 0..100 {
            let dry = 0.1 + i as f32 * 0.001;
            let (l, r) = s.process_sample(dry, -dry);
            assert!((l - dry).abs() < 1e-6, "sample {i} L not dry: {l}");
            assert!((r + dry).abs() < 1e-6, "sample {i} R not dry: {r}");
        }
    }

    #[test]
    fn stretch_capture_window_scales_with_pace() {
        // capture_window = capture_period × pace. Verify two different
        // Pace values produce proportional capture windows on the same
        // Refresh setting and BPM.
        let mut s = StretchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_bpm(120.0);
        s.set_param(1, 8.0); // Refresh → 1/4 note (= 24000 samples @ 120 BPM)
        s.set_param(0, 0.5); // Pace → 0.5
        // Pump enough samples to fire several Refresh ticks at the
        // smaller capture window (12000 samples) so primed flips.
        for _ in 0..48_000 {
            let _ = s.process_sample(0.5, 0.5);
        }
        let window_at_half = s.capture_window;
        assert!(
            (11_990.0..=12_010.0).contains(&window_at_half),
            "Pace=0.5 should give ~12000-sample window, got {}",
            window_at_half
        );

        // Reset and try Pace = 0.25.
        s.reset();
        s.set_param(0, 0.25);
        for _ in 0..48_000 {
            let _ = s.process_sample(0.5, 0.5);
        }
        let window_at_quarter = s.capture_window;
        assert!(
            (5_990.0..=6_010.0).contains(&window_at_quarter),
            "Pace=0.25 should give ~6000-sample window, got {}",
            window_at_quarter
        );
    }

    #[test]
    fn stretch_output_is_bounded_under_aggressive_settings() {
        // Worst-case: Pace at minimum, Refresh = 1/16, Grain at min
        // (long grains), Smooth = 0 (boxcar). The COLA correction
        // (1 / (2 - smooth)) keeps the output near unit amplitude
        // even at Smooth=0 where adjacent grains' boxcar windows
        // would otherwise double.
        let mut s = StretchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_bpm(120.0);
        s.set_param(0, 0.05); // Pace minimum
        s.set_param(1, 4.0); // Refresh → 1/16
        s.set_param(2, 5.0); // Grain minimum
        s.set_param(3, 0.0); // Smooth = 0 (boxcar)
        for i in 0..48_000 {
            let t = i as f32 / 48_000.0;
            let dry = 0.7 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, r) = s.process_sample(dry, dry);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            // 1.5× input amplitude headroom — even with 2 boxcar
            // grains briefly summing at scale 0.5 the peak shouldn't
            // exceed ~1.0.
            assert!(
                l.abs() < 1.5 && r.abs() < 1.5,
                "sample {i} blew up: ({l}, {r})"
            );
        }
    }

    #[test]
    fn stretch_reset_clears_state_and_returns_to_passthrough() {
        let mut s = StretchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_param(0, 0.5);
        // Pump enough audio to prime.
        for _ in 0..48_000 {
            let _ = s.process_sample(0.5, 0.5);
        }
        assert!(s.primed);
        s.reset();
        assert!(!s.primed);
        assert_eq!(s.samples_since_reset, 0);
        assert_eq!(s.write_idx, 0);
        assert_eq!(s.read_pos, 0.0);
        assert_eq!(s.capture_window, 0.0);
        assert!(s.grains.iter().all(|g| !g.active));
        assert!(s.ring_l.iter().all(|&v| v == 0.0));
        assert!(s.ring_r.iter().all(|&v| v == 0.0));
        // First post-reset sample is dry passthrough.
        let (l, r) = s.process_sample(0.42, -0.42);
        assert!((l - 0.42).abs() < 1e-6);
        assert!((r + 0.42).abs() < 1e-6);
    }

    #[test]
    fn stretch_reports_zero_pdc_latency() {
        // The granulation delay (one Refresh interval) IS the effect —
        // not a fixed plugin delay that PDC should compensate for.
        // Reports zero; the host sees Stretch as sample-aligned.
        let s = StretchEffect::new();
        assert_eq!(s.latency_samples(), 0);
    }
}
