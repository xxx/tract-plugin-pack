//! Six saturation algorithms used by Six Pack.
//!
//! Each is a pure function `fn(x: f32, drive: f32) -> f32`. Drive scales
//! the input before the shaper (drive=1.0 is unity input).

/// Tube: symmetric tanh-style soft clipper. Most versatile; default.
pub fn tube(x: f32, drive: f32) -> f32 {
    (drive * x).tanh()
}

#[cfg(test)]
mod test_tube {
    use super::*;

    #[test]
    fn tube_at_zero() {
        assert_eq!(tube(0.0, 1.0), 0.0);
    }

    #[test]
    fn tube_is_symmetric() {
        for x in [0.1, 0.3, 0.5, 0.9, 1.5, 5.0] {
            let pos = tube(x, 1.0);
            let neg = tube(-x, 1.0);
            assert!(
                (pos + neg).abs() < 1e-6,
                "tube must be symmetric: f({x})={pos} f(-{x})={neg}"
            );
        }
    }

    #[test]
    fn tube_is_finite_for_extreme_inputs() {
        for x in [0.0, 1e-30, -1e-30, 1.0, -1.0, 10.0, -10.0, 1e9, -1e9] {
            for d in [0.5, 1.0, 2.0, 8.0] {
                let y = tube(x, d);
                assert!(y.is_finite(), "tube({x}, {d}) = {y} (not finite)");
            }
        }
    }

    #[test]
    fn tube_quiet_input_is_linear() {
        let y = tube(0.001, 1.0);
        assert!(
            (y - 0.001).abs() < 1e-4,
            "tube quiet input should be linear: f(0.001)={y}"
        );
    }
}

/// Tape: asymmetric soft clip. Slightly biased toward the negative rail —
/// punchier on bass, naturally rolls off highs. Not for high frequencies.
pub fn tape(x: f32, drive: f32) -> f32 {
    let bias = 0.18;
    let driven = drive * x + bias;
    let dc_offset = bias.tanh();
    driven.tanh() - dc_offset
}

#[cfg(test)]
mod test_tape {
    use super::*;

    #[test]
    fn tape_at_zero() {
        let y = tape(0.0, 1.0);
        assert!(y.abs() < 1e-6, "tape(0, 1) = {y}");
    }

    #[test]
    fn tape_is_asymmetric() {
        let pos = tape(0.5, 1.0);
        let neg = tape(-0.5, 1.0);
        let asymmetry = (pos + neg).abs();
        assert!(
            asymmetry > 0.001,
            "tape must be visibly asymmetric: f(0.5)={pos} f(-0.5)={neg}"
        );
    }

    #[test]
    fn tape_is_finite_for_extreme() {
        for x in [0.0, 1.0, -1.0, 10.0, -10.0, 1e9, -1e9] {
            for d in [0.5, 1.0, 2.0, 8.0] {
                let y = tape(x, d);
                assert!(y.is_finite(), "tape({x}, {d}) = {y}");
            }
        }
    }
}

/// Diode: symmetric soft clip with extra high-frequency content.
/// Similar to tube but generates more odd-order harmonics, brighter.
pub fn diode(x: f32, drive: f32) -> f32 {
    let driven = drive * x;
    let abs_cubed = driven * driven * driven.abs();
    driven / (1.0 + abs_cubed).powf(1.0 / 3.0)
}

#[cfg(test)]
mod test_diode {
    use super::*;

    #[test]
    fn diode_at_zero() {
        assert_eq!(diode(0.0, 1.0), 0.0);
    }

    #[test]
    fn diode_is_symmetric() {
        for x in [0.1, 0.3, 0.5, 0.9, 1.5, 5.0] {
            let p = diode(x, 1.0);
            let n = diode(-x, 1.0);
            assert!(
                (p + n).abs() < 1e-6,
                "diode({x}, 1)={p}, diode(-{x}, 1)={n}"
            );
        }
    }

    #[test]
    fn diode_is_finite() {
        for x in [0.0, 1.0, -1.0, 10.0, 1e9, -1e9] {
            for d in [0.5, 1.0, 2.0, 8.0] {
                assert!(diode(x, d).is_finite());
            }
        }
    }
}

/// Digital: hard clip at ±1 (after drive scaling).
pub fn digital(x: f32, drive: f32) -> f32 {
    (drive * x).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod test_digital {
    use super::*;

    #[test]
    fn digital_at_zero() {
        assert_eq!(digital(0.0, 1.0), 0.0);
    }

    #[test]
    fn digital_is_symmetric() {
        for x in [0.1, 0.5, 1.0, 1.5, 5.0] {
            assert_eq!(digital(x, 1.0), -digital(-x, 1.0));
        }
    }

    #[test]
    fn digital_clips_at_unity() {
        assert!((digital(2.0, 1.0) - 1.0).abs() < 1e-7);
        assert!((digital(-2.0, 1.0) + 1.0).abs() < 1e-7);
    }

    #[test]
    fn digital_below_threshold_is_linear() {
        for x in [-0.5_f32, -0.1, 0.1, 0.5] {
            assert!((digital(x, 1.0) - x).abs() < 1e-7);
        }
    }
}

/// Class B: crossover distortion — symmetric soft clip with a small dead zone
/// near zero. Adds harmonics on transients (drum/percussion character).
pub fn class_b(x: f32, drive: f32) -> f32 {
    let driven = drive * x;
    let dead_zone = 0.05;
    let s = driven.signum();
    let mag = driven.abs();
    if mag <= dead_zone {
        s * (mag * mag) / dead_zone * 0.5
    } else {
        let above = mag - dead_zone;
        s * (dead_zone * 0.5 + above.tanh())
    }
}

#[cfg(test)]
mod test_class_b {
    use super::*;

    #[test]
    fn class_b_at_zero() {
        assert_eq!(class_b(0.0, 1.0), 0.0);
    }

    #[test]
    fn class_b_is_symmetric() {
        for x in [0.05, 0.1, 0.3, 0.5, 0.9, 1.5] {
            let p = class_b(x, 1.0);
            let n = class_b(-x, 1.0);
            assert!(
                (p + n).abs() < 1e-6,
                "class_b({x}, 1)={p}, class_b(-{x}, 1)={n}"
            );
        }
    }

    #[test]
    fn class_b_has_dead_zone() {
        let small = class_b(0.01, 1.0).abs();
        let mid = class_b(0.5, 1.0).abs();
        let ratio = small / mid;
        assert!(
            ratio < 0.005,
            "class_b dead zone ratio: small={small} mid={mid} ratio={ratio}"
        );
    }

    #[test]
    fn class_b_is_finite() {
        for x in [0.0, 1.0, -1.0, 10.0, 1e9, -1e9] {
            for d in [0.5, 1.0, 2.0, 8.0] {
                assert!(class_b(x, d).is_finite());
            }
        }
    }
}

/// Wavefold: west-coast wavefolder. Input exceeding ±1 folds back rather
/// than clipping, generating dense odd+even harmonic content. Symmetric.
///
/// Pre-clipped to ±64 internally to keep f32 modulo precision meaningful.
pub fn wavefold(x: f32, drive: f32) -> f32 {
    let driven = (drive * x).clamp(-64.0, 64.0);
    let shifted = driven + 1.0;
    let m = shifted - 4.0 * (shifted * 0.25).floor();
    1.0 - (m - 2.0).abs()
}

#[cfg(test)]
mod test_wavefold {
    use super::*;

    #[test]
    fn wavefold_at_zero() {
        assert_eq!(wavefold(0.0, 1.0), 0.0);
    }

    #[test]
    fn wavefold_is_symmetric() {
        for x in [0.1, 0.5, 0.9, 1.5, 3.0, 7.0] {
            let p = wavefold(x, 1.0);
            let n = wavefold(-x, 1.0);
            assert!(
                (p + n).abs() < 1e-5,
                "wavefold({x}, 1)={p}, wavefold(-{x}, 1)={n}"
            );
        }
    }

    #[test]
    fn wavefold_inside_unit_is_identity() {
        for x in [-0.99_f32, -0.5, -0.1, 0.0, 0.1, 0.5, 0.99] {
            let y = wavefold(x, 1.0);
            assert!(
                (y - x).abs() < 1e-6,
                "wavefold inside [-1, 1] should pass through: f({x})={y}"
            );
        }
    }

    #[test]
    fn wavefold_is_finite_at_extreme() {
        for x in [0.0, 1.0, -1.0, 10.0, 100.0, 1e9, -1e9] {
            for d in [0.5, 1.0, 2.0, 8.0] {
                let y = wavefold(x, d);
                assert!(y.is_finite(), "wavefold({x}, {d}) = {y}");
                assert!(y.abs() <= 1.0 + 1e-5, "wavefold output out of range: {y}");
            }
        }
    }

    #[test]
    fn wavefold_zero_crossings_rise_with_drive() {
        let count_zc = |drive: f32| -> usize {
            let n = 4096;
            let mut last = 0.0;
            let mut count = 0;
            for i in 0..n {
                let phase = (i as f32) / (n as f32) * 2.0 * std::f32::consts::PI;
                let y = wavefold(phase.sin(), drive);
                if (last <= 0.0 && y > 0.0) || (last >= 0.0 && y < 0.0) {
                    count += 1;
                }
                last = y;
            }
            count
        };
        let zc_low = count_zc(1.0);
        let zc_mid = count_zc(2.0);
        let zc_high = count_zc(4.0);
        assert!(
            zc_low < zc_mid,
            "drive 1.0 -> {zc_low} zc; drive 2.0 -> {zc_mid} zc"
        );
        assert!(
            zc_mid < zc_high,
            "drive 2.0 -> {zc_mid} zc; drive 4.0 -> {zc_high} zc"
        );
    }
}

/// All saturation algorithms shipped by Six Pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Algorithm {
    Tube,
    Tape,
    Diode,
    Digital,
    ClassB,
    Wavefold,
}

impl Algorithm {
    /// Apply the selected algorithm to a sample.
    pub fn apply(self, x: f32, drive: f32) -> f32 {
        match self {
            Algorithm::Tube => tube(x, drive),
            Algorithm::Tape => tape(x, drive),
            Algorithm::Diode => diode(x, drive),
            Algorithm::Digital => digital(x, drive),
            Algorithm::ClassB => class_b(x, drive),
            Algorithm::Wavefold => wavefold(x, drive),
        }
    }
}

#[cfg(test)]
mod test_dispatch {
    use super::*;

    #[test]
    fn dispatch_each_algo() {
        for algo in [
            Algorithm::Tube,
            Algorithm::Tape,
            Algorithm::Diode,
            Algorithm::Digital,
            Algorithm::ClassB,
            Algorithm::Wavefold,
        ] {
            let y = algo.apply(0.0, 1.0);
            assert!(y.abs() < 1e-6, "{:?}: f(0, 1) = {y}", algo);
        }
    }
}
