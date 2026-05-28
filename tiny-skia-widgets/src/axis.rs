//! Log-frequency and linear-dB axis mapping for spectrum / response views.
//!
//! Spectrum and filter-response editors all map a frequency in Hz onto a
//! log-spaced horizontal axis and a magnitude in dB onto a linear vertical
//! axis. The math is identical; only the axis *ranges* differ per plugin
//! (e.g. six-pack plots −3..21 dB, miff −48..ceil dB). These helpers take the
//! range as arguments so each caller keeps its own constants while the shared
//! log/lerp body lives in one place.

/// Map a frequency in Hz to a normalised x in `[0, 1]` on a log axis spanning
/// `[freq_min, freq_max]`. The result is clamped to `[0, 1]`; frequencies below
/// `freq_min` map to 0 and above `freq_max` map to 1.
#[inline]
pub fn freq_to_norm_x(freq_hz: f32, freq_min: f32, freq_max: f32) -> f32 {
    let log_min = freq_min.ln();
    let log_max = freq_max.ln();
    ((freq_hz.max(freq_min).ln() - log_min) / (log_max - log_min)).clamp(0.0, 1.0)
}

/// Inverse of [`freq_to_norm_x`]: map a normalised x in `[0, 1]` back to Hz on
/// the same log axis. The input is clamped to `[0, 1]` first.
#[inline]
pub fn norm_x_to_freq(x_norm: f32, freq_min: f32, freq_max: f32) -> f32 {
    let log_min = freq_min.ln();
    let log_max = freq_max.ln();
    (log_min + x_norm.clamp(0.0, 1.0) * (log_max - log_min)).exp()
}

/// Map a dB value to a normalised fraction in `[0, 1]` where 0 == `db_min` and
/// 1 == `db_max`. This is the fraction measured from the *bottom* of the range;
/// callers that draw with y increasing downward flip it themselves (typically
/// `plot_y + plot_h - frac * plot_h`).
#[inline]
pub fn db_to_norm_y(db: f32, db_min: f32, db_max: f32) -> f32 {
    ((db - db_min) / (db_max - db_min)).clamp(0.0, 1.0)
}

/// Inverse of [`db_to_norm_y`]: map a normalised fraction in `[0, 1]` back to
/// dB. The input is clamped to `[0, 1]` first.
#[inline]
pub fn norm_y_to_db(y_norm: f32, db_min: f32, db_max: f32) -> f32 {
    db_min + y_norm.clamp(0.0, 1.0) * (db_max - db_min)
}

#[cfg(test)]
mod tests {
    use super::*;

    const F_MIN: f32 = 20.0;
    const F_MAX: f32 = 20_000.0;

    #[test]
    fn freq_endpoints_map_to_unit_range() {
        assert!((freq_to_norm_x(F_MIN, F_MIN, F_MAX) - 0.0).abs() < 1e-4);
        assert!((freq_to_norm_x(F_MAX, F_MIN, F_MAX) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn freq_clamps_out_of_range() {
        assert_eq!(freq_to_norm_x(1.0, F_MIN, F_MAX), 0.0);
        assert_eq!(freq_to_norm_x(40_000.0, F_MIN, F_MAX), 1.0);
    }

    #[test]
    fn freq_roundtrips_through_inverse() {
        let mid = norm_x_to_freq(0.5, F_MIN, F_MAX);
        let back = freq_to_norm_x(mid, F_MIN, F_MAX);
        assert!((back - 0.5).abs() < 1e-4, "round-trip drifted: {back}");
    }

    #[test]
    fn db_endpoints_and_roundtrip() {
        assert!((db_to_norm_y(-48.0, -48.0, 0.0) - 0.0).abs() < 1e-4);
        assert!((db_to_norm_y(0.0, -48.0, 0.0) - 1.0).abs() < 1e-4);
        let mid = norm_y_to_db(0.5, -48.0, 0.0);
        assert!((db_to_norm_y(mid, -48.0, 0.0) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn db_clamps_out_of_range() {
        assert_eq!(db_to_norm_y(-100.0, -48.0, 0.0), 0.0);
        assert_eq!(db_to_norm_y(12.0, -48.0, 0.0), 1.0);
    }
}
