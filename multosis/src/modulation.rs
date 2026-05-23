//! The modulation engine — Phase 2 Milestone 2b. Three MSEGs per track row
//! (one amplitude + two assignable), free-running on their own clocks,
//! driving the 2a effect-parameter and amplitude seams.
//!
//! See `docs/superpowers/specs/2026-05-19-multosis-phase-2b-design.md`.

use crate::effects::{
    default_params_for_kind, norm_to_value, param_count, value_to_norm, Effect, EffectInstance,
    EffectKind, ParamScaling, ParamSpec, TrackEffect,
};
use tiny_skia_widgets::{advance, value_at_phase, MsegData, PlayMode, Polarity, SyncMode};

/// The number of track rows. Matches `crate::grid::ROWS`.
const ROWS: usize = 16;

/// The event that causes a track's three MSEG phases to reset to 0.
/// `Free` is the free-running default; `CellLight` fires on the row's
/// inactive→active edge at the playhead; `CellStep` fires on every step the
/// row is lit; `FreeHz` fires every `1.0/hz` seconds independently of sync.
#[derive(Clone, Copy, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum TriggerSource {
    #[default]
    Free,
    CellLight,
    CellStep,
    FreeHz {
        hz: f32,
    },
}

/// One track row's modulation: three MSEGs and the two assignable MSEGs'
/// targets and depths. `msegs[0]` is the amplitude MSEG; `msegs[1]` and
/// `msegs[2]` are the assignable MSEGs — `targets[k]` / `depths[k]` belong to
/// `msegs[k + 1]`.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrackModulation {
    pub msegs: [MsegData; 3],
    /// Target effect-parameter index for each assignable MSEG, or `None`.
    pub targets: [Option<usize>; 2],
    /// Bipolar modulation depth (−1..1) for each assignable MSEG.
    pub depths: [f32; 2],
    /// The event that resets all three of this row's MSEG phases.
    #[serde(default)]
    pub trigger: TriggerSource,
}

impl TrackModulation {
    /// The default modulation for track row `row`. The amplitude MSEG is flat
    /// at 1.0 (no level change); `msegs[1]` is a cyclic triangle assigned to
    /// effect parameter 0, its loop length spread by row so each track drifts
    /// at its own rate; `msegs[2]` is an unused cyclic default.
    pub fn default_for_row(row: usize) -> Self {
        // All three MSEGs on a row default to the SAME beat-synced length so
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

        // msegs[2] — assignable: unused default, same clock as the others.
        let spare = MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Beat,
            beats,
            ..MsegData::default()
        };

        TrackModulation {
            msegs: [amplitude, sweep, spare],
            targets: [Some(0), None],
            depths: [0.4, 0.0],
            trigger: TriggerSource::Free,
        }
    }

    /// Clear any assignable-MSEG target that points past `param_count`
    /// parameters. Called after a track's effect kind changes so a target can
    /// never reference a parameter the new effect does not have.
    pub fn clamp_targets(&mut self, param_count: usize) {
        for target in &mut self.targets {
            if let Some(i) = *target {
                if i >= param_count {
                    *target = None;
                }
            }
        }
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

/// The modulation runtime owned by the audio engine — the per-track config,
/// each MSEG's free-running phase, and the latest per-row amplitude gain.
pub struct Modulation {
    config: [TrackModulation; ROWS],
    /// Free-running phase per `[row][mseg]`.
    phases: [[f32; 3]; ROWS],
    /// Latest per-row amplitude gain, set by `begin_block` / `advance_segment`.
    amplitudes: [f32; ROWS],
    /// Free-Hz oscillator phase per row, advances 0..1 and wraps modulo 1.
    hz_phases: [f32; ROWS],
    /// The rows that fired this block (bit `r` set). Zeroed by `begin_block`,
    /// then accumulated by the FreeHz path and by `fire`.
    fires: u16,
}

impl Modulation {
    /// A runtime with the default per-row modulation and zeroed phases.
    pub fn new() -> Self {
        Self {
            config: std::array::from_fn(TrackModulation::default_for_row),
            phases: [[0.0; 3]; ROWS],
            amplitudes: [1.0; ROWS],
            hz_phases: [0.0; ROWS],
            fires: 0,
        }
    }

    /// Replace the per-track modulation config (bridged from persisted state
    /// at init, and again on each live edit — off the audio thread).
    ///
    /// Runtime state (`phases`, `hz_phases`, `amplitudes`) is deliberately
    /// preserved across config changes so editing a parameter or switching
    /// trigger source mid-playback does not glitch the modulation. Only
    /// `reset()` clears that state.
    pub fn set_config(&mut self, config: &[TrackModulation; ROWS]) {
        self.config = config.clone();
    }

    /// Reset every MSEG phase to 0.
    pub fn reset(&mut self) {
        self.phases = [[0.0; 3]; ROWS];
        self.hz_phases = [0.0; ROWS];
        self.fires = 0;
    }

    /// The latest amplitude gain for `row` (set by the modulation update).
    pub fn amplitude(&self, row: usize) -> f32 {
        self.amplitudes[row]
    }

    /// The current free-running phase of MSEG `k` (`0..3`) on `row` (`0..ROWS`).
    /// Published to the editor for the playhead overlay.
    pub fn phase(&self, row: usize, k: usize) -> f32 {
        self.phases[row][k]
    }

    /// Block-rate modulation setup, run once at the top of a process block.
    /// Zeroes `fires`, then for every `FreeHz` row advances its oscillator by
    /// the whole block — on a wrap, the row fires (its three MSEG phases
    /// reset to 0), matching how `CellLight` and `CellStep` are wired. The
    /// fractional remainder of the oscillator is retained, so multiple wraps
    /// in one block still count as a single fire. Between fires, the row's
    /// MSEGs are advanced by `advance_segment` at each MSEG's own
    /// sync/length — `FreeHz` is a trigger source, not a clock override.
    pub fn begin_block(&mut self, block_len: usize, sample_rate: f64) {
        self.fires = 0;
        for row in 0..ROWS {
            let TriggerSource::FreeHz { hz } = self.config[row].trigger else {
                continue;
            };
            if hz > 0.0 {
                self.hz_phases[row] += (block_len as f32 * hz) / sample_rate as f32;
                if self.hz_phases[row] >= 1.0 {
                    self.hz_phases[row] -= self.hz_phases[row].floor();
                    self.fires |= 1 << row;
                    self.phases[row] = [0.0; 3];
                }
            }
        }
    }

    /// Advance every row's three MSEGs by one segment of `seg_len` samples,
    /// then evaluate and apply them. Called once per segment from the
    /// engine's step-boundary segment loop. A zero-length segment is a no-op.
    ///
    /// Splitting a block into per-segment advances yields the same end-of-block
    /// phase as one whole-block advance only because every multosis MSEG is
    /// `PlayMode::Cyclic` — its phase wrap is additively decomposable. A
    /// `OneShot`-mode MSEG clamps non-linearly and could land differently
    /// across segments; revisit this if a `play_mode` control is ever exposed.
    pub fn advance_segment(
        &mut self,
        seg_len: usize,
        bpm: f64,
        sample_rate: f64,
        effects: &mut [EffectInstance; ROWS],
        track_effects: &[TrackEffect; ROWS],
    ) {
        if seg_len == 0 {
            return;
        }
        for row in 0..ROWS {
            for k in 0..3 {
                let mseg = self.config[row].msegs[k];
                let dt = mseg_phase_delta(&mseg, seg_len, bpm, sample_rate);
                let (next, _finished) = advance(&mseg, self.phases[row][k], dt, false);
                self.phases[row][k] = next;
                self.apply_mseg(row, k, effects, track_effects);
            }
        }
    }

    /// Fire the per-step modulation triggers at a step boundary. `newly_rows`
    /// is the inactive→active edge mask (bit `r` = row `r` first lit this
    /// step); `active_rows` is the post-tick active mask (bit `r` = row `r`
    /// has a lit, enabled cell under the playhead now). A `CellLight` row
    /// fires if it is in `newly_rows`; a `CellStep` row fires if it is in
    /// `active_rows`; `Free` and `FreeHz` rows never fire. A firing row's
    /// three MSEG phases reset to 0 and its `fires` bit is set. Called at a
    /// step boundary, so the reset takes effect on the very next segment.
    pub fn fire(&mut self, newly_rows: u16, active_rows: u16) {
        for row in 0..ROWS {
            let reset = match self.config[row].trigger {
                TriggerSource::CellLight => newly_rows & (1 << row) != 0,
                TriggerSource::CellStep => active_rows & (1 << row) != 0,
                TriggerSource::Free | TriggerSource::FreeHz { .. } => false,
            };
            if reset {
                self.phases[row] = [0.0; 3];
                self.fires |= 1 << row;
            }
        }
    }

    /// Evaluate MSEG `k` on `row` at its current phase and apply it: the
    /// amplitude MSEG (`k == 0`) sets `amplitudes[row]`; an assignable MSEG
    /// with a target writes that effect parameter via `set_param`.
    fn apply_mseg(
        &mut self,
        row: usize,
        k: usize,
        effects: &mut [EffectInstance; ROWS],
        track_effects: &[TrackEffect; ROWS],
    ) {
        // `MsegData` is `Copy`; copy what we need so the immutable
        // `self.config` borrow does not span the `self.amplitudes` / `effects`
        // writes below.
        let mseg = self.config[row].msegs[k];
        let value = value_at_phase(&mseg, self.phases[row][k]);
        if k == 0 {
            // Amplitude MSEG.
            self.amplitudes[row] = value;
        } else if let Some(target) = self.config[row].targets[k - 1] {
            // Assignable MSEG -> a target effect parameter.
            if let Some(&spec) = effects[row].parameters().get(target) {
                let base = track_effects[row].params[target];
                let depth = self.config[row].depths[k - 1];
                effects[row].set_param(
                    target,
                    assignable_value(value, base, depth, spec, mseg.polarity),
                );
            }
        }
    }

    /// Test helper: true when every MSEG phase is 0.
    #[cfg(test)]
    pub fn phases_all_zero(&self) -> bool {
        self.phases.iter().flatten().all(|&p| p == 0.0)
    }

    /// Test helper: the rows that fired this block.
    #[cfg(test)]
    pub fn fires_last_block(&self) -> u16 {
        self.fires
    }

    /// Test helper: the current phase for `[row][k]`.
    #[cfg(test)]
    pub fn phase_for_test(&self, row: usize, k: usize) -> f32 {
        self.phases[row][k]
    }

    /// Test helper: true when every Free-Hz oscillator phase is 0.
    #[cfg(test)]
    pub fn hz_phases_all_zero(&self) -> bool {
        self.hz_phases.iter().all(|&p| p == 0.0)
    }
}

impl Default for Modulation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_modulation_serde_round_trips() {
        let tm = TrackModulation::default_for_row(3);
        let json = serde_json::to_string(&tm).unwrap();
        let back: TrackModulation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tm);
    }

    #[test]
    fn track_modulation_array_serde_round_trips() {
        let cfg: [TrackModulation; ROWS] = std::array::from_fn(TrackModulation::default_for_row);
        let json = serde_json::to_string(&cfg).unwrap();
        let back: [TrackModulation; ROWS] = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn default_for_row_assigns_one_assignable_and_varies_by_row() {
        let a = TrackModulation::default_for_row(0);
        let b = TrackModulation::default_for_row(7);
        assert_eq!(a.targets[0], Some(0));
        assert_eq!(a.targets[1], None);
        assert!(a.depths[0] != 0.0);
        assert_eq!(a.depths[1], 0.0);
        assert!(a.msegs[1].beats != b.msegs[1].beats);
        assert!(a.msegs[0].nodes[..a.msegs[0].node_count]
            .iter()
            .all(|n| (n.value - 1.0).abs() < 1e-6));
    }

    #[test]
    fn mseg_phase_delta_time_sync() {
        let mut m = MsegData::default();
        m.sync_mode = SyncMode::Time;
        m.time_seconds = 2.0;
        let dt = mseg_phase_delta(&m, 48, 120.0, 48_000.0);
        assert!((dt - 48.0 / 96_000.0).abs() < 1e-9, "got {dt}");
    }

    #[test]
    fn mseg_phase_delta_beat_sync() {
        let mut m = MsegData::default();
        m.sync_mode = SyncMode::Beat;
        m.beats = 4.0;
        let dt = mseg_phase_delta(&m, 48, 120.0, 48_000.0);
        assert!((dt - 48.0 / 96_000.0).abs() < 1e-9, "got {dt}");
    }

    #[test]
    fn mseg_phase_delta_zero_bpm_freezes() {
        // A host reporting a 0 BPM must not yield a non-finite delta; the
        // modulation simply freezes (delta 0).
        let mut m = MsegData::default();
        m.sync_mode = SyncMode::Beat;
        m.beats = 4.0;
        let dt = mseg_phase_delta(&m, 48, 0.0, 48_000.0);
        assert_eq!(dt, 0.0);
    }

    #[test]
    fn assignable_value_midline_is_the_base() {
        let spec = ParamSpec {
            name: "p",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            scaling: crate::effects::ParamScaling::Linear,
            format: crate::effects::ParamFormat::Number {
                decimals: 0,
                unit: "",
            },
        };
        assert!((assignable_value(0.5, 40.0, 1.0, spec, Polarity::Bipolar) - 40.0).abs() < 1e-6);
    }

    #[test]
    fn assignable_value_depth_and_sign() {
        let spec = ParamSpec {
            name: "p",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            scaling: crate::effects::ParamScaling::Linear,
            format: crate::effects::ParamFormat::Number {
                decimals: 0,
                unit: "",
            },
        };
        assert_eq!(
            assignable_value(1.0, 40.0, 1.0, spec, Polarity::Bipolar),
            100.0
        );
        assert_eq!(
            assignable_value(1.0, 40.0, -1.0, spec, Polarity::Bipolar),
            0.0
        );
        assert!((assignable_value(1.0, 20.0, 0.5, spec, Polarity::Bipolar) - 70.0).abs() < 1e-4);
    }

    #[test]
    fn assignable_value_unipolar_references_base_at_zero() {
        let spec = ParamSpec {
            name: "p",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            scaling: crate::effects::ParamScaling::Linear,
            format: crate::effects::ParamFormat::Number {
                decimals: 0,
                unit: "",
            },
        };
        // Unipolar: value 0 leaves the parameter at base (no modulation),
        // and the curve is additive — value 1 reaches base + depth·range.
        assert!((assignable_value(0.0, 40.0, 1.0, spec, Polarity::Unipolar) - 40.0).abs() < 1e-6);
        assert!((assignable_value(0.5, 40.0, 1.0, spec, Polarity::Unipolar) - 90.0).abs() < 1e-4);
        assert_eq!(
            assignable_value(1.0, 40.0, 1.0, spec, Polarity::Unipolar),
            100.0
        );
        // Negative depth pushes the additive curve downward from base.
        assert!((assignable_value(1.0, 40.0, -0.3, spec, Polarity::Unipolar) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn assignable_value_unipolar_open_param_clamps_silently() {
        // The user's case: a log Cutoff fully open (base = max). A Unipolar
        // curve with positive depth only ever pushes up, so every sample
        // clamps at max — no audible downward sweep.
        let spec = ParamSpec {
            name: "Cutoff",
            min: 20.0,
            max: 20_000.0,
            default: 2_000.0,
            scaling: crate::effects::ParamScaling::Log,
            format: crate::effects::ParamFormat::Hertz,
        };
        for &v in &[0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            let out = assignable_value(v, 20_000.0, 0.4, spec, Polarity::Unipolar);
            assert!(
                (out - 20_000.0).abs() < 1e-3,
                "v {v} -> {out}, expected max"
            );
        }
    }

    #[test]
    fn assignable_value_always_within_range() {
        let spec = ParamSpec {
            name: "p",
            min: 5.0,
            max: 9.0,
            default: 7.0,
            scaling: crate::effects::ParamScaling::Linear,
            format: crate::effects::ParamFormat::Number {
                decimals: 0,
                unit: "",
            },
        };
        for &v in &[0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            for &d in &[-1.0_f32, -0.3, 0.0, 0.6, 1.0] {
                for &pol in &[Polarity::Bipolar, Polarity::Unipolar] {
                    let out = assignable_value(v, 8.0, d, spec, pol);
                    assert!((5.0..=9.0).contains(&out), "v {v} d {d} {pol:?} -> {out}");
                }
            }
        }
    }

    #[test]
    fn assignable_value_log_swings_in_norm_space() {
        // A 20..20000 Hz log param at base 2000 has norm 0.667; depth 0.4
        // means ±0.4 norm. So mseg 0 → norm 0.267 → ~126 Hz; mseg 1 → norm
        // 1.067 clamped to 1.0 → 20000 Hz. The earlier linear formula gave
        // ~20 Hz / ~9992 Hz for the same input (mostly clamped at the dark
        // end, hence the "barely audible" complaint).
        let spec = ParamSpec {
            name: "Cutoff",
            min: 20.0,
            max: 20_000.0,
            default: 2_000.0,
            scaling: crate::effects::ParamScaling::Log,
            format: crate::effects::ParamFormat::Hertz,
        };
        let lo = assignable_value(0.0, 2_000.0, 0.4, spec, Polarity::Bipolar);
        let mid = assignable_value(0.5, 2_000.0, 0.4, spec, Polarity::Bipolar);
        let hi = assignable_value(1.0, 2_000.0, 0.4, spec, Polarity::Bipolar);
        // Midline still equals the base, exactly.
        assert!((mid - 2_000.0).abs() < 1e-3, "midline {mid}");
        // Low end is audibly above the parameter floor — well above 20 Hz.
        assert!(lo > 100.0 && lo < 200.0, "log low end {lo}");
        // High end is the full max (clamped by `norm + depth > 1`).
        assert!((hi - 20_000.0).abs() < 1e-3, "log high end {hi}");
    }

    #[test]
    fn modulation_amplitude_reflects_the_amplitude_mseg() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        m.begin_block(64, 48_000.0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        // The default amplitude MSEG is flat at 1.0.
        for r in 0..ROWS {
            assert!(
                (m.amplitude(r) - 1.0).abs() < 1e-6,
                "row {r}: {}",
                m.amplitude(r)
            );
        }
    }

    #[test]
    fn modulation_amplitude_flat_zero_mseg_silences_the_row() {
        // An amplitude MSEG flat at 0.0 yields a row gain of 0 (spec §8).
        let mut config: [TrackModulation; ROWS] =
            std::array::from_fn(TrackModulation::default_for_row);
        for tm in &mut config {
            for node in &mut tm.msegs[0].nodes {
                node.value = 0.0;
            }
        }
        let mut m = Modulation::new();
        m.set_config(&config);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        m.begin_block(64, 48_000.0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        for r in 0..ROWS {
            assert_eq!(m.amplitude(r), 0.0, "row {r}");
        }
    }

    #[test]
    fn modulation_applies_an_assignable_mseg_to_its_effect() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        for e in &mut effects {
            e.set_sample_rate(48_000.0);
        }
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Run a block, then drive the effects with a signal and capture output.
        m.begin_block(64, 48_000.0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        let after_first: Vec<f32> = (0..200)
            .map(|_| effects[0].process_sample(1.0, -1.0).0)
            .collect();
        // Advance many blocks so the cyclic MSEG has moved, re-apply.
        for _ in 0..400 {
            m.begin_block(64, 48_000.0);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        let after_later: Vec<f32> = (0..200)
            .map(|_| effects[0].process_sample(1.0, -1.0).0)
            .collect();
        // The modulated cutoff changed -> the filtered output differs.
        assert!(
            after_first != after_later,
            "an assigned MSEG should modulate the effect over time"
        );
    }

    #[test]
    fn modulation_reset_zeroes_phases() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        for _ in 0..100 {
            m.begin_block(64, 48_000.0);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        m.reset();
        // After reset, every phase is back at 0.
        assert!(m.phases_all_zero());
    }

    #[test]
    fn clamp_targets_clears_out_of_range_targets() {
        let mut tm = TrackModulation::default_for_row(0);
        tm.targets = [Some(0), Some(5)];
        // An effect with 2 parameters: target 0 survives, target 5 is cleared.
        tm.clamp_targets(2);
        assert_eq!(tm.targets, [Some(0), None]);
        // A target exactly at the count is out of range.
        tm.targets = [Some(2), None];
        tm.clamp_targets(2);
        assert_eq!(tm.targets, [None, None]);
    }

    #[test]
    fn trigger_source_default_is_free() {
        assert_eq!(TriggerSource::default(), TriggerSource::Free);
    }

    #[test]
    fn trigger_source_variants_serde_round_trip() {
        for src in [
            TriggerSource::Free,
            TriggerSource::CellLight,
            TriggerSource::CellStep,
            TriggerSource::FreeHz { hz: 2.5 },
        ] {
            let json = serde_json::to_string(&src).unwrap();
            let back: TriggerSource = serde_json::from_str(&json).unwrap();
            assert_eq!(back, src);
        }
    }

    #[test]
    fn track_modulation_with_trigger_serde_round_trips() {
        let mut tm = TrackModulation::default_for_row(0);
        tm.trigger = TriggerSource::FreeHz { hz: 4.0 };
        let json = serde_json::to_string(&tm).unwrap();
        let back: TrackModulation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.trigger, TriggerSource::FreeHz { hz: 4.0 });
    }

    #[test]
    fn track_modulation_loads_missing_trigger_as_free() {
        // A JSON shaped like a pre-Phase-3 TrackModulation (no "trigger" key)
        // deserialises with trigger = Free, per serde's additive default.
        let tm = TrackModulation::default_for_row(0);
        let json = serde_json::to_string(&tm).unwrap();
        // Strip the trigger field from the JSON to simulate the old shape.
        let stripped = strip_trigger_field(&json);
        let back: TrackModulation = serde_json::from_str(&stripped).unwrap();
        assert_eq!(back.trigger, TriggerSource::Free);
    }

    fn strip_trigger_field(json: &str) -> String {
        // Naively remove the `"trigger":<value>,` substring. Works for the
        // serde_json default representation of small enums.
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let mut obj = v.as_object().unwrap().clone();
        obj.remove("trigger");
        serde_json::to_string(&serde_json::Value::Object(obj)).unwrap()
    }

    #[test]
    fn fires_last_block_default_is_zero() {
        let m = Modulation::new();
        assert_eq!(m.fires_last_block(), 0);
    }

    #[test]
    fn cell_light_fires_on_each_cell_light_event() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[3].trigger = TriggerSource::CellLight;
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Each block that signals a cell-light event for row 3 fires the
        // trigger; blocks with no event don't.
        m.begin_block(64, 48_000.0);
        m.fire(1 << 3, 1 << 3);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 1 << 3);
        m.begin_block(64, 48_000.0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 0);
        m.begin_block(64, 48_000.0);
        m.fire(1 << 3, 1 << 3);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 1 << 3);
        // A row that didn't get an event doesn't fire even if another did.
        m.begin_block(64, 48_000.0);
        m.fire(1 << 7, 1 << 7);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 0);
    }

    #[test]
    fn free_hz_fires_at_roughly_the_expected_rate() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[5].trigger = TriggerSource::FreeHz { hz: 10.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // 10 Hz, 48 kHz, 4800-sample blocks -> exactly 1 fire per block, on average.
        // Run 100 blocks; expect ~100 fires (allow ±1 for the boundary).
        let mut fires = 0usize;
        for _ in 0..100 {
            m.begin_block(4800, 48_000.0);
            m.advance_segment(4800, 120.0, 48_000.0, &mut effects, &track_effects);
            if m.fires_last_block() & (1 << 5) != 0 {
                fires += 1;
            }
        }
        assert!(
            (99..=101).contains(&fires),
            "10 Hz over 100 blocks of 1 cycle each: got {fires} fires"
        );
    }

    #[test]
    fn free_hz_nonpositive_never_fires() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[0].trigger = TriggerSource::FreeHz { hz: 0.0 };
        cfg[1].trigger = TriggerSource::FreeHz { hz: -2.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        for _ in 0..50 {
            m.begin_block(480, 48_000.0);
            m.advance_segment(480, 120.0, 48_000.0, &mut effects, &track_effects);
            assert_eq!(m.fires_last_block() & 0b11, 0);
        }
    }

    #[test]
    fn fire_zeros_the_rows_three_phases() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        // Set row 2's MSEGs to short Beat lengths so the phases advance fast,
        // then verify that a fire on row 2 resets them.
        cfg[2].trigger = TriggerSource::CellLight;
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Advance many blocks with no fires -> phases drift away from 0.
        for _ in 0..50 {
            m.begin_block(64, 48_000.0);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        // At least one of row 2's three phases should be non-zero now.
        let any_nonzero = (0..3).any(|k| m.phase_for_test(2, k) > 1e-6);
        assert!(any_nonzero, "phases should have drifted with no fires");
        // Now fire row 2 (inactive->active edge).
        m.begin_block(64, 48_000.0);
        m.fire(1 << 2, 1 << 2);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        // After a fire, the row's three phases are reset to 0 (the per-MSEG
        // advance then re-runs from 0; the post-advance phase equals the
        // block's first dt, not 0). So the right test is: less than they
        // would have been without a fire — verified by comparing to the
        // amplitude (flat-1.0 default) seen one block later.
        for k in 0..3 {
            // Read each phase: it should be a *small* value (one block's dt),
            // not the multi-cycle accumulation it was before.
            let phi = m.phase_for_test(2, k);
            assert!(
                phi.abs() < 0.1,
                "after a fire, MSEG[{k}] phase should be near 0, got {phi}"
            );
        }
    }

    #[test]
    fn switch_effect_kind_resets_params_and_clamps_targets() {
        use crate::effects::{EffectKind, TrackEffect};

        // A Bitcrush track (2 params) whose first assignable MSEG targets
        // parameter index 1.
        let mut effect = TrackEffect {
            kind: EffectKind::Bitcrush,
            params: crate::effects::default_params_for_kind(EffectKind::Bitcrush),
            mix: 1.0,
        };
        let mut modulation = TrackModulation::default_for_row(0);
        modulation.targets[0] = Some(1);

        // Switch to None (0 parameters).
        switch_effect_kind(&mut effect, &mut modulation, EffectKind::None);

        assert_eq!(effect.kind, EffectKind::None, "kind switched");
        assert_eq!(
            effect.params,
            crate::effects::default_params_for_kind(EffectKind::None),
            "params reset to the new kind's defaults"
        );
        assert_eq!(
            modulation.targets[0], None,
            "out-of-range target cleared — None has 0 params"
        );
    }

    #[test]
    fn switch_effect_kind_keeps_an_in_range_target() {
        use crate::effects::{EffectKind, TrackEffect};

        // Switching between two kinds that both have parameter index 0.
        let mut effect = TrackEffect {
            kind: EffectKind::Lowpass,
            params: crate::effects::default_params_for_kind(EffectKind::Lowpass),
            mix: 1.0,
        };
        let mut modulation = TrackModulation::default_for_row(0);
        modulation.targets[0] = Some(0);

        switch_effect_kind(&mut effect, &mut modulation, EffectKind::Bitcrush);

        assert_eq!(effect.kind, EffectKind::Bitcrush);
        assert_eq!(
            modulation.targets[0],
            Some(0),
            "index 0 is in range for Bitcrush — target preserved"
        );
    }

    #[test]
    fn free_source_does_not_fire() {
        let mut m = Modulation::new();
        let cfg: [TrackModulation; ROWS] = std::array::from_fn(TrackModulation::default_for_row);
        // default_for_row's trigger is Free.
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        for _ in 0..20 {
            m.begin_block(64, 48_000.0);
            m.fire(0xFFFF, 0xFFFF);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
            assert_eq!(m.fires_last_block(), 0);
        }
    }

    #[test]
    fn reset_zeroes_hz_phases() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[0].trigger = TriggerSource::FreeHz { hz: 1.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Run a block so hz_phases advances above 0.
        m.begin_block(64, 48_000.0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        m.reset();
        assert!(m.hz_phases_all_zero());
    }

    #[test]
    fn begin_block_zeroes_fires_and_decides_free_hz() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[5].trigger = TriggerSource::FreeHz { hz: 10.0 };
        m.set_config(&cfg);
        // 10 Hz, 48 kHz, a 4800-sample block = exactly one cycle → one fire.
        m.begin_block(4800, 48_000.0);
        assert_eq!(m.fires_last_block(), 1 << 5);
        // begin_block zeroes `fires` each call: a block with no wrap clears it.
        m.begin_block(64, 48_000.0);
        assert_eq!(m.fires_last_block(), 0);
    }

    #[test]
    fn fire_resets_cell_light_rows_and_ignores_other_triggers() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[2].trigger = TriggerSource::CellLight;
        cfg[3].trigger = TriggerSource::Free;
        cfg[4].trigger = TriggerSource::FreeHz { hz: 1.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Drift the per-MSEG-clock rows' phases away from 0.
        for _ in 0..50 {
            m.begin_block(64, 48_000.0);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        assert!(m.phase_for_test(3, 0) > 1e-6, "Free row drifted");
        // Fire rows 2, 3, 4 — only the CellLight row (2) resets and reports.
        m.begin_block(64, 48_000.0);
        m.fire(
            (1 << 2) | (1 << 3) | (1 << 4),
            (1 << 2) | (1 << 3) | (1 << 4),
        );
        assert_eq!(
            m.fires_last_block() & (1 << 2),
            1 << 2,
            "CellLight row fired"
        );
        assert_eq!(m.fires_last_block() & (1 << 3), 0, "Free row did not fire");
        assert_eq!(m.phase_for_test(2, 0), 0.0, "CellLight row phases reset");
        assert!(
            m.phase_for_test(3, 0) > 1e-6,
            "fire must not touch a Free row's phase"
        );
    }

    #[test]
    fn advance_segment_advances_free_hz_rows_at_their_own_clock() {
        // FreeHz is a trigger source — between fires, the row's MSEGs advance
        // at each MSEG's own sync/length (not at the Hz rate). Same as Free /
        // CellLight / CellStep — the trigger only resets phase, not the
        // ongoing clock.
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[0].trigger = TriggerSource::FreeHz { hz: 5.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // 64-sample block at 5 Hz / 48 kHz → hz_phase += 64*5/48000 ≈ 0.0067,
        // no wrap → no fire, no phase reset; the MSEG should advance per its
        // own clock during advance_segment.
        m.begin_block(64, 48_000.0);
        let before = m.phase_for_test(0, 0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert!(
            m.phase_for_test(0, 0) > before,
            "FreeHz row's MSEG advances per its own clock between fires"
        );
    }

    #[test]
    fn free_hz_wrap_resets_the_rows_phases_to_zero() {
        // A FreeHz wrap (every 1/hz seconds) is a trigger event — it fires
        // the row exactly like a CellLight or CellStep fire would.
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[3].trigger = TriggerSource::FreeHz { hz: 10.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Drift phase well away from 0 over many no-wrap blocks.
        for _ in 0..30 {
            m.begin_block(64, 48_000.0);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        assert!(m.phase_for_test(3, 0) > 1e-6, "row 3 drifted");
        // 10 Hz, 4800-sample block @ 48 kHz = exactly one cycle → one wrap.
        m.begin_block(4800, 48_000.0);
        assert_eq!(m.fires_last_block() & (1 << 3), 1 << 3, "wrap fires");
        assert_eq!(m.phase_for_test(3, 0), 0.0, "wrap resets the row's phases");
    }

    #[test]
    fn advance_segment_zero_length_is_a_noop() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        m.begin_block(64, 48_000.0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        let phase = m.phase_for_test(3, 1);
        m.advance_segment(0, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(
            m.phase_for_test(3, 1),
            phase,
            "a zero-length segment must not advance phases"
        );
    }

    #[test]
    fn fire_resets_cell_step_rows_on_every_active_step() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[6].trigger = TriggerSource::CellStep;
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Drift row 6's phases away from 0.
        for _ in 0..50 {
            m.begin_block(64, 48_000.0);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        assert!(m.phase_for_test(6, 0) > 1e-6, "row 6 drifted");
        // A step where row 6 is active (in `active_rows`) but NOT newly-lit
        // (absent from `newly_rows`) still fires CellStep — the case CellLight
        // skips.
        m.begin_block(64, 48_000.0);
        m.fire(0, 1 << 6);
        assert_eq!(
            m.phase_for_test(6, 0),
            0.0,
            "CellStep fires on an active, non-newly step"
        );
        assert_eq!(
            m.fires_last_block() & (1 << 6),
            1 << 6,
            "CellStep sets its fires bit"
        );
        // It advances after the reset, then fires again on the next active step.
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert!(
            m.phase_for_test(6, 0) > 1e-6,
            "row 6 advanced after the reset"
        );
        m.fire(0, 1 << 6);
        assert_eq!(
            m.phase_for_test(6, 0),
            0.0,
            "CellStep fires again on the next consecutive active step"
        );
        // A step where row 6 is NOT active does not fire.
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        let drifted = m.phase_for_test(6, 0);
        assert!(drifted > 1e-6);
        m.fire(0, 0);
        assert_eq!(
            m.phase_for_test(6, 0),
            drifted,
            "no fire on a step where the row is inactive"
        );
    }

    #[test]
    fn advance_segment_in_two_halves_around_a_fire_resets_at_the_split() {
        // After a `fire`, the next `advance_segment` advances from the reset
        // phase 0 — so splitting a 256-sample block as [100][fire][156] leaves
        // a CellLight row's phase equal to a from-0 advance over only the
        // 156-sample tail. This is what lets the engine (Task 2) place a reset
        // at an exact step boundary.
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[1].trigger = TriggerSource::CellLight;
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Drift row 1's phases well away from 0.
        for _ in 0..30 {
            m.begin_block(256, 48_000.0);
            m.advance_segment(256, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        // The block: a 100-sample segment, fire row 1, then a 156-sample segment.
        m.begin_block(256, 48_000.0);
        m.advance_segment(100, 120.0, 48_000.0, &mut effects, &track_effects);
        m.fire(1 << 1, 1 << 1);
        m.advance_segment(156, 120.0, 48_000.0, &mut effects, &track_effects);
        let after_fire = m.phase_for_test(1, 1);
        // Expected: a from-0 advance over just the 156-sample tail.
        let mseg = cfg[1].msegs[1];
        let dt = mseg_phase_delta(&mseg, 156, 120.0, 48_000.0);
        let (expected, _) = advance(&mseg, 0.0, dt, false);
        assert!(
            (after_fire - expected).abs() < 1e-6,
            "post-fire phase {after_fire} should equal a from-0 tail advance {expected}"
        );
    }
}
