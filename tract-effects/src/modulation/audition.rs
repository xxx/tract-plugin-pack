use super::{
    AudioTriggerSettings, DetectorInput, TrackModulation, TriggerSource, audio_trigger::AudioFilter,
};
use crate::effects::EffectKind;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditionSource {
    Trigger,
    Follower,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AuditionSettings {
    pub detector: AudioTriggerSettings,
    pub gain_db: f32,
}
impl AuditionSettings {
    pub fn for_source(config: &TrackModulation, source: AuditionSource) -> Option<Self> {
        match source {
            AuditionSource::Trigger
                if matches!(config.trigger, TriggerSource::TransientSample { .. }) =>
            {
                Some(Self {
                    detector: AudioTriggerSettings {
                        level_db: None,
                        ..config.audio_trigger.normalized()
                    },
                    gain_db: 0.0,
                })
            }
            AuditionSource::Trigger => None,
            AuditionSource::Follower => {
                let config = config.follower.normalized();
                Some(Self {
                    detector: AudioTriggerSettings {
                        level_db: None,
                        ..config.detector
                    },
                    gain_db: config.gain_db,
                })
            }
        }
    }
    fn normalized(self, source: AuditionSource) -> Self {
        Self {
            detector: AudioTriggerSettings {
                level_db: None,
                ..self.detector.normalized()
            },
            gain_db: if source == AuditionSource::Trigger || !self.gain_db.is_finite() {
                0.0
            } else {
                self.gain_db.clamp(-24.0, 24.0)
            },
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AuditionRequest {
    pub token: u64,
    pub row: usize,
    pub kind: EffectKind,
    pub source: AuditionSource,
    pub settings: AuditionSettings,
    pub generation: u64,
}
impl AuditionRequest {
    pub fn matches(
        self,
        row: usize,
        kind: EffectKind,
        config: &TrackModulation,
        generation: u64,
    ) -> bool {
        self.token != 0
            && self.row == row
            && self.kind == kind
            && self.generation == generation
            && AuditionSettings::for_source(config, self.source) == Some(self.settings)
    }
}

#[derive(Default)]
pub struct AuditionControl {
    token: AtomicU64,
    epoch: AtomicU32,
    stamp: AtomicU64,
    generation: AtomicU64,
    input: AtomicU32,
    gain: AtomicU32,
    filters: [AtomicU64; 3],
}
impl AuditionControl {
    pub fn resolve(
        &self,
        request: AuditionRequest,
        kind: EffectKind,
        config: &TrackModulation,
        generation: u64,
    ) -> Option<AuditionRequest> {
        let settings = AuditionSettings::for_source(config, request.source);
        if request.kind != kind || request.generation != generation || settings.is_none() {
            self.release(request.token);
            None
        } else {
            (settings == Some(request.settings)).then_some(request)
        }
    }
    fn store_settings(&self, settings: AuditionSettings) {
        self.input
            .store(settings.detector.input as u32, Ordering::Release);
        self.gain
            .store(settings.gain_db.to_bits(), Ordering::Release);
        for (dest, value) in self.filters.iter().zip([
            settings.detector.level_db,
            settings.detector.highpass_hz,
            settings.detector.lowpass_hz,
        ]) {
            dest.store(
                value.map_or(0, |v| (1 << 32) | v.to_bits() as u64),
                Ordering::Release,
            );
        }
    }
    pub fn begin(
        &self,
        row: usize,
        kind: EffectKind,
        source: AuditionSource,
        settings: AuditionSettings,
        generation: u64,
    ) -> u64 {
        if row >= 16 {
            return 0;
        }
        let epoch = self.epoch.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let token = ((epoch as u64) << 32)
            | 1
            | ((row as u64) << 1)
            | ((kind as u64) << 5)
            | ((u64::from(source == AuditionSource::Follower)) << 21);
        let stamp = self.stamp.load(Ordering::Relaxed).wrapping_add(2) & !1;
        self.stamp.store(stamp | 1, Ordering::Release);
        self.store_settings(settings.normalized(source));
        self.generation.store(generation, Ordering::Release);
        self.token.store(token, Ordering::Release);
        self.stamp.store(stamp, Ordering::Release);
        token
    }
    pub fn update(&self, token: u64, settings: AuditionSettings) -> bool {
        if !self.is_current(token) {
            return false;
        }
        let source = if token & (1 << 21) == 0 {
            AuditionSource::Trigger
        } else {
            AuditionSource::Follower
        };
        let stamp = self.stamp.load(Ordering::Relaxed).wrapping_add(2) & !1;
        self.stamp.store(stamp | 1, Ordering::Release);
        self.store_settings(settings.normalized(source));
        self.stamp.store(stamp, Ordering::Release);
        self.is_current(token)
    }
    pub fn release(&self, token: u64) {
        let _ = self
            .token
            .compare_exchange(token, 0, Ordering::AcqRel, Ordering::Relaxed);
    }
    pub fn invalidate(&self) {
        self.token.store(0, Ordering::Release);
    }
    pub fn is_current(&self, token: u64) -> bool {
        token != 0 && self.token.load(Ordering::Acquire) == token
    }
    pub fn snapshot(&self) -> Option<AuditionRequest> {
        let stamp = self.stamp.load(Ordering::Acquire);
        if stamp == 0 || stamp & 1 != 0 {
            return None;
        }
        let token = self.token.load(Ordering::Acquire);
        if token == 0 {
            return None;
        }
        let generation = self.generation.load(Ordering::Acquire);
        let input = match self.input.load(Ordering::Acquire) {
            0 => DetectorInput::Main,
            1 => DetectorInput::Sidechain,
            _ => return None,
        };
        let values: [Option<f32>; 3] = std::array::from_fn(|i| {
            let bits = self.filters[i].load(Ordering::Acquire);
            (bits >> 32 != 0).then_some(f32::from_bits(bits as u32))
        });
        let gain_db = f32::from_bits(self.gain.load(Ordering::Acquire));
        if self.stamp.load(Ordering::Acquire) != stamp || !self.is_current(token) {
            return None;
        }
        let kind = EffectKind::ALL
            .iter()
            .copied()
            .find(|k| *k as u64 == (token >> 5) & 0xffff)?;
        Some(AuditionRequest {
            token,
            row: ((token >> 1) & 15) as usize,
            kind,
            generation,
            source: if token & (1 << 21) == 0 {
                AuditionSource::Trigger
            } else {
                AuditionSource::Follower
            },
            settings: AuditionSettings {
                detector: AudioTriggerSettings {
                    input,
                    level_db: values[0],
                    highpass_hz: values[1],
                    lowpass_hz: values[2],
                },
                gain_db,
            },
        })
    }
}

pub struct AuditionRuntime {
    stream: Option<AuditionRequest>,
    enabled: bool,
    filters: [AudioFilter; 2],
    gain: f64,
    sample_rate: f64,
    samples: Box<[[f32; 2]]>,
    cursor: usize,
    queued: usize,
    latency: usize,
    blend: f64,
    fade_step: f64,
}
impl AuditionRuntime {
    pub fn new(max_latency: usize) -> Self {
        Self {
            stream: None,
            enabled: false,
            filters: [AudioFilter::default(); 2],
            gain: 1.0,
            sample_rate: 0.0,
            samples: vec![[0.0; 2]; max_latency.min(1 << 20) + 1].into_boxed_slice(),
            cursor: 0,
            queued: 0,
            latency: 0,
            blend: 0.0,
            fade_step: 1.0,
        }
    }
    pub fn reset(&mut self) {
        self.stream = None;
        self.enabled = false;
        self.queued = 0;
        self.blend = 0.0;
        for filter in &mut self.filters {
            filter.reset();
        }
    }
    /// Reserve only during construction/sample-rate setup.
    pub fn reserve(&mut self, max_latency: usize) {
        let needed = max_latency.min(1 << 20) + 1;
        if needed > self.samples.len() {
            self.samples = vec![[0.0; 2]; needed].into_boxed_slice();
            self.reset();
        }
    }
    pub fn prepare(&mut self, request: Option<AuditionRequest>, sr: f64) {
        self.enabled = request.is_some() && sr.is_finite() && sr > 0.0;
        if !self.enabled {
            return;
        }
        let Some(mut request) = request else {
            return;
        };
        request.settings = request.settings.normalized(request.source);
        let reset = self.sample_rate != sr
            || self.stream.is_none_or(|old| {
                old.row != request.row
                    || old.kind != request.kind
                    || old.generation != request.generation
                    || old.source != request.source
                    || old.settings.detector.input != request.settings.detector.input
            });
        if reset {
            self.queued = 0;
            self.cursor = 0;
            self.blend = 0.0;
            for filter in &mut self.filters {
                filter.reset();
            }
        }
        for filter in &mut self.filters {
            filter.configure(request.settings.detector, sr);
        }
        self.gain = 10.0_f64.powf(request.settings.gain_db as f64 / 20.0);
        self.sample_rate = sr;
        self.fade_step = 1.0 / (sr * 0.005).ceil().max(1.0);
        self.stream = Some(request);
    }
    pub fn needs_processing(&self) -> bool {
        self.stream.is_some()
    }
    pub fn next(
        &mut self,
        ordinary: [f32; 2],
        main: [f32; 2],
        sidechain: [f32; 2],
        control: &AuditionControl,
        latency: usize,
        allow: bool,
    ) -> [f32; 2] {
        let Some(request) = self.stream else {
            return ordinary;
        };
        if latency >= self.samples.len() {
            self.blend = 0.0;
            self.queued = 0;
            return ordinary;
        }
        let wanted = self.enabled && control.is_current(request.token);
        if !wanted && self.blend == 0.0 {
            self.stream = None;
            self.queued = 0;
            return ordinary;
        }
        if latency != self.latency {
            self.latency = latency;
            self.queued = 0;
            self.blend = 0.0;
        }
        let input = match request.settings.detector.input {
            DetectorInput::Main => main,
            DetectorInput::Sidechain => sidechain,
        };
        let clean = |v: f32| {
            if v.is_finite() {
                v.clamp(-64.0, 64.0)
            } else {
                0.0
            }
        };
        let filtered = match request.source {
            AuditionSource::Trigger => {
                [self.filters[0].process(clean((input[0] + input[1]) * 0.5)); 2]
            }
            AuditionSource::Follower => {
                std::array::from_fn(|i| self.filters[i].process(clean(input[i])))
            }
        };
        let bounded = filtered.map(|v| {
            let v = v as f64 * self.gain;
            if v.is_finite() {
                v.clamp(-1.0, 1.0) as f32
            } else {
                0.0
            }
        });
        if !allow {
            self.blend = 0.0;
            self.queued = 0;
            return ordinary;
        }
        self.samples[self.cursor] = bounded;
        let ready = self.queued >= latency;
        let index = (self.cursor + self.samples.len() - latency) % self.samples.len();
        let monitored = self.samples[index];
        self.cursor = (self.cursor + 1) % self.samples.len();
        self.queued = (self.queued + 1).min(self.samples.len());
        self.blend = if wanted && ready {
            (self.blend + self.fade_step).min(1.0)
        } else {
            (self.blend - self.fade_step).max(0.0)
        };
        if self.blend < 1e-12 {
            self.blend = 0.0;
        } else if self.blend > 1.0 - 1e-12 {
            self.blend = 1.0;
        }
        if self.blend == 0.0 {
            ordinary
        } else if self.blend == 1.0 {
            monitored
        } else {
            std::array::from_fn(|i| ordinary[i] + (monitored[i] - ordinary[i]) * self.blend as f32)
        }
    }
}

#[cfg(test)]
mod tests;
