use super::*;
use crate::modulation::{FollowerConfig, FollowerMode, FollowerReading, FollowerRoute};

#[derive(Default)]
pub(super) struct AtomicFollower {
    active: AtomicBool,
    levels: AtomicU64,
    response: [AtomicU32; 4],
    filters: [AtomicU64; 3],
    input: AtomicU32,
    routes: [AtomicU64; 4],
}
impl AtomicFollower {
    pub(super) fn store(&self, reading: Option<FollowerReading>) {
        self.active.store(reading.is_some(), Ordering::Release);
        let Some(reading) = reading else {
            return;
        };
        self.levels
            .store(pair(reading.level, reading.envelope), Ordering::Release);
        let cfg = reading.config;
        for (dest, value) in
            self.response
                .iter()
                .zip([cfg.attack_ms, cfg.hold_ms, cfg.release_ms, cfg.gain_db])
        {
            dest.store(value.to_bits(), Ordering::Release);
        }
        for (dest, value) in self.filters.iter().zip([
            cfg.detector.level_db,
            cfg.detector.highpass_hz,
            cfg.detector.lowpass_hz,
        ]) {
            dest.store(optional_float(value), Ordering::Release);
        }
        self.input
            .store(cfg.detector.input as u32, Ordering::Release);
        for (dest, route) in self.routes.iter().zip(cfg.routes) {
            let mode = match route.mode {
                FollowerMode::Follow => 0,
                FollowerMode::Add => 1,
                FollowerMode::Multiply => 2,
            };
            dest.store(
                ((route.amount.to_bits() as u64) << 32) | route.enabled as u64 | (mode << 1),
                Ordering::Release,
            );
        }
    }
    pub(super) fn load(&self) -> Option<FollowerReading> {
        if !self.active.load(Ordering::Acquire) {
            return None;
        }
        let (level, envelope) = unpair(self.levels.load(Ordering::Acquire));
        let response: [f32; 4] =
            std::array::from_fn(|i| f32::from_bits(self.response[i].load(Ordering::Acquire)));
        let filter: [Option<f32>; 3] =
            std::array::from_fn(|i| read_optional_float(self.filters[i].load(Ordering::Acquire)));
        let input = match self.input.load(Ordering::Acquire) {
            0 => DetectorInput::Main,
            1 => DetectorInput::Sidechain,
            _ => return None,
        };
        let mut routes = [FollowerRoute::default(); 4];
        for (route, source) in routes.iter_mut().zip(&self.routes) {
            let bits = source.load(Ordering::Acquire);
            let mode = match (bits >> 1) & 3 {
                0 => FollowerMode::Follow,
                1 => FollowerMode::Add,
                2 => FollowerMode::Multiply,
                _ => return None,
            };
            *route = FollowerRoute {
                enabled: bits & 1 != 0,
                mode,
                amount: f32::from_bits((bits >> 32) as u32),
            };
        }
        Some(FollowerReading {
            level,
            envelope,
            config: FollowerConfig {
                attack_ms: response[0],
                hold_ms: response[1],
                release_ms: response[2],
                gain_db: response[3],
                detector: AudioTriggerSettings {
                    input,
                    level_db: filter[0],
                    highpass_hz: filter[1],
                    lowpass_hz: filter[2],
                },
                routes,
            },
        })
    }
}
