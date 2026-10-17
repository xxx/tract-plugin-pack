//! Effect modulation configuration and single-effect runtime.

use crate::effects::{
    Effect, EffectKind, ParamScaling, ParamSpec, TrackEffect, default_params_for_kind,
    norm_to_value, param_count, value_to_norm,
};
use tiny_skia_widgets::{
    MsegData, MsegNode, PlayMode, Polarity, SyncMode, advance, value_at_phase,
};

/// Default threshold for a fresh `TriggerSource::Transient` (≈ +3.5 dB on the
/// fast/slow envelope ratio — fires on clear percussive onsets without
/// over-triggering on sustained material).
pub const TRANSIENT_THRESHOLD_DEFAULT: f32 = 1.5;
/// Threshold knob range (linear ratio). `1.05` ≈ +0.4 dB on the envelope
/// ratio (very sensitive); `6.0` ≈ +15.6 dB (only firm onsets fire).
pub const TRANSIENT_THRESHOLD_MIN: f32 = 1.05;
pub const TRANSIENT_THRESHOLD_MAX: f32 = 6.0;
/// Default refractory after a fire (milliseconds) — enough to swallow a
/// snare's body without missing 16ths at 200 bpm (≈ 75 ms).
pub const TRANSIENT_HOLD_MS_DEFAULT: f32 = 50.0;
pub const TRANSIENT_HOLD_MS_MIN: f32 = 5.0;
pub const TRANSIENT_HOLD_MS_MAX: f32 = 500.0;

/// Internal envelope-follower time constants for the transient detector.
/// Fast envelope rises quickly to track the peak; slow envelope rises slowly
/// so it doesn't catch up to a transient before the ratio test fires.
const TRANSIENT_FAST_ATTACK_MS: f32 = 0.5;
const TRANSIENT_FAST_RELEASE_MS: f32 = 5.0;
const TRANSIENT_SLOW_ATTACK_MS: f32 = 30.0;
const TRANSIENT_SLOW_RELEASE_MS: f32 = 200.0;
/// Below this slow-envelope floor, the ratio test is inhibited — otherwise
/// digital noise floors produce huge ratios on near-silence. ≈ −60 dBFS.
const TRANSIENT_NOISE_FLOOR: f32 = 1e-3;

/// Per-row dual-envelope onset detector backing `TriggerSource::Transient`.
/// State is preserved across config changes (matches FreeHz's `hz_phases`)
/// and zeroed by `Modulation::reset`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TransientDetector {
    fast_env: f32,
    slow_env: f32,
    /// Samples remaining until the detector is re-armed after a fire.
    refractory: u32,
}

impl TransientDetector {
    /// Process one mono input sample. Returns `true` exactly on the sample a
    /// transient is detected — fast envelope exceeds slow envelope by
    /// `threshold`, the slow envelope is above the inhibit floor, and the
    /// detector is not in its refractory window. After a fire the detector
    /// is gated for `hold_ms` regardless of the signal.
    pub fn process_sample(
        &mut self,
        input: f32,
        threshold: f32,
        hold_ms: f32,
        sample_rate: f32,
    ) -> bool {
        let target = input.abs();
        let fast_a = one_pole_coef(TRANSIENT_FAST_ATTACK_MS, sample_rate);
        let fast_r = one_pole_coef(TRANSIENT_FAST_RELEASE_MS, sample_rate);
        let slow_a = one_pole_coef(TRANSIENT_SLOW_ATTACK_MS, sample_rate);
        let slow_r = one_pole_coef(TRANSIENT_SLOW_RELEASE_MS, sample_rate);
        let fast_coef = if target > self.fast_env {
            fast_a
        } else {
            fast_r
        };
        let slow_coef = if target > self.slow_env {
            slow_a
        } else {
            slow_r
        };
        self.fast_env = target + (self.fast_env - target) * fast_coef;
        self.slow_env = target + (self.slow_env - target) * slow_coef;
        if self.refractory > 0 {
            self.refractory -= 1;
            return false;
        }
        if self.slow_env < TRANSIENT_NOISE_FLOOR {
            return false;
        }
        if self.fast_env > self.slow_env * threshold {
            let hold = (hold_ms.max(0.0) * 0.001 * sample_rate) as u32;
            self.refractory = hold.max(1);
            return true;
        }
        false
    }

    /// Zero the detector — both envelopes back to silence, refractory cleared.
    pub fn reset(&mut self) {
        self.fast_env = 0.0;
        self.slow_env = 0.0;
        self.refractory = 0;
    }
}

/// One-pole envelope-follower coefficient for the given time constant. Larger
/// `time_ms` → coefficient closer to 1 → smoother follower.
fn one_pole_coef(time_ms: f32, sample_rate: f32) -> f32 {
    if time_ms <= 0.0 || sample_rate <= 0.0 {
        return 0.0;
    }
    (-1.0 / (time_ms * 0.001 * sample_rate)).exp()
}

/// The event that causes a track's three MSEG phases to reset to 0.
///
/// * `Free` — free-running default, never resets.
/// * `CellLight` — fires on the row's inactive→active edge at the playhead.
/// * `CellStep` — fires on every step the row is lit.
/// * `FreeHz { hz }` — fires every `1.0/hz` seconds, independent of sync.
/// * `Transient { threshold, hold_ms }` — dual-envelope ratio onset detector
///   on the plugin's input. Fires when the fast envelope exceeds the slow
///   envelope by `threshold`; the `hold_ms` refractory window suppresses
///   re-fires while the transient's tail decays.
#[derive(Clone, Copy, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum TriggerSource {
    #[default]
    Free,
    CellLight,
    CellStep,
    FreeHz {
        hz: f32,
    },
    Transient {
        /// Fast / slow envelope ratio required to fire (e.g. 1.5 = +3.5 dB).
        threshold: f32,
        /// Refractory period in milliseconds.
        hold_ms: f32,
    },
}

/// One track row's modulation: four MSEGs and the three assignable MSEGs'
/// targets and depths. `msegs[0]` is the amplitude MSEG; `msegs[1..=3]` are
/// the assignable MSEGs — `targets[k]` / `depths[k]` belong to
/// `msegs[k + 1]`.
///
/// Backward compat: legacy presets persisted three MSEGs and two
/// targets/depths. A custom `Deserialize` impl below accepts both
/// shapes -- short arrays are padded with default cyclic MSEGs and
/// no-target / zero-depth assignable slots.
#[derive(Clone, PartialEq, Debug, serde::Serialize)]
pub struct TrackModulation {
    pub msegs: [MsegData; 4],
    /// Effect parameter or Mix assigned to each MSEG, or `None`.
    pub targets: [Option<ModulationTarget>; 3],
    /// Bipolar modulation depth (−1..1) for each assignable MSEG.
    pub depths: [f32; 3],
    /// The event that resets all four of this row's MSEG phases.
    #[serde(default)]
    pub trigger: TriggerSource,
}

/// A persisted routing destination, independent of dropdown ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModulationTarget {
    EffectParameter(usize),
    Mix,
}

impl ModulationTarget {
    pub fn spec(self, kind: EffectKind) -> Option<ParamSpec> {
        match self {
            Self::EffectParameter(index) => {
                crate::effects::parameter_specs(kind).get(index).copied()
            }
            Self::Mix => Some(ParamSpec {
                name: "Mix",
                min: 0.0,
                max: 100.0,
                default: 100.0,
                scaling: ParamScaling::Linear,
                format: crate::effects::ParamFormat::Number {
                    decimals: 0,
                    unit: "%",
                },
            }),
        }
    }

    pub fn base_value(self, track: &TrackEffect) -> f32 {
        match self {
            Self::EffectParameter(index) => track.params.get(index).copied().unwrap_or(0.0),
            Self::Mix => track.mix * 100.0,
        }
    }
}

impl serde::Serialize for ModulationTarget {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::EffectParameter(index) => serde::Serialize::serialize(index, serializer),
            Self::Mix => serializer.serialize_str("mix"),
        }
    }
}

impl<'de> serde::Deserialize<'de> for ModulationTarget {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        enum NamedTarget {
            #[serde(rename = "mix")]
            Mix,
        }
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum SavedTarget {
            Parameter(usize),
            Named(NamedTarget),
        }
        Ok(
            match <SavedTarget as serde::Deserialize>::deserialize(deserializer)? {
                SavedTarget::Parameter(index) => Self::EffectParameter(index),
                SavedTarget::Named(NamedTarget::Mix) => Self::Mix,
            },
        )
    }
}

impl TrackModulation {
    /// The default modulation for track row `row`. The amplitude MSEG is flat
    /// at 1.0 (no level change); `msegs[1]` is a cyclic triangle assigned to
    /// effect parameter 0, its loop length spread by row so each track drifts
    /// at its own rate; `msegs[2]` and `msegs[3]` are unused cyclic defaults.
    pub fn default_for_row(row: usize) -> Self {
        // All four MSEGs on a row default to the SAME beat-synced length so
        // their playheads visibly stay in lockstep. The length varies by row
        // (4..34 beats) for audible per-track variety. The user can re-tune
        // any single MSEG's clock via its strip.
        let beats = 4.0 + row as f32 * 2.0;

        // msegs[0] — amplitude: flat at 1.0.
        let mut amplitude = MsegData::default();
        amplitude.nodes[0].value = 1.0;
        amplitude.nodes[1].value = 1.0;
        amplitude.play_mode = PlayMode::Cyclic;
        amplitude.sync_mode = SyncMode::Beat;
        amplitude.beats = beats;

        // msegs[1] — assignable: a cyclic triangle.
        let mut sweep = MsegData::default(); // nodes (0,0) and (1,1)
        let _ = sweep.insert_node(0.5, 1.0); // -> (0,0) (0.5,1.0) (1,1.0)
        sweep.move_node(2, 1.0, 0.0); // last node value -> 0: triangle
        sweep.play_mode = PlayMode::Cyclic;
        sweep.sync_mode = SyncMode::Beat;
        sweep.beats = beats;

        // msegs[2] — assignable: unused cyclic default, same clock as
        // the others. Flat-line at 0..1 with no curvature.
        let spare = MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Beat,
            beats,
            ..MsegData::default()
        };

        // msegs[3] — assignable: a 5-node sine-approximating cycle
        // (mid -> peak -> mid -> trough -> mid). Each segment has
        // tension applied to round its corner so the rendered curve
        // looks sinusoidal rather than triangular. Magnitude 0.5
        // is enough to clearly distinguish the bowed shape from a
        // straight line without going so far it warps into a
        // saw / hold profile.
        let mut sine = MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Beat,
            beats,
            ..MsegData::default()
        };
        // The default Mseg has 2 nodes; overwrite with 5.
        sine.node_count = 5;
        // Sign convention (see `warp` in tiny-skia-widgets/mseg):
        //   tension > 0 = slow-start  (concave bow)
        //   tension < 0 = fast-start  (convex bow)
        // For a rising sine quarter, value moves FAST near the
        // midline and SLOWS into the peak -> fast-start = neg.
        // For the falling quarter from the peak, value moves SLOW
        // near the peak and ACCELERATES downward -> slow-start = pos.
        sine.nodes[0] = MsegNode {
            time: 0.0,
            value: 0.5,
            tension: -0.5,
            stepped: false,
        };
        sine.nodes[1] = MsegNode {
            time: 0.25,
            value: 1.0,
            tension: 0.5,
            stepped: false,
        };
        sine.nodes[2] = MsegNode {
            time: 0.5,
            value: 0.5,
            tension: -0.5,
            stepped: false,
        };
        sine.nodes[3] = MsegNode {
            time: 0.75,
            value: 0.0,
            tension: 0.5,
            stepped: false,
        };
        sine.nodes[4] = MsegNode {
            time: 1.0,
            value: 0.5,
            tension: 0.0,
            stepped: false,
        };

        TrackModulation {
            msegs: [amplitude, sweep, spare, sine],
            targets: [Some(ModulationTarget::EffectParameter(0)), None, None],
            depths: [0.4, 0.0, 0.0],
            trigger: TriggerSource::Free,
        }
    }

    /// Clear any assignable-MSEG target that points past `param_count`
    /// parameters. Called after a track's effect kind changes so a target can
    /// never reference a parameter the new effect does not have.
    pub fn clamp_targets(&mut self, param_count: usize) {
        for target in &mut self.targets {
            if let Some(ModulationTarget::EffectParameter(i)) = *target
                && i >= param_count
            {
                *target = None;
            }
        }
    }
}

/// Backward-compatible deserializer for `TrackModulation`.
///
/// Legacy presets (before the MSEG count grew from 3 to 4) serialize
/// `msegs` as a 3-element array and `targets` / `depths` as 2-element
/// arrays. Modern serialization uses 4 / 3 / 3. This impl accepts
/// either shape: it deserializes the fields as Vec<_>, validates
/// the per-field lengths against the legacy or modern length, and
/// pads short arrays with sensible defaults (a cyclic spare MSEG
/// matching the existing default-style for new assignable slots,
/// no-target / zero-depth for the new assignable's bookkeeping).
impl<'de> serde::Deserialize<'de> for TrackModulation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Intermediate shape: same fields as TrackModulation but with
        // Vec lengths so we can accept either the legacy or current
        // sizes. Deserialized with serde's derive machinery via a
        // local Helper struct.
        #[derive(serde::Deserialize)]
        struct Helper {
            msegs: Vec<MsegData>,
            targets: Vec<Option<ModulationTarget>>,
            depths: Vec<f32>,
            #[serde(default)]
            trigger: TriggerSource,
        }
        let h = Helper::deserialize(deserializer)?;

        // Pad / truncate the mseg array to exactly 4 entries. Use a
        // cyclic-default spare for any short-form padding, matching
        // what `default_for_row` does for fresh tracks.
        let mut msegs: [MsegData; 4] = std::array::from_fn(|_| MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Beat,
            beats: 4.0,
            ..MsegData::default()
        });
        for (i, m) in h.msegs.into_iter().take(4).enumerate() {
            msegs[i] = m;
        }

        // Pad targets / depths to 3 (the 3 assignable slots) with
        // "unassigned, no depth" defaults.
        let mut targets = [None; 3];
        for (i, t) in h.targets.into_iter().take(3).enumerate() {
            targets[i] = t;
        }
        let mut depths: [f32; 3] = [0.0; 3];
        for (i, d) in h.depths.into_iter().take(3).enumerate() {
            depths[i] = d;
        }

        Ok(TrackModulation {
            msegs,
            targets,
            depths,
            trigger: h.trigger,
        })
    }
}

impl Default for TrackModulation {
    fn default() -> Self {
        Self::default_for_row(0)
    }
}

/// The phase delta (0..1 space) one `block_len`-sample process block advances
/// `mseg`, given the host `bpm` and `sample_rate`. Honours the MSEG's
/// `sync_mode`: `Time` uses `time_seconds`, `Beat` converts `beats` via the
/// tempo. Returns 0.0 for a degenerate (zero/negative) length — a `bpm` of 0
/// or below is treated as such (the modulation simply freezes).
pub fn mseg_phase_delta(mseg: &MsegData, block_len: usize, bpm: f64, sample_rate: f64) -> f32 {
    let length_samples = match mseg.sync_mode {
        SyncMode::Time => mseg.time_seconds as f64 * sample_rate,
        // A non-positive `bpm` gives a degenerate length (freezes below).
        SyncMode::Beat if bpm > 0.0 => mseg.beats as f64 * (60.0 / bpm) * sample_rate,
        SyncMode::Beat => 0.0,
    };
    if length_samples.is_finite() && length_samples > 0.0 {
        (block_len as f64 / length_samples) as f32
    } else {
        0.0
    }
}

/// The effective effect-parameter value for an assignable MSEG modulating
/// parameter `spec` around `base`. `mseg_value` is the MSEG's 0..1 output;
/// `depth` is the modulation depth (−1..1); `polarity` decides where the
/// MSEG's zero-deviation reference sits. The result is clamped to the
/// parameter's range.
///
/// - `Bipolar`: the MSEG midline (value 0.5) leaves the parameter at `base`;
///   the curve sweeps ±`depth` either side of it.
/// - `Unipolar`: value 0 references `base` (no modulation) and the curve is
///   additive — value 1 reaches `base + depth·range`. With an open Cutoff
///   and positive depth a Unipolar curve only ever pushes up, so it clamps
///   silently instead of pulling the filter closed.
///
/// Honours `spec.scaling`: Linear params modulate in value-space (`depth` is
/// a fraction of the full value range); Log params modulate in norm-space
/// (`depth` is a fraction of the log range), so a log-scaled cutoff sweeps
/// audibly even-handed across its decades instead of being clamped at the
/// dark end half the time.
pub fn assignable_value(
    mseg_value: f32,
    base: f32,
    depth: f32,
    spec: ParamSpec,
    polarity: Polarity,
) -> f32 {
    // Signed deviation factor: Bipolar centres on the midline (0.5 → 0),
    // Unipolar references `base` at value 0 (the raw 0..1 output).
    let factor = match polarity {
        Polarity::Bipolar => mseg_value * 2.0 - 1.0,
        Polarity::Unipolar => mseg_value,
    };
    match spec.scaling {
        ParamScaling::Linear => {
            let deviation = factor * depth * (spec.max - spec.min);
            (base + deviation).clamp(spec.min, spec.max)
        }
        ParamScaling::Log => {
            let norm_base = value_to_norm(base, spec.min, spec.max, ParamScaling::Log);
            let norm_eff = (norm_base + factor * depth).clamp(0.0, 1.0);
            norm_to_value(norm_eff, spec.min, spec.max, ParamScaling::Log)
        }
    }
}

/// Switch one track to effect `kind`: set the kind, reset its parameters to
/// the kind's defaults, and clamp the track's assignable-MSEG targets to the
/// new kind's parameter count (so a target can never reference a parameter
/// the new effect lacks). The composable core of the editor's kind-switch.
pub fn switch_effect_kind(
    effect: &mut TrackEffect,
    modulation: &mut TrackModulation,
    kind: EffectKind,
) {
    effect.kind = kind;
    effect.params = default_params_for_kind(kind);
    modulation.clamp_targets(param_count(kind));
}

/// Four envelope phases and trigger state for one effect. Configuration is
/// borrowed on each update, so editing it preserves the running phases.
#[derive(Clone, Debug, PartialEq)]
pub struct ModulationState {
    phases: [f32; 4],
    amplitude: f32,
    hz_phase: f32,
    transient: TransientDetector,
    fired: bool,
    mix: Option<MixRamp>,
}

#[derive(Clone, Debug, PartialEq)]
struct MixRamp {
    current: f32,
    target: f32,
    step: f32,
    remaining: usize,
    release: bool,
}

impl Default for ModulationState {
    fn default() -> Self {
        Self {
            phases: [0.0; 4],
            amplitude: 1.0,
            hz_phase: 0.0,
            transient: TransientDetector::default(),
            fired: false,
            mix: None,
        }
    }
}

impl ModulationState {
    /// Clear clocks and trigger history, preserving the last evaluated gain.
    pub fn reset(&mut self) {
        self.phases = [0.0; 4];
        self.hz_phase = 0.0;
        self.transient.reset();
        self.fired = false;
    }

    /// Restart all four envelopes and mark this block as triggered.
    pub fn trigger(&mut self) {
        self.phases = [0.0; 4];
        self.fired = true;
    }

    /// Analyze the dry input for block-aligned Free Hz or Transient triggers.
    /// Cell triggers are supplied explicitly through `trigger()` by the host.
    pub fn begin_block(
        &mut self,
        config: &TrackModulation,
        left: &[f32],
        right: &[f32],
        sample_rate: f64,
    ) {
        self.fired = false;
        let block_len = left.len().min(right.len());
        let sr = sample_rate as f32;
        match config.trigger {
            TriggerSource::FreeHz { hz } if hz > 0.0 => {
                self.hz_phase += (block_len as f32 * hz) / sr;
                if self.hz_phase >= 1.0 {
                    self.hz_phase -= self.hz_phase.floor();
                    self.trigger();
                }
            }
            TriggerSource::Transient { threshold, hold_ms } => {
                for i in 0..block_len {
                    let mono = (left[i] + right[i]) * 0.5;
                    if self.transient.process_sample(mono, threshold, hold_ms, sr) && !self.fired {
                        self.trigger();
                    }
                }
            }
            _ => {}
        }
    }

    /// Advance before evaluating, then apply Amp followed by slots 1–3.
    /// When targets overlap, the last assignable slot wins.
    pub fn advance_segment(
        &mut self,
        config: &TrackModulation,
        track: &TrackEffect,
        effect: &mut impl Effect,
        seg_len: usize,
        bpm: f64,
        sample_rate: f64,
    ) {
        if seg_len == 0 {
            return;
        }
        let mut mix = None;
        for k in 0..4 {
            let mseg = &config.msegs[k];
            let dt = mseg_phase_delta(mseg, seg_len, bpm, sample_rate);
            let (next, _) = advance(mseg, self.phases[k], dt, false);
            self.phases[k] = next;
            let value = value_at_phase(mseg, next);
            if k == 0 {
                self.amplitude = value;
            } else if let Some(target) = config.targets[k - 1] {
                let spec = match target {
                    ModulationTarget::EffectParameter(index) => {
                        effect.parameters().get(index).copied()
                    }
                    ModulationTarget::Mix => target.spec(track.kind),
                };
                let Some(spec) = spec else {
                    continue;
                };
                let effective = assignable_value(
                    value,
                    target.base_value(track),
                    config.depths[k - 1],
                    spec,
                    mseg.polarity,
                );
                match target {
                    ModulationTarget::EffectParameter(index) => effect.set_param(index, effective),
                    ModulationTarget::Mix => mix = Some(effective * 0.01),
                }
            }
        }
        if mix.is_some() || self.mix.is_some() {
            let target = mix.unwrap_or(track.mix).clamp(0.0, 1.0);
            // The previous endpoint follows the clock even if the row skipped audio.
            let current = self.mix.as_ref().map_or(track.mix, |ramp| ramp.target);
            self.mix = if mix.is_none() && current == target {
                None
            } else {
                Some(MixRamp {
                    current,
                    target,
                    step: (target - current) / seg_len as f32,
                    remaining: seg_len,
                    release: mix.is_none(),
                })
            };
        }
    }

    /// Resolve one sample's blend without changing the base Mix control.
    pub fn next_mix(&mut self, base: f32) -> f32 {
        let Some(ramp) = self.mix.as_mut() else {
            return base;
        };
        if ramp.remaining > 0 {
            ramp.remaining -= 1;
            ramp.current = if ramp.remaining == 0 {
                ramp.target
            } else {
                ramp.current + ramp.step
            };
        }
        let value = ramp.current.clamp(0.0, 1.0);
        if ramp.release && ramp.remaining == 0 {
            self.mix = None;
        }
        value
    }

    pub fn mix_is_active(&self) -> bool {
        self.mix.is_some()
    }

    pub fn phase(&self, slot: usize) -> f32 {
        self.phases[slot]
    }

    pub fn amplitude(&self) -> f32 {
        self.amplitude
    }

    pub fn fired(&self) -> bool {
        self.fired
    }

    pub fn hz_phase(&self) -> f32 {
        self.hz_phase
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_followers_match_their_attack_and_release_time_constants() {
        let mut detector = TransientDetector::default();
        for _ in 0..10 {
            detector.process_sample(1.0, 6.0, 3.0, 1000.0);
        }
        let fast = 1.0 - (-20.0_f32).exp();
        let slow = 1.0 - (-1.0_f32 / 3.0).exp();
        assert!((detector.fast_env - fast).abs() < 1e-6);
        assert!((detector.slow_env - slow).abs() < 1e-6);
        for _ in 0..10 {
            detector.process_sample(0.0, 6.0, 3.0, 1000.0);
        }
        assert!((detector.fast_env - fast * (-2.0_f32).exp()).abs() < 1e-6);
        assert!((detector.slow_env - slow * (-0.05_f32).exp()).abs() < 1e-6);
    }

    #[test]
    fn transient_refractory_period_suppresses_exactly_the_requested_samples() {
        for (hold_ms, expected) in [
            (3.0, vec![0, 4, 8]),
            (0.0, vec![0, 2, 4, 6, 8]),
            (-10.0, vec![0, 2, 4, 6, 8]),
        ] {
            let mut detector = TransientDetector::default();
            let fired: Vec<_> = (0..10)
                .filter(|_| detector.process_sample(1.0, 1.5, hold_ms, 1000.0))
                .collect();
            assert_eq!(fired, expected, "hold {hold_ms} ms");
        }
    }

    #[test]
    fn transient_noise_floor_includes_the_boundary_and_rejects_silence() {
        for (level, expected) in [(0.0005, false), (0.001, true), (0.002, true)] {
            let mut detector = TransientDetector {
                fast_env: 0.01,
                slow_env: level,
                refractory: 0,
            };
            assert_eq!(detector.process_sample(level, 1.5, 0.0, 1000.0), expected);
        }
        let mut detector = TransientDetector::default();
        for _ in 0..100 {
            assert!(!detector.process_sample(0.0, 1.5, 3.0, 1000.0));
            assert!(!detector.process_sample(1e-5, 1.5, 3.0, 1000.0));
        }
        assert!(detector.process_sample(-1.0, 6.0, 3.0, 1000.0));
        detector.reset();
        assert!(detector.process_sample(1.0, 6.0, 3.0, 1000.0));
    }

    #[test]
    fn transient_threshold_requires_strictly_greater_envelope_ratio() {
        let mut detector = TransientDetector {
            fast_env: 0.5,
            slow_env: 0.25,
            refractory: 0,
        };
        let threshold = 1.0 + one_pole_coef(5.0, 1000.0);
        assert!(!detector.process_sample(0.25, threshold, 0.0, 1000.0));
    }

    #[test]
    fn follower_coefficient_handles_invalid_times_and_sample_rates() {
        for (time_ms, rate) in [(0.0, 1000.0), (-1.0, 1000.0), (1.0, 0.0), (1.0, -1000.0)] {
            assert_eq!(one_pole_coef(time_ms, rate), 0.0);
        }
        assert!((one_pole_coef(10.0, 1000.0) - (-0.1_f32).exp()).abs() < 1e-7);
    }

    #[test]
    fn phase_delta_converts_seconds_and_beats_at_the_host_tempo() {
        let mut mseg = MsegData {
            sync_mode: SyncMode::Time,
            time_seconds: 2.0,
            ..MsegData::default()
        };
        assert_eq!(mseg_phase_delta(&mseg, 24_000, 120.0, 48_000.0), 0.25);
        mseg.sync_mode = SyncMode::Beat;
        mseg.beats = 4.0;
        assert_eq!(mseg_phase_delta(&mseg, 24_000, 120.0, 48_000.0), 0.25);
        assert_eq!(mseg_phase_delta(&mseg, 24_000, 60.0, 48_000.0), 0.125);
        assert_eq!(mseg_phase_delta(&mseg, 24_000, 120.0, 96_000.0), 0.125);
    }

    #[test]
    fn phase_delta_freezes_for_degenerate_lengths_rates_and_tempos() {
        for sync_mode in [SyncMode::Time, SyncMode::Beat] {
            for length in [0.0, -1.0, f32::INFINITY, f32::NAN] {
                let mseg = MsegData {
                    sync_mode,
                    time_seconds: length,
                    beats: length,
                    ..MsegData::default()
                };
                assert_eq!(mseg_phase_delta(&mseg, 100, 120.0, 1000.0), 0.0);
            }
        }
        for beats in [-2.0, 0.0, 4.0] {
            let mseg = MsegData {
                sync_mode: SyncMode::Beat,
                beats,
                ..MsegData::default()
            };
            for bpm in [-120.0, 0.0, f64::NAN] {
                assert_eq!(mseg_phase_delta(&mseg, 100, bpm, 1000.0), 0.0);
            }
        }
        for rate in [0.0, -1000.0, f64::INFINITY, f64::NAN] {
            assert_eq!(
                mseg_phase_delta(&MsegData::default(), 100, 120.0, rate),
                0.0
            );
        }
    }

    #[test]
    fn linear_assignments_use_the_full_range_and_honor_polarity_and_depth() {
        let spec = ParamSpec {
            name: "Offset range",
            min: 10.0,
            max: 110.0,
            default: 30.0,
            scaling: ParamScaling::Linear,
            format: crate::effects::ParamFormat::Number {
                decimals: 0,
                unit: "",
            },
        };
        for (value, depth, polarity, expected) in [
            (1.0, 0.2, Polarity::Unipolar, 50.0),
            (0.0, 0.2, Polarity::Unipolar, 30.0),
            (0.0, 0.2, Polarity::Bipolar, 10.0),
            (0.5, 0.2, Polarity::Bipolar, 30.0),
            (1.0, -0.2, Polarity::Bipolar, 10.0),
            (1.0, -1.0, Polarity::Unipolar, 10.0),
            (1.0, 1.0, Polarity::Bipolar, 110.0),
        ] {
            assert!((assignable_value(value, 30.0, depth, spec, polarity) - expected).abs() < 1e-5);
        }
    }

    #[test]
    fn logarithmic_assignments_move_in_decades_and_clamp_at_endpoints() {
        let spec = ParamSpec {
            name: "Frequency",
            min: 20.0,
            max: 20_000.0,
            default: 200.0,
            scaling: ParamScaling::Log,
            format: crate::effects::ParamFormat::Number {
                decimals: 0,
                unit: "Hz",
            },
        };
        for (value, depth, polarity, expected) in [
            (1.0, 1.0 / 3.0, Polarity::Unipolar, 2000.0),
            (0.5, 1.0, Polarity::Bipolar, 200.0),
            (0.0, 1.0 / 3.0, Polarity::Bipolar, 20.0),
            (1.0, -1.0 / 3.0, Polarity::Unipolar, 20.0),
            (1.0, 1.0, Polarity::Unipolar, 20_000.0),
        ] {
            let actual = assignable_value(value, 200.0, depth, spec, polarity);
            assert!(
                (actual - expected).abs() < expected * 2e-6,
                "{actual} vs {expected}"
            );
        }
    }

    #[test]
    fn row_defaults_share_the_row_clock_and_keep_distinct_envelope_shapes() {
        let config = TrackModulation::default_for_row(3);
        for mseg in &config.msegs {
            assert_eq!(mseg.sync_mode, SyncMode::Beat);
            assert_eq!(mseg.play_mode, PlayMode::Cyclic);
            assert_eq!(mseg.beats, 10.0);
        }
        for (phase, sweep, sine) in [
            (0.0, 0.0, 0.5),
            (0.25, 0.5, 1.0),
            (0.5, 1.0, 0.5),
            (0.75, 0.5, 0.0),
        ] {
            assert_eq!(value_at_phase(&config.msegs[0], phase), 1.0);
            assert!((value_at_phase(&config.msegs[1], phase) - sweep).abs() < 1e-6);
            assert!((value_at_phase(&config.msegs[3], phase) - sine).abs() < 1e-6);
        }
        assert!(value_at_phase(&config.msegs[3], 0.125) > 0.75);
        assert!(value_at_phase(&config.msegs[3], 0.625) < 0.25);
    }

    #[test]
    fn legacy_state_preserves_existing_envelopes_and_pads_a_cyclic_spare() {
        let original = TrackModulation::default_for_row(2);
        let mut json = serde_json::to_value(&original).unwrap();
        json["msegs"].as_array_mut().unwrap().truncate(3);
        json["targets"].as_array_mut().unwrap().truncate(2);
        json["depths"].as_array_mut().unwrap().truncate(2);
        json.as_object_mut().unwrap().remove("trigger");
        let restored: TrackModulation = serde_json::from_value(json).unwrap();
        assert_eq!(restored.msegs[..3], original.msegs[..3]);
        assert_eq!(restored.targets[..2], original.targets[..2]);
        assert_eq!(restored.depths[..2], original.depths[..2]);
        assert_eq!(restored.targets[2], None);
        assert_eq!(restored.depths[2], 0.0);
        assert_eq!(restored.msegs[3].play_mode, PlayMode::Cyclic);
        assert_eq!(restored.msegs[3].sync_mode, SyncMode::Beat);
        assert_eq!(restored.msegs[3].beats, 4.0);
        assert_eq!(restored.trigger, TriggerSource::Free);
    }

    #[test]
    fn parameter_target_clamping_preserves_mix_and_depth_settings() {
        let mut config = TrackModulation {
            targets: [
                Some(ModulationTarget::EffectParameter(0)),
                Some(ModulationTarget::EffectParameter(2)),
                Some(ModulationTarget::Mix),
            ],
            depths: [0.2, -0.3, 0.4],
            ..TrackModulation::default()
        };
        config.clamp_targets(2);
        assert_eq!(
            config.targets,
            [
                Some(ModulationTarget::EffectParameter(0)),
                None,
                Some(ModulationTarget::Mix)
            ]
        );
        assert_eq!(config.depths, [0.2, -0.3, 0.4]);
        config.targets[0] = Some(ModulationTarget::EffectParameter(3));
        config.clamp_targets(2);
        assert_eq!(config.targets[0], None);
        config.targets[0] = Some(ModulationTarget::EffectParameter(0));
        config.clamp_targets(0);
        assert_eq!(config.targets, [None, None, Some(ModulationTarget::Mix)]);
    }

    #[test]
    fn free_hz_clock_fires_at_boundaries_preserves_remainders_and_ignores_nonpositive_hz() {
        let mut config = TrackModulation {
            trigger: TriggerSource::FreeHz { hz: 2.0 },
            ..TrackModulation::default()
        };
        let samples = [0.0; 1250];
        let mut state = ModulationState::default();
        state.begin_block(&config, &samples[..250], &samples[..250], 1000.0);
        assert_eq!(state.hz_phase(), 0.5);
        assert!(!state.fired());
        state.begin_block(&config, &samples[..250], &samples[..250], 1000.0);
        assert_eq!(state.hz_phase(), 0.0);
        assert!(state.fired());
        state.begin_block(&config, &samples, &samples, 1000.0);
        assert_eq!(state.hz_phase(), 0.5);
        assert!(state.fired());
        for hz in [0.0, -2.0] {
            config.trigger = TriggerSource::FreeHz { hz };
            state.begin_block(&config, &samples, &samples, 1000.0);
            assert_eq!(state.hz_phase(), 0.5);
            assert!(!state.fired());
        }
    }

    #[test]
    fn transient_block_detection_uses_the_stereo_average_and_available_pairs() {
        let config = TrackModulation {
            trigger: TriggerSource::Transient {
                threshold: 6.0,
                hold_ms: 3.0,
            },
            ..TrackModulation::default()
        };
        let mut state = ModulationState::default();
        state.begin_block(&config, &[0.5, 1.0], &[0.25], 1000.0);
        assert!(state.fired());
        let expected = 0.375 * (1.0 - (-2.0_f32).exp());
        assert!((state.transient.fast_env - expected).abs() < 1e-6);
        state.reset();
        state.begin_block(&config, &[1.0; 8], &[-1.0; 8], 1000.0);
        assert!(!state.fired());
        assert_eq!(state.transient.fast_env, 0.0);
        state.begin_block(&config, &[1.0], &[], 1000.0);
        assert!(!state.fired());
        assert_eq!(state.transient.fast_env, 0.0);
    }

    struct ProbeEffect(f32);

    impl Effect for ProbeEffect {
        fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
            (left, right)
        }
        fn set_sample_rate(&mut self, _: f32) {}
        fn reset(&mut self) {}
        fn parameters(&self) -> &'static [ParamSpec] {
            &[ParamSpec {
                name: "Value",
                min: 0.0,
                max: 1.0,
                default: 0.5,
                scaling: ParamScaling::Linear,
                format: crate::effects::ParamFormat::Number {
                    decimals: 2,
                    unit: "",
                },
            }]
        }
        fn set_param(&mut self, _: usize, value: f32) {
            self.0 = value;
        }
    }

    #[test]
    fn mix_targets_keep_legacy_parameter_ids_and_round_trip() {
        let config = TrackModulation {
            targets: [
                Some(ModulationTarget::EffectParameter(0)),
                Some(ModulationTarget::Mix),
                None,
            ],
            ..TrackModulation::default()
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["targets"], serde_json::json!([0, "mix", null]));
        assert_eq!(
            serde_json::from_value::<TrackModulation>(json).unwrap(),
            config
        );
    }

    #[test]
    fn mix_modulation_ramps_to_the_effective_value_without_editing_the_base() {
        let mut config = TrackModulation::default();
        config.targets[0] = Some(ModulationTarget::Mix);
        config.depths[0] = 1.0;
        config.msegs[1].polarity = Polarity::Unipolar;
        for node in &mut config.msegs[1].nodes {
            node.value = 1.0;
        }
        let track = TrackEffect {
            mix: 0.25,
            ..TrackEffect::default()
        };
        let mut effect = ProbeEffect(0.5);
        let mut state = ModulationState::default();
        assert!(!state.mix_is_active());
        state.advance_segment(&config, &track, &mut effect, 4, 120.0, 48000.0);
        assert!(state.mix_is_active());
        let actual: [f32; 4] = std::array::from_fn(|_| state.next_mix(track.mix));
        assert_eq!(actual, [0.4375, 0.625, 0.8125, 1.0]);
        assert_eq!(track.mix, 0.25);
        assert_eq!(effect.0, 0.5);
        for _ in 0..3 {
            assert_eq!(state.next_mix(0.75), 1.0);
            assert!(state.mix_is_active());
        }

        config.targets[0] = None;
        state.advance_segment(&config, &track, &mut effect, 4, 120.0, 48000.0);
        let restored: [f32; 4] = std::array::from_fn(|_| state.next_mix(track.mix));
        assert_eq!(restored, [0.8125, 0.625, 0.4375, 0.25]);
        assert!(!state.mix_is_active());
        assert_eq!(state.next_mix(0.75), 0.75);
    }

    #[test]
    fn amplitude_evaluates_flat_curves_and_survives_reset() {
        let mut state = ModulationState::default();
        assert_eq!(state.amplitude(), 1.0);
        let mut config = TrackModulation::default();
        for node in &mut config.msegs[0].nodes {
            node.value = 0.375;
        }
        config.targets = [
            Some(ModulationTarget::EffectParameter(usize::MAX)),
            None,
            None,
        ];
        let mut effect = ProbeEffect(0.5);
        state.advance_segment(
            &config,
            &TrackEffect::default(),
            &mut effect,
            100,
            120.0,
            48000.0,
        );
        assert_eq!(state.amplitude(), 0.375);
        assert_eq!(effect.0, 0.5, "an invalid parameter target is ignored");
        state.reset();
        assert_eq!(state.amplitude(), 0.375);
    }

    #[test]
    fn targets_expose_parameter_metadata_and_percent_mix_values() {
        let mut track = TrackEffect::default_for_row(0);
        track.kind = EffectKind::Comb;
        track.params[0] = 321.0;
        track.mix = 0.375;
        let parameter = ModulationTarget::EffectParameter(0);
        assert_eq!(parameter.base_value(&track), 321.0);
        let spec = parameter.spec(track.kind).unwrap();
        let declared = crate::effects::parameter_specs(track.kind)[0];
        assert_eq!(
            (spec.name, spec.min, spec.max),
            (declared.name, declared.min, declared.max)
        );
        let invalid = ModulationTarget::EffectParameter(usize::MAX);
        assert_eq!(invalid.base_value(&track), 0.0);
        assert!(invalid.spec(track.kind).is_none());
        let mix = ModulationTarget::Mix.spec(track.kind).unwrap();
        assert_eq!((mix.min, mix.max, mix.default), (0.0, 100.0, 100.0));
        assert_eq!(mix.scaling, ParamScaling::Linear);
        assert_eq!(ModulationTarget::Mix.base_value(&track), 37.5);
    }

    #[test]
    fn mix_ramp_follows_the_clock_even_when_an_inactive_row_skips_audio() {
        let mut config = TrackModulation::default();
        config.targets[0] = Some(ModulationTarget::Mix);
        config.depths[0] = 1.0;
        config.msegs[1].polarity = Polarity::Unipolar;
        for node in &mut config.msegs[1].nodes {
            node.value = 1.0;
        }
        let track = TrackEffect {
            mix: 0.0,
            ..TrackEffect::default()
        };
        let mut effect = ProbeEffect(0.5);
        let mut state = ModulationState::default();
        state.advance_segment(&config, &track, &mut effect, 4, 120.0, 48000.0);
        for node in &mut config.msegs[1].nodes {
            node.value = 0.0;
        }
        state.advance_segment(&config, &track, &mut effect, 4, 120.0, 48000.0);
        assert_eq!(state.next_mix(0.0), 0.75);
    }

    #[test]
    fn mix_modulation_honors_signed_depth_polarity_clamps_and_last_slot_wins() {
        for (base, value, depth, polarity, expected) in [
            (1.0, 1.0, -1.0, Polarity::Unipolar, 0.0),
            (0.0, 1.0, 1.0, Polarity::Unipolar, 1.0),
            (0.5, 0.0, 0.25, Polarity::Bipolar, 0.25),
            (0.75, 1.0, 1.0, Polarity::Bipolar, 1.0),
            (0.25, 0.0, 1.0, Polarity::Bipolar, 0.0),
            (0.3, 1.0, 0.0, Polarity::Unipolar, 0.3),
        ] {
            let mut config = TrackModulation {
                targets: [Some(ModulationTarget::Mix); 3],
                depths: [1.0, 1.0, depth],
                ..TrackModulation::default()
            };
            config.msegs[3].polarity = polarity;
            for node in &mut config.msegs[3].nodes {
                node.value = value;
            }
            let track = TrackEffect {
                mix: base,
                ..TrackEffect::default()
            };
            let mut state = ModulationState::default();
            let mut effect = ProbeEffect(0.5);
            state.advance_segment(&config, &track, &mut effect, 1, 120.0, 48000.0);
            assert!((state.next_mix(base) - expected).abs() < 1e-6);
            assert_eq!(effect.0, 0.5);
        }
    }

    #[test]
    fn mix_assignment_survives_switching_to_an_effect_without_parameters() {
        let mut config = TrackModulation {
            targets: [
                Some(ModulationTarget::Mix),
                Some(ModulationTarget::EffectParameter(0)),
                None,
            ],
            ..TrackModulation::default()
        };
        let mut track = TrackEffect::default_for_row(0);
        switch_effect_kind(&mut track, &mut config, EffectKind::None);
        assert_eq!(config.targets, [Some(ModulationTarget::Mix), None, None]);
    }

    #[test]
    fn single_effect_runtime_advances_then_applies_last_assignable_slot() {
        let mut config = TrackModulation {
            targets: [Some(ModulationTarget::EffectParameter(0)); 3],
            depths: [1.0; 3],
            ..TrackModulation::default()
        };
        for (i, mseg) in config.msegs.iter_mut().enumerate() {
            mseg.sync_mode = SyncMode::Time;
            mseg.time_seconds = 1.0;
            mseg.polarity = Polarity::Unipolar;
            mseg.nodes[0].value = i as f32 * 0.2;
            mseg.nodes[1].value = i as f32 * 0.2;
        }
        let mut state = ModulationState::default();
        let mut effect = ProbeEffect(0.0);
        state.advance_segment(
            &config,
            &TrackEffect::default(),
            &mut effect,
            12000,
            120.0,
            48000.0,
        );
        assert_eq!(state.phase(3), 0.25);
        assert_eq!(state.amplitude(), 0.0);
        assert!((effect.0 - 0.6).abs() < 1e-6);
    }

    #[test]
    fn config_edits_preserve_running_state_and_explicit_trigger_restarts_it() {
        let mut config = TrackModulation::default();
        let mut state = ModulationState::default();
        let mut effect = ProbeEffect(0.0);
        let track = TrackEffect::default();
        state.advance_segment(&config, &track, &mut effect, 24000, 120.0, 48000.0);
        let before = state.phase(1);
        config.depths[0] = -0.7;
        state.advance_segment(&config, &track, &mut effect, 24000, 120.0, 48000.0);
        assert_eq!(state.phase(1), before * 2.0);
        state.trigger();
        assert_eq!(state.phases, [0.0; 4]);
        assert!(state.fired());
    }

    #[test]
    fn free_hz_trigger_keeps_fractional_remainder_and_clears_block_flag() {
        let config = TrackModulation {
            trigger: TriggerSource::FreeHz { hz: 10.0 },
            ..TrackModulation::default()
        };
        let mut state = ModulationState::default();
        let silence = [0.0; 6000];
        state.begin_block(&config, &silence, &silence, 48000.0);
        assert!(state.fired());
        assert_eq!(state.hz_phase(), 0.25);
        state.begin_block(&config, &[], &[], 48000.0);
        assert!(!state.fired());
        assert_eq!(state.hz_phase(), 0.25);
    }

    #[test]
    fn reset_clears_transient_detector_and_clocks_without_changing_gain() {
        let config = TrackModulation {
            trigger: TriggerSource::Transient {
                threshold: 1.5,
                hold_ms: 50.0,
            },
            ..TrackModulation::default()
        };
        let mut state = ModulationState::default();
        let hot = [0.9; 1024];
        state.begin_block(&config, &hot, &hot, 48000.0);
        assert_ne!(state.transient, TransientDetector::default());
        let gain = state.amplitude();
        state.reset();
        assert_eq!(state.transient, TransientDetector::default());
        assert_eq!(state.phases, [0.0; 4]);
        assert_eq!(state.hz_phase(), 0.0);
        assert_eq!(state.amplitude(), gain);
        assert!(!state.fired());
    }

    #[test]
    fn zero_length_segment_preserves_all_runtime_state() {
        let config = TrackModulation::default();
        let mut state = ModulationState::default();
        let mut effect = ProbeEffect(0.0);
        state.advance_segment(
            &config,
            &TrackEffect::default(),
            &mut effect,
            1000,
            120.0,
            48000.0,
        );
        let before = state.clone();
        state.advance_segment(
            &config,
            &TrackEffect::default(),
            &mut effect,
            0,
            120.0,
            48000.0,
        );
        assert_eq!(state, before);
    }
}
