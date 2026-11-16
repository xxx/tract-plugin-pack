//! Request-aware, bounded audio-to-editor modulation measurements.
use crate::effects::{EffectKind, MAX_EFFECT_PARAMS};
use crate::modulation::{AudioTriggerSettings, DetectorInput, DetectorReading};
use crate::modulation::{MidiWaveMapping, ModulationState, ModulationTarget, TrackModulation};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const TRACE_CAPACITY: usize = 512;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModulationFrame {
    pub kind: EffectKind,
    pub targets: [Option<ModulationTarget>; 3],
    pub phases: [f32; 4],
    pub outputs: [f32; 4],
    pub parameters: [Option<(f32, u8)>; MAX_EFFECT_PARAMS],
    pub mix: Option<(f32, u8)>,
    pub detector: Option<DetectorReading>,
    pub wave: Option<(MidiWaveMapping, Option<u8>)>,
}
impl ModulationFrame {
    pub fn capture(kind: EffectKind, config: &TrackModulation, state: &ModulationState) -> Self {
        Self {
            kind,
            targets: config.targets,
            phases: std::array::from_fn(|i| state.phase(i)),
            outputs: std::array::from_fn(|i| state.output(i)),
            parameters: state.parameter_norms(),
            mix: state.delivered_mix(),
            detector: state.detector_reading(),
            wave: state.midi_wave(),
        }
    }
    pub fn matches(&self, kind: EffectKind, config: &TrackModulation) -> bool {
        self.kind == kind
            && self.targets == config.targets
            && self.wave.map(|(mapping, _)| mapping)
                == config.wave_switch.filter(|mapping| mapping.is_valid())
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TracePoint {
    pub phase: f32,
    pub value: f32,
}
fn pair(a: f32, b: f32) -> u64 {
    a.to_bits() as u64 | ((b.to_bits() as u64) << 32)
}
fn unpair(bits: u64) -> (f32, f32) {
    (
        f32::from_bits(bits as u32),
        f32::from_bits((bits >> 32) as u32),
    )
}
fn norm(value: Option<(f32, u8)>) -> u64 {
    value.map_or(0, |(v, slot)| {
        v.to_bits() as u64 | ((slot as u64 + 1) << 32)
    })
}
fn unnorm(bits: u64) -> Option<(f32, u8)> {
    let slot = (bits >> 32) as u8;
    if slot == 0 {
        None
    } else {
        Some((f32::from_bits(bits as u32), slot - 1))
    }
}
fn target(value: Option<ModulationTarget>) -> u32 {
    match value {
        None => 0,
        Some(ModulationTarget::Mix) => u32::MAX,
        Some(ModulationTarget::EffectParameter(i)) => {
            u32::try_from(i.saturating_add(1)).unwrap_or(u32::MAX - 1)
        }
    }
}
fn untarget(bits: u32) -> Option<ModulationTarget> {
    match bits {
        0 => None,
        u32::MAX => Some(ModulationTarget::Mix),
        n => Some(ModulationTarget::EffectParameter(n as usize - 1)),
    }
}

fn optional_float(value: Option<f32>) -> u64 {
    value.map_or(0, |value| value.to_bits() as u64 | (1 << 32))
}
fn read_optional_float(bits: u64) -> Option<f32> {
    (bits >> 32 != 0).then_some(f32::from_bits(bits as u32))
}

fn wave_tag(wave: Option<(MidiWaveMapping, Option<u8>)>) -> u32 {
    wave.map_or(0, |(mapping, slot)| {
        (mapping.envelope as u32 + 1)
            | ((mapping.first_note as u32) << 8)
            | ((mapping.channel.map_or(0, |channel| channel + 1) as u32) << 16)
            | ((slot.map_or(0, |slot| slot + 1) as u32) << 24)
    })
}
fn read_wave_tag(bits: u32) -> Option<(MidiWaveMapping, Option<u8>)> {
    let envelope = bits as u8;
    let channel = (bits >> 16) as u8;
    let slot = (bits >> 24) as u8;
    if envelope == 0 || envelope > 4 || channel > 16 || slot > 8 {
        return None;
    }
    let mapping = MidiWaveMapping {
        envelope: envelope - 1,
        first_note: (bits >> 8) as u8,
        channel: channel.checked_sub(1),
    };
    mapping.is_valid().then_some((mapping, slot.checked_sub(1)))
}

struct AtomicFrame {
    stamp: AtomicU64,
    token: AtomicU64,
    kind: AtomicU32,
    targets: [AtomicU32; 3],
    sources: [AtomicU64; 4],
    parameters: [AtomicU64; MAX_EFFECT_PARAMS],
    mix: AtomicU64,
    detector: [AtomicU64; 4],
    detector_input: AtomicU32,
    wave: AtomicU32,
}
impl Default for AtomicFrame {
    fn default() -> Self {
        Self {
            stamp: AtomicU64::new(0),
            token: AtomicU64::new(0),
            kind: AtomicU32::new(0),
            targets: std::array::from_fn(|_| AtomicU32::new(0)),
            sources: std::array::from_fn(|_| AtomicU64::new(0)),
            parameters: std::array::from_fn(|_| AtomicU64::new(0)),
            mix: AtomicU64::new(0),
            detector: std::array::from_fn(|_| AtomicU64::new(0)),
            detector_input: AtomicU32::new(0),
            wave: AtomicU32::new(0),
        }
    }
}
impl AtomicFrame {
    fn store(&self, token: u64, frame: &ModulationFrame) {
        let stamp = self.stamp.load(Ordering::Relaxed).wrapping_add(2) & !1;
        self.stamp.store(stamp | 1, Ordering::Release);
        self.token.store(token, Ordering::Release);
        self.kind.store(frame.kind as u32, Ordering::Release);
        for i in 0..3 {
            self.targets[i].store(target(frame.targets[i]), Ordering::Release);
        }
        for i in 0..4 {
            self.sources[i].store(pair(frame.phases[i], frame.outputs[i]), Ordering::Release);
        }
        for i in 0..MAX_EFFECT_PARAMS {
            self.parameters[i].store(norm(frame.parameters[i]), Ordering::Release);
        }
        self.mix.store(norm(frame.mix), Ordering::Release);
        let settings = frame
            .detector
            .map_or(AudioTriggerSettings::default(), |reading| reading.settings);
        for (dest, value) in self.detector.iter().zip([
            frame.detector.map(|reading| reading.level),
            settings.level_db,
            settings.highpass_hz,
            settings.lowpass_hz,
        ]) {
            dest.store(optional_float(value), Ordering::Release);
        }
        self.detector_input
            .store(settings.input as u32, Ordering::Release);
        self.wave.store(wave_tag(frame.wave), Ordering::Release);
        self.stamp.store(stamp, Ordering::Release);
    }
    fn load(&self, token: u64) -> Option<ModulationFrame> {
        let stamp = self.stamp.load(Ordering::Acquire);
        if stamp == 0 || stamp & 1 != 0 {
            return None;
        }
        let actual_token = self.token.load(Ordering::Acquire);
        let tag = self.kind.load(Ordering::Acquire);
        let targets = std::array::from_fn(|i| untarget(self.targets[i].load(Ordering::Acquire)));
        let sources: [(f32, f32); 4] =
            std::array::from_fn(|i| unpair(self.sources[i].load(Ordering::Acquire)));
        let parameters =
            std::array::from_fn(|i| unnorm(self.parameters[i].load(Ordering::Acquire)));
        let mix = unnorm(self.mix.load(Ordering::Acquire));
        let detector: [Option<f32>; 4] =
            std::array::from_fn(|i| read_optional_float(self.detector[i].load(Ordering::Acquire)));
        let detector_input = match self.detector_input.load(Ordering::Acquire) {
            1 => DetectorInput::Sidechain,
            _ => DetectorInput::Main,
        };
        let wave = read_wave_tag(self.wave.load(Ordering::Acquire));
        if self.stamp.load(Ordering::Acquire) != stamp || actual_token != token {
            return None;
        }
        let kind = EffectKind::ALL
            .iter()
            .copied()
            .find(|kind| *kind as u32 == tag)?;
        Some(ModulationFrame {
            kind,
            targets,
            phases: sources.map(|p| p.0),
            outputs: sources.map(|p| p.1),
            parameters,
            mix,
            detector: detector[0].map(|level| DetectorReading {
                level,
                settings: AudioTriggerSettings {
                    input: detector_input,
                    level_db: detector[1],
                    highpass_hz: detector[2],
                    lowpass_hz: detector[3],
                },
            }),
            wave,
        })
    }
}
struct TraceCell {
    stamp: AtomicU64,
    token: AtomicU64,
    value: AtomicU64,
}
impl Default for TraceCell {
    fn default() -> Self {
        Self {
            stamp: AtomicU64::new(0),
            token: AtomicU64::new(0),
            value: AtomicU64::new(0),
        }
    }
}

pub struct ModulationTelemetry {
    pub waveform: crate::waveform::WaveformTelemetry,
    request: AtomicU64,
    epoch: AtomicU32,
    head: AtomicU64,
    latest: AtomicFrame,
    trace: [TraceCell; TRACE_CAPACITY],
}
impl Default for ModulationTelemetry {
    fn default() -> Self {
        Self {
            waveform: crate::waveform::WaveformTelemetry::default(),
            request: AtomicU64::new(0),
            epoch: AtomicU32::new(0),
            head: AtomicU64::new(0),
            latest: AtomicFrame::default(),
            trace: std::array::from_fn(|_| TraceCell::default()),
        }
    }
}
impl ModulationTelemetry {
    pub fn request(&self, row: usize, slot: usize, kind: EffectKind, trace: bool) -> u64 {
        if row >= 16 || slot >= 4 {
            return 0;
        }
        let key = 1_u64
            | ((slot as u64) << 1)
            | ((row as u64) << 3)
            | ((kind as u64) << 7)
            | ((trace as u64) << 23);
        let current = self.request.load(Ordering::Acquire);
        if current as u32 == key as u32 {
            return current;
        }
        let epoch = self.epoch.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let token = ((epoch as u64) << 32) | key;
        self.request.store(token, Ordering::Release);
        token
    }
    /// End history across processor/state replacement, including same-kind reloads.
    pub fn invalidate(&self) {
        self.waveform.invalidate();
        self.request.store(0, Ordering::Release);
    }
    pub fn release(&self, token: u64) {
        let _ = self
            .request
            .compare_exchange(token, 0, Ordering::AcqRel, Ordering::Relaxed);
    }
    pub fn frame(&self, token: u64) -> Option<ModulationFrame> {
        if token == 0 {
            None
        } else {
            self.latest.load(token)
        }
    }
    pub fn head(&self) -> u64 {
        self.head.load(Ordering::Acquire)
    }
    pub fn point(&self, index: u64, token: u64) -> Option<TracePoint> {
        let cell = &self.trace[index as usize % TRACE_CAPACITY];
        let stamp = cell.stamp.load(Ordering::Acquire);
        if stamp != index.wrapping_add(1) {
            return None;
        }
        let actual = cell.token.load(Ordering::Acquire);
        let (phase, value) = unpair(cell.value.load(Ordering::Acquire));
        if cell.stamp.load(Ordering::Acquire) != stamp || actual != token {
            return None;
        }
        Some(TracePoint { phase, value })
    }
    fn record(&self, token: u64, point: TracePoint) {
        let index = self.head.load(Ordering::Relaxed);
        let cell = &self.trace[index as usize % TRACE_CAPACITY];
        cell.stamp.store(0, Ordering::Release);
        cell.token.store(token, Ordering::Release);
        cell.value
            .store(pair(point.phase, point.value), Ordering::Release);
        cell.stamp.store(index.wrapping_add(1), Ordering::Release);
        self.head.store(index.wrapping_add(1), Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TraceRequest {
    pub token: u64,
    pub row: usize,
    pub slot: usize,
    pub kind: u32,
    pub trace: bool,
}
impl TraceRequest {
    pub fn matches(self, row: usize, kind: EffectKind) -> bool {
        self.token != 0 && self.row == row && self.kind == kind as u32
    }
}
#[derive(Default)]
pub struct TraceClock {
    request: TraceRequest,
    remaining: f64,
    period: f64,
    sample_rate: f64,
}
impl TraceClock {
    pub fn begin_block(
        &mut self,
        telemetry: &ModulationTelemetry,
        sample_rate: f64,
    ) -> TraceRequest {
        let token = telemetry.request.load(Ordering::Acquire);
        if token != self.request.token || sample_rate != self.sample_rate {
            self.remaining = 0.0;
        }
        self.period = (sample_rate / 1000.0).max(1.0);
        self.sample_rate = sample_rate;
        self.request = TraceRequest {
            token,
            row: ((token >> 3) & 15) as usize,
            slot: ((token >> 1) & 3) as usize,
            kind: ((token >> 7) & 0xffff) as u32,
            trace: token & (1 << 23) != 0,
        };
        self.request
    }
    pub fn tick(&mut self) -> bool {
        if self.request.token == 0 || !self.request.trace {
            return false;
        }
        let emit = self.remaining < 1.0;
        if emit {
            self.remaining += self.period;
        }
        self.remaining -= 1.0;
        emit
    }
    pub fn record(&self, telemetry: &ModulationTelemetry, row: usize, frame: &ModulationFrame) {
        if self.request.matches(row, frame.kind) {
            telemetry.record(
                self.request.token,
                TracePoint {
                    phase: frame.phases[self.request.slot],
                    value: frame.outputs[self.request.slot],
                },
            );
        }
    }
    pub fn publish(&self, telemetry: &ModulationTelemetry, row: usize, frame: &ModulationFrame) {
        if self.request.matches(row, frame.kind) {
            telemetry.latest.store(self.request.token, frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> ModulationFrame {
        ModulationFrame {
            kind: EffectKind::Svf,
            targets: [None; 3],
            phases: [0.25; 4],
            outputs: [0.75; 4],
            parameters: [None; MAX_EFFECT_PARAMS],
            mix: Some((0.4, 2)),
            detector: None,
            wave: None,
        }
    }
    #[test]
    fn snapshots_and_trace_preserve_exact_payload_and_reject_other_epochs() {
        let telemetry = ModulationTelemetry::default();
        let token = telemetry.request(0, 1, EffectKind::Svf, true);
        let mut clock = TraceClock::default();
        clock.begin_block(&telemetry, 48000.0);
        let frame = frame();
        clock.publish(&telemetry, 0, &frame);
        clock.record(&telemetry, 0, &frame);
        assert_eq!(telemetry.frame(token), Some(frame));
        assert_eq!(
            telemetry.point(0, token),
            Some(TracePoint {
                phase: 0.25,
                value: 0.75
            })
        );
        telemetry.release(token);
        let next = telemetry.request(0, 1, EffectKind::Svf, true);
        assert_ne!(next, token);
        assert!(telemetry.frame(next).is_none());
        assert!(telemetry.point(0, next).is_none());
        telemetry.release(token);
        assert_eq!(telemetry.request.load(Ordering::Acquire), next);
        telemetry.invalidate();
        let replaced = telemetry.request(0, 1, EffectKind::Svf, true);
        assert_ne!(replaced, next);
        assert!(telemetry.frame(replaced).is_none());
        assert!(telemetry.point(0, replaced).is_none());
    }

    #[test]
    fn wave_readings_round_trip_and_mapping_changes_reject_old_frames() {
        let telemetry = ModulationTelemetry::default();
        let token = telemetry.request(0, 3, EffectKind::Svf, true);
        let mut clock = TraceClock::default();
        clock.begin_block(&telemetry, 48000.0);
        let mut config = TrackModulation::default();
        config.targets = [None; 3];
        for envelope in 0..4 {
            for channel in [None, Some(0), Some(15)] {
                for slot in [None, Some(0), Some(7)] {
                    let mapping = MidiWaveMapping {
                        envelope,
                        first_note: 120,
                        channel,
                    };
                    config.wave_switch = Some(mapping);
                    let mut value = frame();
                    value.wave = Some((mapping, slot));
                    clock.publish(&telemetry, 0, &value);
                    let actual = telemetry.frame(token).unwrap();
                    assert_eq!(actual, value);
                    assert!(actual.matches(EffectKind::Svf, &config));
                    config.wave_switch = Some(MidiWaveMapping {
                        first_note: 36,
                        ..mapping
                    });
                    assert!(!actual.matches(EffectKind::Svf, &config));
                }
            }
        }
    }

    #[test]
    fn measured_zero_level_is_distinct_from_an_unavailable_detector() {
        let shared = ModulationTelemetry::default();
        let token = shared.request(0, 0, EffectKind::Svf, false);
        let mut clock = TraceClock::default();
        clock.begin_block(&shared, 48000.0);
        let mut value = frame();
        clock.publish(&shared, 0, &value);
        assert_eq!(shared.frame(token).unwrap().detector, None);
        value.detector = Some(DetectorReading {
            level: 0.0,
            settings: AudioTriggerSettings {
                level_db: Some(0.0),
                ..AudioTriggerSettings::default()
            },
        });
        clock.publish(&shared, 0, &value);
        assert_eq!(shared.frame(token).unwrap().detector, value.detector);
    }
    #[test]
    fn overwrite_is_bounded_and_snapshots_reject_incomplete_writes() {
        let telemetry = ModulationTelemetry::default();
        let token = telemetry.request(0, 0, EffectKind::Svf, true);
        for i in 0..TRACE_CAPACITY + 5 {
            telemetry.record(
                token,
                TracePoint {
                    phase: i as f32,
                    value: 0.25,
                },
            );
        }
        assert!(telemetry.point(0, token).is_none());
        assert_eq!(telemetry.head(), TRACE_CAPACITY as u64 + 5);
        assert_eq!(
            telemetry
                .point(TRACE_CAPACITY as u64 + 4, token)
                .unwrap()
                .phase,
            TRACE_CAPACITY as f32 + 4.0
        );
        telemetry.latest.store(token, &frame());
        telemetry.latest.stamp.store(3, Ordering::Release);
        assert!(telemetry.frame(token).is_none());
    }
    #[test]
    fn concurrent_snapshots_never_mix_values_from_different_frames() {
        use std::sync::{Arc, atomic::AtomicBool};
        let shared = Arc::new(ModulationTelemetry::default());
        let token = shared.request(0, 0, EffectKind::Svf, true);
        let done = Arc::new(AtomicBool::new(false));
        let writer = shared.clone();
        let finished = done.clone();
        let thread = std::thread::spawn(move || {
            for i in 0..10000 {
                let value = (i % 1000) as f32 / 1000.0;
                let mut frame = frame();
                frame.outputs.fill(value);
                frame.phases.fill(1.0 - value);
                frame.mix = Some((value, 1));
                frame.detector = Some(DetectorReading {
                    level: value,
                    settings: AudioTriggerSettings {
                        input: if value < 0.5 {
                            DetectorInput::Main
                        } else {
                            DetectorInput::Sidechain
                        },
                        level_db: Some(-60.0 * value),
                        highpass_hz: Some(20.0 + 1000.0 * value),
                        lowpass_hz: None,
                    },
                });
                writer.latest.store(token, &frame);
            }
            finished.store(true, Ordering::Release);
        });
        while !done.load(Ordering::Acquire) {
            if let Some(frame) = shared.frame(token) {
                let value = frame.outputs[0];
                assert!(frame.outputs.iter().all(|v| *v == value));
                assert!(frame.phases.iter().all(|v| *v == 1.0 - value));
                assert_eq!(frame.mix, Some((value, 1)));
                assert_eq!(
                    frame.detector,
                    Some(DetectorReading {
                        level: value,
                        settings: AudioTriggerSettings {
                            input: if value < 0.5 {
                                DetectorInput::Main
                            } else {
                                DetectorInput::Sidechain
                            },
                            level_db: Some(-60.0 * value),
                            highpass_hz: Some(20.0 + 1000.0 * value),
                            lowpass_hz: None
                        }
                    })
                );
            }
        }
        thread.join().unwrap();
    }
    #[test]
    fn fractional_trace_clock_does_not_restart_on_host_blocks() {
        for sr in [44100.0, 48000.0] {
            let telemetry = ModulationTelemetry::default();
            telemetry.request(0, 0, EffectKind::Svf, true);
            let mut a = TraceClock::default();
            let mut b = TraceClock::default();
            let mut ca = 0;
            let mut cb = 0;
            a.begin_block(&telemetry, sr);
            for i in 0..sr as usize {
                if i % 17 == 0 {
                    b.begin_block(&telemetry, sr);
                }
                ca += usize::from(a.tick());
                cb += usize::from(b.tick());
            }
            assert_eq!(ca, cb);
            assert!((999..=1001).contains(&ca), "{sr}: {ca}");
        }
    }
}
