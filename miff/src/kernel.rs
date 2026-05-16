//! Curve → FIR-kernel bake, normalization, and the lock-free GUI→audio handoff.
//!
//! A self-contained module — no plugin or GUI types — so a future
//! workspace-wide shared-DSP refactor can lift it cleanly.

use tiny_skia_widgets::mseg::{warp, MsegData, MsegNode};

/// Maximum kernel length, and the fixed Phaseless STFT frame size.
pub const MAX_KERNEL: usize = 4096;
/// Real-FFT magnitude-spectrum bin count for a `MAX_KERNEL`-point transform.
pub const MAG_BINS: usize = MAX_KERNEL / 2 + 1;

/// A baked, normalized FIR kernel. Fixed-size and `Copy` so it crosses the
/// GUI→audio boundary with a lock-free copy that never allocates.
///
/// `taps[..len]` is the time-domain kernel (Raw mode). `mags` is the
/// magnitude spectrum of the zero-padded `MAX_KERNEL`-point kernel (Phaseless
/// mode and the response view). `is_zero` marks an all-zero kernel — miff
/// treats that as dry passthrough.
#[derive(Clone, Copy)]
pub struct Kernel {
    pub taps: [f32; MAX_KERNEL],
    pub len: usize,
    pub mags: [f32; MAG_BINS],
    pub is_zero: bool,
}

impl Default for Kernel {
    /// An all-zero kernel — dry passthrough.
    fn default() -> Self {
        Self {
            taps: [0.0; MAX_KERNEL],
            len: 256,
            mags: [0.0; MAG_BINS],
            is_zero: true,
        }
    }
}

/// miff's default document: a flat curve at value 0.5. Two nodes, both 0.5.
/// Bakes (bipolar map) to an all-zero kernel, i.e. clean dry passthrough — a
/// fresh miff colors nothing. NOT the MSEG core's `default()`, which is a ramp.
pub fn default_flat_curve() -> MsegData {
    let mut d = MsegData::default(); // ramp; we overwrite the two node values
    d.nodes[0] = MsegNode {
        time: 0.0,
        value: 0.5,
        tension: 0.0,
        stepped: false,
    };
    d.nodes[1] = MsegNode {
        time: 1.0,
        value: 0.5,
        tension: 0.0,
        stepped: false,
    };
    d.debug_assert_valid();
    d
}

/// Sample the curve at `phase` using a forward-only segment cursor `*seg`.
/// Callers walk `phase` monotonically increasing; `*seg` only advances. This
/// reproduces `tiny_skia_widgets::mseg::value_at_phase` exactly (stepped hold
/// + exponential `warp`), but without that function's rescan-from-node-0.
fn curve_value(data: &MsegData, phase: f32, seg: &mut usize) -> f32 {
    let a = data.active();
    let last = data.node_count - 1;
    // At or past the last node -> its value (matches value_at_phase).
    if phase >= a[last].time {
        return a[last].value;
    }
    // Advance the cursor to the last node (in 0..=last-1) with time <= phase.
    while *seg < last - 1 && a[*seg + 1].time <= phase {
        *seg += 1;
    }
    let n0 = a[*seg];
    let n1 = a[*seg + 1];
    if n0.stepped {
        return n0.value;
    }
    let span = n1.time - n0.time;
    let t = if span > 1e-9 {
        (phase - n0.time) / span
    } else {
        0.0
    };
    n0.value + (n1.value - n0.value) * warp(t, n0.tension)
}

/// Bake the curve into raw (un-normalized) bipolar FIR taps, into
/// `out[..len]`. `kernel[i] = 2*curve_value(i/(len-1)) - 1` — the bipolar map
/// puts the MSEG midline (value 0.5) at a zero tap. `len` is clamped to
/// `[16, MAX_KERNEL]` and rounded down to a multiple of 16 (SIMD requirement).
/// Returns the effective `len` used.
pub fn bake_taps(data: &MsegData, len: usize, out: &mut [f32; MAX_KERNEL]) -> usize {
    let len = (len.clamp(16, MAX_KERNEL) / 16) * 16;
    *out = [0.0; MAX_KERNEL];
    let mut seg = 0usize;
    let denom = (len - 1).max(1) as f32;
    for (i, tap) in out.iter_mut().take(len).enumerate() {
        let phase = i as f32 / denom;
        *tap = 2.0 * curve_value(data, phase, &mut seg) - 1.0;
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia_widgets::mseg::value_at_phase;

    #[test]
    fn flat_half_curve_bakes_to_all_zero() {
        let d = default_flat_curve();
        let mut out = [9.9_f32; MAX_KERNEL];
        let len = bake_taps(&d, 256, &mut out);
        assert_eq!(len, 256);
        assert!(out[..len].iter().all(|&t| t.abs() < 1e-6), "flat 0.5 -> zero taps");
    }

    #[test]
    fn flat_one_curve_bakes_to_all_plus_one() {
        let mut d = default_flat_curve();
        d.nodes[0].value = 1.0;
        d.nodes[1].value = 1.0;
        let mut out = [0.0_f32; MAX_KERNEL];
        let len = bake_taps(&d, 128, &mut out);
        assert!(out[..len].iter().all(|&t| (t - 1.0).abs() < 1e-6), "value 1.0 -> +1 taps");
    }

    #[test]
    fn ramp_curve_bakes_to_a_ramp() {
        let d = MsegData::default(); // 0->1 ramp
        let mut out = [0.0_f32; MAX_KERNEL];
        let len = bake_taps(&d, 64, &mut out);
        assert!((out[0] - (-1.0)).abs() < 1e-5);
        assert!((out[len - 1] - 1.0).abs() < 1e-5);
        for w in out[..len].windows(2) {
            assert!(w[1] >= w[0] - 1e-5);
        }
    }

    #[test]
    fn single_walk_bake_matches_value_at_phase_tap_for_tap() {
        let mut d = MsegData::default();
        d.insert_node(0.3, 0.8);
        d.insert_node(0.6, 0.2);
        d.nodes[1].tension = 0.7;
        d.nodes[2].stepped = true;
        let len = 512;
        let mut out = [0.0_f32; MAX_KERNEL];
        bake_taps(&d, len, &mut out);
        let denom = (len - 1) as f32;
        for i in 0..len {
            let phase = i as f32 / denom;
            let reference = 2.0 * value_at_phase(&d, phase) - 1.0;
            assert!(
                (out[i] - reference).abs() < 1e-5,
                "tap {i} (phase {phase}): bake {} vs reference {reference}",
                out[i]
            );
        }
    }

    #[test]
    fn length_is_clamped_and_rounded_to_multiple_of_16() {
        let d = MsegData::default();
        let mut out = [0.0_f32; MAX_KERNEL];
        assert_eq!(bake_taps(&d, 4, &mut out), 16);
        assert_eq!(bake_taps(&d, 100, &mut out), 96);
        assert_eq!(bake_taps(&d, 99_999, &mut out), MAX_KERNEL);
    }

    #[test]
    fn default_flat_curve_is_valid() {
        assert!(default_flat_curve().is_valid());
    }
}
