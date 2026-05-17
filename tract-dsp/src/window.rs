//! Window functions for spectral analysis.

use std::f32::consts::PI;

/// Periodic (DFT) Hann window of `n` samples: `w[i] = 0.5·(1 − cos(2π·i/n))`.
///
/// The correct variant for STFT analysis windows — it gives clean
/// constant-overlap-add reconstruction. Returns an empty `Vec` for `n == 0`.
pub fn hann_periodic(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / n as f32).cos()))
        .collect()
}

/// Symmetric Hann window of `n` samples: `w[i] = 0.5 − 0.5·cos(2π·i/(n−1))`.
///
/// For one-shot spectral analysis. For `n < 2` the `n−1` denominator is
/// degenerate, so a flat `vec![1.0; n]` is returned.
pub fn hann_symmetric(n: usize) -> Vec<f32> {
    if n < 2 {
        return vec![1.0; n];
    }
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / (n - 1) as f32).cos())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_periodic_endpoints_and_midpoint() {
        let w = hann_periodic(8);
        assert_eq!(w.len(), 8);
        assert!(w[0].abs() < 1e-6, "periodic Hann starts at 0");
        assert!((w[4] - 1.0).abs() < 1e-6, "periodic Hann peaks at n/2");
    }

    #[test]
    fn hann_symmetric_endpoints_and_symmetry() {
        let w = hann_symmetric(9);
        assert_eq!(w.len(), 9);
        assert!(w[0].abs() < 1e-6, "symmetric Hann starts at 0");
        assert!(w[8].abs() < 1e-6, "symmetric Hann ends at 0");
        for i in 0..9 {
            assert!(
                (w[i] - w[8 - i]).abs() < 1e-6,
                "not mirror-symmetric at {i}"
            );
        }
    }

    #[test]
    fn periodic_and_symmetric_differ_by_the_denominator() {
        let p = hann_periodic(16);
        let s = hann_symmetric(16);
        assert!(
            p.iter().zip(&s).any(|(a, b)| (a - b).abs() > 1e-4),
            "the two variants must not be identical"
        );
    }

    #[test]
    fn degenerate_sizes() {
        assert!(hann_periodic(0).is_empty());
        assert!(hann_symmetric(0).is_empty());
        assert_eq!(hann_symmetric(1), vec![1.0]);
    }
}
