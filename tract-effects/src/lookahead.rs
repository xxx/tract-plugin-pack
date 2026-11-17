//! Preallocated audio input lookahead; setup/reserve stays off the audio thread.

pub const LOOKAHEAD_MS: f64 = 4.0;
pub const MAX_LOOKAHEAD_SAMPLES: usize = 4096;
pub(crate) fn is_off(value: &bool) -> bool {
    !*value
}
pub fn lookahead_samples(sr: f64) -> usize {
    if sr.is_finite() {
        (LOOKAHEAD_MS * sr * 0.001)
            .ceil()
            .clamp(0.0, MAX_LOOKAHEAD_SAMPLES as f64) as usize
    } else {
        0
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookaheadFrame {
    pub signal: [f32; 2],
    pub main: [f32; 2],
    pub sidechain: [f32; 2],
}
pub struct LookaheadDelay {
    samples: Vec<LookaheadFrame>,
    cursor: usize,
    queued: usize,
    delay: usize,
    path: u16,
}
impl LookaheadDelay {
    pub fn new(max_delay: usize) -> Self {
        Self {
            samples: vec![LookaheadFrame::default(); max_delay.min(MAX_LOOKAHEAD_SAMPLES) + 1],
            cursor: 0,
            queued: 0,
            delay: 0,
            path: 0,
        }
    }
    /// Call only during construction/sample-rate setup.
    pub fn reserve(&mut self, max_delay: usize) {
        let needed = max_delay.min(MAX_LOOKAHEAD_SAMPLES) + 1;
        if needed > self.samples.len() {
            self.samples.resize(needed, LookaheadFrame::default());
            self.reset();
        }
    }
    pub fn reset(&mut self) {
        self.cursor = 0;
        self.queued = 0;
    }
    pub fn delay(&self) -> usize {
        self.delay
    }
    pub fn prepare(&mut self, delay: usize) -> bool {
        let delay = delay.min(self.samples.len() - 1);
        let changed = delay != self.delay;
        if changed {
            self.reset();
            self.delay = delay;
        }
        changed
    }
    pub fn prepare_path(&mut self, delay: usize, path: u16) -> bool {
        let changed = self.prepare(delay);
        let topology = path != self.path;
        if topology {
            self.reset();
            self.path = path;
        }
        changed || topology
    }
    pub fn next(&mut self, input: LookaheadFrame) -> LookaheadFrame {
        if self.delay == 0 {
            return input;
        }
        self.samples[self.cursor] = input;
        let index = (self.cursor + self.samples.len() - self.delay) % self.samples.len();
        let output = if self.queued >= self.delay {
            self.samples[index]
        } else {
            LookaheadFrame::default()
        };
        self.cursor = (self.cursor + 1) % self.samples.len();
        self.queued = (self.queued + 1).min(self.samples.len());
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(i: usize) -> LookaheadFrame {
        let x = i as f32;
        LookaheadFrame {
            signal: [x, -x],
            main: [x + 10.0, x + 11.0],
            sidechain: [x + 20.0, x + 21.0],
        }
    }
    #[test]
    fn exact_frame_delay_and_zero_branch_keep_signal_and_original_inputs_aligned() {
        for delay in [0, 1, 4, 177, 4096] {
            let mut line = LookaheadDelay::new(delay);
            line.prepare(delay);
            for i in 0..delay + 8 {
                assert_eq!(
                    line.next(frame(i)),
                    if i < delay {
                        LookaheadFrame::default()
                    } else {
                        frame(i - delay)
                    }
                );
            }
        }
        let mut line = LookaheadDelay::new(8);
        let input = LookaheadFrame {
            signal: [-0.0, 0.0],
            ..LookaheadFrame::default()
        };
        assert_eq!(line.next(input).signal[0].to_bits(), (-0.0_f32).to_bits());
    }
    #[test]
    fn changes_and_resets_discard_old_audio_before_new_warmup() {
        let mut line = LookaheadDelay::new(8);
        line.prepare(4);
        for i in 0..8 {
            line.next(frame(i));
        }
        line.prepare(2);
        assert_eq!(line.next(frame(100)), LookaheadFrame::default());
        assert_eq!(line.next(frame(101)), LookaheadFrame::default());
        assert_eq!(line.next(frame(102)), frame(100));
        line.reset();
        assert_eq!(line.next(frame(200)), LookaheadFrame::default());
        line.prepare(0);
        assert_eq!(line.next(frame(201)), frame(201));
        assert_eq!(lookahead_samples(44100.0), 177);
        assert_eq!(lookahead_samples(48000.0), 192);
        assert_eq!(lookahead_samples(f64::NAN), 0);
    }
    #[test]
    fn redistributing_a_path_with_the_same_delay_still_discards_old_timestamps() {
        let mut line = LookaheadDelay::new(8);
        line.prepare_path(4, 1);
        for i in 0..12 {
            line.next(frame(i));
        }
        assert!(line.prepare_path(4, 2));
        for i in 0..4 {
            assert_eq!(line.next(frame(100 + i)), LookaheadFrame::default());
        }
        assert_eq!(line.next(frame(104)), frame(100));
        assert!(!line.prepare_path(4, 2));
        assert_eq!(line.next(frame(105)), frame(101));
    }

    #[test]
    fn reserve_grows_for_a_new_sample_rate_and_preserves_history_without_growth() {
        let mut line = LookaheadDelay::new(2);
        assert!(line.prepare(20));
        assert_eq!(line.delay(), 2);
        assert_eq!(line.next(frame(1)), LookaheadFrame::default());
        assert_eq!(line.next(frame(2)), LookaheadFrame::default());
        line.reserve(2);
        assert_eq!(line.next(frame(3)), frame(1));
        line.reserve(1);
        assert_eq!(line.next(frame(4)), frame(2));

        line.reserve(5);
        assert!(line.prepare(20));
        assert_eq!(line.delay(), 5);
        for i in 0..5 {
            assert_eq!(line.next(frame(10 + i)), LookaheadFrame::default());
        }
        assert_eq!(line.next(frame(15)), frame(10));

        line.reserve(8192);
        assert!(line.prepare(8192));
        assert_eq!(line.delay(), MAX_LOOKAHEAD_SAMPLES);
        for i in 0..MAX_LOOKAHEAD_SAMPLES {
            assert_eq!(line.next(frame(i)), LookaheadFrame::default());
        }
        assert_eq!(line.next(frame(MAX_LOOKAHEAD_SAMPLES)), frame(0));
    }

    #[test]
    fn invalid_sample_rates_use_no_lookahead_and_extreme_finite_rates_are_capped() {
        for rate in [
            0.0,
            -48000.0,
            -f64::MAX,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ] {
            assert_eq!(lookahead_samples(rate), 0);
        }
        assert_eq!(lookahead_samples(1.0), 1);
        assert_eq!(lookahead_samples(f64::MAX), MAX_LOOKAHEAD_SAMPLES);
    }
}
