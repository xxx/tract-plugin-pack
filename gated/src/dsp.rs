//! Musical-position gate evaluation with constant per-sample work.

use crate::pattern::{MAX_STEPS, Pattern};

pub const RESOLUTIONS: [&str; 21] = [
    "1/1", "1/1 D", "1/1 T", "1/2", "1/2 D", "1/2 T", "1/4", "1/4 D", "1/4 T", "1/8", "1/8 D",
    "1/8 T", "1/16", "1/16 D", "1/16 T", "1/32", "1/32 D", "1/32 T", "1/64", "1/64 D", "1/64 T",
];

pub fn step_beats(resolution: usize) -> f64 {
    let index = resolution.min(RESOLUTIONS.len() - 1);
    let base = 4.0 / (1_u32 << (index / 3)) as f64;
    base * match index % 3 {
        1 => 1.5,
        2 => 2.0 / 3.0,
        _ => 1.0,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Envelope {
    pub attack: f64,
    pub decay: f64,
    pub sustain: f64,
    pub release: f64,
}

impl Envelope {
    fn held(&self, age: f64) -> f64 {
        if age < self.attack {
            age / self.attack
        } else if age < self.attack + self.decay {
            1.0 + (self.sustain - 1.0) * (age - self.attack) / self.decay
        } else {
            self.sustain
        }
    }

    pub fn value(&self, age: f64, duration: f64) -> f32 {
        if age < 0.0 {
            0.0
        } else if age < duration {
            self.held(age).clamp(0.0, 1.0) as f32
        } else if self.release > 0.0 && age < duration + self.release {
            (self.held(duration) * (1.0 - (age - duration) / self.release)).clamp(0.0, 1.0) as f32
        } else {
            0.0
        }
    }
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            attack: 0.002,
            decay: 0.05,
            sustain: 1.0,
            release: 0.02,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Note {
    on: f64,
    off: f64,
    exists: bool,
}

pub struct Sequence {
    pattern: Pattern,
    swing: f64,
    gate: f64,
    notes: [Note; MAX_STEPS],
    held: bool,
}

impl Default for Sequence {
    fn default() -> Self {
        let mut sequence = Self {
            pattern: Pattern::default(),
            swing: -1.0,
            gate: -1.0,
            notes: [Note::default(); MAX_STEPS],
            held: false,
        };
        sequence.prepare(Pattern::default(), 0.0, 0.5);
        sequence
    }
}

impl Sequence {
    pub fn prepare(&mut self, pattern: Pattern, swing: f64, gate: f64) -> bool {
        let swing = swing.clamp(0.0, 1.0);
        let gate = gate.clamp(0.01, 1.0);
        if self.pattern == pattern && self.swing == swing && self.gate == gate {
            return false;
        }
        self.pattern = pattern;
        self.swing = swing;
        self.gate = gate;
        let len = pattern.len();
        self.held = (0..len).all(|i| pattern.tied(i));
        for step in 0..len {
            let mut note = Note::default();
            for back in 0..len {
                let start = (step + len - back) % len;
                if pattern.is_on(start) && !pattern.tied((start + len - 1) % len) {
                    let cycle = if start > step { -(len as f64) } else { 0.0 };
                    let on = self.start(start) + cycle;
                    let mut final_step = start;
                    let mut off_cycle = cycle;
                    for _ in 0..len {
                        if !pattern.tied(final_step) {
                            break;
                        }
                        final_step += 1;
                        if final_step == len {
                            final_step = 0;
                            off_cycle += len as f64;
                        }
                    }
                    note = Note {
                        on,
                        off: self.start(final_step) + off_cycle + self.duration(final_step) * gate,
                        exists: true,
                    };
                    break;
                }
            }
            self.notes[step] = note;
        }
        true
    }

    fn start(&self, step: usize) -> f64 {
        step as f64 + if step % 2 == 1 { 0.5 * self.swing } else { 0.0 }
    }

    fn duration(&self, step: usize) -> f64 {
        if step % 2 == 1 {
            1.0 - 0.5 * self.swing
        } else if step + 1 < self.pattern.len() {
            1.0 + 0.5 * self.swing
        } else {
            1.0
        }
    }

    pub fn position(&self, beats: f64, resolution: usize) -> (usize, f64) {
        let (step, phase, _) = self.position_details(beats, resolution, 0.0);
        (step, phase)
    }

    fn position_details(
        &self,
        beats: f64,
        resolution: usize,
        epsilon_beats: f64,
    ) -> (usize, f64, f64) {
        let unit = step_beats(resolution);
        let units = beats / unit;
        let length = self.pattern.len() as f64;
        let epsilon = epsilon_beats / unit;
        let mut cycle = (units / length).floor();
        let mut phase = units.rem_euclid(length);
        if length - phase <= epsilon {
            cycle += 1.0;
            phase = 0.0;
        } else if phase <= epsilon {
            phase = 0.0;
        }
        let mut step = ((phase + epsilon).floor() as usize).min(self.pattern.len() - 1);
        if step % 2 == 1 && phase < self.start(step) - epsilon {
            step -= 1;
        }
        (step, phase, cycle * length * unit)
    }

    pub fn value(&self, beats: f64, resolution: usize, bpm: f64, envelope: Envelope) -> f32 {
        if self.held {
            return envelope.sustain.clamp(0.0, 1.0) as f32;
        }
        let (step, phase) = self.position(beats, resolution);
        let note = self.notes[step];
        if !note.exists {
            return 0.0;
        }
        let seconds_per_unit = step_beats(resolution) * 60.0 / bpm;
        envelope.value(
            (phase - note.on) * seconds_per_unit,
            (note.off - note.on) * seconds_per_unit,
        )
    }
}

/// Beat positions for one host callback, including a loop boundary within it.
pub struct BeatClock {
    pub start: f64,
    pub increment: f64,
    pub loop_range: Option<(f64, f64)>,
}

impl BeatClock {
    pub fn at(&self, offset: usize) -> f64 {
        let beat = self.start + offset as f64 * self.increment;
        let epsilon = self.increment.abs() * 1e-4;
        if let Some((start, end)) = self.loop_range
            && start.is_finite()
            && end.is_finite()
            && end > start
            && beat >= end - epsilon
        {
            let phase = (beat - end).rem_euclid(end - start);
            if phase < epsilon || end - start - phase < epsilon {
                start
            } else {
                start + phase
            }
        } else {
            beat
        }
    }
}

#[derive(Clone, Copy)]
pub struct RenderTime {
    pub beat: f64,
    pub resolution: usize,
    pub bpm: f64,
    pub sample_rate: f64,
}

#[derive(Default)]
pub struct EnvelopeClock {
    note_on: Option<f64>,
    age: f64,
    release: Option<(f64, f64)>,
    previous_beat: f64,
}

impl EnvelopeClock {
    pub fn value(
        &mut self,
        sequence: &Sequence,
        time: RenderTime,
        envelope: Envelope,
        reseed: bool,
    ) -> f32 {
        let RenderTime {
            beat,
            resolution,
            bpm,
            sample_rate,
        } = time;
        let dt = 1.0 / sample_rate;
        // Callback partitioning can round exact boundaries to either side.
        let epsilon = bpm / (60.0 * sample_rate) * 1e-4;
        let (step, phase, origin) = sequence.position_details(beat, resolution, epsilon);
        let note = sequence.notes[step];
        if sequence.held || !note.exists {
            self.note_on = None;
            return if sequence.held {
                envelope.sustain.clamp(0.0, 1.0) as f32
            } else {
                0.0
            };
        }
        let unit = step_beats(resolution);
        let on_beat = origin + note.on * unit;
        let off_beat = origin + note.off * unit;
        let new_note = self
            .note_on
            .is_none_or(|previous| (previous - on_beat).abs() > 1e-8);
        if reseed || new_note || beat < self.previous_beat {
            self.age = ((phase - note.on) * unit * 60.0 / bpm).max(0.0);
            let duration = (note.off - note.on) * unit * 60.0 / bpm;
            self.release = (beat >= off_beat - epsilon)
                .then(|| ((self.age - duration).max(0.0), envelope.held(duration)));
        } else {
            self.age += dt;
            if let Some((age, _)) = &mut self.release {
                *age += dt;
            } else if beat >= off_beat - epsilon {
                let fraction = ((beat - off_beat) / (beat - self.previous_beat)).clamp(0.0, 1.0);
                let overshoot = fraction * dt;
                self.release = Some((overshoot, envelope.held((self.age - overshoot).max(0.0))));
            }
        }
        self.note_on = Some(on_beat);
        self.previous_beat = beat;
        if let Some((age, level)) = self.release {
            if envelope.release > 0.0 {
                (level * (1.0 - age / envelope.release)).clamp(0.0, 1.0) as f32
            } else {
                0.0
            }
        } else {
            envelope.held(self.age).clamp(0.0, 1.0) as f32
        }
    }
}

pub struct GainSlew {
    value: f32,
    step: f32,
}

impl Default for GainSlew {
    fn default() -> Self {
        Self {
            value: 1.0,
            step: 1.0 / 96.0,
        }
    }
}

impl GainSlew {
    pub fn set_sample_rate(&mut self, rate: f32) {
        self.step = 1.0 / (0.002 * rate.max(1.0));
    }
    pub fn next(&mut self, target: f32) -> f32 {
        self.value += (target - self.value).clamp(-self.step, self.step);
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Envelope {
        Envelope {
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.0,
        }
    }

    #[test]
    fn adsr_releases_from_the_level_reached_during_attack() {
        let env = Envelope {
            attack: 0.1,
            decay: 0.2,
            sustain: 0.4,
            release: 0.1,
        };
        assert!((env.value(0.025, 0.05) - 0.25).abs() < 1e-6);
        assert!((env.value(0.075, 0.05) - 0.375).abs() < 1e-6);
        assert_eq!(env.value(0.151, 0.05), 0.0);
        assert!((env.value(0.2, 1.0) - 0.7).abs() < 1e-6);
        assert!((env.value(0.5, 1.0) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn ties_hold_until_the_final_steps_gate_and_wrap_across_zero() {
        let mut pattern = Pattern {
            length: 4,
            active: [0b1001, 0],
            ties: [0b1000, 0],
        };
        let mut seq = Sequence::default();
        seq.prepare(pattern, 0.0, 0.5);
        assert_eq!(seq.value(0.1, 12, 120.0, square()), 1.0);
        assert_eq!(seq.value(0.2, 12, 120.0, square()), 0.0);
        assert_eq!(seq.value(-0.1, 12, 120.0, square()), 1.0);
        pattern.ties[0] = 0;
        seq.prepare(pattern, 0.0, 0.5);
        assert_eq!(seq.value(-0.1, 12, 120.0, square()), 0.0);
    }

    #[test]
    fn fully_tied_circular_pattern_holds_sustain_and_empty_pattern_is_silent() {
        let mut seq = Sequence::default();
        seq.prepare(
            Pattern {
                ties: [0xffff, 0],
                ..Pattern::default()
            },
            1.0,
            0.01,
        );
        assert_eq!(
            seq.value(
                123.45,
                12,
                120.0,
                Envelope {
                    sustain: 0.3,
                    ..square()
                }
            ),
            0.3
        );
        seq.prepare(
            Pattern {
                active: [0, 0],
                ..Pattern::default()
            },
            0.0,
            1.0,
        );
        assert_eq!(seq.value(0.1, 12, 120.0, square()), 0.0);
    }

    #[test]
    fn swing_delays_alternate_steps_without_drifting_even_or_odd_cycles() {
        for len in [1, 2, 3, 16, 63, 64] {
            let mut seq = Sequence::default();
            seq.prepare(
                Pattern {
                    length: len as u16,
                    ..Pattern::default()
                },
                1.0,
                0.5,
            );
            assert!(((0..len).map(|i| seq.duration(i)).sum::<f64>() - len as f64).abs() < 1e-12);
            for cycle in [-10.0, 0.0, 1.0, 1000.0] {
                assert_eq!(seq.position(cycle * len as f64 * 0.25, 12).0, 0);
            }
            if len > 1 {
                assert_eq!(seq.position(0.3, 12).0, 0);
                assert_eq!(seq.position(0.375, 12).0, 1);
            }
        }
    }

    #[test]
    fn resolution_includes_straight_dotted_and_triplet_lengths() {
        assert_eq!(step_beats(0), 4.0);
        assert_eq!(step_beats(12), 0.25);
        assert_eq!(step_beats(13), 0.375);
        assert!((step_beats(14) - 1.0 / 6.0).abs() < 1e-12);
        assert_eq!(step_beats(18), 0.0625);
    }

    #[test]
    fn direct_seek_matches_continuous_evaluation_and_output_is_bounded() {
        let mut seq = Sequence::default();
        seq.prepare(
            Pattern {
                active: [0xedd9, 0],
                ties: [0x8801, 0],
                ..Pattern::default()
            },
            0.7,
            0.6,
        );
        for n in 0..96_000 {
            let beat = n as f64 / 24_000.0;
            let value = seq.value(beat, 12, 120.0, Envelope::default());
            assert!(value.is_finite() && (0.0..=1.0).contains(&value));
            if n % 512 == 0 {
                let mut sought = Sequence::default();
                sought.prepare(seq.pattern, 0.7, 0.6);
                assert_eq!(sought.value(beat, 12, 120.0, Envelope::default()), value);
            }
        }
    }

    #[test]
    fn slew_bounds_switches_and_reaches_exact_passthrough() {
        for rate in [44_100.0, 48_000.0, 192_000.0] {
            let mut slew = GainSlew::default();
            slew.set_sample_rate(rate);
            let mut previous = 1.0;
            for _ in 0..1000 {
                let value = slew.next(0.0);
                assert!((value - previous).abs() <= 1.0 / (rate * 0.002) + 1e-7);
                previous = value;
            }
            for _ in 0..1000 {
                slew.next(1.0);
            }
            assert_eq!(slew.next(1.0), 1.0);
        }
    }

    #[test]
    fn tempo_changes_preserve_millisecond_attack_and_release_times() {
        let mut sequence = Sequence::default();
        sequence.prepare(
            Pattern {
                length: 2,
                active: [1, 0],
                ties: [0, 0],
            },
            0.0,
            0.5,
        );
        let envelope = Envelope {
            attack: 2.0,
            decay: 0.0,
            sustain: 1.0,
            release: 2.0,
        };
        let mut clock = EnvelopeClock::default();
        let mut beat = 0.0;
        let mut previous = 0.0;
        for sample in 0..1500 {
            let bpm = if sample < 500 { 120.0 } else { 240.0 };
            let value = clock.value(
                &sequence,
                RenderTime {
                    beat,
                    resolution: 0,
                    bpm,
                    sample_rate: 1000.0,
                },
                envelope,
                sample == 0,
            );
            if sample == 500 {
                assert!((value - 0.25).abs() < 1e-5);
            }
            if sample > 750 {
                assert!(value <= previous + 1e-6, "release must not regain level");
            }
            previous = value;
            beat += bpm / 60_000.0;
        }
    }

    #[test]
    fn rendering_a_loop_is_invariant_to_callback_partitioning() {
        fn render(block_size: usize, rate: f64, envelope: Envelope) -> Vec<f32> {
            let mut sequence = Sequence::default();
            sequence.prepare(
                Pattern {
                    length: 3,
                    active: [1, 0],
                    ties: [0, 0],
                },
                0.8,
                0.6,
            );
            let mut envelope_clock = EnvelopeClock::default();
            let mut gain = GainSlew::default();
            gain.set_sample_rate(rate as f32);
            let mut output = Vec::new();
            let mut start = -0.1;
            let samples = (rate * 3.0) as usize;
            while output.len() < samples {
                let count = block_size.min(samples - output.len());
                let beats = BeatClock {
                    start,
                    increment: 2.0 / rate,
                    loop_range: Some((0.0, 4.0)),
                };
                for index in 0..count {
                    let value = envelope_clock.value(
                        &sequence,
                        RenderTime {
                            beat: beats.at(index),
                            resolution: 12,
                            bpm: 120.0,
                            sample_rate: rate,
                        },
                        envelope,
                        output.is_empty(),
                    );
                    output.push(gain.next(value));
                }
                start = beats.at(count);
            }
            output
        }
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            for envelope in [
                Envelope::default(),
                square(),
                Envelope {
                    release: 4.0,
                    ..square()
                },
            ] {
                let reference = render(1, rate, envelope);
                for block in [17, 64, 511, 1024] {
                    for (i, (a, b)) in render(block, rate, envelope)
                        .iter()
                        .zip(&reference)
                        .enumerate()
                    {
                        assert!(
                            (a - b).abs() < 1e-4,
                            "buffer-size difference at {rate} Hz, sample {i}: {a} vs {b}"
                        );
                    }
                }
            }
        }
        let clock = BeatClock {
            start: -1.0,
            increment: 0.5,
            loop_range: Some((1.0, 3.0)),
        };
        assert_eq!(clock.at(0), -1.0);
        assert_eq!(clock.at(8), 1.0);
    }
}
