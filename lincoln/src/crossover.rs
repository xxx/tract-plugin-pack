use std::f64::consts::{PI, SQRT_2};

pub(crate) const MIN_HZ: f32 = 20.0;
pub(crate) const MAX_HZ: f32 = 20_000.0;
const GAP: f32 = 1.059_463_1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Splits {
    pub lower: f32,
    pub upper: f32,
}

impl Splits {
    pub const FACTORY: Self = Self {
        lower: 88.3,
        upper: 2500.0,
    };

    pub fn maximum(sample_rate: f32) -> f32 {
        (sample_rate * 0.45).clamp(MIN_HZ * GAP, MAX_HZ)
    }

    pub fn new(lower: f32, upper: f32, sample_rate: f32) -> Self {
        let lower = if lower.is_finite() {
            lower
        } else {
            Self::FACTORY.lower
        };
        let upper = if upper.is_finite() {
            upper
        } else {
            Self::FACTORY.upper
        };
        let upper = upper.clamp(MIN_HZ * GAP, Self::maximum(sample_rate));
        let lower = lower.clamp(MIN_HZ, upper / GAP);
        Self { lower, upper }
    }

    pub fn limit_upper(self, upper: f32, sample_rate: f32) -> f32 {
        let max = Self::maximum(sample_rate);
        upper.clamp((self.lower * GAP).min(max), max)
    }

    pub fn to_bits(self) -> u64 {
        (u64::from(self.lower.to_bits()) << 32) | u64::from(self.upper.to_bits())
    }

    pub fn from_bits(bits: u64) -> Self {
        Self {
            lower: f32::from_bits((bits >> 32) as u32),
            upper: f32::from_bits(bits as u32),
        }
    }
}

#[derive(Clone, Copy)]
struct Coefficients {
    g: f64,
    integrator: f64,
    input: f64,
}

impl Coefficients {
    fn butterworth(frequency: f32, sample_rate: f32) -> Self {
        let g = (PI * f64::from(frequency) / f64::from(sample_rate)).tan();
        let integrator = (1.0 + g * (g + SQRT_2)).recip();
        Self {
            g,
            integrator,
            input: g * integrator,
        }
    }
}

#[derive(Default)]
struct Butterworth {
    band_state: f64,
    low_state: f64,
}

impl Butterworth {
    #[inline]
    fn outputs(&mut self, x: f64, c: Coefficients) -> (f64, f64, f64) {
        let band = c.integrator * self.band_state + c.input * (x - self.low_state);
        let low = self.low_state + c.g * band;
        self.band_state = 2.0 * band - self.band_state;
        self.low_state = 2.0 * low - self.low_state;
        (low, x - SQRT_2 * band - low, x - 2.0 * SQRT_2 * band)
    }
}

#[derive(Default)]
struct LinkwitzRiley {
    first: Butterworth,
    low: Butterworth,
    high: Butterworth,
}

impl LinkwitzRiley {
    #[inline]
    fn split(&mut self, x: f64, c: Coefficients) -> (f64, f64) {
        let (low, high, _) = self.first.outputs(x, c);
        (self.low.outputs(low, c).0, self.high.outputs(high, c).1)
    }
}

#[derive(Default)]
pub(crate) struct Channel {
    lower: LinkwitzRiley,
    upper: LinkwitzRiley,
    low_allpass: Butterworth,
}

#[derive(Clone, Copy)]
pub(crate) struct Crossover {
    lower: Coefficients,
    upper: Coefficients,
    low_compensation: f64,
}

impl Crossover {
    pub fn new(frequencies: Splits, sample_rate: f32) -> Self {
        let spacing = f64::from(frequencies.upper / frequencies.lower);
        let blend = ((20.0 - spacing) / 10.0).clamp(0.0, 1.0);
        Self {
            lower: Coefficients::butterworth(frequencies.lower, sample_rate),
            upper: Coefficients::butterworth(frequencies.upper, sample_rate),
            low_compensation: blend * blend * (3.0 - 2.0 * blend),
        }
    }

    #[inline]
    pub fn process(self, channel: &mut Channel, sample: f32) -> [f32; 3] {
        let (low, upper) = channel.lower.split(f64::from(sample), self.lower);
        let (mid, high) = channel.upper.split(upper, self.upper);
        let aligned_low = channel.low_allpass.outputs(low, self.upper).2;
        let low = low + self.low_compensation * (aligned_low - low);
        [low as f32, mid as f32, high as f32]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(splits: Splits, frequency: f32, gains: [f32; 3]) -> (f64, f64) {
        let crossover = Crossover::new(splits, 48_000.0);
        let mut state = Channel::default();
        (0..8192).fold((0.0, 0.0), |(real, imag), i| {
            let bands = crossover.process(&mut state, f32::from(i == 0));
            let sample: f64 = bands
                .into_iter()
                .zip(gains)
                .map(|(x, g)| f64::from(x * g))
                .sum();
            let phase = -2.0 * PI * f64::from(frequency) * f64::from(i) / 48_000.0;
            (real + sample * phase.cos(), imag + sample * phase.sin())
        })
    }

    #[test]
    fn factory_splits_match_measured_amplitude_and_phase() {
        let low = response(Splits::FACTORY, 88.3, [1.0, 0.0, 0.0]);
        assert!((low.0 + 0.5).abs() < 1e-6 && low.1.abs() < 1e-6);
        let mid = response(Splits::FACTORY, 2500.0, [0.0, 1.0, 0.0]);
        let high = response(Splits::FACTORY, 2500.0, [0.0, 0.0, 1.0]);
        assert!((mid.0.hypot(mid.1) - 0.5).abs() < 1e-5);
        assert!((mid.0 - high.0).abs() < 1e-7 && (mid.1 - high.1).abs() < 1e-7);
        let mut state = Channel::default();
        let first = Crossover::new(Splits::FACTORY, 48_000.0).process(&mut state, 1.0);
        assert!((first.into_iter().sum::<f32>() - 0.619_384_05).abs() < 2e-5);
    }

    #[test]
    fn close_bands_sum_without_cancellation() {
        let splits = Splits::new(1000.0, 1250.0, 48_000.0);
        for frequency in [750.0, 1000.0, 1125.0, 1250.0, 2000.0] {
            let (real, imag) = response(splits, frequency, [1.0; 3]);
            assert!((real.hypot(imag) - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn automation_targets_stay_ordered_below_nyquist() {
        for sample_rate in [8000.0, 32_000.0, 48_000.0, 192_000.0] {
            for upper in [MIN_HZ, 500.0, MAX_HZ] {
                let splits = Splits::new(MAX_HZ, upper, sample_rate);
                assert!(splits.lower < splits.upper && splits.upper < sample_rate * 0.5);
            }
        }
    }
}
