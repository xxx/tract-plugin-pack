//! Curve → FIR-kernel bake, normalization, and the lock-free GUI→audio handoff.
//!
//! A self-contained module — no plugin or GUI types — so a future
//! workspace-wide shared-DSP refactor can lift it cleanly.

use std::sync::Mutex;

use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;
use tiny_skia_widgets::mseg::{warp, MsegData, MsegNode};

/// Maximum kernel length, and the fixed Phaseless STFT frame size.
pub const MAX_KERNEL: usize = 4096;
/// Real-FFT magnitude-spectrum bin count for a `MAX_KERNEL`-point transform.
pub const MAG_BINS: usize = MAX_KERNEL / 2 + 1;

/// A baked, normalized FIR kernel. Fixed-size and `Copy` so it crosses the
/// GUI→audio boundary with an allocation-free copy.
///
/// `taps[..len]` is the time-domain kernel (Raw mode). `mags` is the
/// magnitude spectrum of the zero-padded `MAX_KERNEL`-point kernel (Phaseless
/// mode and the response view). `is_zero` marks an all-zero kernel — miff
/// treats that as dry passthrough.
///
/// It is ~40 KB — designed for a single buffered hand-off copy, not repeated
/// stack copies in a loop.
#[derive(Clone, Copy)]
pub struct Kernel {
    pub taps: [f32; MAX_KERNEL],
    /// The first `len` taps of `taps` reversed (`rev_taps[j] == taps[len-1-j]`);
    /// the convolution MAC reads this contiguously. Zero beyond `len`.
    pub rev_taps: [f32; MAX_KERNEL],
    pub len: usize,
    pub mags: [f32; MAG_BINS],
    pub is_zero: bool,
}

impl Default for Kernel {
    /// An all-zero kernel — dry passthrough.
    fn default() -> Self {
        Self {
            taps: [0.0; MAX_KERNEL],
            rev_taps: [0.0; MAX_KERNEL],
            len: 256,
            mags: [0.0; MAG_BINS],
            is_zero: true,
        }
    }
}

/// GUI→audio `Kernel` handoff. `Mutex<Kernel>` + an audio-thread `try_lock`,
/// matching wavetable-filter's reload-handoff pattern. The kernel only changes
/// on a GUI edit, so audio-thread lock contention is negligible; on the rare
/// miss the audio thread keeps its previous kernel for one buffer.
///
/// Wrap this in an `Arc` to share it between the GUI editor and the audio
/// `process()`.
pub struct KernelHandoff {
    shared: Mutex<Kernel>,
}

impl KernelHandoff {
    /// A fresh handoff holding the default (zero) kernel.
    pub fn new() -> Self {
        Self {
            shared: Mutex::new(Kernel::default()),
        }
    }

    /// Publish a freshly-baked kernel. GUI thread.
    pub fn publish(&self, kernel: Kernel) {
        if let Ok(mut slot) = self.shared.lock() {
            *slot = kernel;
        }
    }

    /// Try to read the latest kernel. Audio thread — non-blocking. Returns
    /// `None` on a (rare) lock miss; the caller keeps its previous kernel.
    pub fn try_read(&self) -> Option<Kernel> {
        self.shared.try_lock().ok().map(|slot| *slot)
    }
}

impl Default for KernelHandoff {
    fn default() -> Self {
        Self::new()
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

/// Below this peak magnitude the kernel is treated as all-zero (dry
/// passthrough) — guards the normalization divide.
const ZERO_EPS: f32 = 1e-9;

/// Bake `data` into a normalized `Kernel` for a kernel of `len` taps.
///
/// Runs on the GUI thread (an O(`len` log `len`) FFT) — never the audio
/// thread. Steps: single-walk bake -> `MAX_KERNEL`-point FFT -> peak |H(k)|
/// -> divide taps and mags by the peak. A peak below `ZERO_EPS` (e.g. the
/// flat-0.5 default) yields an all-zero `is_zero` kernel.
pub fn bake(data: &MsegData, len: usize) -> Kernel {
    let mut taps = [0.0_f32; MAX_KERNEL];
    let len = bake_taps(data, len, &mut taps);

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(MAX_KERNEL);
    let mut fft_in = taps; // a MAX_KERNEL copy (taps beyond len are zero)
    let mut spectrum = vec![Complex::new(0.0_f32, 0.0); MAG_BINS];
    fft.process(&mut fft_in, &mut spectrum)
        .expect("FFT length matches planner");

    let mut mags = [0.0_f32; MAG_BINS];
    let mut peak = 0.0_f32;
    for (m, c) in mags.iter_mut().zip(spectrum.iter()) {
        *m = c.norm();
        peak = peak.max(*m);
    }

    if peak <= ZERO_EPS {
        return Kernel {
            taps: [0.0; MAX_KERNEL],
            rev_taps: [0.0; MAX_KERNEL],
            len,
            mags: [0.0; MAG_BINS],
            is_zero: true,
        };
    }

    let inv = 1.0 / peak;
    for t in taps[..len].iter_mut() {
        *t *= inv;
    }
    for m in mags.iter_mut() {
        *m *= inv;
    }
    let mut rev_taps = [0.0_f32; MAX_KERNEL];
    for j in 0..len {
        rev_taps[j] = taps[len - 1 - j];
    }
    Kernel {
        taps,
        rev_taps,
        len,
        mags,
        is_zero: false,
    }
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
        assert!(
            out[len..].iter().all(|&t| t == 0.0),
            "bake_taps must zero-pad the tail beyond len"
        );
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

    #[test]
    fn bake_of_flat_half_is_a_zero_kernel() {
        let k = bake(&default_flat_curve(), 256);
        assert!(k.is_zero, "flat 0.5 default must bake to a zero kernel");
        assert_eq!(k.len, 256);
    }

    #[test]
    fn nonzero_kernel_has_unity_peak_magnitude() {
        let k = bake(&MsegData::default(), 512);
        assert!(!k.is_zero);
        let peak = k.mags.iter().cloned().fold(0.0_f32, f32::max);
        assert!((peak - 1.0).abs() < 1e-3, "peak magnitude {peak}, expected 1.0");
    }

    #[test]
    fn zero_kernel_skips_normalization_without_panic() {
        let k = bake(&default_flat_curve(), 256);
        assert!(k.is_zero);
        assert!(k.taps[..k.len].iter().all(|&t| t == 0.0));
        assert!(k.mags.iter().all(|&m| m == 0.0));
    }

    #[test]
    fn bake_length_round_trips_into_kernel_len() {
        let k = bake(&MsegData::default(), 100);
        assert_eq!(k.len, 96); // rounded to a multiple of 16
    }

    #[test]
    fn handoff_hands_off_the_latest_kernel() {
        let h = KernelHandoff::new();
        // Initially: a zero kernel.
        assert!(h.try_read().unwrap().is_zero);
        // Publish a non-zero kernel.
        h.publish(bake(&MsegData::default(), 512));
        let k = h.try_read().unwrap();
        assert!(!k.is_zero);
        assert_eq!(k.len, 512);
    }

    #[test]
    fn handoff_publish_is_visible_to_next_read() {
        let h = KernelHandoff::new();
        h.publish(bake(&MsegData::default(), 128));
        assert_eq!(h.try_read().unwrap().len, 128);
        h.publish(bake(&MsegData::default(), 256));
        assert_eq!(h.try_read().unwrap().len, 256); // newest wins
    }

    #[test]
    fn handoff_read_without_publish_is_the_default_zero_kernel() {
        let h = KernelHandoff::new();
        assert!(h.try_read().unwrap().is_zero);
    }
}
