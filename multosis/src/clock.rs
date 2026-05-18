//! The tempo-synced step clock that drives wavefront propagation.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §5.2.

/// How fast the wavefront advances — a musical note division.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Speed {
    /// 1/32 note.
    Div32,
    /// 1/16 note.
    Div16,
    /// 1/8 note.
    Div8,
    /// 1/4 note.
    Div4,
    /// 1/2 note.
    Div2,
    /// Whole note.
    Div1,
}

impl Speed {
    /// All six speeds, fastest to slowest.
    pub const ALL: [Speed; 6] = [
        Speed::Div32,
        Speed::Div16,
        Speed::Div8,
        Speed::Div4,
        Speed::Div2,
        Speed::Div1,
    ];

    /// The length of one step in quarter notes (a 1/16 note is 0.25 quarter
    /// notes; a whole note is 4.0).
    pub fn quarter_notes(self) -> f64 {
        match self {
            Speed::Div32 => 0.125,
            Speed::Div16 => 0.25,
            Speed::Div8 => 0.5,
            Speed::Div4 => 1.0,
            Speed::Div2 => 2.0,
            Speed::Div1 => 4.0,
        }
    }
}

/// Samples spanning one step at the given speed, tempo, and sample rate.
/// `bpm` is quarter notes per minute.
pub fn samples_per_step(speed: Speed, bpm: f64, sample_rate: f64) -> f64 {
    let seconds_per_quarter = 60.0 / bpm;
    speed.quarter_notes() * seconds_per_quarter * sample_rate
}

/// Accumulates elapsed samples and reports when step boundaries are crossed.
/// The accumulator is the number of samples elapsed since the last boundary;
/// it is always kept in `[0, samples_per_step)`.
#[derive(Clone, Copy, Debug)]
pub struct StepClock {
    accum: f64,
}

impl StepClock {
    /// A clock with its accumulator at zero — the first boundary is a full
    /// step away.
    pub fn new() -> Self {
        Self { accum: 0.0 }
    }

    /// Clear the accumulator. Used when the sequence is reset so the next
    /// step boundary is a full step away.
    pub fn reset(&mut self) {
        self.accum = 0.0;
    }

    /// Advance the clock across a process block of `block_len` samples at the
    /// given `samples_per_step`. `on_step` is called once per step boundary
    /// that falls within the block, with the sample offset of the boundary
    /// inside the block. A non-positive `samples_per_step` fires nothing.
    pub fn advance(
        &mut self,
        block_len: usize,
        samples_per_step: f64,
        mut on_step: impl FnMut(usize),
    ) {
        if samples_per_step <= 0.0 {
            return;
        }
        let block = block_len as f64;
        // The first boundary lands `samples_per_step - accum` samples in.
        let mut boundary = samples_per_step - self.accum;
        while boundary < block {
            let offset = if boundary < 0.0 { 0 } else { boundary as usize };
            on_step(offset);
            boundary += samples_per_step;
        }
        self.accum = (self.accum + block).rem_euclid(samples_per_step);
    }
}

impl Default for StepClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_all_lists_six_divisions() {
        assert_eq!(Speed::ALL.len(), 6);
    }

    #[test]
    fn speed_quarter_notes_are_correct() {
        assert_eq!(Speed::Div32.quarter_notes(), 0.125);
        assert_eq!(Speed::Div16.quarter_notes(), 0.25);
        assert_eq!(Speed::Div8.quarter_notes(), 0.5);
        assert_eq!(Speed::Div4.quarter_notes(), 1.0);
        assert_eq!(Speed::Div2.quarter_notes(), 2.0);
        assert_eq!(Speed::Div1.quarter_notes(), 4.0);
    }

    #[test]
    fn samples_per_step_at_120_bpm() {
        // 120 BPM -> 0.5 s per quarter note. A 1/16 step is 0.25 quarter
        // notes -> 0.125 s -> 6000 samples at 48 kHz.
        let n = samples_per_step(Speed::Div16, 120.0, 48_000.0);
        assert!((n - 6000.0).abs() < 1e-6, "got {n}");
        // A 1/4 step at 120 BPM is 0.5 s -> 24000 samples.
        let q = samples_per_step(Speed::Div4, 120.0, 48_000.0);
        assert!((q - 24_000.0).abs() < 1e-6, "got {q}");
    }

    #[test]
    fn step_clock_fires_at_each_boundary_with_offsets() {
        let mut clk = StepClock::new();
        let mut offsets = Vec::new();
        // 100 samples per step; a 250-sample block crosses two boundaries.
        clk.advance(250, 100.0, |off| offsets.push(off));
        assert_eq!(offsets, vec![100, 200]);
    }

    #[test]
    fn step_clock_carries_the_remainder_across_blocks() {
        let mut clk = StepClock::new();
        let mut offsets = Vec::new();
        clk.advance(250, 100.0, |off| offsets.push(off)); // accum left at 50
        offsets.clear();
        // Next boundary is 50 samples in (100 - 50 carried).
        clk.advance(100, 100.0, |off| offsets.push(off));
        assert_eq!(offsets, vec![50]);
    }

    #[test]
    fn step_clock_block_shorter_than_a_step_fires_nothing() {
        let mut clk = StepClock::new();
        let mut count = 0;
        clk.advance(40, 100.0, |_| count += 1);
        clk.advance(40, 100.0, |_| count += 1);
        assert_eq!(count, 0); // 80 samples total, no boundary yet
        clk.advance(40, 100.0, |_| count += 1);
        assert_eq!(count, 1); // 120 samples total crosses the 100 boundary
    }

    #[test]
    fn step_clock_zero_samples_per_step_fires_nothing() {
        let mut clk = StepClock::new();
        let mut count = 0;
        clk.advance(1000, 0.0, |_| count += 1);
        assert_eq!(count, 0);
    }

    #[test]
    fn step_clock_reset_clears_the_accumulator() {
        let mut clk = StepClock::new();
        clk.advance(70, 100.0, |_| {});
        clk.reset();
        let mut offsets = Vec::new();
        // After reset the next boundary is a full step away.
        clk.advance(150, 100.0, |off| offsets.push(off));
        assert_eq!(offsets, vec![100]);
    }
}
