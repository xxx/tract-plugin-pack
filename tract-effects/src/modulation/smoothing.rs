use super::*;
use crate::effects::ParamFormat;
use tiny_skia_widgets::HoldMode;

pub const MAX_SMOOTH_MS: f32 = 1000.0;
pub fn normalized_smooth_ms(ms: f32) -> f32 {
    if ms.is_finite() {
        ms.clamp(0.0, MAX_SMOOTH_MS)
    } else {
        0.0
    }
}
pub fn smooth_to_norm(ms: f32) -> f32 {
    normalized_smooth_ms(ms).ln_1p() / MAX_SMOOTH_MS.ln_1p()
}
pub fn smooth_from_norm(norm: f32) -> f32 {
    (norm.clamp(0.0, 1.0) * MAX_SMOOTH_MS.ln_1p())
        .exp_m1()
        .clamp(0.0, MAX_SMOOTH_MS)
}
pub(super) fn all_off(values: &[f32; 4]) -> bool {
    values.iter().all(|v| *v == 0.0)
}

impl TrackModulation {
    pub fn smoothing_supported(&self, slot: usize, kind: EffectKind) -> bool {
        if slot == 0 {
            return true;
        }
        match self.targets.get(slot - 1).copied().flatten() {
            None => slot < 4,
            Some(target) => target
                .spec(kind)
                .is_some_and(|s| !matches!(s.format, ParamFormat::Enum { .. })),
        }
    }
    pub fn smoothing_mask(&self, kind: EffectKind) -> u8 {
        (0..4).fold(0, |mask, slot| {
            mask | if normalized_smooth_ms(self.smooth_ms[slot]) > 0.0
                && self.smoothing_supported(slot, kind)
            {
                1 << slot
            } else {
                0
            }
        })
    }
    pub fn continuous_mask(&self, kind: EffectKind) -> u8 {
        if self.trigger == TriggerSource::DawPosition {
            15
        } else {
            self.smoothing_mask(kind)
        }
    }
    pub fn continuous_parameter(&self, kind: EffectKind, index: usize) -> bool {
        self.winning_slot(ModulationTarget::EffectParameter(index))
            .is_some_and(|slot| self.continuous_mask(kind) & (1 << slot) != 0)
    }
    pub fn winning_slot(&self, target: ModulationTarget) -> Option<usize> {
        self.targets
            .iter()
            .rposition(|t| *t == Some(target))
            .map(|i| i + 1)
    }
    pub fn smoothed_parameter(&self, kind: EffectKind, index: usize) -> bool {
        self.winning_slot(ModulationTarget::EffectParameter(index))
            .is_some_and(|slot| {
                normalized_smooth_ms(self.smooth_ms[slot]) > 0.0
                    && self.smoothing_supported(slot, kind)
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Route {
    target: ModulationTarget,
    kind: EffectKind,
    spec: ParamSpec,
    base: f32,
    depth: f32,
    polarity: Polarity,
}
impl Route {
    fn value(self, source: f32) -> f32 {
        assignable_value(source, self.base, self.depth, self.spec, self.polarity)
    }
    fn norm(self, value: f32) -> f32 {
        value_to_norm(value, self.spec.min, self.spec.max, self.spec.scaling)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
struct Slot {
    active: bool,
    phase: f64,
    step: f64,
    value: f64,
    raw: f32,
    gain: f64,
    ms: f32,
    sample_rate: f64,
    force_raw: bool,
    force_apply: bool,
    route: Option<Route>,
    held: Option<(f32, f32)>,
    position_seen: Option<(PositionKey, u64)>,
    loop_iteration: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PositionKey {
    mode: SyncMode,
    length: f32,
    play: PlayMode,
    hold: HoldMode,
    markers: [f32; 2],
}
impl PositionKey {
    fn new(doc: &MsegData) -> Self {
        let length = match doc.sync_mode {
            SyncMode::Beat => doc.beats,
            SyncMode::Time => doc.time_seconds,
        };
        let hold = match (doc.play_mode, doc.hold) {
            (PlayMode::Cyclic, HoldMode::Sustain(_)) => HoldMode::None,
            _ => doc.hold,
        };
        let markers = match hold {
            HoldMode::None => [0.0; 2],
            HoldMode::Sustain(i) => [doc.nodes[i].time; 2],
            HoldMode::Loop { start, end } => [doc.nodes[start].time, doc.nodes[end].time],
        };
        Self {
            mode: doc.sync_mode,
            length: if length.is_finite() && length > 0.0 {
                length
            } else {
                0.0
            },
            play: doc.play_mode,
            hold,
            markers,
        }
    }
}
impl Slot {
    fn sync_position(&mut self, doc: &MsegData, epoch: u64, iteration: f64) -> bool {
        let key = (PositionKey::new(doc), epoch);
        if self.position_seen == Some(key) {
            return false;
        }
        self.position_seen = Some(key);
        self.loop_iteration = iteration;
        self.force_raw = true;
        self.force_apply = true;
        true
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub(super) struct SmoothRuntime {
    slots: [Slot; 4],
    mask: u8,
    clock: f64,
    period: f64,
    sample_rate: f64,
    pub(super) mix_value: Option<f32>,
}
impl SmoothRuntime {
    pub(super) fn wave_changed(&mut self, slot: usize) -> bool {
        let source = &mut self.slots[slot];
        if !source.active {
            return false;
        }
        source.force_raw = true;
        source.force_apply = true;
        true
    }
    pub(super) fn deactivate(&mut self) {
        self.mask = 0;
        self.mix_value = None;
        self.clock = 0.0;
        for s in &mut self.slots {
            s.active = false;
        }
    }
    pub(super) fn trigger(&mut self) {
        for s in &mut self.slots {
            s.phase = 0.0;
        }
        self.refresh();
    }
    pub(super) fn refresh(&mut self) {
        for s in &mut self.slots {
            s.force_raw = true;
            s.force_apply = true;
        }
    }
}

pub(crate) fn phase_step(data: &MsegData, bpm: f64, sr: f64) -> f64 {
    let length = match data.sync_mode {
        SyncMode::Time => data.time_seconds as f64 * sr,
        SyncMode::Beat if bpm > 0.0 => data.beats as f64 * 60.0 / bpm * sr,
        _ => 0.0,
    };
    if length.is_finite() && length > 0.0 {
        1.0 / length
    } else {
        0.0
    }
}

pub(crate) fn phase_next(data: &MsegData, phase: f64, step: f64) -> f64 {
    let nodes = data.active();
    let next = phase + step;
    let wrap = |lo: f64, hi: f64| lo + (next - lo).rem_euclid((hi - lo).max(1e-9));
    match (data.play_mode, data.hold) {
        (PlayMode::Cyclic, HoldMode::Loop { start, end }) => {
            wrap(nodes[start].time as f64, nodes[end].time as f64)
        }
        (PlayMode::Cyclic, _) => wrap(0.0, 1.0),
        (PlayMode::OneShot, HoldMode::Sustain(i)) => next.min(nodes[i].time as f64),
        (PlayMode::OneShot, HoldMode::Loop { start, end }) if next >= nodes[end].time as f64 => {
            wrap(nodes[start].time as f64, nodes[end].time as f64)
        }
        (PlayMode::OneShot, HoldMode::Loop { .. }) => next,
        _ => next.min(1.0),
    }
}

impl ModulationState {
    pub(super) fn restore_released_targets(
        &mut self,
        config: &TrackModulation,
        track: &TrackEffect,
        effect: &mut impl Effect,
    ) {
        for slot in &self.smooth.slots {
            if !slot.active {
                continue;
            }
            if let Some(Route {
                target: ModulationTarget::EffectParameter(index),
                kind,
                ..
            }) = slot.route
                && kind == track.kind
                && !config.continuous_parameter(kind, index)
                && let Some(base) = track.params.get(index)
            {
                effect.set_param(index, *base);
            }
        }
    }
    pub fn set_telemetry_enabled(&mut self, enabled: bool) {
        self.telemetry_enabled = enabled;
    }
    pub fn reset_smoothing(&mut self) {
        self.smooth = SmoothRuntime::default();
        self.output_valid = [false; 4];
    }
    /// Restart free clocks and refresh DAW playback without moving its phases.
    pub fn restart_clock(&mut self, config: &TrackModulation) {
        self.trigger_epoch = self.trigger_epoch.wrapping_add(1);
        if config.trigger == TriggerSource::DawPosition {
            for epoch in &mut self.capture_epochs {
                *epoch = epoch.wrapping_add(1);
            }
            self.smooth.refresh();
        } else {
            self.phases = [0.0; 4];
            self.smooth.trigger();
        }
        self.hz_phase = 0.0;
        self.transient.reset();
        self.precise.reset();
        self.fired = false;
    }
    pub fn smoothing_active(&self) -> bool {
        self.smooth.mask != 0
    }
    pub fn trigger_epoch(&self) -> u64 {
        self.trigger_epoch
    }
    pub fn precise_phase(&self, slot: usize) -> f64 {
        if self.smooth.slots[slot].active {
            self.smooth.slots[slot].phase
        } else {
            self.phases[slot] as f64
        }
    }
    pub fn holds_smoothed_parameter(&self, kind: EffectKind, index: usize) -> bool {
        self.smooth.slots.iter().any(|slot| {
            slot.active
                && slot.held.is_some()
                && slot.route.is_some_and(|route| {
                    route.kind == kind && route.target == ModulationTarget::EffectParameter(index)
                })
        })
    }
    pub fn output(&self, slot: usize) -> f32 {
        self.outputs[slot]
    }
    pub fn parameter_norms(&self) -> [Option<(f32, u8)>; crate::effects::MAX_EFFECT_PARAMS] {
        self.parameter_norms
    }
    pub fn delivered_mix(&self) -> Option<(f32, u8)> {
        self.delivered_mix
            .filter(|(_, slot)| self.mix_slot == Some(*slot))
    }

    pub(super) fn prepare_smoothing(
        &mut self,
        config: &TrackModulation,
        track: &TrackEffect,
        effect: &mut impl Effect,
        seg_len: usize,
        bpm: f64,
        sr: f64,
    ) {
        let mask = config.continuous_mask(track.kind);
        if self.smooth.mask == 0 || self.smooth.sample_rate != sr {
            self.smooth.clock = 0.0;
        }
        self.smooth.mask = mask;
        self.smooth.sample_rate = sr;
        self.smooth.period = (sr / 1000.0).max(1.0);
        self.smooth.mix_value = None;
        self.parameter_norms.fill(None);
        self.mix_slot = None;
        let mut legacy_mix = None;
        for k in 0..4 {
            let doc = self.automated.curve(self.wave.curve(config, k), k);
            let position = self.position.phase(doc);
            let route = if k > 0 {
                config.targets[k - 1].and_then(|target| {
                    if config.winning_slot(target) != Some(k) {
                        return None;
                    }
                    let spec = match target {
                        ModulationTarget::EffectParameter(i) => effect.parameters().get(i).copied(),
                        ModulationTarget::Mix => target.spec(track.kind),
                    };
                    spec.map(|spec| Route {
                        target,
                        kind: track.kind,
                        spec,
                        base: target.base_value(track),
                        depth: config.depths[k - 1],
                        polarity: doc.polarity,
                    })
                })
            } else {
                None
            };
            let slot = &mut self.smooth.slots[k];
            if self.position.active {
                if slot.sync_position(
                    doc,
                    self.position.epoch(doc.sync_mode),
                    self.position.loop_iteration(doc).unwrap_or(0.0),
                ) {
                    self.capture_epochs[k] = self.capture_epochs[k].wrapping_add(1);
                }
            } else {
                slot.position_seen = None;
                slot.loop_iteration = 0.0;
            }
            if mask & (1 << k) != 0 {
                if !slot.active {
                    slot.phase = position.unwrap_or(self.phases[k] as f64);
                    slot.value = if self.output_valid[k] {
                        self.outputs[k] as f64
                    } else {
                        value_at_phase(doc, slot.phase as f32) as f64
                    };
                    slot.raw = value_at_phase(doc, slot.phase as f32).clamp(0.0, 1.0);
                    slot.force_raw = true;
                    slot.force_apply = true;
                    slot.held = None;
                }
                if let Some(position) = position {
                    slot.phase = position;
                    self.phases[k] = position as f32;
                }
                let ms = if config.smoothing_supported(k, track.kind) {
                    normalized_smooth_ms(config.smooth_ms[k])
                } else {
                    0.0
                };
                if slot.ms != ms || slot.sample_rate != sr {
                    slot.gain = if ms > 0.0 {
                        -(-1.0 / (ms as f64 * 0.001 * sr)).exp_m1()
                    } else {
                        1.0
                    };
                    slot.ms = ms;
                    slot.sample_rate = sr;
                }
                if slot.route != route {
                    let same_destination = slot
                        .route
                        .zip(route)
                        .is_some_and(|(a, b)| a.target == b.target && a.kind == b.kind);
                    if !same_destination {
                        slot.held = None;
                        slot.force_apply = true;
                    }
                }
                slot.route = route;
                slot.active = true;
                slot.step = phase_step(doc, bpm, sr);
                if let Some(route) = route {
                    match route.target {
                        ModulationTarget::EffectParameter(index) => {
                            if let Some((value, _)) = slot.held {
                                let norm = if self.telemetry_enabled {
                                    route.norm(value)
                                } else {
                                    0.0
                                };
                                if let Some(value) = self.parameter_norms.get_mut(index) {
                                    *value = Some((norm, k as u8));
                                }
                            }
                        }
                        ModulationTarget::Mix => {
                            self.mix_slot = Some(k as u8);
                            self.smooth.mix_value = Some(route.value(slot.value as f32) * 0.01);
                        }
                    }
                }
            } else {
                slot.active = false;
                slot.route = None;
                slot.held = None;
                let (phase, _) = advance(
                    doc,
                    self.phases[k],
                    mseg_phase_delta(doc, seg_len, bpm, sr),
                    false,
                );
                self.phases[k] = phase;
                let source = value_at_phase(doc, phase);
                self.outputs[k] = source;
                self.output_valid[k] = true;
                if k == 0 {
                    self.amplitude = source;
                } else if let Some(route) = route {
                    let value = route.value(source);
                    match route.target {
                        ModulationTarget::EffectParameter(index) => {
                            effect.set_param(index, value);
                            if self.telemetry_enabled
                                && let Some(norm) = self.parameter_norms.get_mut(index)
                            {
                                *norm = Some((route.norm(value), k as u8));
                            }
                        }
                        ModulationTarget::Mix => {
                            legacy_mix = Some(value * 0.01);
                            self.mix_slot = Some(k as u8);
                        }
                    }
                }
            }
        }
        if self.smooth.mix_value.is_some() {
            self.mix = None;
        } else if legacy_mix.is_some() || self.mix.is_some() {
            let target = legacy_mix.unwrap_or(track.mix).clamp(0.0, 1.0);
            let current = self.mix.as_ref().map_or(track.mix, |r| r.target);
            self.mix = if legacy_mix.is_none() && current == target {
                None
            } else {
                Some(MixRamp {
                    current,
                    target,
                    step: (target - current) / seg_len as f32,
                    remaining: seg_len,
                    release: legacy_mix.is_none(),
                })
            };
        }
    }

    pub fn process_smoothing_sample(&mut self, config: &TrackModulation, effect: &mut impl Effect) {
        if self.smooth.mask == 0
            || (config.trigger == TriggerSource::DawPosition && !self.position.active)
        {
            return;
        }
        self.position.bump_epochs(self.position.jump_mask());
        let tick = self.smooth.clock < 1.0;
        if tick {
            self.smooth.clock += self.smooth.period;
        }
        self.smooth.clock -= 1.0;
        for k in 0..4 {
            let slot = &mut self.smooth.slots[k];
            if !slot.active {
                continue;
            }
            let doc = self.automated.curve(self.wave.curve(config, k), k);
            if self.position.active
                && slot
                    .position_seen
                    .is_none_or(|(_, epoch)| epoch != self.position.epoch(doc.sync_mode))
                && slot.sync_position(
                    doc,
                    self.position.epoch(doc.sync_mode),
                    self.position.loop_iteration(doc).unwrap_or(0.0),
                )
            {
                self.capture_epochs[k] = self.capture_epochs[k].wrapping_add(1);
            }
            if self.position.active
                && let Some(iteration) = self.position.loop_iteration(doc)
            {
                if slot.loop_iteration != iteration {
                    self.capture_epochs[k] = self.capture_epochs[k].wrapping_add(1);
                }
                slot.loop_iteration = iteration;
            }
            let next = if self.position.active {
                self.position.phase(doc).unwrap_or(slot.phase)
            } else {
                phase_next(doc, slot.phase, slot.step)
            };
            let wrapped = next < slot.phase;
            slot.phase = next;
            self.phases[k] = next as f32;
            if self.position.active || tick || slot.force_raw || wrapped {
                slot.raw = value_at_phase(doc, next as f32).clamp(0.0, 1.0);
                slot.force_raw = false;
            }
            slot.value += slot.gain * (slot.raw as f64 - slot.value);
            let source = slot.value.clamp(0.0, 1.0) as f32;
            self.outputs[k] = source;
            self.output_valid[k] = true;
            if k == 0 {
                self.amplitude = source;
            }
            if let Some(route) = slot.route {
                match route.target {
                    ModulationTarget::Mix => {
                        self.smooth.mix_value = Some(route.value(source) * 0.01)
                    }
                    ModulationTarget::EffectParameter(index) if tick || slot.force_apply => {
                        let value = route.value(source);
                        let norm = if self.telemetry_enabled {
                            route.norm(value)
                        } else {
                            0.0
                        };
                        if slot.force_apply
                            || slot.held.is_none_or(|(previous, _)| previous != value)
                        {
                            effect.set_param(index, value);
                        }
                        slot.held = Some((value, norm));
                        slot.force_apply = false;
                        if let Some(out) = self.parameter_norms.get_mut(index) {
                            *out = Some((norm, k as u8));
                        }
                    }
                    _ => {}
                }
            }
        }
        self.position.advance_sample();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Probe {
        value: f32,
        sets: usize,
    }
    impl Effect for Probe {
        fn process_sample(&mut self, _: f32, _: f32) -> (f32, f32) {
            (self.value, self.value)
        }
        fn set_sample_rate(&mut self, _: f32) {}
        fn reset(&mut self) {}
        fn parameters(&self) -> &'static [ParamSpec] {
            static SPEC: [ParamSpec; 1] = [ParamSpec {
                name: "Value",
                min: 0.0,
                max: 1.0,
                default: 0.0,
                scaling: ParamScaling::Linear,
                format: ParamFormat::Number {
                    decimals: 2,
                    unit: "",
                },
            }];
            &SPEC
        }
        fn set_param(&mut self, index: usize, value: f32) {
            assert_eq!(index, 0);
            self.value = value;
            self.sets += 1;
        }
    }
    fn config() -> (TrackModulation, TrackEffect) {
        let mut config = TrackModulation::default();
        config.targets = [Some(ModulationTarget::EffectParameter(0)), None, None];
        config.depths = [1.0, 0.0, 0.0];
        config.smooth_ms = [0.0, 20.0, 0.0, 0.0];
        config.msegs[1] = MsegData {
            play_mode: PlayMode::Cyclic,
            time_seconds: 0.2,
            snap: false,
            ..MsegData::default()
        };
        let track = TrackEffect {
            kind: EffectKind::Svf,
            params: [0.0; crate::effects::MAX_EFFECT_PARAMS],
            mix: 0.3,
            ..TrackEffect::default()
        };
        (config, track)
    }
    fn render(parts: &[usize], sr: f64) -> (Vec<[f32; 4]>, usize) {
        let (config, track) = config();
        let mut state = ModulationState::default();
        let mut effect = Probe::default();
        let mut out = Vec::new();
        let samples = sr as usize / 2;
        let mut offset = 0;
        let mut part = 0;
        while offset < samples {
            let len = parts[part % parts.len()].min(samples - offset);
            state.advance_segment(&config, &track, &mut effect, len, 120.0, sr);
            for _ in 0..len {
                state.process_smoothing_sample(&config, &mut effect);
                out.push([
                    state.output(1),
                    state.phase(1),
                    effect.value,
                    state.next_mix(track.mix),
                ]);
            }
            offset += len;
            part += 1;
        }
        (out, effect.sets)
    }
    #[test]
    fn smoothing_and_held_targets_are_partition_invariant_on_a_fractional_control_clock() {
        for sr in [44100.0, 48000.0, 96000.0] {
            let (reference, sets) = render(&[1], sr);
            let (actual, other_sets) = render(&[17, 64, 511, 1024], sr);
            assert_eq!(actual, reference);
            assert_eq!(sets, other_sets);
            assert!(sets <= 502, "{sr}: {sets}");
        }
    }
    #[test]
    fn time_constant_retrigger_and_flush_seeding_are_defined() {
        for sr in [44100.0, 48000.0, 96000.0] {
            let (mut config, track) = config();
            for n in &mut config.msegs[1].nodes {
                n.value = 0.0;
            }
            let mut state = ModulationState::default();
            let mut effect = Probe::default();
            state.advance_segment(&config, &track, &mut effect, 1, 120.0, sr);
            state.process_smoothing_sample(&config, &mut effect);
            for n in &mut config.msegs[1].nodes {
                n.value = 1.0;
            }
            state.trigger();
            let len = (sr * 0.020) as usize;
            state.advance_segment(&config, &track, &mut effect, len, 120.0, sr);
            for _ in 0..len {
                state.process_smoothing_sample(&config, &mut effect);
            }
            assert!((state.output(1) - (1.0 - (-1.0_f32).exp())).abs() < 1e-6);
            let value = state.output(1);
            for n in &mut config.msegs[1].nodes {
                n.value = 0.0;
            }
            state.trigger();
            state.advance_segment(&config, &track, &mut effect, 1, 120.0, sr);
            state.process_smoothing_sample(&config, &mut effect);
            assert!(state.output(1) > 0.0 && state.output(1) < value);
            let phase = state.phase(1);
            state.reset_smoothing();
            assert_eq!(state.phase(1), phase);
            state.advance_segment(&config, &track, &mut effect, 1, 120.0, sr);
            state.process_smoothing_sample(&config, &mut effect);
            assert_eq!(state.output(1), 0.0);
        }
    }
    #[test]
    fn last_slot_wins_for_mixed_off_and_smooth_routes_and_releases_to_base() {
        for smooth in [[20.0, 0.0], [0.0, 20.0]] {
            let (mut config, track) = config();
            config.targets[1] = config.targets[0];
            config.depths[1] = 1.0;
            config.smooth_ms[1] = smooth[0];
            config.smooth_ms[2] = smooth[1];
            for n in &mut config.msegs[1].nodes {
                n.value = 0.8;
            }
            for n in &mut config.msegs[2].nodes {
                n.value = 0.2;
            }
            let mut state = ModulationState::default();
            let mut effect = Probe::default();
            for _ in 0..3 {
                state.advance_segment(&config, &track, &mut effect, 64, 120.0, 48000.0);
                for _ in 0..64 {
                    state.process_smoothing_sample(&config, &mut effect);
                    assert_eq!(effect.value, 0.2);
                }
            }
            config.targets = [None; 3];
            if smooth[1] == 0.0 {
                effect.set_param(0, track.params[0]);
            }
            state.advance_segment(&config, &track, &mut effect, 64, 120.0, 48000.0);
            assert_eq!(effect.value, track.params[0]);
        }
    }
    #[test]
    fn old_presets_default_off_and_selector_targets_bypass_smoothing() {
        let (mut config, _) = config();
        config.smooth_ms = [0.0; 4];
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("smooth_ms"));
        assert_eq!(
            serde_json::from_str::<TrackModulation>(&json)
                .unwrap()
                .smooth_ms,
            [0.0; 4]
        );
        let selector = crate::effects::parameter_specs(EffectKind::Svf)
            .iter()
            .position(|s| matches!(s.format, ParamFormat::Enum { .. }))
            .unwrap();
        config.targets[0] = Some(ModulationTarget::EffectParameter(selector));
        config.smooth_ms[1] = 100.0;
        assert_eq!(config.smoothing_mask(EffectKind::Svf), 0);
    }
    #[test]
    fn double_phase_advance_preserves_hold_and_loop_semantics() {
        let mut doc = MsegData::default();
        doc.insert_node(0.25, 0.5).unwrap();
        for mode in [PlayMode::Cyclic, PlayMode::OneShot] {
            for hold in [
                HoldMode::None,
                HoldMode::Sustain(1),
                HoldMode::Loop { start: 0, end: 1 },
            ] {
                doc.play_mode = mode;
                doc.hold = hold;
                for phase in [0.0, 0.125, 0.25, 0.75] {
                    let expected = advance(&doc, phase, 0.125, false).0;
                    assert!((phase_next(&doc, phase as f64, 0.125) as f32 - expected).abs() < 1e-6);
                }
            }
        }
    }
}
