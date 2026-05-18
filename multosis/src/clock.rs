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
}
