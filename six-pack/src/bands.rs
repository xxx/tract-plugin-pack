//! Per-band state: SVF pair (L, R), saturator selection, M/S routing.

use crate::saturation::Algorithm;
use crate::svf::Svf;

/// Filter shape: which SVF method to call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterShape {
    LowShelf,
    Peak,
    HighShelf,
}

/// Per-band channel routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMode {
    Stereo,
    Mid,
    Side,
}

/// Output pair from one band's processing for one input sample.
#[derive(Debug, Clone, Copy, Default)]
pub struct BandOut {
    pub sat_l: f32,
    pub sat_r: f32,
    /// Linear (unsaturated) routed boost — used for de-emphasis subtraction.
    pub boost_l: f32,
    pub boost_r: f32,
}

/// Per-band state. SVFs are per-channel for L/R independence; routing is
/// applied to the diff before saturation.
pub struct BandState {
    pub shape: FilterShape,
    pub algo: Algorithm,
    pub mode: ChannelMode,
    pub freq_hz: f32,
    pub q: f32,
    pub gain_db: f32,
    pub enable: f32, // 0..1 ramp factor

    pub svf_l: Svf,
    pub svf_r: Svf,
}

impl BandState {
    pub fn new(shape: FilterShape) -> Self {
        Self {
            shape,
            algo: Algorithm::Tube,
            mode: ChannelMode::Stereo,
            freq_hz: 1_000.0,
            q: 0.71,
            gain_db: 0.0,
            enable: 1.0,
            svf_l: Svf::default(),
            svf_r: Svf::default(),
        }
    }

    /// Recompute coefficients for both per-channel SVFs.
    pub fn recompute_coefs(&mut self, sample_rate: f32) {
        match self.shape {
            FilterShape::LowShelf => {
                self.svf_l
                    .set_low_shelf(self.freq_hz, self.q, self.gain_db, sample_rate);
                self.svf_r
                    .set_low_shelf(self.freq_hz, self.q, self.gain_db, sample_rate);
            }
            FilterShape::Peak => {
                self.svf_l
                    .set_peak(self.freq_hz, self.q, self.gain_db, sample_rate);
                self.svf_r
                    .set_peak(self.freq_hz, self.q, self.gain_db, sample_rate);
            }
            FilterShape::HighShelf => {
                self.svf_l
                    .set_high_shelf(self.freq_hz, self.q, self.gain_db, sample_rate);
                self.svf_r
                    .set_high_shelf(self.freq_hz, self.q, self.gain_db, sample_rate);
            }
        }
    }

    pub fn reset(&mut self) {
        self.svf_l.reset();
        self.svf_r.reset();
    }

    /// Process one stereo sample and return the per-channel saturated output
    /// and per-channel routed boost (for de-emphasis subtraction).
    pub fn process_sample(&mut self, dry_l: f32, dry_r: f32, drive_k: f32) -> BandOut {
        // Linear filter outputs:
        let eq_l = match self.shape {
            FilterShape::LowShelf | FilterShape::HighShelf => self.svf_l.process_shelf(dry_l),
            FilterShape::Peak => self.svf_l.process_peak(dry_l),
        };
        let eq_r = match self.shape {
            FilterShape::LowShelf | FilterShape::HighShelf => self.svf_r.process_shelf(dry_r),
            FilterShape::Peak => self.svf_r.process_peak(dry_r),
        };
        let diff_l = eq_l - dry_l;
        let diff_r = eq_r - dry_r;
        let e = self.enable;

        match self.mode {
            ChannelMode::Stereo => {
                let sat_l = self.algo.apply(diff_l * drive_k, 1.0);
                let sat_r = self.algo.apply(diff_r * drive_k, 1.0);
                BandOut {
                    sat_l: sat_l * e,
                    sat_r: sat_r * e,
                    boost_l: diff_l * e,
                    boost_r: diff_r * e,
                }
            }
            ChannelMode::Mid => {
                let m_diff = (diff_l + diff_r) * 0.5;
                let m_sat = self.algo.apply(m_diff * drive_k, 1.0);
                BandOut {
                    sat_l: m_sat * e,
                    sat_r: m_sat * e,
                    boost_l: m_diff * e,
                    boost_r: m_diff * e,
                }
            }
            ChannelMode::Side => {
                let s_diff = (diff_l - diff_r) * 0.5;
                let s_sat = self.algo.apply(s_diff * drive_k, 1.0);
                BandOut {
                    sat_l: s_sat * e,
                    sat_r: -s_sat * e,
                    boost_l: s_diff * e,
                    boost_r: -s_diff * e,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_band(mode: ChannelMode, gain_db: f32) -> BandState {
        let mut band = BandState::new(FilterShape::Peak);
        band.mode = mode;
        band.algo = Algorithm::Digital; // hard clip — easier to predict at small inputs
        band.freq_hz = 1_000.0;
        band.q = 0.71;
        band.gain_db = gain_db;
        band.recompute_coefs(48_000.0);
        band
    }

    #[test]
    fn stereo_routing_independent_per_channel() {
        let mut band = make_test_band(ChannelMode::Stereo, 9.0);
        // Drive a sine in L only — R should produce no boost.
        for i in 0..1000 {
            let phase = i as f32 / 1000.0 * std::f32::consts::TAU;
            let l = phase.sin();
            let r = 0.0;
            let out = band.process_sample(l, r, 1.0);
            assert!(
                (out.boost_r).abs() < 1e-3,
                "R-channel boost should be ~0 for R=0 input: {}",
                out.boost_r
            );
        }
    }

    #[test]
    fn mid_routing_outputs_equal_on_both_channels() {
        let mut band = make_test_band(ChannelMode::Mid, 9.0);
        for i in 0..200 {
            let phase = i as f32 / 200.0 * std::f32::consts::TAU;
            let l = phase.sin() * 0.5;
            let r = (phase * 1.3).sin() * 0.5;
            let out = band.process_sample(l, r, 1.0);
            assert!(
                (out.sat_l - out.sat_r).abs() < 1e-6,
                "Mid: sat_l ({}) must equal sat_r ({})",
                out.sat_l,
                out.sat_r
            );
            assert!(
                (out.boost_l - out.boost_r).abs() < 1e-6,
                "Mid: boost_l ({}) must equal boost_r ({})",
                out.boost_l,
                out.boost_r
            );
        }
    }

    #[test]
    fn side_routing_anti_phase_on_r() {
        let mut band = make_test_band(ChannelMode::Side, 9.0);
        for i in 0..200 {
            let phase = i as f32 / 200.0 * std::f32::consts::TAU;
            let l = phase.sin() * 0.5;
            let r = (phase * 1.3).sin() * 0.5;
            let out = band.process_sample(l, r, 1.0);
            assert!(
                (out.sat_l + out.sat_r).abs() < 1e-6,
                "Side: sat_r ({}) must be -sat_l ({})",
                out.sat_r,
                out.sat_l
            );
            assert!(
                (out.boost_l + out.boost_r).abs() < 1e-6,
                "Side: boost_r ({}) must be -boost_l ({})",
                out.boost_r,
                out.boost_l
            );
        }
    }

    /// At gain = 0 dB, every band must produce zero diff for any input.
    #[test]
    fn zero_db_band_produces_zero_diff() {
        for shape in [
            FilterShape::LowShelf,
            FilterShape::Peak,
            FilterShape::HighShelf,
        ] {
            let mut band = BandState::new(shape);
            band.gain_db = 0.0;
            band.recompute_coefs(48_000.0);
            for i in 0..1_000 {
                let phase = (i as f32) / 1_000.0;
                let dry_l = (phase * 17.0).sin();
                let dry_r = (phase * 23.0).cos();
                let out = band.process_sample(dry_l, dry_r, 1.0);
                assert!(
                    out.sat_l.abs() < 1e-6,
                    "{:?}: sat_l should be 0, got {}",
                    shape,
                    out.sat_l
                );
                assert!(
                    out.sat_r.abs() < 1e-6,
                    "{:?}: sat_r should be 0, got {}",
                    shape,
                    out.sat_r
                );
                assert!(
                    out.boost_l.abs() < 1e-6,
                    "{:?}: boost_l should be 0, got {}",
                    shape,
                    out.boost_l
                );
                assert!(
                    out.boost_r.abs() < 1e-6,
                    "{:?}: boost_r should be 0, got {}",
                    shape,
                    out.boost_r
                );
            }
        }
    }
}
