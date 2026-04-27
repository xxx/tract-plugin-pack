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
