use rubato::{FftFixedIn, Resampler};

/// Integer-ratio oversampler (2x, 4x, 8x) using rubato's FFT-based resampling.
///
/// At ratio=1, all operations are zero-cost passthrough (plain copies).
/// Pre-allocates all buffers at construction time so that `process_up` and
/// `process_down` never allocate on the audio thread.
pub struct Oversampler {
    ratio: usize,
    #[allow(dead_code)]
    channels: usize,
    max_block_size: usize,
    up_resampler: Option<FftFixedIn<f32>>,
    down_resampler: Option<FftFixedIn<f32>>,
    /// Padded input buffer for upsample (channels × max_block_size)
    up_input_buf: Vec<Vec<f32>>,
    /// Padded input buffer for downsample (channels × max_block_size*ratio)
    down_input_buf: Vec<Vec<f32>>,
    /// Internal output buffer for upsample resampler
    up_output_buf: Vec<Vec<f32>>,
    /// Internal output buffer for downsample resampler
    down_output_buf: Vec<Vec<f32>>,
}

impl Oversampler {
    /// Create a new Oversampler. At ratio=1, all ops are passthrough.
    ///
    /// # Panics
    /// Panics if `ratio` is not 1, 2, 4, or 8, or if `max_block_size` or `channels` is 0.
    pub fn new(ratio: usize, max_block_size: usize, channels: usize) -> Self {
        assert!(
            matches!(ratio, 1 | 2 | 4 | 8),
            "ratio must be 1, 2, 4, or 8"
        );
        assert!(max_block_size > 0, "max_block_size must be > 0");
        assert!(channels > 0, "channels must be > 0");

        if ratio == 1 {
            return Oversampler {
                ratio,
                channels,
                max_block_size,
                up_resampler: None,
                down_resampler: None,
                up_input_buf: Vec::new(),
                down_input_buf: Vec::new(),
                up_output_buf: Vec::new(),
                down_output_buf: Vec::new(),
            };
        }

        // Upsample: sr_in=1, sr_out=ratio, chunk_size=max_block_size
        let up = FftFixedIn::<f32>::new(1, ratio, max_block_size, 2, channels)
            .expect("failed to create upsample resampler");

        // Downsample: sr_in=ratio, sr_out=1, chunk_size=max_block_size*ratio
        let down = FftFixedIn::<f32>::new(ratio, 1, max_block_size * ratio, 2, channels)
            .expect("failed to create downsample resampler");

        // Pre-allocate all buffers using rubato's helpers
        let up_input_buf = up.input_buffer_allocate(true);
        let up_output_buf = up.output_buffer_allocate(true);
        let down_input_buf = down.input_buffer_allocate(true);
        let down_output_buf = down.output_buffer_allocate(true);

        Oversampler {
            ratio,
            channels,
            max_block_size,
            up_resampler: Some(up),
            down_resampler: Some(down),
            up_input_buf,
            down_input_buf,
            up_output_buf,
            down_output_buf,
        }
    }

    /// Upsample input into pre-allocated output. Returns number of output samples per channel.
    ///
    /// `n_samples`: actual number of input samples to process (may be <= input[0].len())
    pub fn process_up(&mut self, input: &[Vec<f32>], output: &mut [Vec<f32>], n_samples: usize) -> usize {
        let n_in = n_samples;

        if self.ratio == 1 {
            for (ch_out, ch_in) in output.iter_mut().zip(input.iter()) {
                ch_out[..n_in].copy_from_slice(&ch_in[..n_in]);
            }
            return n_in;
        }

        let up = self.up_resampler.as_mut().unwrap();

        // Copy input into padded buffer (zero-pad if input < max_block_size)
        for (buf, ch_in) in self.up_input_buf.iter_mut().zip(input.iter()) {
            buf[..n_in].copy_from_slice(&ch_in[..n_in]);
            for s in buf[n_in..self.max_block_size].iter_mut() {
                *s = 0.0;
            }
        }

        let (_in_used, out_written) = up
            .process_into_buffer(&self.up_input_buf, &mut self.up_output_buf, None)
            .expect("upsample process_into_buffer failed");

        let n_out = n_in * self.ratio;
        let copy_len = n_out.min(out_written);
        for (ch_out, ch_buf) in output.iter_mut().zip(self.up_output_buf.iter()) {
            ch_out[..copy_len].copy_from_slice(&ch_buf[..copy_len]);
        }

        n_out
    }

    /// Downsample input into pre-allocated output. Returns number of output samples per channel.
    ///
    /// `n_samples`: actual number of oversampled input samples to process
    pub fn process_down(&mut self, input: &[Vec<f32>], output: &mut [Vec<f32>], n_samples: usize) -> usize {
        let n_in = n_samples;

        if self.ratio == 1 {
            for (ch_out, ch_in) in output.iter_mut().zip(input.iter()) {
                ch_out[..n_in].copy_from_slice(&ch_in[..n_in]);
            }
            return n_in;
        }

        let down = self.down_resampler.as_mut().unwrap();
        let expected_in = self.max_block_size * self.ratio;

        // Copy input into padded buffer (zero-pad if input < max_block_size*ratio)
        for (buf, ch_in) in self.down_input_buf.iter_mut().zip(input.iter()) {
            buf[..n_in].copy_from_slice(&ch_in[..n_in]);
            for s in buf[n_in..expected_in].iter_mut() {
                *s = 0.0;
            }
        }

        let (_in_used, out_written) = down
            .process_into_buffer(&self.down_input_buf, &mut self.down_output_buf, None)
            .expect("downsample process_into_buffer failed");

        let n_out = n_in / self.ratio;
        let copy_len = n_out.min(out_written);
        for (ch_out, ch_buf) in output.iter_mut().zip(self.down_output_buf.iter()) {
            ch_out[..copy_len].copy_from_slice(&ch_buf[..copy_len]);
        }

        n_out
    }

    pub fn ratio(&self) -> usize {
        self.ratio
    }

    /// Get the latency introduced by the resamplers in host-rate samples.
    pub fn latency_samples(&self) -> u32 {
        if self.ratio <= 1 {
            return 0;
        }
        let up_delay = self.up_resampler.as_ref().map_or(0, |r| r.output_delay());
        let down_delay = self.down_resampler.as_ref().map_or(0, |r| r.output_delay());
        (up_delay / self.ratio + down_delay) as u32
    }

    /// Reset internal resampler state.
    pub fn reset(&mut self) {
        if let Some(ref mut r) = self.up_resampler {
            r.reset();
        }
        if let Some(ref mut r) = self.down_resampler {
            r.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_1x_is_passthrough() {
        let mut os = Oversampler::new(1, 256, 2);
        let input = vec![vec![1.0, 2.0, 3.0, 4.0]; 2];
        let mut output = vec![vec![0.0; 4]; 2];
        os.process_up(&input, &mut output, 4);
        assert_eq!(output, input);
        let mut down_out = vec![vec![0.0; 4]; 2];
        os.process_down(&output, &mut down_out, 4);
        assert_eq!(down_out, input);
    }

    #[test]
    fn test_upsample_output_length() {
        for ratio in [2, 4, 8] {
            let mut os = Oversampler::new(ratio, 256, 1);
            let input = vec![vec![0.5; 256]];
            let mut output = vec![vec![0.0; 256 * ratio]];
            let n = os.process_up(&input, &mut output, 256);
            assert_eq!(n, 256 * ratio, "ratio={ratio}");
        }
    }

    #[test]
    fn test_round_trip_preserves_dc() {
        let mut os = Oversampler::new(2, 512, 1);
        let dc_val = 0.7;
        let n = 512;
        let input = vec![vec![dc_val; n]];
        let mut up_out = vec![vec![0.0; n * 2]];
        let mut down_out = vec![vec![0.0; n]];

        for _ in 0..8 {
            let n_up = os.process_up(&input, &mut up_out, n);
            os.process_down(&up_out, &mut down_out, n_up);
        }

        let avg: f32 = down_out[0][n / 2..].iter().sum::<f32>() / (n / 2) as f32;
        assert!(
            (avg - dc_val).abs() < 0.02,
            "DC not preserved: expected {dc_val}, got {avg}"
        );
    }

    #[test]
    fn test_latency_zero_at_1x() {
        let os = Oversampler::new(1, 256, 2);
        assert_eq!(os.latency_samples(), 0);
    }

    #[test]
    fn test_latency_nonzero_at_higher_ratios() {
        for ratio in [2, 4, 8] {
            let os = Oversampler::new(ratio, 256, 1);
            assert!(
                os.latency_samples() > 0,
                "ratio={ratio} should have nonzero latency"
            );
        }
    }

    #[test]
    fn test_ratio_change_at_runtime() {
        let max_block = 512;
        let channels = 1;
        let block_len = 256;

        let mut os1 = Oversampler::new(1, max_block, channels);
        let input = vec![vec![0.5_f32; block_len]; channels];

        let mut up_out_1 = vec![vec![0.0_f32; block_len]; channels];
        let n_up = os1.process_up(&input, &mut up_out_1, block_len);

        let mut down_out_1 = vec![vec![0.0_f32; block_len]; channels];
        let n_down = os1.process_down(&up_out_1, &mut down_out_1, n_up);

        let mut os4 = Oversampler::new(4, max_block, channels);

        let mut up_out_4 = vec![vec![0.0_f32; block_len * 4]; channels];
        let n_up4 = os4.process_up(&input, &mut up_out_4, block_len);

        let mut down_out_4 = vec![vec![0.0_f32; block_len]; channels];
        let n_down4 = os4.process_down(&up_out_4, &mut down_out_4, n_up4);

        assert_eq!(n_up, block_len);
        assert_eq!(n_down, block_len);
        assert_eq!(n_up4, block_len * 4);
        assert_eq!(n_down4, block_len);
    }

    #[test]
    fn test_smaller_buffer_than_max_block() {
        let max_block = 512;
        let ratio = 4;
        let channels = 1;
        let block_len = 128;

        let mut os = Oversampler::new(ratio, max_block, channels);
        let input = vec![vec![0.3_f32; block_len]; channels];

        let mut up_out = vec![vec![0.0_f32; max_block * ratio]; channels];
        let n_up = os.process_up(&input, &mut up_out, block_len);
        assert_eq!(n_up, block_len * ratio);

        let mut down_out = vec![vec![0.0_f32; max_block]; channels];
        let n_down = os.process_down(&up_out, &mut down_out, n_up);
        assert_eq!(n_down, block_len);
    }
}
