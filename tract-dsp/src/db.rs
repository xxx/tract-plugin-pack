//! Decibel ↔ linear-amplitude conversions for raw DSP / metering code.
//!
//! Parameter-side conversions should keep using `nih_plug::util::db_to_gain`
//! / `gain_to_db` — those are correct and framework-integrated. These helpers
//! are for DSP modules that have no `nih-plug` dependency.

/// Convert linear amplitude to dB. Returns `f32::NEG_INFINITY` for a
/// non-positive input (silence has no finite dB value).
#[inline]
pub fn linear_to_db(linear: f32) -> f32 {
    if linear <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * linear.log10()
    }
}

/// Convert dB to linear amplitude: `10^(dB / 20)`.
#[inline]
pub fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Convert dB to linear amplitude using `exp()` instead of `powf()`.
///
/// `10^(dB/20)` is rewritten as `exp(dB · ln(10) / 20)`. `exp()` is roughly
/// twice as fast as `powf()`; prefer this on hot per-sample paths. The result
/// matches [`db_to_linear`] to within f32 rounding.
#[inline]
pub fn db_to_linear_fast(db: f32) -> f32 {
    (db * (std::f32::consts::LN_10 / 20.0)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn linear_to_db_known_values() {
        assert!(approx_eq(linear_to_db(1.0), 0.0));
        assert!(approx_eq(linear_to_db(0.5), -6.0206));
        assert_eq!(linear_to_db(0.0), f32::NEG_INFINITY);
        assert_eq!(linear_to_db(-1.0), f32::NEG_INFINITY);
    }

    #[test]
    fn db_to_linear_known_values() {
        assert!(approx_eq(db_to_linear(0.0), 1.0));
        assert!(approx_eq(db_to_linear(-6.0206), 0.5));
    }

    #[test]
    fn db_roundtrip() {
        for db in [-40.0, -20.0, -6.0, 0.0, 6.0, 20.0] {
            let rt = linear_to_db(db_to_linear(db));
            assert!(
                (rt - db).abs() < 0.001,
                "roundtrip failed for {db} dB: got {rt}"
            );
        }
    }

    #[test]
    fn db_to_linear_fast_matches_powf() {
        for db in [-60.0, -24.0, -6.0, -0.1, 0.0, 0.1, 6.0, 24.0] {
            let slow = db_to_linear(db);
            let fast = db_to_linear_fast(db);
            assert!(
                (slow - fast).abs() < 1e-4 * slow.max(1.0),
                "mismatch at {db} dB: powf={slow} exp={fast}"
            );
        }
    }
}
