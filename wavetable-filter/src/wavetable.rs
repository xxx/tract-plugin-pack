use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WavetableFormat {
    Wav,
    Wt,
}

#[derive(Debug, Clone)]
pub struct Wavetable {
    pub frames: Vec<Vec<f32>>,
    pub frame_size: usize,
    pub frame_count: usize,
}

impl Wavetable {
    /// Create a new wavetable from raw samples
    pub fn new(samples: Vec<f32>, frame_size: usize) -> Result<Self, String> {
        if !Self::is_valid_frame_size(frame_size) {
            return Err(format!(
                "Invalid frame size: {}. Must be 256, 512, 1024, or 2048",
                frame_size
            ));
        }

        if !samples.len().is_multiple_of(frame_size) {
            return Err(format!(
                "Sample count {} is not a multiple of frame size {}",
                samples.len(),
                frame_size
            ));
        }

        let frame_count = samples.len() / frame_size;

        if frame_count == 0 {
            return Err("Wavetable must have at least one frame".to_string());
        }

        if frame_count > 256 {
            return Err(format!(
                "Frame count {} exceeds maximum of 256",
                frame_count
            ));
        }

        let frames: Vec<Vec<f32>> = samples
            .chunks(frame_size)
            .map(|chunk| chunk.to_vec())
            .collect();

        Ok(Self {
            frames,
            frame_size,
            frame_count,
        })
    }

    /// Load a wavetable from a .wav file
    pub fn from_wav<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let mut reader =
            hound::WavReader::open(path).map_err(|e| format!("Failed to open WAV file: {}", e))?;

        let spec = reader.spec();

        // Read all samples
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to read samples: {}", e))?,
            hound::SampleFormat::Int => {
                let bits = spec.bits_per_sample;
                let max_value = (1 << (bits - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|v| v as f32 / max_value))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| format!("Failed to read samples: {}", e))?
            }
        };

        // Determine frame size based on total samples
        let frame_size = Self::detect_frame_size(samples.len())?;

        Self::new(samples, frame_size)
    }

    /// Load a wavetable from a .wt file (Serum/vawt format)
    pub fn from_wt<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("Failed to open WT file: {}", e))?;
        let mut reader = BufReader::new(file);

        // Read header (first 4 bytes)
        let mut header = [0u8; 4];
        reader
            .read_exact(&mut header)
            .map_err(|e| format!("Failed to read header: {}", e))?;

        // Support both "WATR" (Serum) and "vawt" formats
        let is_vawt = &header == b"vawt";
        let is_watr = &header == b"WATR";

        if !is_vawt && !is_watr {
            return Err(format!(
                "Invalid WT file format (header: {:?})",
                std::str::from_utf8(&header).unwrap_or("invalid UTF-8")
            ));
        }

        // Read frame size and count based on format
        let (frame_size, frame_count, use_float) = if is_vawt {
            // vawt format: 4 bytes frame size, 2 bytes frame count, 2 bytes flags
            let mut frame_size_bytes = [0u8; 4];
            reader
                .read_exact(&mut frame_size_bytes)
                .map_err(|e| format!("Failed to read frame size: {}", e))?;
            let frame_size = u32::from_le_bytes(frame_size_bytes) as usize;

            let mut frame_count_bytes = [0u8; 2];
            reader
                .read_exact(&mut frame_count_bytes)
                .map_err(|e| format!("Failed to read frame count: {}", e))?;
            let frame_count = u16::from_le_bytes(frame_count_bytes) as usize;

            let mut flags_bytes = [0u8; 2];
            reader
                .read_exact(&mut flags_bytes)
                .map_err(|e| format!("Failed to read flags: {}", e))?;
            let flags = u16::from_le_bytes(flags_bytes);

            // Bit 0x0004: 0 = float32, 1 = int16
            let use_float = (flags & 0x0004) == 0;

            (frame_size, frame_count, use_float)
        } else {
            // WATR format: 4 bytes frame size, 4 bytes frame count
            let mut frame_size_bytes = [0u8; 4];
            reader
                .read_exact(&mut frame_size_bytes)
                .map_err(|e| format!("Failed to read frame size: {}", e))?;
            let frame_size = u32::from_le_bytes(frame_size_bytes) as usize;

            let mut frame_count_bytes = [0u8; 4];
            reader
                .read_exact(&mut frame_count_bytes)
                .map_err(|e| format!("Failed to read frame count: {}", e))?;
            let frame_count = u32::from_le_bytes(frame_count_bytes) as usize;

            (frame_size, frame_count, true) // WATR uses floats
        };

        if !Self::is_valid_frame_size(frame_size) {
            return Err(format!("Invalid frame size in WT file: {}", frame_size));
        }

        if frame_count > 256 {
            return Err(format!(
                "Frame count {} exceeds maximum of 256",
                frame_count
            ));
        }

        // Read all samples based on format
        let total_samples = frame_size * frame_count;
        let samples = if use_float {
            // float32 format
            let mut buffer = vec![0u8; total_samples * 4];
            reader
                .read_exact(&mut buffer)
                .map_err(|e| format!("Failed to read sample data: {}", e))?;

            buffer
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()
        } else {
            // int16 format
            let mut buffer = vec![0u8; total_samples * 2];
            reader
                .read_exact(&mut buffer)
                .map_err(|e| format!("Failed to read sample data: {}", e))?;

            buffer
                .chunks_exact(2)
                .map(|chunk| {
                    let sample_i16 = i16::from_le_bytes([chunk[0], chunk[1]]);
                    sample_i16 as f32 / 32768.0 // Convert to -1.0 to 1.0 range
                })
                .collect()
        };

        Self::new(samples, frame_size)
    }

    /// Load a wavetable from a file, automatically detecting format
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let path_ref = path.as_ref();
        let extension = path_ref
            .extension()
            .and_then(|s| s.to_str())
            .ok_or("No file extension")?;

        match extension.to_lowercase().as_str() {
            "wav" => Self::from_wav(path),
            "wt" => Self::from_wt(path),
            _ => Err(format!("Unsupported file format: {}", extension)),
        }
    }

    /// Write the interpolated frame into a pre-allocated buffer (no heap allocation).
    /// `out` is resized only if its length does not match `frame_size`.
    pub fn interpolate_frame_into(&self, frame_position: f32, out: &mut Vec<f32>) {
        if out.len() != self.frame_size {
            out.resize(self.frame_size, 0.0);
        }
        if self.frames.is_empty() {
            out.fill(0.0);
            return;
        }
        let frame_position = frame_position.clamp(0.0, 1.0);
        let frame_float = frame_position * (self.frame_count - 1) as f32;
        let frame_index = frame_float.floor() as usize;
        let frame_frac = frame_float.fract();
        let frame1 = &self.frames[frame_index];
        let frame2 = if frame_index + 1 < self.frame_count {
            &self.frames[frame_index + 1]
        } else {
            frame1
        };
        for ((o, &s1), &s2) in out.iter_mut().zip(frame1).zip(frame2) {
            *o = s1 + (s2 - s1) * frame_frac;
        }
    }

    /// Get an interpolated frame at a given position (0.0 to 1.0)
    /// Returns the frame data to be used as a filter kernel
    pub fn get_frame_interpolated(&self, frame_position: f32) -> Vec<f32> {
        if self.frames.is_empty() {
            return vec![0.0; self.frame_size];
        }

        let frame_position = frame_position.clamp(0.0, 1.0);

        // Calculate frame indices for interpolation
        let frame_float = frame_position * (self.frame_count - 1) as f32;
        let frame_index = frame_float.floor() as usize;
        let frame_frac = frame_float.fract();

        // Get the two frames to interpolate between
        let frame1 = &self.frames[frame_index];
        let frame2 = if frame_index + 1 < self.frame_count {
            &self.frames[frame_index + 1]
        } else {
            frame1
        };

        // Linearly interpolate between the two frames
        frame1
            .iter()
            .zip(frame2.iter())
            .map(|(s1, s2)| s1 + (s2 - s1) * frame_frac)
            .collect()
    }

    /// Sample the wavetable at a given phase (0.0 to 1.0) and frame position (0.0 to 1.0)
    pub fn sample_at_phase(&self, phase: f32, frame_position: f32) -> f32 {
        if self.frames.is_empty() {
            return 0.0;
        }

        // Clamp inputs
        let phase = phase.clamp(0.0, 1.0);
        let frame_position = frame_position.clamp(0.0, 1.0);

        // Calculate frame indices for interpolation
        let frame_float = frame_position * (self.frame_count - 1) as f32;
        let frame_index = frame_float.floor() as usize;
        let frame_frac = frame_float.fract();

        // Get the two frames to interpolate between
        let frame1 = &self.frames[frame_index];
        let frame2 = if frame_index + 1 < self.frame_count {
            &self.frames[frame_index + 1]
        } else {
            frame1
        };

        // Calculate sample position within frame
        let sample_pos = phase * self.frame_size as f32;
        let sample_index = sample_pos.floor() as usize % self.frame_size;
        let sample_frac = sample_pos.fract();

        // Linear interpolation within each frame
        let next_index = (sample_index + 1) % self.frame_size;

        let sample1_frame1 = frame1[sample_index];
        let sample2_frame1 = frame1[next_index];
        let value_frame1 = sample1_frame1 + (sample2_frame1 - sample1_frame1) * sample_frac;

        let sample1_frame2 = frame2[sample_index];
        let sample2_frame2 = frame2[next_index];
        let value_frame2 = sample1_frame2 + (sample2_frame2 - sample1_frame2) * sample_frac;

        // Interpolate between frames
        value_frame1 + (value_frame2 - value_frame1) * frame_frac
    }

    fn is_valid_frame_size(size: usize) -> bool {
        matches!(size, 256 | 512 | 1024 | 2048)
    }

    fn detect_frame_size(total_samples: usize) -> Result<usize, String> {
        if total_samples == 0 {
            return Err("WAV file contains no audio data".to_string());
        }
        for &size in &[2048, 1024, 512, 256] {
            if total_samples.is_multiple_of(size) && total_samples / size <= 256 {
                return Ok(size);
            }
        }
        Err(format!(
            "Cannot determine valid frame size for {} samples",
            total_samples
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wavetable_creation() {
        // Create a simple sine wave
        let frame_size = 256;
        let frame_count = 2;
        let mut samples = Vec::new();

        for _frame in 0..frame_count {
            for i in 0..frame_size {
                let phase = i as f32 / frame_size as f32;
                let value = (phase * 2.0 * std::f32::consts::PI).sin();
                samples.push(value);
            }
        }

        let wavetable = Wavetable::new(samples, frame_size).unwrap();
        assert_eq!(wavetable.frame_size, frame_size);
        assert_eq!(wavetable.frame_count, frame_count);
    }

    #[test]
    fn test_invalid_frame_size() {
        let samples = vec![0.0; 128]; // 128 is not a valid frame size
        let result = Wavetable::new(samples, 128);
        assert!(result.is_err());
    }

    #[test]
    fn test_sampling() {
        let frame_size = 256;
        let samples: Vec<f32> = (0..frame_size)
            .map(|i| (i as f32 / frame_size as f32 * 2.0 * std::f32::consts::PI).sin())
            .collect();

        let wavetable = Wavetable::new(samples, frame_size).unwrap();

        // Sample at phase 0.0 should give us the first sample
        let sample = wavetable.sample_at_phase(0.0, 0.0);
        assert!((sample - 0.0).abs() < 0.01);

        // Sample at phase 0.25 should give us approximately 1.0
        let sample = wavetable.sample_at_phase(0.25, 0.0);
        assert!((sample - 1.0).abs() < 0.1);
    }
}
