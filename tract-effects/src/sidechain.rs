//! Borrowed auxiliary input with silent missing samples and mono duplication.

#[derive(Clone, Copy, Debug, Default)]
pub struct SidechainInput<'a> {
    left: &'a [f32],
    right: &'a [f32],
}

impl<'a> SidechainInput<'a> {
    pub fn new(left: &'a [f32], right: &'a [f32]) -> Self {
        Self { left, right }
    }

    pub fn from_channels(channels: &'a [&mut [f32]]) -> Self {
        let left = channels.first().map_or(&[][..], |channel| &channel[..]);
        let right = channels.get(1).map_or(left, |channel| &channel[..]);
        Self::new(left, right)
    }

    pub fn sample(self, index: usize) -> [f32; 2] {
        [
            self.left.get(index).copied().unwrap_or(0.0),
            self.right.get(index).copied().unwrap_or(0.0),
        ]
    }

    pub fn offset(self, samples: usize) -> Self {
        Self::new(
            self.left.get(samples..).unwrap_or_default(),
            self.right.get(samples..).unwrap_or_default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_duplicates_and_missing_channels_are_silent_without_writing_input() {
        let mut mono = [0.1, -0.8];
        let original = mono;
        let channels = [&mut mono[..]];
        let input = SidechainInput::from_channels(&channels);
        assert_eq!(input.sample(0), [0.1; 2]);
        assert_eq!(input.sample(1), [-0.8; 2]);
        assert_eq!(input.sample(2), [0.0; 2]);
        assert_eq!(SidechainInput::from_channels(&[]).sample(0), [0.0; 2]);
        assert_eq!(mono, original);
    }

    #[test]
    fn short_stereo_buffers_keep_their_independent_sample_offsets() {
        let input = SidechainInput::new(&[0.1, 0.2, 0.3], &[-0.8]);
        assert_eq!(input.sample(0), [0.1, -0.8]);
        assert_eq!(input.sample(1), [0.2, 0.0]);
        assert_eq!(input.offset(1).sample(1), [0.3, 0.0]);
        assert_eq!(input.offset(3).sample(0), [0.0; 2]);
        assert_eq!(input.offset(usize::MAX).sample(0), [0.0; 2]);
    }
}
