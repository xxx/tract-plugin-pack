//! One effect, shared modulation, and latency-aligned dry audio.

use tract_effects::effects::{Effect, EffectInstance, EffectKind, TrackEffect};
use tract_effects::modulation::{ModulationState, TrackModulation};

pub const MAX_LATENCY: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrepareKey {
    pub kind: EffectKind,
    pub generation: u64,
    pub sample_rate_bits: u32,
}

struct DryDelay {
    samples: Box<[[f32; 2]]>,
    position: usize,
}

impl DryDelay {
    fn new() -> Self {
        Self {
            samples: vec![[0.0; 2]; MAX_LATENCY + 1].into_boxed_slice(),
            position: 0,
        }
    }
    fn reset(&mut self) {
        self.samples.fill([0.0; 2]);
        self.position = 0;
    }
    fn next(&mut self, input: [f32; 2], latency: usize) -> [f32; 2] {
        debug_assert!(latency <= MAX_LATENCY);
        self.samples[self.position] = input;
        let index =
            (self.position + self.samples.len() - latency.min(MAX_LATENCY)) % self.samples.len();
        let output = self.samples[index];
        self.position = (self.position + 1) % self.samples.len();
        output
    }
}

pub struct Processor {
    pub key: PrepareKey,
    effect: EffectInstance,
    pub modulation: TrackModulation,
    state: ModulationState,
    dry: DryDelay,
    latency: usize,
}

impl Processor {
    /// Construction and sample-rate setup are restricted to initialize/background work.
    pub fn new(key: PrepareKey, track: &TrackEffect, modulation: TrackModulation) -> Self {
        let mut effect = EffectInstance::new(key.kind);
        effect.set_sample_rate(f32::from_bits(key.sample_rate_bits));
        for (i, _) in effect.parameters().iter().enumerate() {
            effect.set_param(i, track.params[i]);
        }
        let latency = effect.latency_samples();
        Self {
            key,
            effect,
            modulation,
            state: ModulationState::default(),
            dry: DryDelay::new(),
            latency,
        }
    }

    pub fn begin_block(&mut self, track: &TrackEffect, left: &[f32], right: &[f32], bpm: f64) {
        self.effect.set_bpm(bpm as f32);
        for (i, _) in self.effect.parameters().iter().enumerate() {
            self.effect.set_param(i, track.params[i]);
        }
        let sample_rate = f32::from_bits(self.key.sample_rate_bits) as f64;
        self.state
            .begin_block(&self.modulation, left, right, sample_rate);
        self.state.advance_segment(
            &self.modulation,
            track,
            &mut self.effect,
            left.len().min(right.len()),
            bpm,
            sample_rate,
        );
        self.latency = self.effect.latency_samples();
    }

    pub fn process_sample(&mut self, input: [f32; 2], mix: f32, bypass: bool) -> [f32; 2] {
        let dry = self.dry.next(input, self.latency);
        if self.key.kind == EffectKind::None {
            return dry;
        }
        let wet = self.effect.process_sample(input[0], input[1]);
        let mix = self.state.next_mix(mix).clamp(0.0, 1.0);
        if bypass {
            return dry;
        }
        let amp = self.state.amplitude();
        [
            dry[0] * (1.0 - mix) + wet.0 * amp * mix,
            dry[1] * (1.0 - mix) + wet.1 * amp * mix,
        ]
    }

    pub fn reset_modulation(&mut self) {
        self.state.reset();
    }
    pub fn reset(&mut self) {
        self.flush_dsp();
        self.state.reset();
    }
    pub fn flush_dsp(&mut self) {
        self.effect.reset();
        self.dry.reset();
    }
    pub fn latency(&self) -> usize {
        self.latency
    }
    pub fn phase(&self, slot: usize) -> f32 {
        self.state.phase(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::neutral_modulation;
    use tiny_skia_widgets::SyncMode;
    use tract_effects::effects::default_params_for_kind;
    use tract_effects::modulation::ModulationTarget;

    fn processor(kind: EffectKind) -> (Processor, TrackEffect) {
        let track = TrackEffect {
            kind,
            params: default_params_for_kind(kind),
            ..TrackEffect::default()
        };
        let p = Processor::new(
            PrepareKey {
                kind,
                generation: 1,
                sample_rate_bits: 48000.0_f32.to_bits(),
            },
            &track,
            neutral_modulation(),
        );
        (p, track)
    }

    #[test]
    fn mix_mseg_ramps_the_audio_blend_without_allocating_or_editing_the_base() {
        let (mut modulated, mut track) = processor(EffectKind::Bitcrush);
        track.params[0] = 4.0;
        track.mix = 0.0;
        modulated.modulation.targets[0] = Some(ModulationTarget::Mix);
        modulated.modulation.depths[0] = 1.0;
        modulated.modulation.msegs[1].polarity = tiny_skia_widgets::Polarity::Unipolar;
        for node in &mut modulated.modulation.msegs[1].nodes {
            node.value = 1.0;
        }
        let (mut wet, mut wet_track) = processor(EffectKind::Bitcrush);
        wet_track.params = track.params;
        wet_track.mix = 1.0;
        let input = [0.6; 8];
        assert_no_alloc::assert_no_alloc(|| {
            modulated.begin_block(&track, &input, &input, 120.0);
            wet.begin_block(&wet_track, &input, &input, 120.0);
            for sample in 0..input.len() {
                let wet = wet.process_sample([0.6; 2], 1.0, false);
                let actual = modulated.process_sample([0.6; 2], track.mix, false);
                let mix = (sample + 1) as f32 / input.len() as f32;
                assert!((actual[0] - (0.6 + (wet[0] - 0.6) * mix)).abs() < 1e-6);
                assert!((actual[1] - actual[0]).abs() < 1e-6);
            }
        });
        assert_eq!(track.mix, 0.0);
    }

    #[test]
    fn none_is_passthrough_even_with_a_silent_amp_curve() {
        let (mut p, track) = processor(EffectKind::None);
        for node in &mut p.modulation.msegs[0].nodes {
            node.value = 0.0;
        }
        p.begin_block(&track, &[0.25; 32], &[-0.5; 32], 120.0);
        assert_eq!(p.process_sample([0.25, -0.5], 1.0, false), [0.25, -0.5]);
    }

    #[test]
    fn every_effect_processes_and_resets_without_audio_thread_allocations() {
        let input = [0.1; 512];
        for kind in EffectKind::ALL {
            let (mut p, mut track) = processor(kind);
            if kind == EffectKind::Satch {
                // Detail=0 skips the spectral path.
                track.params[2] = 50.0;
            }
            let fft_param = tract_effects::effects::parameter_specs(kind)
                .iter()
                .position(|spec| spec.name == "FFT");
            let fft_settings = if fft_param.is_some() { 4 } else { 1 };
            for fft in 0..fft_settings {
                if let Some(index) = fft_param {
                    track.params[index] = fft as f32;
                }
                assert_no_alloc::assert_no_alloc(|| {
                    // Four frames even at the largest supported 4096-point FFT.
                    for _ in 0..32 {
                        p.begin_block(&track, &input, &input, 123.0);
                        for _ in 0..input.len() {
                            assert!(
                                p.process_sample([0.1, -0.1], 1.0, false)
                                    .iter()
                                    .all(|v| v.is_finite())
                            );
                        }
                    }
                    p.reset_modulation();
                    p.reset();
                });
                assert!(p.latency() <= MAX_LATENCY);
            }
        }
    }

    #[test]
    fn dry_and_bypass_impulses_follow_spectral_latency_at_each_fft_size() {
        for fft in 0..4 {
            for bypass in [false, true] {
                let (mut p, mut track) = processor(EffectKind::SpectralShift);
                track.params[2] = fft as f32;
                p.begin_block(&track, &[1.0], &[1.0], 120.0);
                let latency = p.latency();
                assert_eq!(latency, [512, 1024, 2048, 4096][fft]);
                for i in 0..latency + 2 {
                    let input = if i == 0 { [1.0, -0.5] } else { [0.0; 2] };
                    let expected = if i == latency { [1.0, -0.5] } else { [0.0; 2] };
                    assert_eq!(p.process_sample(input, 0.0, bypass), expected);
                }
            }
        }
    }

    #[test]
    fn clocks_keep_running_and_config_edits_preserve_phase() {
        let (mut p, track) = processor(EffectKind::Svf);
        p.modulation.msegs[1].sync_mode = SyncMode::Time;
        p.modulation.msegs[1].time_seconds = 1.0;
        let input = [0.0; 2400];
        p.begin_block(&track, &input, &input, 120.0);
        assert!((p.phase(1) - 0.05).abs() < 1e-6);
        p.modulation.depths[0] = 0.7;
        p.begin_block(&track, &input, &input, 90.0);
        assert!((p.phase(1) - 0.1).abs() < 1e-6);
        p.reset_modulation();
        assert_eq!(p.phase(1), 0.0);
    }

    #[test]
    fn modulation_reset_keeps_delay_tail_but_host_reset_clears_it() {
        let (mut p, mut track) = processor(EffectKind::Delay);
        track.params[0] = 10.0;
        p.begin_block(&track, &[1.0], &[1.0], 120.0);
        p.process_sample([1.0; 2], 1.0, false);
        p.reset_modulation();
        let mut peak = 0.0_f32;
        for _ in 0..1000 {
            peak = peak.max(p.process_sample([0.0; 2], 1.0, false)[0].abs());
        }
        assert!(peak > 0.01);
        p.reset();
        for _ in 0..1000 {
            assert_eq!(p.process_sample([0.0; 2], 1.0, false), [0.0; 2]);
        }
    }
}
