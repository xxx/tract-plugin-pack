use tract_dsp::db::db_to_linear_fast;

const DETECTOR_FLOOR: f32 = 1e-20;
const MAX_DOWNWARD_KNEE_SHIFT_DB: f32 = 12.0;

#[derive(Clone, Copy)]
pub(crate) struct BandControls {
    pub threshold: f32,
    pub gain: f32,
    pub upward: bool,
    pub downward: bool,
}

#[derive(Clone, Copy)]
struct Calibration {
    upward_knee: f32,
    downward_knee: f32,
    downward_slope: f32,
    threshold_span: f32,
    gain_floor: f32,
    attack_ms: f32,
    release_ms: f32,
    makeup: [f32; 3],
}

// Calibrated with band-isolated OTT tone and level-step measurements.
const BANDS: [Calibration; 3] = [
    Calibration {
        upward_knee: -46.0,
        downward_knee: -38.94,
        downward_slope: 1.0,
        threshold_span: 23.0,
        gain_floor: -26.3,
        attack_ms: 6.2,
        release_ms: 32.44,
        makeup: [22.918, -13.640, 4.662],
    },
    Calibration {
        upward_knee: -46.6,
        downward_knee: -34.82,
        downward_slope: 0.99,
        threshold_span: 23.7,
        gain_floor: -31.7,
        attack_ms: 6.0,
        release_ms: 32.40,
        makeup: [12.453, -4.137, 1.024],
    },
    Calibration {
        upward_knee: -47.44,
        downward_knee: -41.69,
        downward_slope: 0.99,
        threshold_span: 24.0,
        gain_floor: -26.0,
        attack_ms: 9.7,
        release_ms: 15.16,
        makeup: [22.918, -13.640, 4.662],
    },
];

#[derive(Clone, Copy)]
pub(crate) struct Ramp {
    value: f32,
    step: f32,
}

impl Ramp {
    pub fn new(value: bool, sample_rate: f32) -> Self {
        Self {
            value: f32::from(value),
            step: 1.0 / (sample_rate * 0.005).max(1.0),
        }
    }

    #[inline]
    pub fn next(&mut self, target: bool) -> f32 {
        self.value = if target {
            (self.value + self.step).min(1.0)
        } else {
            (self.value - self.step).max(0.0)
        };
        self.value
    }
}

pub(crate) struct Dynamics {
    curve: Calibration,
    power: f32,
    attack: f32,
    release: f32,
    upward: Ramp,
    downward: Ramp,
}

impl Dynamics {
    pub fn new(index: usize, sample_rate: f32, time: f32, controls: BandControls) -> Self {
        let curve = BANDS[index];
        let mut band = Self {
            curve,
            power: 0.0,
            attack: coefficient(curve.attack_ms, sample_rate),
            release: 0.0,
            upward: Ramp::new(controls.upward, sample_rate),
            downward: Ramp::new(controls.downward, sample_rate),
        };
        band.set_time(time, sample_rate);
        band
    }

    pub fn set_time(&mut self, time: f32, sample_rate: f32) {
        self.release = coefficient((time * self.curve.release_ms).max(10.0), sample_rate);
    }

    pub fn tail_samples(&self) -> u32 {
        if self.power == 0.0 {
            return 0;
        }

        // Dividing by the coefficient slightly overestimates the exponential decay time.
        ((self.power.ln() - DETECTOR_FLOOR.ln()) / self.release)
            .ceil()
            .max(1.0) as u32
    }

    #[inline]
    pub fn process(
        &mut self,
        stereo: [f32; 2],
        depth: f32,
        upward: f32,
        downward: f32,
        controls: BandControls,
    ) -> (f32, f32) {
        let [left, right] = stereo;
        let power = (left * left + right * right) * 0.5;
        let coefficient = if power > self.power {
            self.attack
        } else {
            self.release
        };
        self.power += coefficient * (power - self.power);
        if self.power <= DETECTOR_FLOOR {
            self.power = 0.0;
        }
        let up = self.upward.next(controls.upward) * upward;
        let down = self.downward.next(controls.downward) * downward;
        if depth == 0.0 {
            return (controls.gain, 0.0);
        }

        let level = 10.0 * self.power.max(DETECTOR_FLOOR).log10();
        let gain_db = self
            .curve
            .gain_db(level, controls.threshold, depth, up, down);
        let displayed_gain = if self.power > 1e-14 { gain_db } else { 0.0 };
        (db_to_linear_fast(gain_db) * controls.gain, displayed_gain)
    }
}

impl Calibration {
    #[inline]
    fn gain_db(self, level: f32, threshold: f32, depth: f32, up: f32, down: f32) -> f32 {
        let shift = (threshold - 1.0) * self.threshold_span;
        let up_db = (0.75 * depth * up * knee(self.upward_knee + shift - level)).min(36.0);
        let strength = depth * down;
        let overdrive = (strength - 1.0).max(0.0) * MAX_DOWNWARD_KNEE_SHIFT_DB;
        let down_db = strength.min(1.0)
            * self.downward_slope
            * knee(level - self.downward_knee - shift + overdrive);
        let [a, b, c] = self.makeup;
        let makeup_db = depth * (a + depth * (b + depth * c));
        (makeup_db + up_db - down_db).max(self.gain_floor)
    }
}

#[inline]
fn knee(x: f32) -> f32 {
    let t = (x + 0.25).clamp(0.0, 0.5);
    x.max(0.25) - 0.25 + t * t
}

fn coefficient(milliseconds: f32, sample_rate: f32) -> f32 {
    -(-1000.0 / (milliseconds * sample_rate)).exp_m1()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTROLS: BandControls = BandControls {
        threshold: 1.0,
        gain: 1.0,
        upward: true,
        downward: true,
    };

    #[test]
    fn factory_curve_reproduces_high_band_tone_measurements() {
        for (level, measured) in [
            (-100.0, 49.94),
            (-68.04, 29.38),
            (-43.04, 13.94),
            (-28.04, -0.03),
        ] {
            let actual = BANDS[2].gain_db(level, 1.0, 1.0, 1.0, 1.0);
            assert!((actual - measured).abs() < 0.6, "{level}: {actual}");
        }
    }

    #[test]
    fn release_in_milliseconds_is_independent_of_sample_rate() {
        let release = |sample_rate: f32| {
            let mut dynamics = Dynamics::new(2, sample_rate, 1.0, CONTROLS);
            dynamics.power = 1.0;
            for _ in 0..(sample_rate * 0.04) as usize {
                dynamics.process([0.0; 2], 1.0, 1.0, 1.0, CONTROLS);
            }
            dynamics.power
        };
        assert!((release(48_000.0) - release(96_000.0)).abs() < 1e-6);
    }

    #[test]
    fn band_gain_and_direction_controls_are_independent() {
        let low_level = -75.0;
        let high_level = -10.0;
        let curve = BANDS[0];
        assert_eq!(
            curve.gain_db(low_level, 1.0, 1.0, 0.0, 1.0),
            curve.gain_db(-42.0, 1.0, 1.0, 0.0, 0.0)
        );
        assert_eq!(
            curve.gain_db(high_level, 1.0, 1.0, 1.0, 0.0),
            curve.gain_db(-42.0, 1.0, 1.0, 0.0, 0.0)
        );
    }

    #[test]
    fn downward_strength_above_unity_keeps_reducing_gain_at_full_depth() {
        for curve in BANDS {
            let gain = |down| curve.gain_db(-26.0, 1.0, 1.0, 1.0, down);
            assert!(gain(1.0) > gain(1.5) && gain(1.5) > gain(2.0));
        }
    }
}
