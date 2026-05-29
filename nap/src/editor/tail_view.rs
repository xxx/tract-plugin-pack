//! Live visualization of the generated velvet tail: a decaying pulse field.
//!
//! `decimate` compresses the full `VelvetSequence` (potentially tens of
//! thousands of pulses) down to at most `cols` columns — the plot width in
//! pixels — by keeping only the loudest pulse per column. Render cost is
//! therefore O(cols) regardless of pulse count.

use crate::sequence::VelvetSequence;

/// Per-column summary for rendering: the loudest pulse in that column.
#[derive(Clone, Copy, Default)]
pub struct Column {
    pub coeff_abs: f32,
    pub filter_idx: u8,
    /// `location_r - location`, in samples (signed). Positive = R leads L.
    pub lr_split: i32,
    /// Whether any pulse maps into this column.
    pub present: bool,
}

/// Decimate the sequence to `cols` columns by tail phase, keeping the
/// max-|coeff| pulse per column. Bounds render cost independent of `count`.
///
/// `out` is cleared and resized to exactly `cols` entries before filling.
/// Columns with no pulse retain their `Column::default()` (`present = false`).
pub fn decimate(seq: &VelvetSequence, cols: usize, out: &mut Vec<Column>) {
    out.clear();
    out.resize(cols, Column::default());
    if seq.count == 0 || seq.tail_len == 0 || cols == 0 {
        return;
    }
    let tl = seq.tail_len as f32;
    for m in 0..seq.count {
        let phase = (seq.location[m] as f32 / tl).clamp(0.0, 1.0);
        let c = ((phase * (cols as f32 - 1.0)).round() as usize).min(cols - 1);
        let a = seq.coeff[m].abs();
        if !out[c].present || a > out[c].coeff_abs {
            out[c] = Column {
                coeff_abs: a,
                filter_idx: seq.filter_idx[m],
                lr_split: seq.location_r[m] as i32 - seq.location[m] as i32,
                present: true,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimate_keeps_loudest_per_column() {
        let mut seq = VelvetSequence::new();
        seq.count = 3;
        seq.tail_len = 100;
        seq.location[..3].copy_from_slice(&[0, 1, 99]);
        seq.coeff[..3].copy_from_slice(&[0.2, 0.9, 0.5]);
        seq.filter_idx[..3].copy_from_slice(&[0, 3, 5]);
        seq.location_r[..3].copy_from_slice(&[0, 1, 99]);
        let mut cols = Vec::new();
        decimate(&seq, 10, &mut cols);
        assert_eq!(cols.len(), 10);
        // Pulses at location 0 and 1 both map to column 0 (phase ≈ 0.0 → col 0);
        // the louder one (coeff 0.9, filter_idx 3) should win.
        assert!(
            cols[0].present && (cols[0].coeff_abs - 0.9).abs() < 1e-6,
            "loudest in col 0 kept"
        );
        assert_eq!(cols[0].filter_idx, 3);
        // Pulse at location 99 / tail_len 100 → phase ≈ 0.99 → col 9.
        assert!(cols[9].present && (cols[9].coeff_abs - 0.5).abs() < 1e-6);
        // Column 5 has no pulses mapped to it.
        assert!(!cols[5].present, "empty column stays absent");
    }
}
