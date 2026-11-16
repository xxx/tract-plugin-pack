//! Peak-preserving, requested audio measurements for the MSEG backdrop.

use crate::effects::EffectKind;
use crate::modulation::{TrackModulation, phase_next, phase_step};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use tiny_skia_widgets::mseg::{AudioBin, AudioRange};
use tiny_skia_widgets::{HoldMode, MsegData, PlayMode};

pub const WAVEFORM_BINS: usize = 512;
pub const WAVEFORM_CAPACITY: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveformPoint {
    pub bin: usize,
    pub input: f32,
    pub output: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveformBin {
    pub bin: usize,
    pub audio: AudioBin,
}

#[derive(Default)]
struct Cell {
    stamp: AtomicU64,
    token: AtomicU64,
    bin: AtomicU32,
    ranges: [AtomicU64; 4],
}

pub struct WaveformTelemetry {
    request: AtomicU64,
    epoch: AtomicU32,
    generation: AtomicU64,
    head: AtomicU64,
    cells: [Cell; WAVEFORM_CAPACITY],
}

impl Default for WaveformTelemetry {
    fn default() -> Self {
        Self {
            request: AtomicU64::new(0),
            epoch: AtomicU32::new(0),
            generation: AtomicU64::new(0),
            head: AtomicU64::new(0),
            cells: std::array::from_fn(|_| Cell::default()),
        }
    }
}

impl WaveformTelemetry {
    pub fn request(&self, row: usize, slot: usize, kind: EffectKind) -> u64 {
        self.request_source(row, slot, kind, false)
    }

    pub fn request_sidechain(&self, row: usize, slot: usize, kind: EffectKind) -> u64 {
        self.request_source(row, slot, kind, true)
    }

    fn request_source(&self, row: usize, slot: usize, kind: EffectKind, sidechain: bool) -> u64 {
        if row >= 16 || slot >= 4 {
            return 0;
        }
        let key = 1
            | ((slot as u64) << 1)
            | ((row as u64) << 3)
            | ((kind as u64) << 7)
            | ((sidechain as u64) << 23);
        let current = self.request.load(Ordering::Acquire);
        if current as u32 == key as u32 {
            return current;
        }
        let epoch = self.epoch.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let token = ((epoch as u64) << 32) | key;
        self.request.store(token, Ordering::Release);
        token
    }

    pub fn release(&self, token: u64) {
        let _ = self
            .request
            .compare_exchange(token, 0, Ordering::AcqRel, Ordering::Relaxed);
    }

    pub fn invalidate(&self) {
        self.request.store(0, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn head(&self) -> u64 {
        self.head.load(Ordering::Acquire)
    }

    pub fn point(&self, index: u64, token: u64) -> Option<WaveformPoint> {
        self.stereo_point(index, token).map(|point| WaveformPoint {
            bin: point.bin,
            input: point.audio.input_peak(),
            output: point.audio.output_peak(),
        })
    }

    pub fn stereo_point(&self, index: u64, token: u64) -> Option<WaveformBin> {
        let cell = &self.cells[index as usize % WAVEFORM_CAPACITY];
        let stamp = cell.stamp.load(Ordering::Acquire);
        if token == 0 || stamp != index.wrapping_add(1) {
            return None;
        }
        let actual = cell.token.load(Ordering::Acquire);
        let bin = cell.bin.load(Ordering::Acquire) as usize;
        let ranges: [AudioRange; 4] = std::array::from_fn(|i| {
            let bits = cell.ranges[i].load(Ordering::Acquire);
            AudioRange {
                min: f32::from_bits(bits as u32),
                max: f32::from_bits((bits >> 32) as u32),
            }
        });
        if cell.stamp.load(Ordering::Acquire) != stamp || actual != token || bin >= WAVEFORM_BINS {
            return None;
        }
        Some(WaveformBin {
            bin,
            audio: AudioBin {
                input: [ranges[0], ranges[1]],
                output: [ranges[2], ranges[3]],
            },
        })
    }

    fn record_stereo(&self, token: u64, point: WaveformBin) {
        let index = self.head.load(Ordering::Relaxed);
        let cell = &self.cells[index as usize % WAVEFORM_CAPACITY];
        cell.stamp.store(0, Ordering::Release);
        cell.token.store(token, Ordering::Release);
        cell.bin.store(point.bin as u32, Ordering::Release);
        for (dest, range) in cell
            .ranges
            .iter()
            .zip(point.audio.input.iter().chain(&point.audio.output))
        {
            dest.store(
                range.min.to_bits() as u64 | ((range.max.to_bits() as u64) << 32),
                Ordering::Release,
            );
        }
        cell.stamp.store(index.wrapping_add(1), Ordering::Release);
        self.head.store(index.wrapping_add(1), Ordering::Release);
    }

    #[cfg(test)]
    fn record(&self, token: u64, point: WaveformPoint) {
        let mut audio = AudioBin::default();
        audio.include([point.input; 2], [point.output; 2]);
        self.record_stereo(
            token,
            WaveformBin {
                bin: point.bin,
                audio,
            },
        );
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WaveformRequest {
    pub token: u64,
    pub row: usize,
    pub slot: usize,
    pub sidechain: bool,
    kind: u32,
}

impl WaveformRequest {
    pub fn matches(self, row: usize, kind: EffectKind) -> bool {
        self.token != 0 && self.row == row && self.kind == kind as u32
    }
}

pub struct WaveformTiming<'a> {
    pub config: &'a TrackModulation,
    pub kind: EffectKind,
    pub phase: f64,
    pub trigger_epoch: u64,
    pub bpm: f64,
    pub sample_rate: f64,
}

#[derive(Default)]
pub struct WaveformClock {
    request: WaveformRequest,
    doc: MsegData,
    phase: f64,
    step: f64,
    trigger_epoch: u64,
    follow_audio_phase: bool,
    host_position: bool,
    active: bool,
    pending: Option<WaveformBin>,
}

impl WaveformClock {
    pub fn request(&self) -> WaveformRequest {
        self.request
    }

    pub fn begin_block(&mut self, telemetry: &WaveformTelemetry) {
        let token = telemetry.request.load(Ordering::Acquire);
        if token != self.request.token {
            self.pending = None;
        }
        self.request = WaveformRequest {
            token,
            row: ((token >> 3) & 15) as usize,
            slot: ((token >> 1) & 3) as usize,
            sidechain: token & (1 << 23) != 0,
            kind: ((token >> 7) & 0xffff) as u32,
        };
        self.active = false;
    }

    pub fn prepare_segment(
        &mut self,
        telemetry: &WaveformTelemetry,
        row: usize,
        timing: WaveformTiming<'_>,
    ) {
        let doc = &timing.config.msegs[self.request.slot];
        self.prepare_segment_with_curve(telemetry, row, timing, doc);
    }

    pub fn update_curve(&mut self, doc: &MsegData) {
        self.doc = *doc;
    }

    pub fn pause(&mut self) {
        self.active = false;
        self.pending = None;
    }
    pub fn sync_position(&mut self, telemetry: &WaveformTelemetry, epoch: u64, phase: f64) {
        if self.active && epoch != self.trigger_epoch {
            self.finish_block(telemetry);
            self.pending = None;
            self.trigger_epoch = epoch;
            self.phase = phase;
        }
    }

    pub fn prepare_segment_with_curve(
        &mut self,
        telemetry: &WaveformTelemetry,
        row: usize,
        timing: WaveformTiming<'_>,
        doc: &MsegData,
    ) {
        self.active = self.request.matches(row, timing.kind)
            && timing.sample_rate.is_finite()
            && timing.sample_rate > 0.0;
        if !self.active {
            return;
        }
        let host_position = timing.config.trigger == crate::modulation::TriggerSource::DawPosition;
        if timing.trigger_epoch != self.trigger_epoch
            || timing.phase < self.phase - 1e-6
            || host_position != self.host_position
        {
            self.finish_block(telemetry);
            self.pending = None;
        }
        self.trigger_epoch = timing.trigger_epoch;
        self.host_position = host_position;
        self.doc = *doc;
        self.step = phase_step(&self.doc, timing.bpm, timing.sample_rate);
        self.phase = timing.phase;
        self.follow_audio_phase = host_position
            || timing.config.continuous_mask(timing.kind) & (1 << self.request.slot) != 0;
        self.active &= self.step > 0.0;
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn record_sample(
        &mut self,
        telemetry: &WaveformTelemetry,
        input: [f32; 2],
        output: [f32; 2],
        audio_phase: f64,
    ) {
        self.record_sample_with_sidechain(telemetry, input, output, [0.0; 2], audio_phase);
    }

    pub fn record_sample_with_sidechain(
        &mut self,
        telemetry: &WaveformTelemetry,
        input: [f32; 2],
        output: [f32; 2],
        sidechain: [f32; 2],
        audio_phase: f64,
    ) {
        if !self.active {
            return;
        }
        let projected = phase_next(&self.doc, self.phase, self.step);
        let next = if self.follow_audio_phase {
            audio_phase
        } else {
            projected
        };
        if !next.is_finite() {
            return;
        }
        let loops = matches!(self.doc.play_mode, PlayMode::Cyclic)
            || matches!(self.doc.hold, HoldMode::Loop { .. });
        let end = match self.doc.hold {
            HoldMode::Loop { end, .. } => self.doc.nodes[end].time as f64,
            HoldMode::Sustain(i) if self.doc.play_mode == PlayMode::OneShot => {
                self.doc.nodes[i].time as f64
            }
            _ => 1.0,
        };
        if !loops && self.phase >= end && next >= end {
            return;
        }
        // DAW capture epochs report actual traversals, including equal wrapped phases.
        let looped =
            next < self.phase || (!self.host_position && loops && self.phase + self.step >= end);
        if looped {
            self.finish_block(telemetry);
            self.pending = None;
        }
        self.phase = next;
        let bin = ((next.clamp(0.0, 1.0) * WAVEFORM_BINS as f64) as usize).min(WAVEFORM_BINS - 1);
        if self.pending.is_some_and(|point| point.bin != bin) {
            self.finish_block(telemetry);
            self.pending = None;
        }
        let point = self.pending.get_or_insert(WaveformBin {
            bin,
            audio: AudioBin::default(),
        });
        point.audio.include(
            if self.request.sidechain {
                sidechain
            } else {
                input
            },
            output,
        );
    }

    pub fn finish_block(&self, telemetry: &WaveformTelemetry) {
        if let Some(point) = self.pending {
            telemetry.record_stereo(self.request.token, point);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn midi_geometry_uses_effective_hold_free_timing_without_restarting_capture() {
        let telemetry = WaveformTelemetry::default();
        telemetry.request(0, 0, EffectKind::Svf);
        let mut config = TrackModulation::default();
        config.msegs[0] = MsegData {
            sync_mode: tiny_skia_widgets::SyncMode::Time,
            time_seconds: 0.01,
            play_mode: PlayMode::Cyclic,
            ..MsegData::default()
        };
        config.msegs[0].insert_node(0.25, 1.0).unwrap();
        config.msegs[0].hold = HoldMode::Loop { start: 0, end: 1 };
        let mut effective = config.msegs[0];
        effective.hold = HoldMode::None;
        let mut clock = WaveformClock::default();
        clock.begin_block(&telemetry);
        clock.prepare_segment_with_curve(
            &telemetry,
            0,
            WaveformTiming {
                config: &config,
                kind: EffectKind::Svf,
                phase: 0.0,
                trigger_epoch: 7,
                bpm: 120.0,
                sample_rate: 1000.0,
            },
            &effective,
        );
        for _ in 0..5 {
            clock.record_sample(&telemetry, [0.1; 2], [0.1; 2], 0.0);
        }
        assert!((clock.phase - 0.5).abs() < 1e-6);
        let mut selected = MsegData {
            sync_mode: tiny_skia_widgets::SyncMode::Time,
            time_seconds: 0.01,
            play_mode: PlayMode::Cyclic,
            ..MsegData::default()
        };
        selected.nodes[0].value = 1.0;
        clock.update_curve(&selected);
        assert_eq!(clock.trigger_epoch, 7);
        assert!((clock.phase - 0.5).abs() < 1e-6);
        for _ in 0..4 {
            clock.record_sample(&telemetry, [0.1; 2], [0.1; 2], 0.0);
        }
        assert!((clock.phase - 0.9).abs() < 1e-6);
        assert_eq!(clock.trigger_epoch, 7);
    }
    use tiny_skia_widgets::SyncMode;

    #[test]
    fn sidechain_capture_uses_raw_auxiliary_samples_and_rejects_previous_source_tokens() {
        let telemetry = WaveformTelemetry::default();
        let main = telemetry.request(0, 0, EffectKind::Svf);
        let config = config();
        let mut clock = WaveformClock::default();
        prepare(&mut clock, &telemetry, &config, 0.0, 0);
        clock.record_sample_with_sidechain(&telemetry, [0.9; 2], [0.3; 2], [-0.7, 0.4], 0.0);
        clock.finish_block(&telemetry);
        let old_index = telemetry.head() - 1;
        assert_eq!(telemetry.point(old_index, main).unwrap().input, 0.9);
        let sidechain = telemetry.request_sidechain(0, 0, EffectKind::Svf);
        assert_ne!(sidechain, main);
        assert!(telemetry.stereo_point(old_index, sidechain).is_none());
        telemetry.release(main);
        assert_eq!(
            telemetry.request_sidechain(0, 0, EffectKind::Svf),
            sidechain
        );
        prepare(&mut clock, &telemetry, &config, 0.0, 0);
        assert!(clock.request().sidechain);
        clock.record_sample_with_sidechain(&telemetry, [0.9; 2], [0.3; 2], [-0.7, 0.4], 0.0);
        clock.finish_block(&telemetry);
        let measured = telemetry
            .stereo_point(telemetry.head() - 1, sidechain)
            .unwrap();
        assert_eq!(
            measured.audio.input[0],
            AudioRange {
                min: -0.7,
                max: -0.7
            }
        );
        assert_eq!(measured.audio.input[1], AudioRange { min: 0.4, max: 0.4 });
        let next_main = telemetry.request(0, 0, EffectKind::Svf);
        assert_ne!(next_main, sidechain);
        assert!(telemetry.point(telemetry.head() - 1, next_main).is_none());
    }

    fn config() -> TrackModulation {
        let mut config = TrackModulation::default();
        config.msegs[0] = MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Time,
            time_seconds: 1.0,
            ..MsegData::default()
        };
        config
    }

    fn prepare(
        clock: &mut WaveformClock,
        telemetry: &WaveformTelemetry,
        config: &TrackModulation,
        phase: f32,
        epoch: u64,
    ) {
        clock.begin_block(telemetry);
        clock.prepare_segment(
            telemetry,
            0,
            WaveformTiming {
                config,
                kind: EffectKind::Svf,
                phase: phase as f64,
                trigger_epoch: epoch,
                bpm: 120.0,
                sample_rate: 48000.0,
            },
        );
    }

    #[test]
    fn single_sample_transients_and_partial_bins_survive_block_boundaries() {
        let telemetry = WaveformTelemetry::default();
        let token = telemetry.request(0, 0, EffectKind::Svf);
        let config = config();
        let mut clock = WaveformClock::default();
        prepare(&mut clock, &telemetry, &config, 0.0, 0);
        for i in 0..10 {
            clock.record_sample(
                &telemetry,
                [if i == 3 { -0.9 } else { 0.1 }, 0.2],
                [0.4, 0.3],
                0.0,
            );
        }
        clock.finish_block(&telemetry);
        let first = telemetry.point(telemetry.head() - 1, token).unwrap();
        assert_eq!(
            first,
            WaveformPoint {
                bin: 0,
                input: 0.9,
                output: 0.4
            }
        );
        prepare(&mut clock, &telemetry, &config, 10.0 / 48000.0, 0);
        for _ in 0..10 {
            clock.record_sample(&telemetry, [0.1; 2], [0.8; 2], 0.0);
        }
        clock.finish_block(&telemetry);
        assert_eq!(
            telemetry.point(telemetry.head() - 1, token),
            Some(WaveformPoint {
                bin: 0,
                input: 0.9,
                output: 0.8
            })
        );
        prepare(&mut clock, &telemetry, &config, 0.0, 1);
        clock.record_sample(&telemetry, [0.1; 2], [0.2; 2], 0.0);
        clock.finish_block(&telemetry);
        assert_eq!(
            telemetry.point(telemetry.head() - 1, token),
            Some(WaveformPoint {
                bin: 0,
                input: 0.1,
                output: 0.2
            })
        );
    }

    #[test]
    fn selected_off_slot_projects_samples_even_when_another_slot_is_smoothed() {
        let telemetry = WaveformTelemetry::default();
        let token = telemetry.request(0, 0, EffectKind::Svf);
        let mut config = config();
        config.smooth_ms[3] = 20.0;
        let mut clock = WaveformClock::default();
        prepare(&mut clock, &telemetry, &config, 0.0, 0);
        for _ in 0..480 {
            clock.record_sample(&telemetry, [0.5; 2], [0.25; 2], 0.9);
        }
        clock.finish_block(&telemetry);
        let points: Vec<_> = (0..telemetry.head())
            .filter_map(|index| telemetry.point(index, token))
            .collect();
        assert!(points.len() >= 5);
        assert!(points.iter().all(|point| point.bin <= 5));
        config.smooth_ms[0] = 20.0;
        prepare(&mut clock, &telemetry, &config, 0.0, 1);
        clock.record_sample(&telemetry, [0.5; 2], [0.25; 2], 0.5);
        clock.finish_block(&telemetry);
        assert_eq!(
            telemetry.point(telemetry.head() - 1, token).unwrap().bin,
            256
        );
    }

    #[test]
    fn epochs_overruns_and_invalidation_reject_stale_measurements() {
        let telemetry = WaveformTelemetry::default();
        let token = telemetry.request(0, 0, EffectKind::Svf);
        let point = WaveformPoint {
            bin: 3,
            input: 0.5,
            output: 0.25,
        };
        for _ in 0..WAVEFORM_CAPACITY + 5 {
            telemetry.record(token, point);
        }
        assert!(telemetry.point(0, token).is_none());
        assert_eq!(telemetry.point(telemetry.head() - 1, token), Some(point));
        let generation = telemetry.generation();
        telemetry.release(token);
        telemetry.invalidate();
        assert_ne!(telemetry.generation(), generation);
        let next = telemetry.request(0, 0, EffectKind::Svf);
        assert_ne!(next, token);
        assert!(telemetry.point(telemetry.head() - 1, next).is_none());
        telemetry.release(token);
        assert_eq!(telemetry.request(0, 0, EffectKind::Svf), next);
    }

    #[test]
    fn signed_ranges_preserve_each_channel_across_partial_bins_and_reset_on_retrigger() {
        let telemetry = WaveformTelemetry::default();
        let token = telemetry.request(0, 0, EffectKind::Svf);
        let config = config();
        let mut clock = WaveformClock::default();
        prepare(&mut clock, &telemetry, &config, 0.0, 0);
        clock.record_sample(&telemetry, [0.8, -0.6], [0.4, -0.2], 0.0);
        clock.record_sample(&telemetry, [0.3, -0.9], [0.1, -0.5], 0.0);
        clock.finish_block(&telemetry);
        let first = telemetry.stereo_point(telemetry.head() - 1, token).unwrap();
        assert_eq!(
            first.audio,
            AudioBin {
                input: [
                    AudioRange { min: 0.3, max: 0.8 },
                    AudioRange {
                        min: -0.9,
                        max: -0.6
                    }
                ],
                output: [
                    AudioRange { min: 0.1, max: 0.4 },
                    AudioRange {
                        min: -0.5,
                        max: -0.2
                    }
                ]
            }
        );
        prepare(&mut clock, &telemetry, &config, 2.0 / 48000.0, 0);
        clock.record_sample(&telemetry, [f32::NAN, -0.4], [f32::INFINITY, -0.7], 0.0);
        clock.finish_block(&telemetry);
        let partial = telemetry.stereo_point(telemetry.head() - 1, token).unwrap();
        assert_eq!(partial.audio.input[0], first.audio.input[0]);
        assert_eq!(
            partial.audio.input[1],
            AudioRange {
                min: -0.9,
                max: -0.4
            }
        );
        assert_eq!(
            partial.audio.output[1],
            AudioRange {
                min: -0.7,
                max: -0.2
            }
        );
        assert_eq!(
            telemetry.point(telemetry.head() - 1, token),
            Some(WaveformPoint {
                bin: 0,
                input: 0.9,
                output: 0.7
            })
        );
        prepare(&mut clock, &telemetry, &config, 0.0, 1);
        clock.record_sample(
            &telemetry,
            [f32::NAN, -0.05],
            [0.02, f32::NEG_INFINITY],
            0.0,
        );
        clock.finish_block(&telemetry);
        let fresh = telemetry.stereo_point(telemetry.head() - 1, token).unwrap();
        assert!(fresh.audio.input[0].is_empty());
        assert_eq!(
            fresh.audio.input[1],
            AudioRange {
                min: -0.05,
                max: -0.05
            }
        );
        assert_eq!(
            fresh.audio.output[0],
            AudioRange {
                min: 0.02,
                max: 0.02
            }
        );
        assert!(fresh.audio.output[1].is_empty());
    }

    #[test]
    fn concurrent_stereo_snapshots_keep_all_four_ranges_and_bin_coherent() {
        use std::sync::{Arc, atomic::AtomicBool};
        fn point(value: f32) -> WaveformBin {
            WaveformBin {
                bin: ((value * 1000.0).round() as usize) % WAVEFORM_BINS,
                audio: AudioBin {
                    input: [
                        AudioRange {
                            min: value,
                            max: value + 0.1,
                        },
                        AudioRange {
                            min: -value - 0.2,
                            max: -value - 0.1,
                        },
                    ],
                    output: [
                        AudioRange {
                            min: value * 0.5,
                            max: value * 0.5 + 0.1,
                        },
                        AudioRange {
                            min: -value * 0.7 - 0.2,
                            max: -value * 0.7,
                        },
                    ],
                },
            }
        }
        let telemetry = Arc::new(WaveformTelemetry::default());
        let token = telemetry.request(0, 0, EffectKind::Svf);
        let done = Arc::new(AtomicBool::new(false));
        let writer = telemetry.clone();
        let finished = done.clone();
        let thread = std::thread::spawn(move || {
            for i in 0..10000 {
                writer.record_stereo(token, point((i % 1000) as f32 / 1000.0));
            }
            finished.store(true, Ordering::Release);
        });
        while !done.load(Ordering::Acquire) {
            let head = telemetry.head();
            if head > 0
                && let Some(actual) = telemetry.stereo_point(head - 1, token)
            {
                assert_eq!(actual, point(actual.audio.input[0].min));
            }
        }
        thread.join().unwrap();
    }

    #[test]
    fn display_cursor_distributes_transients_across_legacy_host_segments() {
        for parts in [&[1][..], &[17, 64, 511, 1024][..]] {
            let telemetry = WaveformTelemetry::default();
            let token = telemetry.request(0, 0, EffectKind::Svf);
            let config = config();
            let mut clock = WaveformClock::default();
            let mut bins = [0.0_f32; WAVEFORM_BINS];
            let (mut phase, mut offset, mut part, mut cursor) = (0.0, 0, 0, 0);
            let impulses = [(43, 0.9), (1909, 0.6), (17113, 0.3)];
            while offset < 24000 {
                let len = parts[part % parts.len()].min(24000 - offset);
                prepare(&mut clock, &telemetry, &config, phase, 0);
                let endpoint = tiny_skia_widgets::advance(
                    &config.msegs[0],
                    phase,
                    len as f32 / 48000.0,
                    false,
                )
                .0;
                for i in offset..offset + len {
                    let peak = impulses
                        .iter()
                        .find(|(sample, _)| *sample == i)
                        .map_or(0.0, |(_, peak)| *peak);
                    clock.record_sample(&telemetry, [peak; 2], [peak * 0.5; 2], endpoint as f64);
                }
                clock.finish_block(&telemetry);
                for index in cursor..telemetry.head() {
                    if let Some(point) = telemetry.point(index, token) {
                        bins[point.bin] = point.input;
                    }
                }
                cursor = telemetry.head();
                phase = endpoint;
                offset += len;
                part += 1;
            }
            for (sample, peak) in impulses {
                let actual = bins.iter().position(|value| *value == peak).unwrap();
                let expected = ((sample + 1) as f64 / 48000.0 * WAVEFORM_BINS as f64) as usize;
                assert!(
                    actual.abs_diff(expected) <= 1,
                    "sample {sample}: bin {actual}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn sustain_stops_capture_and_tight_loops_replace_peaks_within_the_same_bin() {
        let telemetry = WaveformTelemetry::default();
        let token = telemetry.request(0, 0, EffectKind::Svf);
        let mut config = config();
        config.msegs[0].insert_node(0.25, 0.5).unwrap();
        config.msegs[0].play_mode = PlayMode::OneShot;
        config.msegs[0].hold = HoldMode::Sustain(1);
        let mut clock = WaveformClock::default();
        prepare(&mut clock, &telemetry, &config, 0.25, 0);
        for _ in 0..100 {
            clock.record_sample(&telemetry, [1.0; 2], [1.0; 2], 1.0);
        }
        clock.finish_block(&telemetry);
        assert_eq!(telemetry.head(), 0);
        config.msegs[0].insert_node(0.2501, 0.5).unwrap();
        config.msegs[0].play_mode = PlayMode::Cyclic;
        config.msegs[0].hold = HoldMode::Loop { start: 1, end: 2 };
        prepare(&mut clock, &telemetry, &config, 0.25, 1);
        clock.record_sample(&telemetry, [1.0; 2], [0.5; 2], 0.0);
        for _ in 0..10 {
            clock.record_sample(&telemetry, [0.1; 2], [0.2; 2], 0.0);
        }
        clock.finish_block(&telemetry);
        assert_eq!(
            telemetry.point(telemetry.head() - 1, token),
            Some(WaveformPoint {
                bin: 128,
                input: 0.1,
                output: 0.2
            })
        );
    }

    #[test]
    fn concurrent_waveform_reads_never_mix_bin_and_peak_payloads() {
        use std::sync::{Arc, atomic::AtomicBool};
        let telemetry = Arc::new(WaveformTelemetry::default());
        let token = telemetry.request(0, 0, EffectKind::Svf);
        let done = Arc::new(AtomicBool::new(false));
        let writer = telemetry.clone();
        let finished = done.clone();
        let thread = std::thread::spawn(move || {
            for i in 0..10000 {
                let value = (i % 1000) as f32 / 1000.0;
                writer.record(
                    token,
                    WaveformPoint {
                        bin: i % 1000 % WAVEFORM_BINS,
                        input: value,
                        output: 1.0 - value,
                    },
                );
            }
            finished.store(true, Ordering::Release);
        });
        while !done.load(Ordering::Acquire) {
            let head = telemetry.head();
            if head > 0
                && let Some(point) = telemetry.point(head - 1, token)
            {
                assert_eq!(point.output, 1.0 - point.input);
                assert_eq!(
                    point.bin,
                    ((point.input * 1000.0).round() as usize) % WAVEFORM_BINS
                );
            }
        }
        thread.join().unwrap();
    }

    #[test]
    fn long_smoothed_cycles_keep_late_peaks_until_the_actual_f64_wrap_or_hold() {
        for play_mode in [PlayMode::Cyclic, PlayMode::OneShot] {
            let telemetry = WaveformTelemetry::default();
            let token = telemetry.request(0, 0, EffectKind::Svf);
            let mut config = config();
            config.smooth_ms[0] = 20.0;
            let doc = &mut config.msegs[0];
            doc.sync_mode = tiny_skia_widgets::SyncMode::Beat;
            doc.beats = 256.0;
            doc.play_mode = play_mode;
            let mut phase = f32::from_bits(1.0_f32.to_bits() - 1) as f64;
            let mut clock = WaveformClock::default();
            clock.begin_block(&telemetry);
            clock.prepare_segment(
                &telemetry,
                0,
                WaveformTiming {
                    config: &config,
                    kind: EffectKind::Svf,
                    phase,
                    trigger_epoch: 0,
                    bpm: 30.0,
                    sample_rate: 192000.0,
                },
            );
            let step = phase_step(&config.msegs[0], 30.0, 192000.0);
            for i in 0..8 {
                phase = phase_next(&config.msegs[0], phase, step);
                let peak = if (play_mode == PlayMode::Cyclic && i == 0)
                    || (play_mode == PlayMode::OneShot && i == 3)
                {
                    0.9
                } else {
                    0.1
                };
                clock.record_sample(&telemetry, [peak; 2], [peak; 2], phase);
            }
            clock.finish_block(&telemetry);
            let last_end = (0..telemetry.head())
                .filter_map(|index| telemetry.point(index, token))
                .filter(|point| point.bin == 511)
                .last()
                .unwrap();
            assert_eq!(last_end.input, 0.9, "{play_mode:?}");
        }
    }

    #[test]
    fn daw_one_shot_intros_preserve_same_bin_peaks_until_actual_looping() {
        for start_phase in [0.0, 0.125, 0.25, 0.5] {
            for split in [false, true] {
                let telemetry = WaveformTelemetry::default();
                let token = telemetry.request(0, 0, EffectKind::Svf);
                let mut config = config();
                config.trigger = crate::modulation::TriggerSource::DawPosition;
                let doc = &mut config.msegs[0];
                doc.play_mode = PlayMode::OneShot;
                doc.time_seconds = 0.017066667;
                doc.insert_node(0.5, 0.5).unwrap();
                doc.insert_node(0.50097656, 0.5).unwrap();
                doc.hold = HoldMode::Loop { start: 1, end: 2 };
                let step = phase_step(doc, 120.0, 48000.0);
                let next = phase_next(doc, start_phase, step);
                assert_eq!(
                    (start_phase * WAVEFORM_BINS as f64) as usize,
                    (next * WAVEFORM_BINS as f64) as usize
                );
                let mut clock = WaveformClock::default();
                prepare(&mut clock, &telemetry, &config, start_phase as f32, 0);
                clock.record_sample(&telemetry, [1.0; 2], [1.0; 2], start_phase);
                if split {
                    clock.finish_block(&telemetry);
                    clock.begin_block(&telemetry);
                    clock.prepare_segment(
                        &telemetry,
                        0,
                        WaveformTiming {
                            config: &config,
                            kind: EffectKind::Svf,
                            phase: next,
                            trigger_epoch: 0,
                            bpm: 120.0,
                            sample_rate: 48000.0,
                        },
                    );
                }
                if start_phase >= 0.5 {
                    clock.sync_position(&telemetry, 1, next);
                }
                clock.record_sample(&telemetry, [0.1; 2], [0.1; 2], next);
                clock.finish_block(&telemetry);
                let point = telemetry.point(telemetry.head() - 1, token).unwrap();
                let expected = if start_phase < 0.5 { 1.0 } else { 0.1 };
                assert_eq!(point.input, expected, "start {start_phase}, split {split}");
                assert_eq!(point.output, expected);
            }
        }
    }

    #[test]
    fn daw_one_shot_intro_peaks_survive_a_tempo_change_before_the_loop_return() {
        let telemetry = WaveformTelemetry::default();
        let token = telemetry.request(0, 0, EffectKind::Svf);
        let mut config = config();
        config.trigger = crate::modulation::TriggerSource::DawPosition;
        let doc = &mut config.msegs[0];
        doc.play_mode = PlayMode::OneShot;
        doc.sync_mode = SyncMode::Beat;
        doc.beats = 4.0;
        doc.insert_node(0.5, 0.5).unwrap();
        doc.insert_node(0.50097656, 0.5).unwrap();
        doc.hold = HoldMode::Loop { start: 1, end: 2 };
        let mut clock = WaveformClock::default();
        for (phase, bpm, peak) in [(0.5003, 120.0, 1.0), (0.5008, 480.0, 0.1)] {
            clock.begin_block(&telemetry);
            clock.prepare_segment(
                &telemetry,
                0,
                WaveformTiming {
                    config: &config,
                    kind: EffectKind::Svf,
                    phase,
                    trigger_epoch: 0,
                    bpm,
                    sample_rate: 1000.0,
                },
            );
            clock.record_sample(&telemetry, [peak; 2], [peak; 2], phase);
            clock.finish_block(&telemetry);
        }
        assert_eq!(
            telemetry.point(telemetry.head() - 1, token).unwrap().input,
            1.0
        );
        let next = phase_next(&config.msegs[0], 0.5008, clock.step);
        clock.sync_position(&telemetry, 1, next);
        clock.record_sample(&telemetry, [0.1; 2], [0.1; 2], next);
        clock.finish_block(&telemetry);
        assert_eq!(
            telemetry.point(telemetry.head() - 1, token).unwrap().input,
            0.1
        );
    }

    #[test]
    fn steps_longer_than_a_loop_replace_same_bin_peaks_even_when_phase_increases() {
        for smooth in [0.0, 20.0] {
            let telemetry = WaveformTelemetry::default();
            let token = telemetry.request(0, 0, EffectKind::Svf);
            let mut config = config();
            config.smooth_ms[0] = smooth;
            let doc = &mut config.msegs[0];
            doc.insert_node(0.5, 0.5).unwrap();
            doc.insert_node(0.50097656, 0.5).unwrap();
            doc.hold = HoldMode::Loop { start: 1, end: 2 };
            doc.time_seconds = 0.017066667;
            let mut clock = WaveformClock::default();
            prepare(&mut clock, &telemetry, &config, 0.5, 0);
            let step = phase_step(&config.msegs[0], 120.0, 48000.0);
            let mut phase = 0.5;
            for peak in [1.0, 0.1] {
                phase = phase_next(&config.msegs[0], phase, step);
                clock.record_sample(&telemetry, [peak; 2], [peak; 2], phase);
            }
            clock.finish_block(&telemetry);
            assert_eq!(
                telemetry.point(telemetry.head() - 1, token),
                Some(WaveformPoint {
                    bin: 256,
                    input: 0.1,
                    output: 0.1
                })
            );
        }
    }
}
