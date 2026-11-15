//! Peak-preserving, requested audio measurements for the MSEG backdrop.

use crate::effects::EffectKind;
use crate::modulation::{TrackModulation, phase_next, phase_step};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use tiny_skia_widgets::{HoldMode, MsegData, PlayMode};

pub const WAVEFORM_BINS: usize = 512;
pub const WAVEFORM_CAPACITY: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveformPoint {
    pub bin: usize,
    pub input: f32,
    pub output: f32,
}

#[derive(Default)]
struct Cell {
    stamp: AtomicU64,
    token: AtomicU64,
    bin: AtomicU32,
    peaks: AtomicU64,
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
        if row >= 16 || slot >= 4 {
            return 0;
        }
        let key = 1 | ((slot as u64) << 1) | ((row as u64) << 3) | ((kind as u64) << 7);
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
        let cell = &self.cells[index as usize % WAVEFORM_CAPACITY];
        let stamp = cell.stamp.load(Ordering::Acquire);
        if token == 0 || stamp != index.wrapping_add(1) {
            return None;
        }
        let actual = cell.token.load(Ordering::Acquire);
        let bin = cell.bin.load(Ordering::Acquire) as usize;
        let peaks = cell.peaks.load(Ordering::Acquire);
        if cell.stamp.load(Ordering::Acquire) != stamp || actual != token || bin >= WAVEFORM_BINS {
            return None;
        }
        Some(WaveformPoint {
            bin,
            input: f32::from_bits(peaks as u32),
            output: f32::from_bits((peaks >> 32) as u32),
        })
    }

    fn record(&self, token: u64, point: WaveformPoint) {
        let index = self.head.load(Ordering::Relaxed);
        let cell = &self.cells[index as usize % WAVEFORM_CAPACITY];
        cell.stamp.store(0, Ordering::Release);
        cell.token.store(token, Ordering::Release);
        cell.bin.store(point.bin as u32, Ordering::Release);
        cell.peaks.store(
            point.input.to_bits() as u64 | ((point.output.to_bits() as u64) << 32),
            Ordering::Release,
        );
        cell.stamp.store(index.wrapping_add(1), Ordering::Release);
        self.head.store(index.wrapping_add(1), Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WaveformRequest {
    pub token: u64,
    pub row: usize,
    pub slot: usize,
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
    active: bool,
    pending: Option<WaveformPoint>,
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
        self.active = self.request.matches(row, timing.kind)
            && timing.sample_rate.is_finite()
            && timing.sample_rate > 0.0;
        if !self.active {
            return;
        }
        if timing.trigger_epoch != self.trigger_epoch || timing.phase < self.phase - 1e-6 {
            self.finish_block(telemetry);
            self.pending = None;
        }
        self.trigger_epoch = timing.trigger_epoch;
        self.doc = timing.config.msegs[self.request.slot];
        self.step = phase_step(&self.doc, timing.bpm, timing.sample_rate);
        self.phase = timing.phase;
        self.follow_audio_phase =
            timing.config.smoothing_mask(timing.kind) & (1 << self.request.slot) != 0;
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
        if next < self.phase || (loops && self.phase + self.step >= end) {
            self.finish_block(telemetry);
            self.pending = None;
        }
        self.phase = next;
        let bin = ((next.clamp(0.0, 1.0) * WAVEFORM_BINS as f64) as usize).min(WAVEFORM_BINS - 1);
        if self.pending.is_some_and(|point| point.bin != bin) {
            self.finish_block(telemetry);
            self.pending = None;
        }
        let peak = |samples: [f32; 2]| {
            samples
                .iter()
                .filter(|x| x.is_finite())
                .map(|x| x.abs())
                .fold(0.0, f32::max)
        };
        let point = self.pending.get_or_insert(WaveformPoint {
            bin,
            input: 0.0,
            output: 0.0,
        });
        point.input = point.input.max(peak(input));
        point.output = point.output.max(peak(output));
    }

    pub fn finish_block(&self, telemetry: &WaveformTelemetry) {
        if let Some(point) = self.pending {
            telemetry.record(self.request.token, point);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia_widgets::SyncMode;

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
