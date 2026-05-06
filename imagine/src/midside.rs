//! M/S encode/decode helpers.
//!
//! `M = (L + R) / 2`, `S = (L - R) / 2`. The half-scaling makes the encode/decode
//! pair lossless: `decode(encode(L, R)) == (L, R)` exactly.

use std::simd::f32x16;

#[inline]
pub fn encode(l: f32, r: f32) -> (f32, f32) {
    let m = (l + r) * 0.5;
    let s = (l - r) * 0.5;
    (m, s)
}

#[inline]
pub fn decode(m: f32, s: f32) -> (f32, f32) {
    (m + s, m - s)
}

/// SIMD block encode. All four slices must be the same length.
pub fn encode_block(l: &[f32], r: &[f32], m_out: &mut [f32], s_out: &mut [f32]) {
    assert_eq!(l.len(), r.len());
    assert_eq!(l.len(), m_out.len());
    assert_eq!(l.len(), s_out.len());

    let chunks = l.len() / 16;
    let half = f32x16::splat(0.5);
    for c in 0..chunks {
        let off = c * 16;
        let lv = f32x16::from_slice(&l[off..off + 16]);
        let rv = f32x16::from_slice(&r[off..off + 16]);
        let mv = (lv + rv) * half;
        let sv = (lv - rv) * half;
        m_out[off..off + 16].copy_from_slice(mv.as_array());
        s_out[off..off + 16].copy_from_slice(sv.as_array());
    }
    for i in (chunks * 16)..l.len() {
        let (m, s) = encode(l[i], r[i]);
        m_out[i] = m;
        s_out[i] = s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_identity() {
        for &(l, r) in &[(0.0, 0.0), (1.0, -1.0), (0.5, 0.7), (-0.3, 0.9), (1.0, 1.0)] {
            let (m, s) = encode(l, r);
            let (l2, r2) = decode(m, s);
            assert!((l - l2).abs() < 1e-7, "L: {l} → {l2}");
            assert!((r - r2).abs() < 1e-7, "R: {r} → {r2}");
        }
    }

    #[test]
    fn pure_mono_zero_side() {
        let (_, s) = encode(0.5, 0.5);
        assert!(s.abs() < 1e-7);
    }

    #[test]
    fn pure_side_zero_mid() {
        let (m, _) = encode(0.5, -0.5);
        assert!(m.abs() < 1e-7);
    }

    #[test]
    fn silence_round_trip() {
        let (m, s) = encode(0.0, 0.0);
        assert_eq!(m, 0.0);
        assert_eq!(s, 0.0);
        let (l, r) = decode(m, s);
        assert_eq!(l, 0.0);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn simd_matches_scalar() {
        let l: Vec<f32> = (0..100).map(|i| (i as f32 * 0.1).sin()).collect();
        let r: Vec<f32> = (0..100).map(|i| (i as f32 * 0.13).cos()).collect();
        let mut m = vec![0.0_f32; l.len()];
        let mut s = vec![0.0_f32; l.len()];
        encode_block(&l, &r, &mut m, &mut s);

        for i in 0..l.len() {
            let (m_scalar, s_scalar) = encode(l[i], r[i]);
            assert!(
                (m[i] - m_scalar).abs() < 1e-7,
                "M[{i}]: {} vs {}",
                m[i],
                m_scalar
            );
            assert!(
                (s[i] - s_scalar).abs() < 1e-7,
                "S[{i}]: {} vs {}",
                s[i],
                s_scalar
            );
        }
    }

    #[test]
    fn block_handles_non_multiple_of_16() {
        let l = vec![1.0_f32; 17];
        let r = vec![-1.0_f32; 17];
        let mut m = vec![0.0_f32; 17];
        let mut s = vec![0.0_f32; 17];
        encode_block(&l, &r, &mut m, &mut s);
        for i in 0..17 {
            assert_eq!(m[i], 0.0);
            assert_eq!(s[i], 1.0);
        }
    }
}
