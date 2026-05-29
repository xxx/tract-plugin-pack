//! Hand-designed coloration dictionary: `Q` one-pole lowpass filters ordered
//! dark→bright. Each velvet pulse is routed to one filter; the per-pulse
//! routing (driven by the Tone curve) shapes how the tail's spectrum evolves.

/// Number of dictionary filters.
pub const Q: usize = 6;

/// A single one-pole lowpass: `y[n] = (1-c)·x[n] + c·y[n-1]`, `c ∈ (0,1)`.
#[derive(Clone, Copy)]
pub struct OnePole {
    c: f32,
    z: f32,
}

impl OnePole {
    /// Build from a cutoff in Hz at `sample_rate`.
    pub fn new(cutoff_hz: f32, sample_rate: f32) -> Self {
        let c = (-2.0 * std::f32::consts::PI * cutoff_hz / sample_rate).exp();
        Self {
            c: c.clamp(0.0, 0.9999),
            z: 0.0,
        }
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.z = (1.0 - self.c) * x + self.c * self.z;
        self.z
    }
    pub fn reset(&mut self) {
        self.z = 0.0;
    }
    pub fn coeff(&self) -> f32 {
        self.c
    }
}

/// The full ordered dictionary.
pub struct Dictionary {
    pub filters: [OnePole; Q],
}

impl Dictionary {
    /// Cutoffs log-spaced from `LOW_HZ` (darkest) to `HIGH_HZ` (brightest).
    pub const LOW_HZ: f32 = 500.0;
    pub const HIGH_HZ: f32 = 18_000.0;

    pub fn new(sample_rate: f32) -> Self {
        let mut filters = [OnePole::new(Self::LOW_HZ, sample_rate); Q];
        let ratio = (Self::HIGH_HZ / Self::LOW_HZ).powf(1.0 / (Q - 1) as f32);
        for (i, f) in filters.iter_mut().enumerate() {
            let cutoff = Self::LOW_HZ * ratio.powi(i as i32);
            *f = OnePole::new(cutoff, sample_rate);
        }
        Self { filters }
    }

    pub fn reset(&mut self) {
        for f in &mut self.filters {
            f.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Estimate the normalized spectral centroid of a filter's impulse
    /// response via a coarse DFT magnitude sweep.
    fn centroid(mut f: OnePole, sample_rate: f32) -> f32 {
        // impulse response
        let n = 4096;
        let mut h = vec![0.0f32; n];
        h[0] = f.process(1.0);
        for s in h.iter_mut().skip(1) {
            *s = f.process(0.0);
        }
        // magnitude-weighted mean frequency over a log grid
        let mut num = 0.0f64;
        let mut den = 0.0f64;
        let mut freq = 20.0f32;
        while freq < sample_rate / 2.0 {
            let w = 2.0 * std::f32::consts::PI * freq / sample_rate;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (k, &hk) in h.iter().enumerate() {
                re += (hk as f64) * (w as f64 * k as f64).cos();
                im -= (hk as f64) * (w as f64 * k as f64).sin();
            }
            let mag = (re * re + im * im).sqrt();
            num += mag * freq as f64;
            den += mag;
            freq *= 1.1;
        }
        (num / den) as f32
    }

    #[test]
    fn dictionary_is_ordered_dark_to_bright() {
        let sr = 48_000.0;
        let dict = Dictionary::new(sr);
        let centroids: Vec<f32> = dict.filters.iter().map(|&f| centroid(f, sr)).collect();
        for w in centroids.windows(2) {
            assert!(
                w[1] > w[0],
                "centroid must increase with index: {centroids:?}"
            );
        }
    }

    #[test]
    fn all_filters_stable() {
        let dict = Dictionary::new(48_000.0);
        for f in &dict.filters {
            assert!(
                f.coeff() >= 0.0 && f.coeff() < 1.0,
                "one-pole coeff must be in [0,1)"
            );
        }
    }
}
