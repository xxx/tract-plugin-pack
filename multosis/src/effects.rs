//! The effect abstraction — Phase 2 Milestone 2a. A standardized `Effect`
//! trait, the `EffectKind` registry, enum-dispatch `EffectInstance`, two
//! effects (`SvfEffect`, `BitcrushEffect`), and the persisted per-track
//! `TrackEffect` config. Each track row carries its own effect instance.
//!
//! See `docs/superpowers/specs/2026-05-18-multosis-phase-2a-design.md`.

/// How a dial's normalised 0..1 position maps to its parameter value range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamScaling {
    /// `value = min + norm * (max - min)`; norm `= (value - min) / (max - min)`.
    Linear,
    /// `value = min * (max / min).powf(norm)`; norm `= log_(max/min)(value / min)`.
    /// Requires `min > 0` and `max > min`; degenerate ranges fall back to 0/min.
    Log,
}

/// How a parameter value renders as a string on the dial and how a typed
/// string parses back to a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamFormat {
    /// Fixed-decimals number, optional unit suffix. An empty unit prints no
    /// suffix; a non-empty unit prints with a single space separator.
    Number { decimals: u8, unit: &'static str },
    /// Auto Hz/kHz scaling: < 1 → `"0.05 Hz"` (2 dec); 1..1000 → `"80 Hz"`
    /// (0 dec); ≥ 1000 → `"2.0 kHz"` (1 dec).
    Hertz,
    /// Discrete selector: the value is rounded to an integer index in
    /// `0..labels.len()` and rendered as the matching label. Lets a continuous
    /// dial host a finite-option selector (e.g. the FM effect's Mode dial).
    /// `value_to_norm` / `norm_to_value` still work because the underlying
    /// spec is a normal `min`/`max`/`Linear` param — `Enum` only affects how
    /// the stored value renders and how typed text parses back.
    Enum { labels: &'static [&'static str] },
}

/// A modulatable parameter of an effect: its name and value range. Static per
/// effect kind; used by the 2b modulation engine and the 2c effect editor.
#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub scaling: ParamScaling,
    pub format: ParamFormat,
}

/// Map a parameter value to a `0..1` normalised dial position, given the
/// parameter's range and scaling. Clamps to `0..1`. Degenerate ranges
/// (`max <= min`, or `Log` with `min <= 0`) return `0.0`.
pub fn value_to_norm(value: f32, min: f32, max: f32, scaling: ParamScaling) -> f32 {
    if max <= min {
        return 0.0;
    }
    match scaling {
        ParamScaling::Linear => ((value - min) / (max - min)).clamp(0.0, 1.0),
        ParamScaling::Log => {
            if min <= 0.0 {
                return 0.0;
            }
            ((value / min).log(max / min)).clamp(0.0, 1.0)
        }
    }
}

/// Map a normalised dial position to a parameter value, given the
/// parameter's range and scaling. `norm` is clamped to `0..1`. Degenerate
/// ranges return `min`.
pub fn norm_to_value(norm: f32, min: f32, max: f32, scaling: ParamScaling) -> f32 {
    if max <= min {
        return min;
    }
    let n = norm.clamp(0.0, 1.0);
    match scaling {
        ParamScaling::Linear => min + n * (max - min),
        ParamScaling::Log => {
            if min <= 0.0 {
                return min;
            }
            min * (max / min).powf(n)
        }
    }
}

/// Format a parameter value as a display string.
pub fn format_value(value: f32, format: ParamFormat) -> String {
    match format {
        ParamFormat::Number { decimals, unit } => {
            let dec = decimals as usize;
            if unit.is_empty() {
                format!("{value:.dec$}")
            } else {
                format!("{value:.dec$} {unit}")
            }
        }
        ParamFormat::Hertz => {
            let v = value;
            if v.abs() < 1.0 {
                format!("{v:.2} Hz")
            } else if v.abs() < 1000.0 {
                format!("{v:.0} Hz")
            } else {
                format!("{:.1} kHz", v / 1000.0)
            }
        }
        ParamFormat::Enum { labels } => labels
            .get(enum_index(value, labels.len()))
            .copied()
            .unwrap_or("")
            .to_string(),
    }
}

/// Round a continuous parameter value to its enum-bucket index, clamped to
/// `0..labels_len`. Out-of-range or empty `labels_len` yields 0.
fn enum_index(value: f32, labels_len: usize) -> usize {
    if labels_len == 0 {
        return 0;
    }
    let rounded = value.round();
    if rounded < 0.0 {
        0
    } else if rounded as usize >= labels_len {
        labels_len - 1
    } else {
        rounded as usize
    }
}

/// Format a parameter value as a bare number with no unit suffix and no
/// kHz auto-scaling — for seeding the right-click text-entry buffer, where
/// the user expects to edit a plain number rather than re-type the unit.
/// Decimal precision matches `format_value` (Number uses its declared
/// decimals; Hertz uses 2 decimals below 1 Hz and 0 decimals above, but
/// stays in Hz units regardless of magnitude). `Enum` seeds the buffer
/// with the current label so the user can swap it for another label.
pub fn format_value_bare(value: f32, format: ParamFormat) -> String {
    match format {
        ParamFormat::Number { decimals, .. } => {
            let dec = decimals as usize;
            format!("{value:.dec$}")
        }
        ParamFormat::Hertz => {
            if value.abs() < 1.0 {
                format!("{value:.2}")
            } else {
                format!("{value:.0}")
            }
        }
        ParamFormat::Enum { labels } => labels
            .get(enum_index(value, labels.len()))
            .copied()
            .unwrap_or("")
            .to_string(),
    }
}

/// Parse a user-typed string back to a parameter value. Returns `None` on
/// empty input or an unparseable number. The consumer should clamp the
/// result into the parameter's `[min, max]` range.
pub fn parse_value(text: &str, format: ParamFormat) -> Option<f32> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    match format {
        ParamFormat::Number { unit, .. } => {
            // Strip the unit suffix (case-insensitive) if present.
            let body = if !unit.is_empty()
                && t.to_ascii_lowercase().ends_with(&unit.to_ascii_lowercase())
            {
                t[..t.len() - unit.len()].trim()
            } else {
                t
            };
            body.parse::<f32>().ok()
        }
        ParamFormat::Hertz => {
            let lower = t.to_ascii_lowercase();
            let (body, mult) = if lower.ends_with("khz") {
                (&t[..t.len() - 3], 1000.0)
            } else if lower.ends_with("hz") {
                (&t[..t.len() - 2], 1.0)
            } else if lower.ends_with('k') {
                (&t[..t.len() - 1], 1000.0)
            } else {
                (t, 1.0)
            };
            body.trim().parse::<f32>().ok().map(|v| v * mult)
        }
        ParamFormat::Enum { labels } => {
            // First try a case-insensitive label match — the user typed e.g.
            // "Modulator". Failing that, fall back to a numeric index so a
            // copy/paste of the raw value still works.
            let lower = t.to_ascii_lowercase();
            for (i, label) in labels.iter().enumerate() {
                if label.to_ascii_lowercase() == lower {
                    return Some(i as f32);
                }
            }
            t.parse::<f32>().ok()
        }
    }
}

/// The standardized audio-effect contract. Implemented by each effect struct;
/// dispatched allocation-free through `EffectInstance` (no `dyn`). Audio-thread
/// methods (`process_sample`, `set_param`, `reset`) must not allocate.
pub trait Effect {
    /// Process one stereo sample, returning the wet `(left, right)`. DSP state
    /// persists across calls so the effect does not click on reactivation.
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32);

    /// Recompute sample-rate-dependent coefficients.
    fn set_sample_rate(&mut self, sample_rate: f32);

    /// Clear all DSP state.
    fn reset(&mut self);

    /// The effect's modulatable parameters, in `set_param` index order.
    fn parameters(&self) -> &'static [ParamSpec];

    /// Set parameter `index` to `value` (clamped to the spec's range). An
    /// out-of-range `index` is ignored.
    fn set_param(&mut self, index: usize, value: f32);

    /// Called by the engine once per process block before the per-sample
    /// loop. Effects that synchronize to host tempo (beat-synced delay,
    /// future LFO-style effects) override this to cache the BPM for use
    /// during `process_sample`. The default implementation ignores it.
    fn set_bpm(&mut self, _bpm: f32) {}

    /// `true` when parameter `index` is currently inactive — the editor
    /// renders its dial dimmed (muted colour) instead of accented, but
    /// it stays controllable. Used e.g. by Delay to grey out the Free
    /// dial when a tempo-synced subdivision is selected from the Time
    /// dropdown. The default returns `false` for every index.
    fn param_dimmed(&self, _index: usize) -> bool {
        false
    }

    /// The latency in samples this effect adds to its input. Reported to
    /// the host (via the engine's chain latency sum) so plugin delay
    /// compensation keeps the multosis output aligned with the rest of
    /// the project. Zero-latency effects (SVF, Bitcrush, Delay, Phaser,
    /// FM, …) leave the default; Warp Zone overrides to its FFT size.
    fn latency_samples(&self) -> usize {
        0
    }
}

/// A multimode state-variable filter — `n` cascaded TPT-SVF stages, each
/// contributing 2 poles (12 dB/oct). The `Poles` param picks the cascade
/// length from {2, 4, 6, 8} poles (12 / 24 / 36 / 48 dB/oct slopes). The
/// `Type` param selects which SVF output tap each stage emits: lowpass
/// (LP), bandpass (BP), or highpass (HP).
///
/// Resonance is applied to the **last** stage only; earlier stages run
/// Butterworth (Q = 0.707, no peak). If every stage shared the user's Q
/// the resonance peak at cutoff would compound by `Q^stages` — at 8
/// poles even Q = 2 produces a 16× peak. Keeping the cascade
/// Butterworth except for the final stage makes the Resonance knob
/// mean roughly the same peak height across pole counts.
///
/// Stage state is per-cascade-position; only the first `stages_count()`
/// stages are touched on the audio thread. State is preserved across
/// param changes so a cutoff or resonance sweep doesn't click.
pub struct SvfEffect {
    cutoff: f32,
    resonance: f32,
    /// 0..3 selector index into [2, 4, 6, 8] poles. Stored as f32 so the
    /// existing Enum-format dropdown machinery handles it identically to
    /// FM Mode / FM Topology.
    poles_idx: f32,
    /// 0..2 selector index into [LP, BP, HP]. Stored as f32 like the
    /// other Enum-format params.
    type_idx: f32,
    sample_rate: f32,
    /// Butterworth (Q = 0.707, no peak) coefficients — used by every
    /// stage except the last. Tuple order is `(a1, a2, a3, k)` where
    /// `k = 1/Q` (needed for the HP tap `v3 − k · v1`).
    butter: (f32, f32, f32, f32),
    /// User-resonance coefficients — used by the LAST cascade stage only.
    /// At pole count = 2 (one stage), this is also the only set in play.
    res: (f32, f32, f32, f32),
    stages_ic1: [[f32; 2]; Self::MAX_STAGES],
    stages_ic2: [[f32; 2]; Self::MAX_STAGES],
}

/// Poles-dropdown label list. Order matters: `value.round() as usize`
/// indexes it (0 → "2", 1 → "4", 2 → "6", 3 → "8" poles).
const SVF_POLES_LABELS: &[&str] = &["2", "4", "6", "8"];

/// Type-dropdown label list. Order matters: 0 → LP, 1 → BP, 2 → HP.
const SVF_TYPE_LABELS: &[&str] = &["Lowpass", "Bandpass", "Highpass"];

const SVF_TYPE_LP: usize = 0;
const SVF_TYPE_BP: usize = 1;
// Highpass uses the `_` arm in `svf_step` — `set_param` clamps the index
// to `0..=2`, so the only remaining case after LP and BP is HP.

impl SvfEffect {
    const MAX_STAGES: usize = 4;

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Cutoff",
            min: 20.0,
            max: 20_000.0,
            default: 2_000.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Resonance",
            min: 0.0,
            max: 1.0,
            default: 0.1,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "",
            },
        },
        ParamSpec {
            name: "Type",
            min: 0.0,
            max: (SVF_TYPE_LABELS.len() - 1) as f32,
            // Index 0 → Lowpass.
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: SVF_TYPE_LABELS,
            },
        },
        ParamSpec {
            name: "Poles",
            min: 0.0,
            max: (SVF_POLES_LABELS.len() - 1) as f32,
            // Index 0 → 2 poles (12 dB/oct) — the original behaviour.
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: SVF_POLES_LABELS,
            },
        },
    ];

    /// An `SvfEffect` at its default parameters; call `set_sample_rate`
    /// before processing.
    pub fn new() -> Self {
        let mut svf = Self {
            cutoff: Self::PARAMS[0].default,
            resonance: Self::PARAMS[1].default,
            type_idx: Self::PARAMS[2].default,
            poles_idx: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            butter: (0.0, 0.0, 0.0, 0.0),
            res: (0.0, 0.0, 0.0, 0.0),
            stages_ic1: [[0.0; 2]; Self::MAX_STAGES],
            stages_ic2: [[0.0; 2]; Self::MAX_STAGES],
        };
        svf.recompute();
        svf
    }

    /// Number of cascaded SVF stages: index 0 → 1 stage (2 poles), …,
    /// index 3 → 4 stages (8 poles). Always at least 1.
    fn stages_count(&self) -> usize {
        (self.poles_idx.round() as usize + 1).min(Self::MAX_STAGES)
    }

    /// Build a `(a1, a2, a3, k)` TPT-SVF coefficient tuple for the given
    /// Q. Q < 0.5 critically damps; Q = 0.707 is Butterworth (flat,
    /// 3 dB at cutoff); higher Q peaks the response at cutoff. `k = 1/Q`
    /// is needed for the HP output tap (`v3 − k · v1`).
    fn svf_coefs(g: f32, q: f32) -> (f32, f32, f32, f32) {
        let k = 1.0 / q.max(0.0001);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        (a1, a2, a3, k)
    }

    /// Recompute both coefficient sets from cutoff / resonance / SR.
    fn recompute(&mut self) {
        let sr = self.sample_rate.max(1.0);
        let fc = self.cutoff.clamp(20.0, sr * 0.49);
        let g = (std::f32::consts::PI * fc / sr).tan();
        let q_res = 0.5 + self.resonance.clamp(0.0, 1.0) * 9.5;
        let q_butter = std::f32::consts::FRAC_1_SQRT_2;
        self.butter = Self::svf_coefs(g, q_butter);
        self.res = Self::svf_coefs(g, q_res);
    }

    /// One TPT-SVF integrator step for one (stage, channel). Returns the
    /// output of the tap chosen by `type_idx`:
    ///
    /// * `0` (LP): `v2` — lowpass output.
    /// * `1` (BP): `v1` — bandpass output.
    /// * `2` (HP): `v3 − k · v1` — highpass output.
    ///
    /// `coefs` is the precomputed `(a1, a2, a3, k)` tuple — picks
    /// Butterworth or resonance per stage.
    #[inline]
    fn svf_step(
        &mut self,
        x: f32,
        stage: usize,
        ch: usize,
        coefs: (f32, f32, f32, f32),
        type_idx: usize,
    ) -> f32 {
        let (a1, a2, a3, k) = coefs;
        let ic1 = self.stages_ic1[stage][ch];
        let ic2 = self.stages_ic2[stage][ch];
        let v3 = x - ic2;
        let v1 = a1 * ic1 + a2 * v3;
        let v2 = ic2 + a2 * ic1 + a3 * v3;
        self.stages_ic1[stage][ch] = 2.0 * v1 - ic1;
        self.stages_ic2[stage][ch] = 2.0 * v2 - ic2;
        match type_idx {
            SVF_TYPE_LP => v2,
            SVF_TYPE_BP => v1,
            _ => v3 - k * v1, // HP
        }
    }
}

impl Default for SvfEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for SvfEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let stages = self.stages_count();
        let type_idx = self.type_idx.round() as usize;
        let mut l = left;
        let mut r = right;
        for stage in 0..stages {
            // Last stage carries the resonance peak; earlier stages run
            // Butterworth so the peak doesn't compound across the cascade.
            let coefs = if stage + 1 == stages {
                self.res
            } else {
                self.butter
            };
            l = self.svf_step(l, stage, 0, coefs, type_idx);
            r = self.svf_step(r, stage, 1, coefs, type_idx);
        }
        (l, r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute();
    }

    fn reset(&mut self) {
        self.stages_ic1 = [[0.0; 2]; Self::MAX_STAGES];
        self.stages_ic2 = [[0.0; 2]; Self::MAX_STAGES];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.cutoff = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.resonance = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            // Type: round to the nearest enum index. Selects which SVF
            // output tap (LP / BP / HP) each stage emits.
            2 => {
                let max_idx = (SVF_TYPE_LABELS.len() - 1) as f32;
                self.type_idx = value.round().clamp(0.0, max_idx);
                return;
            }
            // Poles: round to the nearest enum index. Doesn't change
            // coefficients — only the cascade depth.
            3 => {
                let max_idx = (SVF_POLES_LABELS.len() - 1) as f32;
                self.poles_idx = value.round().clamp(0.0, max_idx);
                return;
            }
            _ => return,
        }
        self.recompute();
    }
}

/// Bit-depth reduction plus sample-rate reduction (sample-and-hold).
pub struct BitcrushEffect {
    bit_depth: f32,
    rate_reduction: f32,
    held: [f32; 2],
    phase: [f32; 2],
}

impl BitcrushEffect {
    const PARAMS: [ParamSpec; 2] = [
        ParamSpec {
            name: "Bit Depth",
            min: 1.0,
            max: 16.0,
            default: 16.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "bits",
            },
        },
        ParamSpec {
            name: "Rate Reduction",
            min: 1.0,
            max: 50.0,
            default: 1.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "x",
            },
        },
    ];

    /// A `BitcrushEffect` at its default (near-clean) parameters.
    pub fn new() -> Self {
        Self {
            bit_depth: Self::PARAMS[0].default,
            rate_reduction: Self::PARAMS[1].default,
            held: [0.0; 2],
            phase: [Self::PARAMS[1].default; 2],
        }
    }

    /// Quantize `x` to the current bit depth.
    fn quantize(&self, x: f32) -> f32 {
        let levels = 2.0_f32.powf(self.bit_depth);
        let step = 2.0 / levels;
        (x / step).round() * step
    }

    /// One channel's sample-and-hold + quantization step.
    fn crush_step(&mut self, x: f32, ch: usize) -> f32 {
        self.phase[ch] += 1.0;
        if self.phase[ch] >= self.rate_reduction {
            self.phase[ch] -= self.rate_reduction;
            self.held[ch] = self.quantize(x);
        }
        self.held[ch]
    }
}

impl Default for BitcrushEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for BitcrushEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        (self.crush_step(left, 0), self.crush_step(right, 1))
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {}

    fn reset(&mut self) {
        self.held = [0.0; 2];
        self.phase = [self.rate_reduction; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.bit_depth = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.rate_reduction = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            _ => {}
        }
    }
}

/// Frequency-modulation effect. The input plays one of two roles, chosen by
/// the `Mode` param:
///
/// * **Carrier** — the input is treated as the audio being FMed. An internal
///   sine at the `Freq` rate modulates a delay line's length around a fixed
///   5 ms centre; the output is the input read out at that varying delay
///   (vibrato at slow `Freq`, sideband-rich FM at audio-rate `Freq`). The
///   delay-line approach is the only practical way to "FM" an arbitrary
///   input without an analytic-signal Hilbert transform, since instantaneous
///   frequency is the derivative of phase.
///
/// * **Modulator** — the input modulates an internal sine carrier. The
///   carrier phase advances at `Freq + depth · input_sample` per sample
///   (through-zero phase modulation). Each channel runs its own carrier so
///   stereo input produces stereo output.
///
/// `Feedback` is DX7-style operator self-modulation in both modes: the
/// previous output sample mixes back into the rotation phase, enriching
/// the timbre (sine → sawtooth-ish at high settings).
///
/// **Modulator-mode input gating**: the internal carrier sine plays only
/// when there's input to modulate. An envelope follower tracks the input
/// level (fast attack, slow release) and scales the carrier's amplitude,
/// so a silent input — including the host having stopped the transport —
/// yields a silent output instead of a continuously-ringing bare carrier.
/// Carrier mode is intrinsically input-driven (the analytic signal of
/// silence is silent), so no gate is needed there.
///
/// **Unified architecture**: both modes go through the same PM/FM rotation
/// math; only the role assignment differs:
///
/// * **Modulator**: the carrier is an internal sine whose phase is rotated
///   by the input (modulator) plus self-feedback.
/// * **Carrier**: the carrier is the **input audio**, converted to an
///   analytic signal via a Hilbert FIR (so its phase is well-defined),
///   then rotated by the internal sine modulator plus self-feedback. The
///   Hilbert filter adds ≈ 32 samples (~0.7 ms at 48 kHz) of latency only
///   while this mode is selected.
pub struct FmEffect {
    // Stored parameters.
    mode: f32, // 0 = Carrier, 1 = Modulator (rounded on set_param).
    freq_hz: f32,
    depth_pct: f32,    // 0..100, divided by 100 inside `process_sample`.
    feedback_pct: f32, // 0..100, divided by 100 inside `process_sample`.
    /// 0 = PM (phase offset at output), 1 = true FM (added to increment).
    topology: f32,
    sample_rate: f32,

    // Internal oscillator phases (0..1).
    carrier_phase_l: f32,
    carrier_phase_r: f32,
    mod_phase: f32,

    /// FM-topology theta accumulator for Carrier mode (in cycles, wraps
    /// modulo 1 every sample). PM mode doesn't use this.
    fm_theta_accum: f32,

    // One-sample feedback memory.
    prev_out_l: f32,
    prev_out_r: f32,

    // Modulator-mode input gate: a one-pole envelope follower over
    // `|left| + |right|`, used as the carrier amplitude. Coefficients
    // are cached from `set_sample_rate`.
    input_env: f32,
    env_attack_coef: f32,
    env_release_coef: f32,

    /// Carrier-mode analytic-signal extractors (Hilbert FIR + delay-matched
    /// real branch). Each channel runs its own — together they convert the
    /// raw input into a `(real, imag)` pair that the rotation math operates
    /// on. Allocated once in `new`; allocation-free thereafter.
    analytic_l: tract_dsp::hilbert::AnalyticSignal,
    analytic_r: tract_dsp::hilbert::AnalyticSignal,
}

/// Mode-dial label list. Order matters: `value.round() as usize` indexes it.
const FM_MODE_LABELS: &[&str] = &["Carrier", "Modulator"];

/// Topology-dial label list (Modulator-mode operator topology). PM uses the
/// previous output as a phase OFFSET at output time (no integration → no
/// drift, sounds like a DX7 operator). True FM adds it to the phase
/// INCREMENT — input still bends the carrier's pitch (which PM only does at
/// audio rates), but self-feedback integrates and can wander at high
/// feedback settings.
const FM_TOPOLOGY_LABELS: &[&str] = &["PM", "FM"];

impl FmEffect {
    /// Hilbert FIR length for the Carrier-mode analytic-signal extractor.
    /// 65 gives ~32 samples (~0.7 ms at 48 kHz) of group delay and a clean
    /// passband above ~1 kHz.
    const HILBERT_LEN: usize = 65;
    /// Modulator-mode input-gate time constants. Fast attack catches
    /// transients without clipping the carrier's onset; slow release lets
    /// the carrier ring out smoothly across short silences.
    const ENV_ATTACK_MS: f32 = 1.0;
    const ENV_RELEASE_MS: f32 = 100.0;

    // Order matters: `targets[0]` (the assignable-MSEG-1 default) is `Some(0)`,
    // so the first param is what fresh tracks modulate. Freq is the natural
    // first audible-modulation target; Mode and Topology are Enum-format
    // selectors the editor renders as dropdowns rather than dials.
    const PARAMS: [ParamSpec; 5] = [
        ParamSpec {
            name: "Freq",
            min: 20.0,
            max: 20_000.0,
            default: 100.0,
            // Log-scaled dial across exactly the audio band — three even
            // decades from 20 Hz to 20 kHz, so each decade takes one-third
            // of the arc. Sub-audio vibrato is reachable by modulating
            // Freq via an MSEG rather than dialing it in directly.
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Depth",
            min: 0.0,
            max: 100.0,
            default: 25.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Feedback",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Mode",
            min: 0.0,
            max: 1.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: FM_MODE_LABELS,
            },
        },
        ParamSpec {
            name: "Topology",
            min: 0.0,
            max: 1.0,
            default: 0.0, // PM by default — drift-free; DX7-style.
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: FM_TOPOLOGY_LABELS,
            },
        },
    ];

    /// An `FmEffect` at its default parameters; call `set_sample_rate`
    /// before processing.
    pub fn new() -> Self {
        let mut fm = Self {
            freq_hz: Self::PARAMS[0].default,
            depth_pct: Self::PARAMS[1].default,
            feedback_pct: Self::PARAMS[2].default,
            mode: Self::PARAMS[3].default,
            topology: Self::PARAMS[4].default,
            sample_rate: 48_000.0,
            carrier_phase_l: 0.0,
            carrier_phase_r: 0.0,
            mod_phase: 0.0,
            fm_theta_accum: 0.0,
            prev_out_l: 0.0,
            prev_out_r: 0.0,
            input_env: 0.0,
            env_attack_coef: 0.0,
            env_release_coef: 0.0,
            analytic_l: tract_dsp::hilbert::AnalyticSignal::new(Self::HILBERT_LEN),
            analytic_r: tract_dsp::hilbert::AnalyticSignal::new(Self::HILBERT_LEN),
        };
        fm.recompute_env_coefs();
        fm
    }

    /// Re-derive the input-gate envelope coefficients from the cached
    /// `sample_rate` and `ENV_*_MS` time constants. Cheap (two `exp`); only
    /// called from `new` and `set_sample_rate`.
    fn recompute_env_coefs(&mut self) {
        let sr = self.sample_rate.max(1.0);
        self.env_attack_coef = (-1.0 / (Self::ENV_ATTACK_MS * 0.001 * sr)).exp();
        self.env_release_coef = (-1.0 / (Self::ENV_RELEASE_MS * 0.001 * sr)).exp();
    }
}

impl Default for FmEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for FmEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // ±π rad at feedback = 1 — DX7-style operator self-modulation cap.
        const FB_PHASE_SCALE: f32 = 0.5;
        let two_pi = std::f32::consts::TAU;
        let sr = self.sample_rate.max(1.0);
        let phase_inc = self.freq_hz / sr;
        let depth = self.depth_pct * 0.01;
        let feedback = self.feedback_pct * 0.01;

        if self.mode < 0.5 {
            // Carrier mode: the INPUT plays the role of the carrier. Convert
            // it to its analytic signal `(real, imag)` via a Hilbert FIR so
            // we can phase-rotate it the same way Modulator mode rotates its
            // internal sine. The internal sine LFO acts as the modulator.
            //
            // PM: θ(t) = depth · sin(mod_phase) — instantaneous phase
            //     offset; the input's spectrum is rotated by ±depth cycles.
            // FM: θ(t) = ∫ depth · sin(mod_phase) dτ — accumulated; the
            //     input's instantaneous frequency is shifted by
            //     depth · sin(mod_phase) · sr Hz.
            //
            // Feedback adds the previous output back into θ (DX7-style
            // self-modulation), enriching the timbre.
            //
            // No input gate needed — the analytic signal of silence is
            // silence, so a silent input naturally yields a silent output.
            let mod_sine = (self.mod_phase * two_pi).sin();
            self.mod_phase = (self.mod_phase + phase_inc).rem_euclid(1.0);
            let theta_mod = if self.topology < 0.5 {
                // PM: instantaneous rotation = depth · modulator.
                depth * mod_sine
            } else {
                // FM: accumulate depth · modulator into the rotation phase.
                self.fm_theta_accum = (self.fm_theta_accum + depth * mod_sine).rem_euclid(1.0);
                self.fm_theta_accum
            };
            let theta_l = theta_mod + feedback * FB_PHASE_SCALE * self.prev_out_l;
            let theta_r = theta_mod + feedback * FB_PHASE_SCALE * self.prev_out_r;
            let (real_l, imag_l) = self.analytic_l.process(left);
            let (real_r, imag_r) = self.analytic_r.process(right);
            let (cos_l, sin_l) = {
                let a = theta_l * two_pi;
                (a.cos(), a.sin())
            };
            let (cos_r, sin_r) = {
                let a = theta_r * two_pi;
                (a.cos(), a.sin())
            };
            let out_l = real_l * cos_l - imag_l * sin_l;
            let out_r = real_r * cos_r - imag_r * sin_r;
            self.prev_out_l = out_l;
            self.prev_out_r = out_r;
            (out_l, out_r)
        } else {
            // Modulator mode: the internal sine is the carrier; the input
            // is the modulator. Topology picks PM vs FM; the input-gate
            // envelope follower scales the output so silent input → silent
            // output (avoids the bare carrier ringing when the transport
            // is stopped).
            let target_env = (left.abs() + right.abs()) * 0.5;
            let env_coef = if target_env > self.input_env {
                self.env_attack_coef
            } else {
                self.env_release_coef
            };
            self.input_env = target_env + (self.input_env - target_env) * env_coef;
            let gate = self.input_env.min(1.0);

            let (sin_l, sin_r) = if self.topology < 0.5 {
                // PM: input + feedback applied as a phase OFFSET at output.
                self.carrier_phase_l = (self.carrier_phase_l + phase_inc).rem_euclid(1.0);
                self.carrier_phase_r = (self.carrier_phase_r + phase_inc).rem_euclid(1.0);
                let pm_l = depth * left + feedback * FB_PHASE_SCALE * self.prev_out_l;
                let pm_r = depth * right + feedback * FB_PHASE_SCALE * self.prev_out_r;
                (
                    ((self.carrier_phase_l + pm_l) * two_pi).sin(),
                    ((self.carrier_phase_r + pm_r) * two_pi).sin(),
                )
            } else {
                // FM: input + feedback applied as a phase INCREMENT —
                // the carrier's instantaneous frequency tracks the
                // modulator in cycles/sample.
                let inc_l = phase_inc + depth * left + feedback * FB_PHASE_SCALE * self.prev_out_l;
                let inc_r = phase_inc + depth * right + feedback * FB_PHASE_SCALE * self.prev_out_r;
                self.carrier_phase_l = (self.carrier_phase_l + inc_l).rem_euclid(1.0);
                self.carrier_phase_r = (self.carrier_phase_r + inc_r).rem_euclid(1.0);
                (
                    (self.carrier_phase_l * two_pi).sin(),
                    (self.carrier_phase_r * two_pi).sin(),
                )
            };
            let out_l = gate * sin_l;
            let out_r = gate * sin_r;
            self.prev_out_l = out_l;
            self.prev_out_r = out_r;
            (out_l, out_r)
        }
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute_env_coefs();
    }

    fn reset(&mut self) {
        self.carrier_phase_l = 0.0;
        self.carrier_phase_r = 0.0;
        self.mod_phase = 0.0;
        self.fm_theta_accum = 0.0;
        self.prev_out_l = 0.0;
        self.prev_out_r = 0.0;
        self.input_env = 0.0;
        self.analytic_l.reset();
        self.analytic_r.reset();
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.freq_hz = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.depth_pct = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.feedback_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            // Mode: round to the nearest enum index (0 = Carrier, 1 = Modulator).
            3 => {
                self.mode = if value >= 0.5 { 1.0 } else { 0.0 };
            }
            // Topology: round to the nearest enum index (0 = PM, 1 = FM).
            4 => {
                self.topology = if value >= 0.5 { 1.0 } else { 0.0 };
            }
            _ => {}
        }
    }
}

/// Tempo-syncable delay with feedback and ducking.
///
/// **Time** is a single Enum-format dropdown spanning every musical
/// subdivision (with dotted variants) plus a `Free` slot. When `Free` is
/// selected, the `Free` ms dial sets the delay time directly; otherwise
/// the subdivision is converted to samples using the host's current BPM
/// (cached via `set_bpm`).
///
/// **Feedback** scales the delay-line output back into its input
/// (chorus-style feedback, capped at 0.95 to prevent runaway).
///
/// **Duck** envelope-follows the input level and attenuates the delayed
/// output proportionally — at full Duck, loud input fully silences the
/// delay, leaving the dry signal clear; the delay swells in as the
/// input drops. Same mechanism Bitwig Delay+ uses.
///
/// Output is **additive**: `out = dry + delayed · duck_factor`. The
/// per-row Mix then controls how much of the delayed signal mixes in,
/// matching standard delay-plugin semantics where the dry signal
/// always passes through.
pub struct DelayEffect {
    // Stored parameters.
    time_idx: f32,     // 0..14: subdivisions 0..13 + Free at 14.
    free_ms: f32,      // Used only when time_idx selects Free.
    feedback_pct: f32, // 0..100.
    duck_pct: f32,     // 0..100; 0 = no ducking, 100 = aggressive duck.
    sample_rate: f32,
    bpm: f32,
    /// Per-channel circular delay buffers, sized for the worst-case
    /// 2-second delay at 192 kHz. Allocated once in `new`; reads and
    /// writes are wrap-around with linear-interp fractional reads.
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    write_idx: usize,
    /// Duck envelope follower over the input (rectified-stereo). Cached
    /// coefficients are recomputed in `set_sample_rate`.
    duck_env: f32,
    duck_attack_coef: f32,
    duck_release_coef: f32,
}

/// Time-dropdown labels — order matches `delay_time_beats()`. The last
/// entry, "Free", is special: it tells `process_sample` to use the
/// `free_ms` dial instead of computing samples from BPM.
const DELAY_TIME_LABELS: &[&str] = &[
    "1/64", "1/64.", "1/32", "1/32.", "1/16", "1/16.", "1/8", "1/8.", "1/4", "1/4.", "1/2", "1/2.",
    "1/1", "1/1.", "Free",
];

/// Beat count for time-dropdown index `idx` in the sync slots (0..14).
/// Sub-1 beat for shorter subdivisions, > 1 beat for half/whole notes.
/// Returns `None` for the Free slot (index 14) — the caller switches
/// over to the `Free` dial in that case.
fn delay_time_beats(idx: usize) -> Option<f32> {
    // Whole note = 4 beats in 4/4 time; each smaller subdivision halves.
    // Dotted = 1.5 ×. Index pairs are (straight, dotted) per row.
    let beats = match idx {
        0 => 4.0 / 64.0,       // 1/64
        1 => 1.5 * 4.0 / 64.0, // 1/64.
        2 => 4.0 / 32.0,       // 1/32
        3 => 1.5 * 4.0 / 32.0, // 1/32.
        4 => 4.0 / 16.0,       // 1/16
        5 => 1.5 * 4.0 / 16.0, // 1/16.
        6 => 0.5,              // 1/8
        7 => 0.75,             // 1/8.
        8 => 1.0,              // 1/4
        9 => 1.5,              // 1/4.
        10 => 2.0,             // 1/2
        11 => 3.0,             // 1/2.
        12 => 4.0,             // 1/1 (whole)
        13 => 6.0,             // 1/1.
        _ => return None,      // Free
    };
    Some(beats)
}

impl DelayEffect {
    /// Worst-case sample count = 2 seconds × 192 kHz.
    const BUF_LEN: usize = (2.0 * 192_000.0) as usize;
    const MAX_DELAY_MS: f32 = 2_000.0;
    const MIN_DELAY_MS: f32 = 1.0;
    /// Duck envelope follower time constants — same shape as the FM
    /// effect's input gate: fast attack so transients duck the delay
    /// immediately, slow release so the delay swells back smoothly.
    const DUCK_ATTACK_MS: f32 = 5.0;
    const DUCK_RELEASE_MS: f32 = 150.0;
    const FEEDBACK_CAP: f32 = 0.95;

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Free",
            min: Self::MIN_DELAY_MS,
            max: Self::MAX_DELAY_MS,
            default: 250.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "ms",
            },
        },
        ParamSpec {
            name: "Time",
            min: 0.0,
            max: (DELAY_TIME_LABELS.len() - 1) as f32,
            // Default to the trailing `Free` slot so a fresh delay
            // uses the (continuous) Free dial directly; the user can
            // switch to a tempo-synced subdivision via the dropdown.
            // This also makes the default MSEG target (`Some(0)` →
            // slot 0 = Free) modulate a useful continuous parameter
            // rather than rhythmically switching subdivisions.
            default: (DELAY_TIME_LABELS.len() - 1) as f32,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: DELAY_TIME_LABELS,
            },
        },
        ParamSpec {
            name: "Feedback",
            min: 0.0,
            max: 100.0,
            default: 30.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Duck",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        let mut d = Self {
            free_ms: Self::PARAMS[0].default,
            time_idx: Self::PARAMS[1].default,
            feedback_pct: Self::PARAMS[2].default,
            duck_pct: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            bpm: 120.0,
            delay_l: vec![0.0; Self::BUF_LEN],
            delay_r: vec![0.0; Self::BUF_LEN],
            write_idx: 0,
            duck_env: 0.0,
            duck_attack_coef: 0.0,
            duck_release_coef: 0.0,
        };
        d.recompute_duck_coefs();
        d
    }

    fn recompute_duck_coefs(&mut self) {
        let sr = self.sample_rate.max(1.0);
        self.duck_attack_coef = (-1.0 / (Self::DUCK_ATTACK_MS * 0.001 * sr)).exp();
        self.duck_release_coef = (-1.0 / (Self::DUCK_RELEASE_MS * 0.001 * sr)).exp();
    }

    /// Current delay time in samples, given the cached BPM + sample rate.
    /// Sync subdivisions consult `delay_time_beats`; the Free slot uses
    /// the `Free` ms dial. Clamped to `[1, BUF_LEN − 2]` so the read tap
    /// never wraps onto the write head (which would feed back the
    /// just-written sample = unstable).
    fn delay_samples(&self) -> f32 {
        let idx = self.time_idx.round() as usize;
        let raw_samples = if let Some(beats) = delay_time_beats(idx) {
            let sec_per_beat = 60.0 / self.bpm.max(1.0);
            beats * sec_per_beat * self.sample_rate
        } else {
            self.free_ms * 0.001 * self.sample_rate
        };
        raw_samples.clamp(1.0, (Self::BUF_LEN - 2) as f32)
    }

    /// Linear-interp delay read: returns the sample `delay_samples`
    /// behind the current write head from `buf`.
    fn read_delay(buf: &[f32], write_idx: usize, delay_samples: f32) -> f32 {
        let n = buf.len();
        let read = (write_idx as f32 + n as f32 - delay_samples).rem_euclid(n as f32);
        let i0 = (read.floor() as usize) % n;
        let i1 = (i0 + 1) % n;
        let frac = read - read.floor();
        buf[i0] * (1.0 - frac) + buf[i1] * frac
    }
}

impl Default for DelayEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for DelayEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let delay = self.delay_samples();
        let fb = (self.feedback_pct * 0.01).clamp(0.0, Self::FEEDBACK_CAP);
        let duck_amount = (self.duck_pct * 0.01).clamp(0.0, 1.0);

        // Read the delay tap BEFORE writing, so feedback uses the past
        // value (not the one we're about to write).
        let tap_l = Self::read_delay(&self.delay_l, self.write_idx, delay);
        let tap_r = Self::read_delay(&self.delay_r, self.write_idx, delay);

        // Update the duck envelope from the current input level.
        let target_env = (left.abs() + right.abs()) * 0.5;
        let coef = if target_env > self.duck_env {
            self.duck_attack_coef
        } else {
            self.duck_release_coef
        };
        self.duck_env = target_env + (self.duck_env - target_env) * coef;
        // `1 - duck_amount · env` keeps the delay at full level when env
        // is silent, attenuates by up to `duck_amount` when env is at
        // peak. Clamped to ≥ 0 so a hot input doesn't invert.
        let duck_factor = (1.0 - duck_amount * self.duck_env).max(0.0);

        // Write input + feedback × tap to the buffer; the next call's
        // tap reads from here.
        self.delay_l[self.write_idx] = left + fb * tap_l;
        self.delay_r[self.write_idx] = right + fb * tap_r;
        self.write_idx = (self.write_idx + 1) % self.delay_l.len();

        // Additive output: dry + ducked delay tap.
        let out_l = left + duck_factor * tap_l;
        let out_r = right + duck_factor * tap_r;
        (out_l, out_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute_duck_coefs();
    }

    fn reset(&mut self) {
        for s in self.delay_l.iter_mut() {
            *s = 0.0;
        }
        for s in self.delay_r.iter_mut() {
            *s = 0.0;
        }
        self.write_idx = 0;
        self.duck_env = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.free_ms = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => {
                let max_idx = (DELAY_TIME_LABELS.len() - 1) as f32;
                self.time_idx = value.round().clamp(0.0, max_idx);
            }
            2 => self.feedback_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.duck_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => {}
        }
    }

    fn set_bpm(&mut self, bpm: f32) {
        self.bpm = bpm.max(1.0);
    }

    /// The Free dial (slot 0) is dimmed whenever the Time dropdown points
    /// at a tempo-synced subdivision — Time then drives `delay_samples()`
    /// directly and the Free value is unused. The Free slot itself (the
    /// last `time_idx` value) re-enables it. Every other slot is always
    /// active.
    fn param_dimmed(&self, index: usize) -> bool {
        if index != 0 {
            return false;
        }
        let idx = self.time_idx.round() as usize;
        delay_time_beats(idx).is_some()
    }
}

/// A vintage-character 4-stage all-pass phaser. Four 1st-order all-pass
/// sections cascade per channel; the cascade output feeds back to the
/// cascade input through a 1-sample delay, and the per-channel all-pass
/// centre frequency can be offset for stereo width.
///
/// No internal LFO — matches the multosis pattern that DSP is static and
/// motion comes from MSEGs. The user routes an MSEG to `Center` for the
/// classic sweep.
///
/// `process_sample` returns the additive phaser sound (`dry + cascade`)
/// because the comb-filter notches that make a phaser *sound* like a
/// phaser come from summing the dry against the phase-shifted wet. The
/// engine's per-row Mix then attenuates how much of the (wet-minus-dry)
/// contribution mixes back in — at Mix=1.0 you hear the full `dry +
/// cascade`; at Mix=0 you hear pure dry; in between you get a continuous
/// blend.
pub struct PhaserEffect {
    center: f32,
    feedback_pct: f32,
    stereo_pct: f32,
    sample_rate: f32,
    /// 4 all-pass states per channel — one `f32` per stage (Direct Form
    /// II's single delay register).
    stage_state: [[f32; Self::STAGES]; 2],
    /// 1-sample feedback delay per channel — holds the previous cascade
    /// output so the loop closes one sample late (no zero-delay path).
    fb_state: [f32; 2],
}

impl PhaserEffect {
    const STAGES: usize = 4;
    /// Hard cap on the feedback gain. Each all-pass stage has unity
    /// magnitude, so total loop gain = `fb_pct/100`; 0.95 keeps a
    /// comfortable margin from the unit circle.
    const FB_MAX: f32 = 0.95;
    /// Max ±octaves of L/R centre-frequency offset at Stereo=100 %. A
    /// half-octave per side gives a wide spatial spread without sounding
    /// dislocated. 100 % * 0.005 = 0.5 → ±0.5 octaves.
    const STEREO_OCT_PER_PCT: f32 = 0.005;

    const PARAMS: [ParamSpec; 3] = [
        ParamSpec {
            name: "Center",
            min: 50.0,
            max: 8_000.0,
            default: 500.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Feedback",
            min: 0.0,
            max: 95.0,
            default: 30.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Stereo",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    /// A fresh `PhaserEffect` at default params and 48 kHz. Call
    /// `set_sample_rate` to retune to the host's rate.
    pub fn new() -> Self {
        Self {
            center: Self::PARAMS[0].default,
            feedback_pct: Self::PARAMS[1].default,
            stereo_pct: Self::PARAMS[2].default,
            sample_rate: 48_000.0,
            stage_state: [[0.0; Self::STAGES]; 2],
            fb_state: [0.0; 2],
        }
    }

    /// 1st-order all-pass coefficient placing the phase = -90° point at
    /// frequency `f`. `a = (1 - tan(π·f/sr)) / (1 + tan(π·f/sr))`.
    /// `f` is clamped to `[20.0, sr·0.45]` so `tan` stays well-conditioned
    /// (the divisor never hits zero).
    fn allpass_coef(f: f32, sr: f32) -> f32 {
        let f = f.clamp(20.0, sr * 0.45);
        let t = (std::f32::consts::PI * f / sr).tan();
        (1.0 - t) / (1.0 + t)
    }

    /// One 1st-order all-pass step with a single-register Direct Form II
    /// implementation: `y = -a·x + state`, then `state = x + a·y`. The
    /// `state` slot holds the next-sample contribution; cleared by
    /// `reset()`.
    #[inline]
    fn allpass_step(x: f32, state: &mut f32, a: f32) -> f32 {
        let y = -a * x + *state;
        *state = x + a * y;
        y
    }
}

impl Default for PhaserEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for PhaserEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let stereo_oct = self.stereo_pct * Self::STEREO_OCT_PER_PCT;
        let cl = self.center * (-stereo_oct).exp2();
        let cr = self.center * stereo_oct.exp2();
        let al = Self::allpass_coef(cl, self.sample_rate);
        let ar = Self::allpass_coef(cr, self.sample_rate);
        let fb = (self.feedback_pct * 0.01).clamp(0.0, Self::FB_MAX);

        // Cascade input = dry + feedback × previous cascade output. The
        // 1-sample delay on the feedback path keeps the loop well-defined.
        let mut yl = left + fb * self.fb_state[0];
        let mut yr = right + fb * self.fb_state[1];

        // 4-stage all-pass cascade per channel.
        for i in 0..Self::STAGES {
            yl = Self::allpass_step(yl, &mut self.stage_state[0][i], al);
            yr = Self::allpass_step(yr, &mut self.stage_state[1][i], ar);
        }

        // Save cascade output for next sample's feedback path.
        self.fb_state[0] = yl;
        self.fb_state[1] = yr;

        // Phaser sound = dry + phase-shifted. The engine's Mix dial then
        // attenuates the contribution.
        (left + yl, right + yr)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn reset(&mut self) {
        self.stage_state = [[0.0; Self::STAGES]; 2];
        self.fb_state = [0.0; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.center = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.feedback_pct = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.stereo_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            _ => {}
        }
    }
}

/// A phase-vocoder spectral shifter/stretcher, ported from the `warp-zone`
/// plugin. Wraps two `SpectralShifter`s (one per channel) plus a clamped
/// feedback loop, exposing five modulatable params (Shift, Stretch,
/// Feedback, Low, High).
///
/// **Latency**: 4096 samples (= FFT size) ≈ 85 ms at 48 kHz. The multosis
/// engine does not latency-compensate the per-row Mix dial, so at
/// intermediate Mix the in-time dry comb-filters against the delayed wet
/// — musically usable as sound design, but for a "clean" pitch shift run
/// the row at Mix = 100 %.
pub struct WarpZoneEffect {
    shift_st: f32,
    stretch: f32,
    feedback_pct: f32,
    low_hz: f32,
    high_hz: f32,
    sample_rate: f32,
    /// One shifter per channel — they share params but maintain independent
    /// FFT state so stereo information is preserved through the cascade.
    shifter_l: tract_dsp::spectral_shifter::SpectralShifter,
    shifter_r: tract_dsp::spectral_shifter::SpectralShifter,
    /// Feedback memory per channel — the previous sample's wet output,
    /// clamped to ±4 to keep the loop from running away even at the
    /// 95 % cap.
    fb_l: f32,
    fb_r: f32,
}

impl WarpZoneEffect {
    /// Phase-vocoder FFT size. Matches the warp-zone plugin so the per-
    /// sample behaviour is identical.
    const FFT_SIZE: usize = 4096;
    /// Hop size — 75 % overlap = 4× redundancy with Hann window.
    const HOP_SIZE: usize = 1024;
    /// Feedback gain cap. 95 % stays well clear of runaway after the
    /// per-sample ±4 clamp on `fb_l`/`fb_r`.
    const FB_MAX: f32 = 0.95;
    /// Per-sample feedback safety clamp (mirrors warp-zone). Keeps the
    /// loop bounded even when the spectral path produces a transient
    /// peak above unity.
    const FB_CLAMP: f32 = 4.0;

    const PARAMS: [ParamSpec; 5] = [
        ParamSpec {
            name: "Shift",
            min: -24.0,
            max: 24.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: " st",
            },
        },
        ParamSpec {
            name: "Stretch",
            min: 0.5,
            max: 2.0,
            default: 1.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "x",
            },
        },
        ParamSpec {
            name: "Feedback",
            min: 0.0,
            max: 95.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Low",
            min: 20.0,
            max: 20_000.0,
            default: 20.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "High",
            min: 20.0,
            max: 20_000.0,
            default: 20_000.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
    ];

    pub fn new() -> Self {
        Self {
            shift_st: Self::PARAMS[0].default,
            stretch: Self::PARAMS[1].default,
            feedback_pct: Self::PARAMS[2].default,
            low_hz: Self::PARAMS[3].default,
            high_hz: Self::PARAMS[4].default,
            sample_rate: 48_000.0,
            shifter_l: tract_dsp::spectral_shifter::SpectralShifter::new(
                Self::FFT_SIZE,
                Self::HOP_SIZE,
            ),
            shifter_r: tract_dsp::spectral_shifter::SpectralShifter::new(
                Self::FFT_SIZE,
                Self::HOP_SIZE,
            ),
            fb_l: 0.0,
            fb_r: 0.0,
        }
    }

    /// Convert the Low/High Hz pair into bin indices for the current SR.
    /// Mirrors warp-zone's clamping: low ≥ 1 (skip DC), high ≥ low, both
    /// capped at `fft_size/2 + 1`.
    fn frequency_bins(&self) -> (usize, usize) {
        let half_plus_one = Self::FFT_SIZE / 2 + 1;
        let bin_hz = self.sample_rate / Self::FFT_SIZE as f32;
        let low_bin = (self.low_hz / bin_hz).round() as usize;
        let high_bin = (self.high_hz / bin_hz).round() as usize;
        let low_bin = low_bin.max(1).min(half_plus_one);
        let high_bin = high_bin.max(low_bin).min(half_plus_one);
        (low_bin, high_bin)
    }
}

impl Default for WarpZoneEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for WarpZoneEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let (low_bin, high_bin) = self.frequency_bins();
        let fb = (self.feedback_pct * 0.01).clamp(0.0, Self::FB_MAX);

        // Inject the previous wet (clamped) into the cascade input, then
        // run the shifter. The shifter returns the wet sample; we save it
        // for the next-iteration feedback after a safety clamp.
        let in_l = left + self.fb_l * fb;
        let in_r = right + self.fb_r * fb;
        let wet_l = self.shifter_l.process_sample(
            in_l,
            self.shift_st,
            self.stretch,
            false,
            low_bin,
            high_bin,
        );
        let wet_r = self.shifter_r.process_sample(
            in_r,
            self.shift_st,
            self.stretch,
            false,
            low_bin,
            high_bin,
        );
        self.fb_l = wet_l.clamp(-Self::FB_CLAMP, Self::FB_CLAMP);
        self.fb_r = wet_r.clamp(-Self::FB_CLAMP, Self::FB_CLAMP);
        (wet_l, wet_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn reset(&mut self) {
        self.shifter_l.reset();
        self.shifter_r.reset();
        self.fb_l = 0.0;
        self.fb_r = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.shift_st = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.stretch = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.feedback_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.low_hz = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            4 => self.high_hz = value.clamp(Self::PARAMS[4].min, Self::PARAMS[4].max),
            _ => {}
        }
    }

    /// 4096-sample FFT delay through the phase vocoder. The engine sums
    /// this across non-muted/non-solo-cancelled WarpZone rows to report a
    /// dynamic latency to the host.
    fn latency_samples(&self) -> usize {
        Self::FFT_SIZE
    }
}

/// A detail-preserving spectral saturator, ported from the `satch` plugin.
/// Wraps two `SpectralClipper`s (one per channel) plus per-channel dry-delay
/// buffers (matching the spectral path's 2048-sample latency), exposing
/// four modulatable params (Gain, Threshold, Detail, Knee).
///
/// **Algorithm** (per sample, per channel):
/// 1. Pull the delayed dry from the per-channel dry-delay buffer.
/// 2. Time-domain saturate the delayed dry → `td` (returns tanh value for
///    the clip mask).
/// 3. Spectral path on undelayed input → `sp` (the FFT-based detail-
///    preserving clip with a built-in 2048-sample delay).
/// 4. Detail rescue: `wet = (td + detail * clip² * (sp − td)).clamp(±threshold)`
///    where `clip = tanh²(gain·input/threshold)` only fires where the
///    waveshaper is actively clipping.
///
/// **Latency**: 2048 samples (= FFT size) ≈ 43 ms at 48 kHz. multosis
/// reports this to the host via `Effect::latency_samples`, so PDC keeps
/// the chain aligned automatically. The per-row Mix dial blends in-time
/// dry against this delayed wet — for clean saturation, run Mix at 100 %.
pub struct SatchEffect {
    gain_db: f32,
    threshold_db: f32,
    detail_pct: f32,
    knee_pct: f32,
    sample_rate: f32,
    spectral_l: tract_dsp::spectral_clipper::SpectralClipper,
    spectral_r: tract_dsp::spectral_clipper::SpectralClipper,
    /// Dry delay buffers — must be FFT_SIZE long so the time-domain
    /// waveshaper sees the same sample the spectral path was fed
    /// FFT_SIZE samples ago. Required by the algorithm itself
    /// (`td` and `sp` need to operate on time-aligned signals for
    /// `lost = sp − td` to be the spectral path's detail bonus).
    dry_delay_l: Vec<f32>,
    dry_delay_r: Vec<f32>,
    dry_delay_pos: usize,
}

impl SatchEffect {
    /// FFT frame length — matches the satch plugin.
    const FFT_SIZE: usize = 2048;
    /// Hop size — 75 % overlap = 4× redundancy with Hann window.
    const HOP_SIZE: usize = 512;

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Gain",
            min: 0.0,
            max: 24.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: " dB",
            },
        },
        ParamSpec {
            name: "Threshold",
            min: -24.0,
            max: 0.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: " dB",
            },
        },
        ParamSpec {
            name: "Detail",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Knee",
            min: 0.0,
            max: 100.0,
            default: 100.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        Self {
            gain_db: Self::PARAMS[0].default,
            threshold_db: Self::PARAMS[1].default,
            detail_pct: Self::PARAMS[2].default,
            knee_pct: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            spectral_l: tract_dsp::spectral_clipper::SpectralClipper::new(
                Self::FFT_SIZE,
                Self::HOP_SIZE,
            ),
            spectral_r: tract_dsp::spectral_clipper::SpectralClipper::new(
                Self::FFT_SIZE,
                Self::HOP_SIZE,
            ),
            dry_delay_l: vec![0.0; Self::FFT_SIZE],
            dry_delay_r: vec![0.0; Self::FFT_SIZE],
            dry_delay_pos: 0,
        }
    }

    /// Convert dB to a linear amplitude factor.
    #[inline]
    fn db_to_gain(db: f32) -> f32 {
        10.0_f32.powf(db / 20.0)
    }
}

impl Default for SatchEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for SatchEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let gain = Self::db_to_gain(self.gain_db);
        let threshold = Self::db_to_gain(self.threshold_db);
        let inv_threshold = 1.0 / threshold;
        let detail = self.detail_pct * 0.01;
        let knee = self.knee_pct * 0.01;
        // Skip-FFT optimisation: when Detail is zero, the spectral term
        // is multiplied by zero in the blend, so the FFT pipeline can
        // skip the expensive frame work (ring state still advances so
        // re-enabling Detail doesn't glitch).
        let skip_fft = detail <= 0.0;

        // Pull the delayed dry; the spectral path operates on undelayed
        // input but its output emerges 2048 samples late, so we
        // time-align by waveshaping the same-old dry sample.
        let dry_l = self.dry_delay_l[self.dry_delay_pos];
        let dry_r = self.dry_delay_r[self.dry_delay_pos];
        self.dry_delay_l[self.dry_delay_pos] = left;
        self.dry_delay_r[self.dry_delay_pos] = right;
        self.dry_delay_pos = (self.dry_delay_pos + 1) % self.dry_delay_l.len();

        // Time-domain waveshaper on the delayed dry (returns tanh so we
        // can build the clip mask without recomputing it).
        let (td_l, tanh_l) = tract_dsp::spectral_clipper::saturate_td_with_tanh_fast(
            dry_l,
            gain,
            threshold,
            inv_threshold,
            knee,
        );
        let (td_r, tanh_r) = tract_dsp::spectral_clipper::saturate_td_with_tanh_fast(
            dry_r,
            gain,
            threshold,
            inv_threshold,
            knee,
        );

        // Spectral path: gives the detail-preserved reconstruction. With
        // skip_fft we still advance ring/counter state (the clipper
        // handles that internally) but bypass the FFT frame work.
        let sp_l = if skip_fft {
            self.spectral_l
                .process_sample_skip_fft_fast(left, gain, threshold, inv_threshold, knee)
        } else {
            self.spectral_l
                .process_sample_fast(left, gain, threshold, inv_threshold, knee)
        };
        let sp_r = if skip_fft {
            self.spectral_r.process_sample_skip_fft_fast(
                right,
                gain,
                threshold,
                inv_threshold,
                knee,
            )
        } else {
            self.spectral_r
                .process_sample_fast(right, gain, threshold, inv_threshold, knee)
        };

        // Clip mask: tanh²(gained/threshold) is ~0 below the knee and ~1
        // above, so detail is only added where the waveshaper is
        // actively clipping. Without this gate the detail term would
        // colour sub-threshold material that the user expects to pass
        // through clean.
        let clip_l = tanh_l * tanh_l;
        let clip_r = tanh_r * tanh_r;
        let lost_l = sp_l - td_l;
        let lost_r = sp_r - td_r;
        let wet_l = (td_l + detail * clip_l * lost_l).clamp(-threshold, threshold);
        let wet_r = (td_r + detail * clip_r * lost_r).clamp(-threshold, threshold);
        (wet_l, wet_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn reset(&mut self) {
        self.spectral_l.reset();
        self.spectral_r.reset();
        for s in self.dry_delay_l.iter_mut() {
            *s = 0.0;
        }
        for s in self.dry_delay_r.iter_mut() {
            *s = 0.0;
        }
        self.dry_delay_pos = 0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.gain_db = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.threshold_db = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.detail_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.knee_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => {}
        }
    }

    /// 2048-sample FFT delay through the spectral clipper. Reported to
    /// the host via the engine's chain-latency sum so PDC keeps the
    /// multosis output aligned with the rest of the project.
    fn latency_samples(&self) -> usize {
        Self::FFT_SIZE
    }
}

/// A beat-repeat / stutter / pitched-buzz "loop" effect modelled on
/// Infiltrator's Loop mode. A capture clock fires per **Refresh** sync
/// subdivision; at each tick the loop's origin snaps to the most-recent
/// `loop_length` samples already in the ring buffer. Between ticks the
/// effect plays that captured slice on repeat — short loop lengths
/// audibly become pitched buzz (loop frequency = pitch), longer ones
/// stutter.
///
/// **Free vs Sync**: `Snap` is an enum of musical subdivisions plus a
/// trailing `Free` entry (mirrors Delay's Time control). When Snap is a
/// sync subdivision the loop length is beat-locked; when Snap = Free the
/// `Rate` dial (Hz) takes over. `Rate` is dimmed when Snap is sync.
///
/// **Length-vs-Refresh clamp**: if the configured loop length exceeds
/// the Refresh interval, it's silently clamped — the loop never spans
/// more than one capture window.
///
/// **Smooth**: linear crossfade at the loop seam, duration =
/// `(smooth_pct / 100) · (loop_length / 2)`. The last samples of each
/// iteration blend into the iteration's first samples, so re-looping
/// self-similar audio is smooth at every Smooth level.
pub struct RepeatEffect {
    rate_hz: f32,
    snap_idx: f32,
    refresh_idx: f32,
    smooth_pct: f32,
    sample_rate: f32,
    bpm: f32,
    /// Per-channel write-only ring buffer. Audio is always written here
    /// so the capture trigger has a back-window to pull `loop_length`
    /// samples from.
    ring_l: Vec<f32>,
    ring_r: Vec<f32>,
    write_idx: usize,
    /// Capture-clock phase in `[0, 1)`. Each sample advances by
    /// `1 / capture_period_samples`. When the phase crosses 1.0 the
    /// capture trigger fires (after priming).
    capture_phase: f32,
    /// Index of the first sample of the active loop in the ring buffer.
    /// Updated on each capture trigger to `write_idx − loop_length`.
    capture_origin: usize,
    /// Length of the active loop in samples. Held stable across one
    /// loop iteration; refreshed on the next capture trigger from the
    /// current `Snap` / `Rate` / Refresh settings.
    loop_length: usize,
    /// Playback position within the current loop, `0..loop_length`.
    loop_pos: usize,
    /// True once the ring has been filled with at least one loop
    /// length's worth of audio. Output is dry passthrough until then.
    primed: bool,
    /// Sample counter since the last `reset()`, used to detect priming.
    samples_since_reset: u64,
}

impl RepeatEffect {
    /// Worst-case buffer length: 4 s × 192 kHz. Covers the longest sync
    /// subdivision the Refresh enum can ask for at the slowest practical
    /// host tempo. Buffer is allocated once in `new`; per-sample work
    /// only reads/writes existing slots.
    const BUF_LEN: usize = (4.0 * 192_000.0) as usize;
    /// Floor on the active loop length. Anything shorter degenerates to
    /// noise; `Rate` already maxes at 1 kHz (≈ 48 samples at 48 kHz) so
    /// this is just a safety net.
    const MIN_LOOP_SAMPLES: usize = 16;
    /// Free-mode Rate range (Hz). 0.5 Hz → 2 s loop (long stutter);
    /// 1 kHz → 1 ms loop (high-pitched buzz).
    const RATE_MIN_HZ: f32 = 0.5;
    const RATE_MAX_HZ: f32 = 1_000.0;

    /// Snap subdivisions, in dropdown order. The trailing `Free` entry
    /// makes the Rate dial active.
    const SNAP_LABELS: &'static [&'static str] = &[
        "1/64", "1/64.", "1/32", "1/32.", "1/16", "1/16.", "1/8", "1/8.", "1/4", "1/4.", "1/2",
        "1/2.", "1/1", "1/1.", "Free",
    ];
    /// Index of the `Free` entry in `SNAP_LABELS`.
    const SNAP_FREE_IDX: usize = 14;

    /// Refresh subdivisions, in dropdown order. Sync-only — no Free entry.
    const REFRESH_LABELS: &'static [&'static str] = &[
        "1/64", "1/64.", "1/32", "1/32.", "1/16", "1/16.", "1/8", "1/8.", "1/4", "1/4.", "1/2",
        "1/2.", "1/1", "1/1.",
    ];

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Rate",
            min: Self::RATE_MIN_HZ,
            max: Self::RATE_MAX_HZ,
            default: 30.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Snap",
            min: 0.0,
            max: (Self::SNAP_LABELS.len() - 1) as f32,
            default: 6.0, // 1/8
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: Self::SNAP_LABELS,
            },
        },
        ParamSpec {
            name: "Refresh",
            min: 0.0,
            max: (Self::REFRESH_LABELS.len() - 1) as f32,
            default: 8.0, // 1/4
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: Self::REFRESH_LABELS,
            },
        },
        ParamSpec {
            name: "Smooth",
            min: 0.0,
            max: 100.0,
            default: 10.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        Self {
            rate_hz: Self::PARAMS[0].default,
            snap_idx: Self::PARAMS[1].default,
            refresh_idx: Self::PARAMS[2].default,
            smooth_pct: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            bpm: 120.0,
            ring_l: vec![0.0; Self::BUF_LEN],
            ring_r: vec![0.0; Self::BUF_LEN],
            write_idx: 0,
            capture_phase: 1.0,
            capture_origin: 0,
            loop_length: 0,
            loop_pos: 0,
            primed: false,
            samples_since_reset: 0,
        }
    }

    /// Beats per Snap-enum index. Returns `None` for the trailing `Free`
    /// slot (caller switches to the Rate dial in that case). Shared with
    /// `StretchEffect::refresh_beats` so both effects use the same
    /// subdivision lookup.
    pub(crate) fn snap_beats(idx: usize) -> Option<f32> {
        match idx {
            0 => Some(4.0 / 64.0),
            1 => Some(1.5 * 4.0 / 64.0),
            2 => Some(4.0 / 32.0),
            3 => Some(1.5 * 4.0 / 32.0),
            4 => Some(4.0 / 16.0),
            5 => Some(1.5 * 4.0 / 16.0),
            6 => Some(0.5),
            7 => Some(0.75),
            8 => Some(1.0),
            9 => Some(1.5),
            10 => Some(2.0),
            11 => Some(3.0),
            12 => Some(4.0),
            13 => Some(6.0),
            _ => None,
        }
    }

    /// Beats per Refresh-enum index. Always returns a finite value
    /// (sync-only). Out-of-range defaults to 1/4 so a stray modulation
    /// can't freeze the capture clock.
    fn refresh_beats(idx: usize) -> f32 {
        Self::snap_beats(idx).unwrap_or(1.0)
    }

    /// Loop length in samples — what the user dialled in via Snap or
    /// Rate, before the Refresh-interval clamp.
    fn loop_length_samples_raw(&self) -> f32 {
        let idx = self.snap_idx.round() as usize;
        match Self::snap_beats(idx) {
            Some(beats) => {
                let sec_per_beat = 60.0 / self.bpm.max(1.0);
                beats * sec_per_beat * self.sample_rate
            }
            None => self.sample_rate / self.rate_hz.clamp(Self::RATE_MIN_HZ, Self::RATE_MAX_HZ),
        }
    }

    /// Capture interval in samples — how often a fresh slice is grabbed.
    fn capture_period_samples(&self) -> f32 {
        let beats = Self::refresh_beats(self.refresh_idx.round() as usize);
        let sec_per_beat = 60.0 / self.bpm.max(1.0);
        (beats * sec_per_beat * self.sample_rate).max(1.0)
    }
}

impl Default for RepeatEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for RepeatEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // Always write incoming audio into the ring. This is the
        // back-window the capture trigger pulls from.
        self.ring_l[self.write_idx] = left;
        self.ring_r[self.write_idx] = right;
        self.write_idx = (self.write_idx + 1) % Self::BUF_LEN;
        self.samples_since_reset = self.samples_since_reset.saturating_add(1);

        // Loop length the user asked for, clamped to the Refresh window
        // so the loop never overruns its capture interval — no mid-loop
        // interruption ever.
        let capture_period = self.capture_period_samples();
        let raw = self.loop_length_samples_raw();
        let clamped = raw.min(capture_period).max(Self::MIN_LOOP_SAMPLES as f32) as usize;

        // Advance the capture-clock phase. A wrap past 1.0 means it's
        // time to grab a fresh slice.
        self.capture_phase += 1.0 / capture_period;
        let phase_fired = if self.capture_phase >= 1.0 {
            self.capture_phase -= 1.0;
            true
        } else {
            false
        };

        // Priming: until enough audio has been written into the ring
        // for at least one (clamped) loop_length, fall back to dry
        // passthrough so the user hears something immediately when they
        // enable the effect. Use the clamped value (not raw) because
        // that's the loop we'll actually play — gating on raw would
        // hold off priming for `min(raw, capture_period)` samples even
        // when the Refresh window is much shorter than the user's
        // Snap setting.
        let want_primed = self.samples_since_reset >= clamped as u64;
        let just_primed = want_primed && !self.primed;
        if just_primed {
            self.primed = true;
        }
        let trigger = self.primed && (phase_fired || just_primed);

        // On trigger: snap capture_origin to the most-recent slice in
        // the ring and restart the loop playhead.
        if trigger {
            self.loop_length = clamped;
            self.capture_origin =
                (self.write_idx + Self::BUF_LEN - self.loop_length) % Self::BUF_LEN;
            self.loop_pos = 0;
        }

        // Output: dry passthrough until primed (so the user always
        // hears something), then loop playback with optional
        // crossfade at the seam.
        if !self.primed || self.loop_length == 0 {
            return (left, right);
        }

        let offset = self.loop_pos;
        let main_idx = (self.capture_origin + offset) % Self::BUF_LEN;
        let main_l = self.ring_l[main_idx];
        let main_r = self.ring_r[main_idx];

        // Crossfade region: the last `crossfade` samples of each loop
        // iteration blend with the iteration's own first `crossfade`
        // samples. Linear sum-of-weights = 1 so self-similar audio
        // overlays cleanly.
        let crossfade =
            ((self.smooth_pct.clamp(0.0, 100.0) * 0.01) * (self.loop_length as f32 * 0.5)) as usize;
        let crossfade = crossfade.min(self.loop_length / 2);
        let (out_l, out_r) = if crossfade > 0 && offset + crossfade >= self.loop_length {
            let r = offset + crossfade - self.loop_length;
            let w_start = r as f32 / crossfade as f32;
            let w_end = 1.0 - w_start;
            let head_idx = (self.capture_origin + r) % Self::BUF_LEN;
            let head_l = self.ring_l[head_idx];
            let head_r = self.ring_r[head_idx];
            (
                w_end * main_l + w_start * head_l,
                w_end * main_r + w_start * head_r,
            )
        } else {
            (main_l, main_r)
        };

        self.loop_pos += 1;
        if self.loop_pos >= self.loop_length {
            self.loop_pos = 0;
        }

        (out_l, out_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn set_bpm(&mut self, bpm: f32) {
        self.bpm = bpm.max(1.0);
    }

    fn reset(&mut self) {
        for s in self.ring_l.iter_mut() {
            *s = 0.0;
        }
        for s in self.ring_r.iter_mut() {
            *s = 0.0;
        }
        self.write_idx = 0;
        // Phase at 1.0 so the very first sample after priming fires the
        // first capture (no awkward initial silent loop).
        self.capture_phase = 1.0;
        self.capture_origin = 0;
        self.loop_length = 0;
        self.loop_pos = 0;
        self.primed = false;
        self.samples_since_reset = 0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.rate_hz = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => {
                let max_idx = (Self::SNAP_LABELS.len() - 1) as f32;
                self.snap_idx = value.round().clamp(0.0, max_idx);
            }
            2 => {
                let max_idx = (Self::REFRESH_LABELS.len() - 1) as f32;
                self.refresh_idx = value.round().clamp(0.0, max_idx);
            }
            3 => self.smooth_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => {}
        }
    }

    /// The Rate dial (slot 0) is dimmed whenever Snap points at a sync
    /// subdivision — sync drives the loop length directly and the Rate
    /// value is unused. Mirrors Delay's Free/Time dim behaviour.
    fn param_dimmed(&self, index: usize) -> bool {
        if index != 0 {
            return false;
        }
        let idx = self.snap_idx.round() as usize;
        idx != Self::SNAP_FREE_IDX
    }
}

/// A granular time-stretch effect — slows down captured audio without
/// changing pitch, modelled on Infiltrator's Stretch.
///
/// **How it works:** every Refresh tick captures a slice of incoming
/// audio whose length is `Refresh interval × Pace` — that's the trick
/// that keeps the stretch synchronised. A read pointer crawls through
/// that capture window at `Pace` samples per output sample, so the
/// window is fully traversed in exactly one Refresh interval. While
/// the read pointer crawls, a grain scheduler spawns small windowed
/// snippets that each play at original speed (pitch preserved). The
/// snippets overlap-add into the output.
///
/// `Smooth` lerps each grain's envelope from boxcar (0 %) to Hann
/// (100 %); the adjacent-grain sum is exactly `2 − smooth` at every
/// position, so a matching scale (`1 / (2 − smooth)`) keeps the
/// summed amplitude unit across the whole Smooth range.
///
/// **Trade-offs:**
/// - Output is dry passthrough until the ring has filled with at least
///   one capture window of audio, so the user always hears something
///   when the effect first engages.
/// - Latency reported as 0: the granulation delay (= one Refresh
///   interval) is *the effect*, not a fixed plugin delay PDC should
///   compensate for. Same call as Repeat.
pub struct StretchEffect {
    pace: f32,
    refresh_idx: f32,
    grain_hz: f32,
    smooth_pct: f32,
    sample_rate: f32,
    bpm: f32,
    /// Per-channel input ring. Audio is written here unconditionally so
    /// the capture-and-granulate logic has a back-window to pull from.
    ring_l: Vec<f32>,
    ring_r: Vec<f32>,
    write_idx: usize,
    /// Capture-clock phase in `[0, 1)`. Wrap → fire a Refresh tick.
    capture_phase: f32,
    /// Index into the ring where the active capture window starts.
    /// Updated on each Refresh tick.
    capture_origin: usize,
    /// Length of the active capture window in samples
    /// (= `capture_period × pace`). Held stable between Refresh ticks.
    capture_window: f32,
    /// Read pointer within the capture window, `0..capture_window`.
    /// Advances at `pace` samples per output sample so it traverses the
    /// window in exactly `capture_period` output samples.
    read_pos: f32,
    /// Grain-spawn phase in `[0, 1)`. Wrap → spawn a new grain in the
    /// next available slot.
    grain_spawn_phase: f32,
    /// Active grain pool. 4 slots — at 50 % overlap, never more than 2
    /// are simultaneously active in steady state; the extra two cover
    /// the brief overlap during grain hand-off.
    grains: [StretchGrain; 4],
    /// True once the ring has at least one capture window's worth of
    /// audio. Output is dry passthrough until then.
    primed: bool,
    samples_since_reset: u64,
}

/// One windowed playback voice for `StretchEffect`. `active = false`
/// slots are skipped on the audio loop. Spawned by the grain scheduler;
/// retires itself when `elapsed >= length`.
#[derive(Clone, Copy)]
struct StretchGrain {
    active: bool,
    /// Absolute ring index this grain reads from, set at spawn time.
    start_idx: usize,
    /// Samples played since spawn (fractional for sub-sample accuracy
    /// even though we step by 1.0 per output sample).
    elapsed: f32,
    /// Total grain length in samples.
    length: f32,
}

impl Default for StretchGrain {
    fn default() -> Self {
        Self {
            active: false,
            start_idx: 0,
            elapsed: 0.0,
            length: 0.0,
        }
    }
}

impl StretchEffect {
    /// Worst-case ring buffer length: 4 s × 192 kHz, same as Repeat.
    const BUF_LEN: usize = (4.0 * 192_000.0) as usize;
    /// Floor on the capture window so degenerate (very low Pace × very
    /// short Refresh) settings still produce something playable.
    const MIN_WINDOW_SAMPLES: usize = 16;
    /// Pace range. 0.05 = 20× stretch (extreme but stable); 1.0 = no
    /// stretch (granulator runs at real-time speed).
    const PACE_MIN: f32 = 0.05;
    const PACE_MAX: f32 = 1.0;
    /// Grain Hz range. 5 Hz → 200 ms grains (long stutter), 200 Hz →
    /// 5 ms grains (pitched/timbral). Matches Repeat's Rate envelope.
    const GRAIN_MIN_HZ: f32 = 5.0;
    const GRAIN_MAX_HZ: f32 = 200.0;

    /// Refresh subdivisions, in dropdown order. Sync-only.
    const REFRESH_LABELS: &'static [&'static str] = &[
        "1/64", "1/64.", "1/32", "1/32.", "1/16", "1/16.", "1/8", "1/8.", "1/4", "1/4.", "1/2",
        "1/2.", "1/1", "1/1.",
    ];

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Pace",
            min: Self::PACE_MIN,
            max: Self::PACE_MAX,
            default: 0.5,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "x",
            },
        },
        ParamSpec {
            name: "Refresh",
            min: 0.0,
            max: (Self::REFRESH_LABELS.len() - 1) as f32,
            default: 8.0, // 1/4
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: Self::REFRESH_LABELS,
            },
        },
        ParamSpec {
            name: "Grain",
            min: Self::GRAIN_MIN_HZ,
            max: Self::GRAIN_MAX_HZ,
            default: 30.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Smooth",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        Self {
            pace: Self::PARAMS[0].default,
            refresh_idx: Self::PARAMS[1].default,
            grain_hz: Self::PARAMS[2].default,
            smooth_pct: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            bpm: 120.0,
            ring_l: vec![0.0; Self::BUF_LEN],
            ring_r: vec![0.0; Self::BUF_LEN],
            write_idx: 0,
            capture_phase: 1.0,
            capture_origin: 0,
            capture_window: 0.0,
            read_pos: 0.0,
            grain_spawn_phase: 1.0,
            grains: [StretchGrain::default(); 4],
            primed: false,
            samples_since_reset: 0,
        }
    }

    /// Beats per Refresh-enum index. Reuses Repeat's `snap_beats` table
    /// (same subdivision values, same indexing). Defaults to 1/4 on an
    /// out-of-range index so a stray modulation can't freeze the
    /// capture clock.
    fn refresh_beats(idx: usize) -> f32 {
        RepeatEffect::snap_beats(idx).unwrap_or(1.0)
    }

    /// Capture interval in samples — how often a fresh slice is grabbed.
    fn capture_period_samples(&self) -> f32 {
        let beats = Self::refresh_beats(self.refresh_idx.round() as usize);
        let sec_per_beat = 60.0 / self.bpm.max(1.0);
        (beats * sec_per_beat * self.sample_rate).max(1.0)
    }

    /// Grain length in samples — what each individual snippet plays for
    /// at original speed.
    fn grain_length_samples(&self) -> f32 {
        let hz = self.grain_hz.clamp(Self::GRAIN_MIN_HZ, Self::GRAIN_MAX_HZ);
        (self.sample_rate / hz).max(2.0)
    }
}

impl Default for StretchEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for StretchEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // Always write input to the ring; the capture-and-granulate
        // logic pulls from here on every Refresh tick.
        self.ring_l[self.write_idx] = left;
        self.ring_r[self.write_idx] = right;
        self.write_idx = (self.write_idx + 1) % Self::BUF_LEN;
        self.samples_since_reset = self.samples_since_reset.saturating_add(1);

        let pace = self.pace.clamp(Self::PACE_MIN, Self::PACE_MAX);
        let capture_period = self.capture_period_samples();
        // capture_window = capture_period × pace. Sizes the window so
        // the read pointer (advancing at pace per sample) traverses it
        // in exactly one Refresh interval — no mid-stretch interruption.
        let next_capture_window = (capture_period * pace).max(Self::MIN_WINDOW_SAMPLES as f32);

        // Capture clock.
        self.capture_phase += 1.0 / capture_period;
        let phase_fired = if self.capture_phase >= 1.0 {
            self.capture_phase -= 1.0;
            true
        } else {
            false
        };

        // Priming: until enough audio has been written for one capture
        // window's worth of input, pass the dry signal through so the
        // user always hears something when they enable the effect.
        let want_primed = self.samples_since_reset >= next_capture_window as u64;
        let just_primed = want_primed && !self.primed;
        if just_primed {
            self.primed = true;
        }
        let trigger = self.primed && (phase_fired || just_primed);

        // On trigger: snap capture_origin to the most-recent window in
        // the ring and reset the read pointer.
        if trigger {
            self.capture_window = next_capture_window;
            self.capture_origin =
                (self.write_idx + Self::BUF_LEN - self.capture_window as usize) % Self::BUF_LEN;
            self.read_pos = 0.0;
        }

        // Output: dry passthrough until primed (so the user never hears
        // silence when they first engage the effect).
        if !self.primed || self.capture_window <= 0.0 {
            return (left, right);
        }

        // Grain scheduler: spawn rate = 2 / grain_length so adjacent
        // grains hit 50 % overlap, the standard granular setting.
        let grain_length = self.grain_length_samples();
        let spawn_period = (grain_length * 0.5).max(1.0);
        self.grain_spawn_phase += 1.0 / spawn_period;
        if self.grain_spawn_phase >= 1.0 {
            self.grain_spawn_phase -= 1.0;
            // Grain start lives at (capture_origin + read_pos) — the
            // crawling read pointer determines what audio each grain
            // reads. Find the first inactive slot; if all slots are
            // active, evict the oldest (longest-elapsed).
            let start = (self.capture_origin + self.read_pos as usize) % Self::BUF_LEN;
            let slot = if let Some(i) = self.grains.iter().position(|g| !g.active) {
                i
            } else {
                self.grains
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| {
                        a.elapsed
                            .partial_cmp(&b.elapsed)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            };
            self.grains[slot] = StretchGrain {
                active: true,
                start_idx: start,
                elapsed: 0.0,
                length: grain_length,
            };
        }

        // Advance the read pointer at the stretch rate. Saturate at
        // capture_window so degenerate state can't index past it; the
        // capture-clock math keeps this in sync in normal operation.
        self.read_pos += pace;
        if self.read_pos >= self.capture_window {
            self.read_pos = self.capture_window;
        }

        // Sum active grains with the Smooth-blended window. The window
        // lerps from boxcar (Smooth=0) to Hann (Smooth=100); the
        // adjacent-grain sum is exactly `2 − smooth` everywhere, so a
        // matching scale keeps the output unit-amplitude across the
        // whole Smooth range.
        let smooth = (self.smooth_pct * 0.01).clamp(0.0, 1.0);
        let scale = 1.0 / (2.0 - smooth);
        let two_pi = std::f32::consts::PI * 2.0;
        let mut out_l = 0.0;
        let mut out_r = 0.0;
        for grain in self.grains.iter_mut() {
            if !grain.active {
                continue;
            }
            if grain.elapsed >= grain.length {
                grain.active = false;
                continue;
            }
            let t = grain.elapsed / grain.length;
            // window = (1−smooth)·boxcar + smooth·Hann
            let hann = 0.5 * (1.0 - (two_pi * t).cos());
            let window = (1.0 - smooth) + smooth * hann;
            let idx = (grain.start_idx + grain.elapsed as usize) % Self::BUF_LEN;
            out_l += window * self.ring_l[idx];
            out_r += window * self.ring_r[idx];
            grain.elapsed += 1.0;
        }
        (out_l * scale, out_r * scale)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn set_bpm(&mut self, bpm: f32) {
        self.bpm = bpm.max(1.0);
    }

    fn reset(&mut self) {
        for s in self.ring_l.iter_mut() {
            *s = 0.0;
        }
        for s in self.ring_r.iter_mut() {
            *s = 0.0;
        }
        self.write_idx = 0;
        self.capture_phase = 1.0;
        self.capture_origin = 0;
        self.capture_window = 0.0;
        self.read_pos = 0.0;
        self.grain_spawn_phase = 1.0;
        for g in self.grains.iter_mut() {
            *g = StretchGrain::default();
        }
        self.primed = false;
        self.samples_since_reset = 0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.pace = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => {
                let max_idx = (Self::REFRESH_LABELS.len() - 1) as f32;
                self.refresh_idx = value.round().clamp(0.0, max_idx);
            }
            2 => self.grain_hz = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.smooth_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => {}
        }
    }
}

/// A comb filter with three switchable modes and per-channel stereo offset.
///
/// **Modes:**
/// - **Resonant** = pure feedback comb (`y = x + α·y[n−D]`). Sharp
///   resonant peaks at multiples of `1/D`; sounds like a tuned tube
///   or plucked-string-ish ring. Negative α (signed Depth) shifts the
///   peaks to the half-integer multiples, giving a thinner/octave-up
///   timbre — one knob, two distinct tones.
/// - **Notch** = pure feedforward comb (`y = x + α·x[n−D]`). Notches
///   in the spectrum without resonance. Flatter character; useful for
///   subtle colour and flange-style effects.
/// - **Allpass** = Schroeder allpass (`y = −α·x + x[n−D] + α·y[n−D]`).
///   Magnitude-flat — sounds identical to dry on its own, but summing
///   with dry through the per-row Mix dial reveals the comb-spaced
///   phase pattern as audible filtering.
///
/// **Damping** runs the delay tap through a one-pole low-pass filter
/// in all modes (HF rolloff in the loop tightens FB ring, darkens
/// FF/Allpass timbre).
///
/// **Stereo** spreads the per-channel comb pitch by ±0.5 octaves at
/// 100 %, computed each sample so it tracks Pitch under MSEG
/// modulation. Fractional delay reads via linear interpolation.
pub struct CombEffect {
    pitch_hz: f32,
    mode_idx: f32,
    depth_pct: f32,
    damping_pct: f32,
    stereo_pct: f32,
    sample_rate: f32,
    /// Per-channel delay lines. Sized for the worst case (20 Hz lowest
    /// pitch at 192 kHz = 9600 samples). Indexed by `write_idx`.
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    write_idx: usize,
    /// One-pole LP state for the damping filter, one per channel.
    /// Persists across samples; `damping = 0` collapses the filter to a
    /// passthrough (state isn't read), so leaving it zero across all
    /// samples is safe.
    damp_state_l: f32,
    damp_state_r: f32,
}

impl CombEffect {
    /// 20 Hz lowest pitch → 50 ms longest delay → 9600 samples at
    /// 192 kHz worst case. Sized once in `new`; per-sample work only
    /// reads/writes existing slots.
    const DELAY_BUF_LEN: usize = (1.0 / 20.0 * 192_000.0) as usize + 1;
    /// Hard cap on the loop gain. The user can dial Depth ±100 %, but
    /// internally we clamp to ±0.95 to keep the feedback loop a safe
    /// margin from the unit circle.
    const DEPTH_GAIN_CAP: f32 = 0.95;
    /// Max per-channel pitch offset at Stereo = 100 %, in octaves.
    /// 0.5 octaves per side → up to a full octave of L/R spread.
    const STEREO_OCT_PER_PCT: f32 = 0.005;

    /// Pitch range (Hz). 20 Hz lowest = 50 ms longest delay (audibly
    /// like a short slap echo); 5 kHz highest = 0.2 ms delay (sharp
    /// pitched ring).
    const PITCH_MIN_HZ: f32 = 20.0;
    const PITCH_MAX_HZ: f32 = 5_000.0;

    const MODE_LABELS: &'static [&'static str] = &["Resonant", "Notch", "Allpass"];
    const MODE_RESONANT: usize = 0;
    const MODE_NOTCH: usize = 1;
    const MODE_ALLPASS: usize = 2;

    const PARAMS: [ParamSpec; 5] = [
        ParamSpec {
            name: "Pitch",
            min: Self::PITCH_MIN_HZ,
            max: Self::PITCH_MAX_HZ,
            default: 200.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Mode",
            min: 0.0,
            max: (Self::MODE_LABELS.len() - 1) as f32,
            default: 0.0, // Resonant
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: Self::MODE_LABELS,
            },
        },
        ParamSpec {
            name: "Depth",
            min: -100.0,
            max: 100.0,
            default: 70.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Damping",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Stereo",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        Self {
            pitch_hz: Self::PARAMS[0].default,
            mode_idx: Self::PARAMS[1].default,
            depth_pct: Self::PARAMS[2].default,
            damping_pct: Self::PARAMS[3].default,
            stereo_pct: Self::PARAMS[4].default,
            sample_rate: 48_000.0,
            delay_l: vec![0.0; Self::DELAY_BUF_LEN],
            delay_r: vec![0.0; Self::DELAY_BUF_LEN],
            write_idx: 0,
            damp_state_l: 0.0,
            damp_state_r: 0.0,
        }
    }

    /// Read the delay line `buf` at a (fractional) `delay_samples` back
    /// from `write_idx`. Linear interpolation between adjacent slots so
    /// MSEG-sweeping Pitch doesn't zipper.
    #[inline]
    fn read_tap(buf: &[f32], write_idx: usize, delay_samples: f32) -> f32 {
        let n = buf.len();
        let pos = write_idx as f32 + n as f32 - delay_samples;
        let i_floor = pos.floor();
        let frac = pos - i_floor;
        let i0 = (i_floor as usize) % n;
        let i1 = (i0 + 1) % n;
        buf[i0] * (1.0 - frac) + buf[i1] * frac
    }

    /// Compute the per-channel delay-in-samples from the current Pitch +
    /// Stereo settings. Stereo widens by giving the left channel a
    /// slightly LOWER pitch (longer delay) and the right channel a
    /// slightly HIGHER pitch (shorter delay), up to ±0.5 octaves at
    /// Stereo = 100 %.
    fn channel_delays(&self) -> (f32, f32) {
        let stereo_oct = self.stereo_pct * Self::STEREO_OCT_PER_PCT;
        let pitch_l =
            (self.pitch_hz * (-stereo_oct).exp2()).clamp(Self::PITCH_MIN_HZ, Self::PITCH_MAX_HZ);
        let pitch_r =
            (self.pitch_hz * stereo_oct.exp2()).clamp(Self::PITCH_MIN_HZ, Self::PITCH_MAX_HZ);
        let d_l = (self.sample_rate / pitch_l).clamp(2.0, (Self::DELAY_BUF_LEN - 2) as f32);
        let d_r = (self.sample_rate / pitch_r).clamp(2.0, (Self::DELAY_BUF_LEN - 2) as f32);
        (d_l, d_r)
    }
}

impl Default for CombEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for CombEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let (d_l, d_r) = self.channel_delays();
        // Depth in [-1, +1], capped to ±0.95 for loop stability in FB
        // and Allpass modes.
        let alpha = (self.depth_pct * 0.01).clamp(-Self::DEPTH_GAIN_CAP, Self::DEPTH_GAIN_CAP);
        // Damping coefficient for the one-pole LP that smooths the
        // delay tap: 0 = passthrough, 1 = output never moves
        // (everything HF removed). One-pole form `y = (1-d)·x + d·y`.
        let damping = (self.damping_pct * 0.01).clamp(0.0, 0.99);

        let tap_l_raw = Self::read_tap(&self.delay_l, self.write_idx, d_l);
        let tap_r_raw = Self::read_tap(&self.delay_r, self.write_idx, d_r);
        // Apply damping LP. `damp_state` carries the previous output.
        self.damp_state_l = (1.0 - damping) * tap_l_raw + damping * self.damp_state_l;
        self.damp_state_r = (1.0 - damping) * tap_r_raw + damping * self.damp_state_r;
        let tap_l = self.damp_state_l;
        let tap_r = self.damp_state_r;

        let mode = (self.mode_idx.round() as usize).min(Self::MODE_LABELS.len() - 1);
        let (out_l, out_r, write_l, write_r) = match mode {
            // Resonant: y = x + α·tap_damped; feed y back into the
            // delay so the loop resonates. Write y into delay.
            Self::MODE_RESONANT => {
                let y_l = left + alpha * tap_l;
                let y_r = right + alpha * tap_r;
                (y_l, y_r, y_l, y_r)
            }
            // Notch: y = x + α·tap_damped; feed only the dry x into
            // the delay so there's no resonance. Pure FIR.
            Self::MODE_NOTCH => {
                let y_l = left + alpha * tap_l;
                let y_r = right + alpha * tap_r;
                (y_l, y_r, left, right)
            }
            // Allpass (Schroeder): y = -α·x + tap; write x + α·y into
            // the delay. Magnitude is unity at all frequencies; phase
            // shifts at comb-spaced frequencies become audible when
            // the engine's Mix sums dry against this output.
            Self::MODE_ALLPASS => {
                let y_l = -alpha * left + tap_l;
                let y_r = -alpha * right + tap_r;
                (y_l, y_r, left + alpha * y_l, right + alpha * y_r)
            }
            // `set_param` already clamps `mode_idx` into the valid
            // range, so this arm is unreachable in normal operation.
            // Define it as a safe dry passthrough rather than panic so
            // a degenerate state (e.g. a corrupted preset) audibly
            // bypasses the effect rather than crashing the audio thread.
            _ => (left, right, left, right),
        };

        self.delay_l[self.write_idx] = write_l;
        self.delay_r[self.write_idx] = write_r;
        self.write_idx = (self.write_idx + 1) % Self::DELAY_BUF_LEN;

        (out_l, out_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn reset(&mut self) {
        for s in self.delay_l.iter_mut() {
            *s = 0.0;
        }
        for s in self.delay_r.iter_mut() {
            *s = 0.0;
        }
        self.write_idx = 0;
        self.damp_state_l = 0.0;
        self.damp_state_r = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.pitch_hz = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => {
                let max_idx = (Self::MODE_LABELS.len() - 1) as f32;
                self.mode_idx = value.round().clamp(0.0, max_idx);
            }
            2 => self.depth_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.damping_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            4 => self.stereo_pct = value.clamp(Self::PARAMS[4].min, Self::PARAMS[4].max),
            _ => {}
        }
    }
}

/// Ring modulator: multiplies the input by an internal carrier oscillator.
///
/// The classic Bode-style RM has no dry path — only the sum/difference
/// sidebands survive — which makes it clangy and metallic. Multosis's
/// **Bias** control morphs that hard ring-mod into amplitude modulation
/// (carrier offset toward +1 lets some dry through) or even into phase-
/// inverted dry (carrier toward −1) without leaving the same single
/// multiply. Formally `carrier = bias + (1 − |bias|)·wave`, so
/// `bias = 0` is pure RM, `bias = ±1` is straight ±dry, and intermediate
/// values produce continuous AM-style behaviour.
///
/// **Shape** picks the carrier waveform. Only bandlimited shapes are
/// offered (sine, triangle): saw/square multiplied with audio sprays
/// aliased sidebands that aren't musically useful in this context. Sine
/// is the canonical RM sound; triangle adds a bit of odd-harmonic crunch.
///
/// **Stereo** offsets the right channel's carrier phase by 0..180° (linear
/// in the parameter, so 100 % = π = antiphase). At 0 % the modulator is
/// mono; at 100 % L and R are antiphase. A single phase accumulator is
/// shared so the L/R phase relationship is stable across MSEG-modulated
/// frequency sweeps.
pub struct RingEffect {
    freq_hz: f32,
    shape_idx: f32,
    bias_pct: f32,
    stereo_pct: f32,
    sample_rate: f32,
    /// Carrier phase accumulator in [0, 1). R-channel phase is derived
    /// from this plus a Stereo-controlled offset, so changing Stereo on
    /// the fly doesn't desynchronise the channels.
    phase: f32,
}

impl RingEffect {
    /// 0.1 Hz lower bound → 10 s carrier period (very slow tremolo);
    /// 5 kHz upper bound covers the audible RM range without crowding
    /// Nyquist on lower sample rates.
    const FREQ_MIN_HZ: f32 = 0.1;
    const FREQ_MAX_HZ: f32 = 5_000.0;

    const SHAPE_LABELS: &'static [&'static str] = &["Sine", "Triangle"];
    const SHAPE_TRIANGLE: usize = 1;

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Freq",
            min: Self::FREQ_MIN_HZ,
            max: Self::FREQ_MAX_HZ,
            default: 100.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Shape",
            min: 0.0,
            max: (Self::SHAPE_LABELS.len() - 1) as f32,
            default: 0.0, // Sine
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: Self::SHAPE_LABELS,
            },
        },
        ParamSpec {
            name: "Bias",
            min: -100.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Stereo",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        Self {
            freq_hz: Self::PARAMS[0].default,
            shape_idx: Self::PARAMS[1].default,
            bias_pct: Self::PARAMS[2].default,
            stereo_pct: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            phase: 0.0,
        }
    }

    /// Evaluate the carrier wave at `phase` (in [0, 1) cycles). Both
    /// shapes are returned in the [−1, +1] range.
    #[inline]
    fn carrier_wave(phase: f32, shape_idx: usize) -> f32 {
        // Wrap phase into [0, 1). The accumulator already wraps each
        // sample, but adding the per-channel stereo offset can push it
        // past 1, so the floor-subtract is needed here too.
        let p = phase - phase.floor();
        if shape_idx == Self::SHAPE_TRIANGLE {
            // |p - 0.5| ∈ [0, 0.5]; scale to [-1, +1]: 4·|p-0.5| - 1.
            // At p=0 → +1 (positive peak), at p=0.5 → -1 (negative peak),
            // at p=1 → +1 again. Same phase reference as a cosine, which
            // matches the sine arm's `(2π·p).sin()` only when p=0
            // crosses zero — but for ring-mod purposes the absolute
            // phase reference is irrelevant.
            4.0 * (p - 0.5).abs() - 1.0
        } else {
            // Sine (the default / SHAPE_SINE arm).
            (2.0 * std::f32::consts::PI * p).sin()
        }
    }
}

impl Default for RingEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for RingEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let shape = (self.shape_idx.round() as usize).min(Self::SHAPE_LABELS.len() - 1);
        // bias ∈ [-1, +1]; mix ∈ [0, 1] is how much carrier wave survives.
        // bias=0 → mix=1 (pure RM); bias=±1 → mix=0 (straight ±dry).
        let bias = (self.bias_pct * 0.01).clamp(-1.0, 1.0);
        let mix = 1.0 - bias.abs();
        // Stereo: 0..100 % maps to 0..0.5 cycles = 0..180° phase offset
        // for the right carrier.
        let stereo_offset = (self.stereo_pct * 0.005).clamp(0.0, 0.5);
        let carrier_l = bias + mix * Self::carrier_wave(self.phase, shape);
        let carrier_r = bias + mix * Self::carrier_wave(self.phase + stereo_offset, shape);

        let out = (left * carrier_l, right * carrier_r);

        // Advance phase after evaluating both channels so they share the
        // same time index. `set_param` clamps Freq into the valid range,
        // so `phase_inc` is guaranteed small (< 0.5 even at 5 kHz / 11 kHz
        // SR worst case — comfortably under one full cycle per sample).
        let phase_inc = self.freq_hz / self.sample_rate;
        self.phase += phase_inc;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }

        out
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.freq_hz = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => {
                let max_idx = (Self::SHAPE_LABELS.len() - 1) as f32;
                self.shape_idx = value.round().clamp(0.0, max_idx);
            }
            2 => self.bias_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.stereo_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => {}
        }
    }
}

/// A silent "no-effect" — used when a track has no effect assigned. The row
/// still occupies a lane and the propagation engine still lights its cells,
/// but the lane contributes nothing to the wet sum (an unassigned track has
/// no audio to forward). Declares no modulatable parameters.
pub struct NoneEffect;

impl NoneEffect {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoneEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for NoneEffect {
    fn process_sample(&mut self, _left: f32, _right: f32) -> (f32, f32) {
        (0.0, 0.0)
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {}

    fn reset(&mut self) {}

    fn parameters(&self) -> &'static [ParamSpec] {
        &[]
    }

    fn set_param(&mut self, _index: usize, _value: f32) {}
}

/// The effect registry — which effects exist. `Copy`, serde-derivable.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum EffectKind {
    /// No effect — audio passes through this track unchanged.
    None,
    /// Multimode state-variable filter — cascadable 2/4/6/8-pole with
    /// LP / BP / HP output taps.
    Svf,
    Bitcrush,
    Fm,
    /// Tempo-syncable delay with feedback and input-ducking.
    Delay,
    /// Vintage 4-stage all-pass phaser with feedback and stereo offset.
    Phaser,
    /// Phase-vocoder spectral shifter/stretcher (4096-pt FFT, ~85 ms latency).
    WarpZone,
    /// Detail-preserving spectral saturator (2048-pt FFT, ~43 ms latency).
    Satch,
    /// Beat-repeat / stutter / pitched-buzz loop effect.
    Repeat,
    /// Granular time-stretch effect.
    Stretch,
    /// Switchable-mode comb filter (resonant / notch / Schroeder allpass).
    Comb,
    /// Ring modulator with bias control (morphs RM ↔ AM ↔ ±dry) and
    /// stereo carrier phase offset.
    Ring,
}

impl EffectKind {
    /// Every effect kind, in display / registry order.
    pub const ALL: [EffectKind; 12] = [
        EffectKind::None,
        EffectKind::Svf,
        EffectKind::Bitcrush,
        EffectKind::Fm,
        EffectKind::Delay,
        EffectKind::Phaser,
        EffectKind::WarpZone,
        EffectKind::Satch,
        EffectKind::Repeat,
        EffectKind::Stretch,
        EffectKind::Comb,
        EffectKind::Ring,
    ];

    /// The kind's display name.
    pub fn name(self) -> &'static str {
        match self {
            EffectKind::None => "None",
            EffectKind::Svf => "SVF",
            EffectKind::Bitcrush => "Bitcrush",
            EffectKind::Fm => "FM",
            EffectKind::Delay => "Delay",
            EffectKind::Phaser => "Phaser",
            EffectKind::WarpZone => "Warp Zone",
            EffectKind::Satch => "Satch",
            EffectKind::Repeat => "Repeat",
            EffectKind::Stretch => "Stretch",
            EffectKind::Comb => "Comb",
            EffectKind::Ring => "Ring",
        }
    }
}

/// The number of modulatable parameters effect `kind` declares.
pub fn param_count(kind: EffectKind) -> usize {
    EffectInstance::new(kind).parameters().len()
}

/// The default parameter values for effect `kind`, laid out in the
/// `TrackEffect::params` slot order (slots past the kind's parameter count are
/// zero). Used when a track switches effect kind.
pub fn default_params_for_kind(kind: EffectKind) -> [f32; MAX_EFFECT_PARAMS] {
    let instance = EffectInstance::new(kind);
    let specs = instance.parameters();
    let mut params = [0.0; MAX_EFFECT_PARAMS];
    for (i, spec) in specs.iter().enumerate() {
        params[i] = spec.default;
    }
    params
}

/// A live effect instance — enum dispatch over the effect structs, so the
/// audio engine holds `[EffectInstance; 16]` with no heap and no `dyn`.
pub enum EffectInstance {
    None(NoneEffect),
    Svf(SvfEffect),
    Bitcrush(BitcrushEffect),
    Fm(FmEffect),
    Delay(DelayEffect),
    Phaser(PhaserEffect),
    // Boxed: WarpZoneEffect is ~720 B (two `SpectralShifter`s with FFT
    // scratch buffers); 16 of them sitting unboxed in `EffectInstance`
    // would inflate every track-effect slot to that size. Box keeps the
    // enum compact and matches clippy's `large-enum-variant`. The Box
    // allocation happens once per kind-switch from the GUI thread, never
    // on the audio path.
    WarpZone(Box<WarpZoneEffect>),
    // Boxed for the same reason as WarpZoneEffect: SpectralClipper
    // pair plus dry-delay buffers makes the variant large.
    Satch(Box<SatchEffect>),
    // Not boxed — RepeatEffect itself is small; the two Vec ring
    // buffers it holds are already heap-allocated by Vec.
    Repeat(RepeatEffect),
    // Not boxed for the same reason as Repeat: small struct,
    // ring buffers are already heap-allocated by Vec.
    Stretch(StretchEffect),
    // Not boxed — CombEffect is small (~150 KB stereo of delay
    // line, already heap-allocated by Vec).
    Comb(CombEffect),
    // Not boxed — RingEffect is tiny (six f32s, no heap).
    Ring(RingEffect),
}

impl EffectInstance {
    /// A fresh instance of `kind` at default parameters.
    pub fn new(kind: EffectKind) -> Self {
        match kind {
            EffectKind::None => EffectInstance::None(NoneEffect::new()),
            EffectKind::Svf => EffectInstance::Svf(SvfEffect::new()),
            EffectKind::Bitcrush => EffectInstance::Bitcrush(BitcrushEffect::new()),
            EffectKind::Fm => EffectInstance::Fm(FmEffect::new()),
            EffectKind::Delay => EffectInstance::Delay(DelayEffect::new()),
            EffectKind::Phaser => EffectInstance::Phaser(PhaserEffect::new()),
            EffectKind::WarpZone => EffectInstance::WarpZone(Box::default()),
            EffectKind::Satch => EffectInstance::Satch(Box::default()),
            EffectKind::Repeat => EffectInstance::Repeat(RepeatEffect::new()),
            EffectKind::Stretch => EffectInstance::Stretch(StretchEffect::new()),
            EffectKind::Comb => EffectInstance::Comb(CombEffect::new()),
            EffectKind::Ring => EffectInstance::Ring(RingEffect::new()),
        }
    }

    /// Which kind this instance is.
    pub fn kind(&self) -> EffectKind {
        match self {
            EffectInstance::None(_) => EffectKind::None,
            EffectInstance::Svf(_) => EffectKind::Svf,
            EffectInstance::Bitcrush(_) => EffectKind::Bitcrush,
            EffectInstance::Fm(_) => EffectKind::Fm,
            EffectInstance::Delay(_) => EffectKind::Delay,
            EffectInstance::Phaser(_) => EffectKind::Phaser,
            EffectInstance::WarpZone(_) => EffectKind::WarpZone,
            EffectInstance::Satch(_) => EffectKind::Satch,
            EffectInstance::Repeat(_) => EffectKind::Repeat,
            EffectInstance::Stretch(_) => EffectKind::Stretch,
            EffectInstance::Comb(_) => EffectKind::Comb,
            EffectInstance::Ring(_) => EffectKind::Ring,
        }
    }
}

impl Effect for EffectInstance {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        match self {
            EffectInstance::None(e) => e.process_sample(left, right),
            EffectInstance::Svf(e) => e.process_sample(left, right),
            EffectInstance::Bitcrush(e) => e.process_sample(left, right),
            EffectInstance::Fm(e) => e.process_sample(left, right),
            EffectInstance::Delay(e) => e.process_sample(left, right),
            EffectInstance::Phaser(e) => e.process_sample(left, right),
            EffectInstance::WarpZone(e) => e.process_sample(left, right),
            EffectInstance::Satch(e) => e.process_sample(left, right),
            EffectInstance::Repeat(e) => e.process_sample(left, right),
            EffectInstance::Stretch(e) => e.process_sample(left, right),
            EffectInstance::Comb(e) => e.process_sample(left, right),
            EffectInstance::Ring(e) => e.process_sample(left, right),
        }
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        match self {
            EffectInstance::None(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Svf(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Bitcrush(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Fm(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Delay(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Phaser(e) => e.set_sample_rate(sample_rate),
            EffectInstance::WarpZone(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Satch(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Repeat(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Stretch(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Comb(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Ring(e) => e.set_sample_rate(sample_rate),
        }
    }

    fn reset(&mut self) {
        match self {
            EffectInstance::None(e) => e.reset(),
            EffectInstance::Svf(e) => e.reset(),
            EffectInstance::Bitcrush(e) => e.reset(),
            EffectInstance::Fm(e) => e.reset(),
            EffectInstance::Delay(e) => e.reset(),
            EffectInstance::Phaser(e) => e.reset(),
            EffectInstance::WarpZone(e) => e.reset(),
            EffectInstance::Satch(e) => e.reset(),
            EffectInstance::Repeat(e) => e.reset(),
            EffectInstance::Stretch(e) => e.reset(),
            EffectInstance::Comb(e) => e.reset(),
            EffectInstance::Ring(e) => e.reset(),
        }
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        match self {
            EffectInstance::None(e) => e.parameters(),
            EffectInstance::Svf(e) => e.parameters(),
            EffectInstance::Bitcrush(e) => e.parameters(),
            EffectInstance::Fm(e) => e.parameters(),
            EffectInstance::Delay(e) => e.parameters(),
            EffectInstance::Phaser(e) => e.parameters(),
            EffectInstance::WarpZone(e) => e.parameters(),
            EffectInstance::Satch(e) => e.parameters(),
            EffectInstance::Repeat(e) => e.parameters(),
            EffectInstance::Stretch(e) => e.parameters(),
            EffectInstance::Comb(e) => e.parameters(),
            EffectInstance::Ring(e) => e.parameters(),
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match self {
            EffectInstance::None(e) => e.set_param(index, value),
            EffectInstance::Svf(e) => e.set_param(index, value),
            EffectInstance::Bitcrush(e) => e.set_param(index, value),
            EffectInstance::Fm(e) => e.set_param(index, value),
            EffectInstance::Delay(e) => e.set_param(index, value),
            EffectInstance::Phaser(e) => e.set_param(index, value),
            EffectInstance::WarpZone(e) => e.set_param(index, value),
            EffectInstance::Satch(e) => e.set_param(index, value),
            EffectInstance::Repeat(e) => e.set_param(index, value),
            EffectInstance::Stretch(e) => e.set_param(index, value),
            EffectInstance::Comb(e) => e.set_param(index, value),
            EffectInstance::Ring(e) => e.set_param(index, value),
        }
    }

    fn set_bpm(&mut self, bpm: f32) {
        match self {
            EffectInstance::None(e) => e.set_bpm(bpm),
            EffectInstance::Svf(e) => e.set_bpm(bpm),
            EffectInstance::Bitcrush(e) => e.set_bpm(bpm),
            EffectInstance::Fm(e) => e.set_bpm(bpm),
            EffectInstance::Delay(e) => e.set_bpm(bpm),
            EffectInstance::Phaser(e) => e.set_bpm(bpm),
            EffectInstance::WarpZone(e) => e.set_bpm(bpm),
            EffectInstance::Satch(e) => e.set_bpm(bpm),
            EffectInstance::Repeat(e) => e.set_bpm(bpm),
            EffectInstance::Stretch(e) => e.set_bpm(bpm),
            EffectInstance::Comb(e) => e.set_bpm(bpm),
            EffectInstance::Ring(e) => e.set_bpm(bpm),
        }
    }

    fn param_dimmed(&self, index: usize) -> bool {
        match self {
            EffectInstance::None(e) => e.param_dimmed(index),
            EffectInstance::Svf(e) => e.param_dimmed(index),
            EffectInstance::Bitcrush(e) => e.param_dimmed(index),
            EffectInstance::Fm(e) => e.param_dimmed(index),
            EffectInstance::Delay(e) => e.param_dimmed(index),
            EffectInstance::Phaser(e) => e.param_dimmed(index),
            EffectInstance::WarpZone(e) => e.param_dimmed(index),
            EffectInstance::Satch(e) => e.param_dimmed(index),
            EffectInstance::Repeat(e) => e.param_dimmed(index),
            EffectInstance::Stretch(e) => e.param_dimmed(index),
            EffectInstance::Comb(e) => e.param_dimmed(index),
            EffectInstance::Ring(e) => e.param_dimmed(index),
        }
    }

    fn latency_samples(&self) -> usize {
        match self {
            EffectInstance::None(e) => e.latency_samples(),
            EffectInstance::Svf(e) => e.latency_samples(),
            EffectInstance::Bitcrush(e) => e.latency_samples(),
            EffectInstance::Fm(e) => e.latency_samples(),
            EffectInstance::Delay(e) => e.latency_samples(),
            EffectInstance::Phaser(e) => e.latency_samples(),
            EffectInstance::WarpZone(e) => e.latency_samples(),
            EffectInstance::Satch(e) => e.latency_samples(),
            EffectInstance::Repeat(e) => e.latency_samples(),
            EffectInstance::Stretch(e) => e.latency_samples(),
            EffectInstance::Comb(e) => e.latency_samples(),
            EffectInstance::Ring(e) => e.latency_samples(),
        }
    }
}

/// Maximum modulatable parameters any effect declares — fixes the
/// `TrackEffect::params` array length so the persisted config is stable as
/// effects are added (current max is 2; 4 leaves headroom).
pub const MAX_EFFECT_PARAMS: usize = 5;

/// One track row's persisted effect configuration: which effect, its
/// parameter values, its dry/wet mix, and its mute/solo state. `params[i]`
/// is the value for the kind's `parameters()[i]`; entries past the kind's
/// parameter count are unused.
///
/// **Mute** bypasses the row — the effect is skipped and audio flows
/// straight through to the next row, identical to `kind == None`.
/// **Solo** flips the rest of the chain into bypass: when any row is
/// soloed, every non-soloed row behaves as if muted (a row that is BOTH
/// muted and soloed stays muted — the user's mute intent wins).
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrackEffect {
    pub kind: EffectKind,
    pub params: [f32; MAX_EFFECT_PARAMS],
    /// Per-track dry/wet blend, 0.0 (dry) .. 1.0 (full effect). Defaulted on
    /// deserialize so presets predating this field load as fully wet.
    #[serde(default = "default_track_mix")]
    pub mix: f32,
    /// Per-track Mute toggle — bypasses this row's effect when `true`.
    /// Serde-defaulted to `false` so older presets load unmuted.
    #[serde(default)]
    pub muted: bool,
    /// Per-track Solo toggle — when any row in the chain is soloed, every
    /// non-soloed row is effectively muted. Serde-defaulted to `false`.
    #[serde(default)]
    pub soloed: bool,
}

/// The serde default for `TrackEffect::mix` — fully wet, matching the
/// pre-`mix` behaviour of any older preset.
fn default_track_mix() -> f32 {
    1.0
}

impl TrackEffect {
    /// The default effect for a track row — no effect, fully wet, neither
    /// muted nor soloed. Audio passes through the track unchanged. Users
    /// assign an effect kind via the editor's dropdown.
    pub fn default_for_row(_row: usize) -> Self {
        TrackEffect {
            kind: EffectKind::None,
            params: [0.0; MAX_EFFECT_PARAMS],
            mix: 1.0,
            muted: false,
            soloed: false,
        }
    }
}

impl Default for TrackEffect {
    fn default() -> Self {
        Self::default_for_row(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svf_effect_parameters_are_declared() {
        let svf = SvfEffect::new();
        let specs = svf.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Cutoff");
        assert_eq!(specs[1].name, "Resonance");
        assert_eq!(specs[2].name, "Type");
        assert_eq!(specs[3].name, "Poles");
        assert!(specs[0].min < specs[0].max);
    }

    #[test]
    fn lowpass_effect_dark_cutoff_attenuates_highs() {
        let mut lp = SvfEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(0, 200.0);
        lp.set_param(1, 0.0);
        let mut peak = 0.0_f32;
        for i in 0..2048 {
            let x = if i % 2 == 0 { 1.0 } else { -1.0 };
            let (l, _) = lp.process_sample(x, x);
            if i > 256 {
                peak = peak.max(l.abs());
            }
        }
        assert!(
            peak < 0.5,
            "a 200 Hz lowpass should kill a fast alternation, got {peak}"
        );
    }

    #[test]
    fn lowpass_effect_open_cutoff_passes_a_constant() {
        let mut lp = SvfEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(0, 18_000.0);
        lp.set_param(1, 0.0);
        let mut y = 0.0;
        for _ in 0..2048 {
            y = lp.process_sample(1.0, 1.0).0;
        }
        assert!(y > 0.9, "an open lowpass should pass a constant, got {y}");
    }

    #[test]
    fn lowpass_effect_reset_clears_state() {
        let mut lp = SvfEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(0, 300.0);
        for _ in 0..512 {
            lp.process_sample(1.0, 1.0);
        }
        lp.reset();
        let y = lp.process_sample(1.0, 1.0).0;
        assert!(y.abs() < 0.5, "reset should clear filter state, got {y}");
    }

    #[test]
    fn lowpass_effect_set_param_out_of_range_is_ignored() {
        let mut lp = SvfEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(99, 1.0);
        let y = lp.process_sample(0.25, 0.25);
        assert!(y.0.is_finite());
    }

    #[test]
    fn bitcrush_effect_parameters_are_declared() {
        let bc = BitcrushEffect::new();
        let specs = bc.parameters();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "Bit Depth");
        assert_eq!(specs[1].name, "Rate Reduction");
    }

    #[test]
    fn bitcrush_effect_low_bit_depth_quantizes_coarsely() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(0, 2.0);
        bc.set_param(1, 1.0);
        let crushed = bc.process_sample(0.1, 0.1).0;
        bc.set_param(0, 16.0);
        let clean = bc.process_sample(0.1, 0.1).0;
        assert!(
            (crushed - 0.1).abs() > (clean - 0.1).abs(),
            "2-bit ({crushed}) should distort more than 16-bit ({clean})"
        );
    }

    #[test]
    fn bitcrush_effect_rate_reduction_holds_samples() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(0, 16.0);
        bc.set_param(1, 4.0);
        let first = bc.process_sample(1.0, 1.0).0;
        let held = bc.process_sample(-1.0, -1.0).0;
        assert!(
            (first - held).abs() < 1e-6,
            "rate reduction should hold the sample"
        );
    }

    #[test]
    fn bitcrush_effect_output_is_bounded() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(0, 3.0);
        for &x in &[-1.0_f32, -0.3, 0.0, 0.42, 1.0] {
            let (l, r) = bc.process_sample(x, x);
            assert!(
                l.abs() <= 1.5 && r.abs() <= 1.5,
                "x {x} -> ({l},{r}) out of range"
            );
        }
    }

    #[test]
    fn bitcrush_effect_reset_clears_hold_state() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(1, 8.0);
        bc.process_sample(0.7, 0.7);
        bc.reset();
        let y = bc.process_sample(0.25, 0.25).0;
        assert!((y - 0.25).abs() < 0.1, "reset should re-sample, got {y}");
    }

    #[test]
    fn effect_kind_registry() {
        assert_eq!(EffectKind::ALL.len(), 12);
        assert_eq!(EffectKind::None.name(), "None");
        assert_eq!(EffectKind::Svf.name(), "SVF");
        assert_eq!(EffectKind::Bitcrush.name(), "Bitcrush");
        assert_eq!(EffectKind::Fm.name(), "FM");
        assert_eq!(EffectKind::Delay.name(), "Delay");
        assert_eq!(EffectKind::Phaser.name(), "Phaser");
        assert_eq!(EffectKind::WarpZone.name(), "Warp Zone");
        assert_eq!(EffectKind::Satch.name(), "Satch");
        assert_eq!(EffectKind::Repeat.name(), "Repeat");
        assert_eq!(EffectKind::Stretch.name(), "Stretch");
        assert_eq!(EffectKind::Comb.name(), "Comb");
        assert_eq!(EffectKind::Ring.name(), "Ring");
    }

    #[test]
    fn none_effect_outputs_silence() {
        // An unassigned track must not forward audio: a row with no effect
        // contributes nothing to the wet sum, regardless of input level.
        let mut e = NoneEffect::new();
        assert_eq!(e.process_sample(0.5, -0.3), (0.0, 0.0));
        assert_eq!(e.process_sample(1.0, 1.0), (0.0, 0.0));
        assert_eq!(e.parameters().len(), 0);
        e.set_param(0, 1.0); // no-op
        e.set_sample_rate(48_000.0); // no-op
        e.reset(); // no-op
    }

    #[test]
    fn default_params_for_kind_none_is_all_zero() {
        assert_eq!(
            default_params_for_kind(EffectKind::None),
            [0.0; MAX_EFFECT_PARAMS]
        );
        assert_eq!(param_count(EffectKind::None), 0);
    }

    #[test]
    fn effect_instance_dispatches_to_the_right_effect() {
        let mut lp = EffectInstance::new(EffectKind::Svf);
        assert_eq!(lp.kind(), EffectKind::Svf);
        assert_eq!(lp.parameters().len(), 4);
        let mut bc = EffectInstance::new(EffectKind::Bitcrush);
        assert_eq!(bc.kind(), EffectKind::Bitcrush);
        lp.set_sample_rate(48_000.0);
        bc.set_sample_rate(48_000.0);
        let _ = lp.process_sample(0.5, 0.5);
        let _ = bc.process_sample(0.5, 0.5);
        lp.reset();
        bc.reset();
    }

    #[test]
    fn effect_instance_set_param_changes_behaviour() {
        let mut e = EffectInstance::new(EffectKind::Svf);
        e.set_sample_rate(48_000.0);
        e.set_param(0, 200.0);
        e.set_param(1, 0.0);
        let mut peak = 0.0_f32;
        for i in 0..2048 {
            let x = if i % 2 == 0 { 1.0 } else { -1.0 };
            let (l, _) = e.process_sample(x, x);
            if i > 256 {
                peak = peak.max(l.abs());
            }
        }
        assert!(
            peak < 0.5,
            "the dispatched lowpass should attenuate, got {peak}"
        );
    }

    #[test]
    fn track_effect_serde_round_trips() {
        let te = TrackEffect {
            kind: EffectKind::Bitcrush,
            params: [3.0, 8.0, 0.0, 0.0, 0.0],
            mix: 1.0,
            muted: false,
            soloed: false,
        };
        let json = serde_json::to_string(&te).unwrap();
        let back: TrackEffect = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, EffectKind::Bitcrush);
        assert_eq!(back.params, [3.0, 8.0, 0.0, 0.0, 0.0]);
        assert_eq!(back.mix, 1.0);
    }

    #[test]
    fn track_effect_default_is_fully_wet() {
        assert_eq!(TrackEffect::default_for_row(0).mix, 1.0);
        assert_eq!(TrackEffect::default().mix, 1.0);
    }

    #[test]
    fn track_effect_mix_round_trips_through_serde() {
        let mut te = TrackEffect::default_for_row(0);
        te.mix = 0.35;
        let json = serde_json::to_string(&te).unwrap();
        let back: TrackEffect = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mix, 0.35);
    }

    #[test]
    fn track_effect_legacy_blob_without_mix_loads_fully_wet() {
        // A TrackEffect JSON saved before the `mix` field existed.
        let legacy = r#"{"kind":"Svf","params":[0.0,0.0,0.0,0.0,0.0]}"#;
        let te: TrackEffect = serde_json::from_str(legacy).expect("legacy blob must load");
        assert_eq!(te.mix, 1.0);
    }

    #[test]
    fn track_effect_array_serde_round_trips() {
        let config: [TrackEffect; 16] = std::array::from_fn(TrackEffect::default_for_row);
        let json = serde_json::to_string(&config).unwrap();
        let back: [TrackEffect; 16] = serde_json::from_str(&json).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn default_for_row_is_none_for_every_track() {
        // Every track defaults to the passthrough None effect; users assign
        // an effect kind via the editor's dropdown.
        let config: [TrackEffect; 16] = std::array::from_fn(TrackEffect::default_for_row);
        assert!(config.iter().all(|t| t.kind == EffectKind::None));
        assert!(config.iter().all(|t| t.params == [0.0; MAX_EFFECT_PARAMS]));
    }

    #[test]
    fn default_params_for_kind_matches_the_kinds_specs() {
        let lp = default_params_for_kind(EffectKind::Svf);
        assert_eq!(lp[0], SvfEffect::new().parameters()[0].default);
        assert_eq!(lp[1], SvfEffect::new().parameters()[1].default);
        // Slots past the kind's parameter count are zero.
        assert_eq!(lp[2], 0.0);
        assert_eq!(lp[3], 0.0);
        let bc = default_params_for_kind(EffectKind::Bitcrush);
        assert_eq!(bc[0], BitcrushEffect::new().parameters()[0].default);
    }

    #[test]
    fn param_count_reports_each_kinds_arity() {
        assert_eq!(param_count(EffectKind::Svf), 4);
        assert_eq!(param_count(EffectKind::Bitcrush), 2);
    }

    #[test]
    fn value_to_norm_linear_round_trips() {
        // Linear: midpoint of 0..1 is 0.5; midpoint of 20..40 is 0.5.
        assert!((value_to_norm(0.5, 0.0, 1.0, ParamScaling::Linear) - 0.5).abs() < 1e-6);
        assert!((value_to_norm(30.0, 20.0, 40.0, ParamScaling::Linear) - 0.5).abs() < 1e-6);
        // Round trip.
        for v in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            let n = value_to_norm(v, 0.0, 1.0, ParamScaling::Linear);
            let back = norm_to_value(n, 0.0, 1.0, ParamScaling::Linear);
            assert!((back - v).abs() < 1e-6, "v={v} n={n} back={back}");
        }
        // Out of range clamps.
        assert_eq!(value_to_norm(-1.0, 0.0, 1.0, ParamScaling::Linear), 0.0);
        assert_eq!(value_to_norm(2.0, 0.0, 1.0, ParamScaling::Linear), 1.0);
    }

    #[test]
    fn value_to_norm_log_round_trips() {
        // Log: 20 -> 0.0, 20000 -> 1.0, midpoint (geometric mean) ≈ 632 Hz.
        assert!((value_to_norm(20.0, 20.0, 20_000.0, ParamScaling::Log) - 0.0).abs() < 1e-4);
        assert!((value_to_norm(20_000.0, 20.0, 20_000.0, ParamScaling::Log) - 1.0).abs() < 1e-4);
        // 20 * sqrt(1000) ≈ 632.4555
        let geo = 20.0_f32 * 1000.0_f32.sqrt();
        assert!((value_to_norm(geo, 20.0, 20_000.0, ParamScaling::Log) - 0.5).abs() < 1e-4);
        // Round trip.
        for v in [20.0_f32, 80.0, 200.0, 2_000.0, 20_000.0] {
            let n = value_to_norm(v, 20.0, 20_000.0, ParamScaling::Log);
            let back = norm_to_value(n, 20.0, 20_000.0, ParamScaling::Log);
            assert!((back - v).abs() / v < 1e-4, "v={v} n={n} back={back}");
        }
        // Out of range clamps; degenerate (min<=0) returns 0.
        assert_eq!(value_to_norm(1.0, 20.0, 20_000.0, ParamScaling::Log), 0.0);
        assert_eq!(
            value_to_norm(40_000.0, 20.0, 20_000.0, ParamScaling::Log),
            1.0
        );
        assert_eq!(value_to_norm(5.0, 0.0, 100.0, ParamScaling::Log), 0.0);
    }

    #[test]
    fn format_value_number_with_and_without_unit() {
        assert_eq!(
            format_value(
                0.15,
                ParamFormat::Number {
                    decimals: 2,
                    unit: ""
                }
            ),
            "0.15"
        );
        assert_eq!(
            format_value(
                8.0,
                ParamFormat::Number {
                    decimals: 0,
                    unit: "bits"
                }
            ),
            "8 bits"
        );
        assert_eq!(
            format_value(
                4.0,
                ParamFormat::Number {
                    decimals: 0,
                    unit: "x"
                }
            ),
            "4 x"
        );
    }

    #[test]
    fn format_value_hertz_auto_scales() {
        assert_eq!(format_value(0.05, ParamFormat::Hertz), "0.05 Hz");
        assert_eq!(format_value(80.0, ParamFormat::Hertz), "80 Hz");
        assert_eq!(format_value(2_000.0, ParamFormat::Hertz), "2.0 kHz");
        assert_eq!(format_value(18_500.0, ParamFormat::Hertz), "18.5 kHz");
    }

    #[test]
    fn format_value_bare_drops_unit_and_khz_scaling() {
        // Number: drop the unit suffix; keep decimals.
        assert_eq!(
            format_value_bare(
                8.0,
                ParamFormat::Number {
                    decimals: 0,
                    unit: "bits"
                }
            ),
            "8"
        );
        assert_eq!(
            format_value_bare(
                0.15,
                ParamFormat::Number {
                    decimals: 2,
                    unit: ""
                }
            ),
            "0.15"
        );
        // Hertz: stay in Hz units regardless of magnitude.
        assert_eq!(format_value_bare(0.05, ParamFormat::Hertz), "0.05");
        assert_eq!(format_value_bare(80.0, ParamFormat::Hertz), "80");
        assert_eq!(format_value_bare(2_000.0, ParamFormat::Hertz), "2000");
        assert_eq!(format_value_bare(18_500.0, ParamFormat::Hertz), "18500");
    }

    #[test]
    fn parse_value_number_strips_unit() {
        let fmt = ParamFormat::Number {
            decimals: 0,
            unit: "bits",
        };
        assert_eq!(parse_value("8 bits", fmt), Some(8.0));
        assert_eq!(parse_value("8", fmt), Some(8.0));
        assert_eq!(
            parse_value(
                "0.15",
                ParamFormat::Number {
                    decimals: 2,
                    unit: ""
                }
            ),
            Some(0.15)
        );
        assert_eq!(parse_value("", fmt), None);
        assert_eq!(parse_value("abc", fmt), None);
    }

    #[test]
    fn parse_value_hertz_handles_k_khz_hz() {
        let f = ParamFormat::Hertz;
        assert_eq!(parse_value("80", f), Some(80.0));
        assert_eq!(parse_value("80 Hz", f), Some(80.0));
        assert_eq!(parse_value("80hz", f), Some(80.0));
        assert_eq!(parse_value("2k", f), Some(2_000.0));
        assert_eq!(parse_value("2 kHz", f), Some(2_000.0));
        assert_eq!(parse_value("2.5kHz", f), Some(2_500.0));
        assert_eq!(parse_value("0.5", f), Some(0.5));
        assert_eq!(parse_value("", f), None);
        assert_eq!(parse_value("xyz", f), None);
    }

    #[test]
    fn format_then_parse_round_trips_each_format() {
        let cases: &[(f32, ParamFormat)] = &[
            (
                0.15,
                ParamFormat::Number {
                    decimals: 2,
                    unit: "",
                },
            ),
            (
                8.0,
                ParamFormat::Number {
                    decimals: 0,
                    unit: "bits",
                },
            ),
            (0.05, ParamFormat::Hertz),
            (80.0, ParamFormat::Hertz),
            (2_000.0, ParamFormat::Hertz),
            (18_500.0, ParamFormat::Hertz),
        ];
        for &(v, f) in cases {
            let s = format_value(v, f);
            let back = parse_value(&s, f).unwrap_or_else(|| panic!("parse failed for {s:?}"));
            assert!(
                (back - v).abs() / v.abs().max(1.0) < 0.05,
                "round-trip {v} -> {s} -> {back}"
            );
        }
    }

    #[test]
    fn svf_param_formats_match_spec() {
        let specs = SvfEffect::new().parameters();
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert!(matches!(specs[1].scaling, ParamScaling::Linear));
        assert!(matches!(specs[1].format, ParamFormat::Number { .. }));
        // Type at slot 2: Enum-format dropdown over LP / BP / HP.
        assert_eq!(specs[2].name, "Type");
        if let ParamFormat::Enum { labels } = specs[2].format {
            assert_eq!(labels, &["Lowpass", "Bandpass", "Highpass"]);
        } else {
            panic!("Type spec should use ParamFormat::Enum");
        }
        // Poles at slot 3: Enum-format dropdown over the four cascade
        // lengths (2 / 4 / 6 / 8 poles).
        assert_eq!(specs[3].name, "Poles");
        if let ParamFormat::Enum { labels } = specs[3].format {
            assert_eq!(labels, &["2", "4", "6", "8"]);
        } else {
            panic!("Poles spec should use ParamFormat::Enum");
        }
    }

    #[test]
    fn svf_type_changes_which_band_passes() {
        // LP attenuates ABOVE cutoff; HP attenuates BELOW; BP attenuates
        // BOTH sides. Drive each Type at a 1 kHz cutoff with a sine at
        // 250 Hz (well below cutoff) and at 4 kHz (well above), and
        // check the RMS ratios match each filter's identity.
        let measure = |type_idx: f32, freq_hz: f32| -> f32 {
            let mut svf = SvfEffect::new();
            svf.set_sample_rate(48_000.0);
            svf.set_param(0, 1_000.0); // Cutoff = 1 kHz
            svf.set_param(1, 0.0); // No resonance
            svf.set_param(2, type_idx); // Type
            svf.set_param(3, 0.0); // 2 poles
            for i in 0..2048 {
                let s = (std::f32::consts::TAU * freq_hz * i as f32 / 48_000.0).sin();
                svf.process_sample(s, s);
            }
            let mut sum_sq = 0.0_f32;
            for i in 2048..(2048 + 4096) {
                let s = (std::f32::consts::TAU * freq_hz * i as f32 / 48_000.0).sin();
                let (l, _r) = svf.process_sample(s, s);
                sum_sq += l * l;
            }
            (sum_sq / 4096.0).sqrt()
        };
        // LP: 250 Hz passes (≈ unity input RMS ≈ 0.707), 4 kHz attenuated.
        let lp_low = measure(0.0, 250.0);
        let lp_high = measure(0.0, 4_000.0);
        assert!(
            lp_low > 0.5 && lp_high < 0.2,
            "LP should pass 250 Hz and attenuate 4 kHz (low={lp_low}, high={lp_high})"
        );
        // HP: opposite — low attenuated, high passes.
        let hp_low = measure(2.0, 250.0);
        let hp_high = measure(2.0, 4_000.0);
        assert!(
            hp_low < 0.2 && hp_high > 0.5,
            "HP should attenuate 250 Hz and pass 4 kHz (low={hp_low}, high={hp_high})"
        );
        // BP: both bands attenuated relative to LP-passing or HP-passing
        // levels; the 1 kHz cutoff itself is the peak (we don't measure
        // it here, just confirm the off-band attenuation).
        let bp_low = measure(1.0, 250.0);
        let bp_high = measure(1.0, 4_000.0);
        assert!(
            bp_low < lp_low && bp_high < hp_high,
            "BP should attenuate both bands relative to their passing types \
             (bp_low={bp_low} vs lp_low={lp_low}, bp_high={bp_high} vs hp_high={hp_high})"
        );
    }

    #[test]
    fn lowpass_higher_pole_count_attenuates_above_cutoff_more() {
        // Decade-above-cutoff response should grow steeper with more
        // stages: each additional 2-pole stage adds 12 dB/oct of rolloff,
        // so the RMS at 10× the cutoff is monotonically smaller as the
        // pole count rises 2 → 8.
        let measure_rms_decade_above = |poles_idx: f32| -> f32 {
            let mut lp = SvfEffect::new();
            lp.set_sample_rate(48_000.0);
            lp.set_param(0, 1_000.0); // Cutoff = 1 kHz
            lp.set_param(1, 0.0); // Resonance = 0 (no peaking)
            lp.set_param(3, poles_idx); // Poles index (slot 3)
            let sr = 48_000.0_f32;
            let f_test = 10_000.0_f32; // one decade above cutoff
                                       // Warm up the cascade, then measure 4096 samples of the
                                       // single-channel RMS.
            for i in 0..2048 {
                let s = (std::f32::consts::TAU * f_test * i as f32 / sr).sin();
                lp.process_sample(s, s);
            }
            let mut sum_sq = 0.0_f32;
            for i in 2048..(2048 + 4096) {
                let s = (std::f32::consts::TAU * f_test * i as f32 / sr).sin();
                let (l, _r) = lp.process_sample(s, s);
                sum_sq += l * l;
            }
            (sum_sq / 4096.0).sqrt()
        };
        let rms_2 = measure_rms_decade_above(0.0); // 2 poles
        let rms_4 = measure_rms_decade_above(1.0); // 4 poles
        let rms_6 = measure_rms_decade_above(2.0); // 6 poles
        let rms_8 = measure_rms_decade_above(3.0); // 8 poles
                                                   // Strict ordering: each step adds at least some attenuation. (The
                                                   // exact ratio is 1 / 4^N for N additional 2-pole stages — but
                                                   // even with shared coefficients we expect a clear monotone
                                                   // ordering on a steady sine well above cutoff.)
        assert!(
            rms_2 > rms_4 && rms_4 > rms_6 && rms_6 > rms_8,
            "rolloff at 10× cutoff should strictly steepen with pole count \
             (2p={rms_2}, 4p={rms_4}, 6p={rms_6}, 8p={rms_8})"
        );
        // Sanity: 8 poles should be substantially quieter than 2 poles.
        assert!(
            rms_8 < rms_2 * 0.25,
            "8-pole at 10× cutoff should be much quieter than 2-pole \
             (2p={rms_2}, 8p={rms_8})"
        );
    }

    #[test]
    fn bitcrush_param_formats_carry_their_units() {
        let specs = BitcrushEffect::new().parameters();
        if let ParamFormat::Number { unit, .. } = specs[0].format {
            assert_eq!(unit, "bits");
        } else {
            panic!("bit-depth format should be Number");
        }
        if let ParamFormat::Number { unit, .. } = specs[1].format {
            assert_eq!(unit, "x");
        } else {
            panic!("rate-reduction format should be Number");
        }
    }

    #[test]
    fn enum_format_renders_the_label_at_the_rounded_index() {
        let labels: &[&str] = &["Carrier", "Modulator"];
        let format = ParamFormat::Enum { labels };
        assert_eq!(format_value(0.0, format), "Carrier");
        assert_eq!(format_value(0.49, format), "Carrier");
        assert_eq!(format_value(0.5, format), "Modulator");
        assert_eq!(format_value(1.0, format), "Modulator");
        // Out-of-range values clamp to the nearest end.
        assert_eq!(format_value(-1.0, format), "Carrier");
        assert_eq!(format_value(99.0, format), "Modulator");
    }

    #[test]
    fn enum_format_parses_labels_case_insensitively_or_a_numeric_index() {
        let labels: &[&str] = &["Carrier", "Modulator"];
        let format = ParamFormat::Enum { labels };
        assert_eq!(parse_value("Carrier", format), Some(0.0));
        assert_eq!(parse_value("modulator", format), Some(1.0));
        assert_eq!(parse_value("CARRIER", format), Some(0.0));
        // Numeric fallback so a copy-pasted raw value still works.
        assert_eq!(parse_value("0", format), Some(0.0));
        assert_eq!(parse_value("1", format), Some(1.0));
        // Unknown label fails.
        assert!(parse_value("frobnicate", format).is_none());
    }

    #[test]
    fn fm_effect_lists_five_parameters_with_the_expected_specs() {
        let fm = FmEffect::new();
        let specs = fm.parameters();
        assert_eq!(specs.len(), 5);
        // Freq is param 0 so the default `targets[0] = Some(0)` modulation
        // assignment naturally points at the most useful audible parameter.
        assert_eq!(specs[0].name, "Freq");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert_eq!(specs[1].name, "Depth");
        assert_eq!(specs[2].name, "Feedback");
        // Mode and Topology are Enum-format — the editor renders dropdowns
        // for both, not dials.
        assert_eq!(specs[3].name, "Mode");
        assert!(matches!(specs[3].format, ParamFormat::Enum { .. }));
        assert_eq!(specs[4].name, "Topology");
        assert!(matches!(specs[4].format, ParamFormat::Enum { .. }));
    }

    #[test]
    fn fm_mode_set_param_rounds_to_zero_or_one() {
        // Mode is at param index 3. Any value < 0.5 collapses to Carrier (0);
        // ≥ 0.5 to Modulator (1). With Mode = Modulator and Depth = 0, the
        // carrier sine is gated by the input envelope — so a constant unity
        // input plays the bare carrier at full amplitude.
        let mut fm = FmEffect::new();
        fm.set_sample_rate(48_000.0);
        fm.set_param(3, 0.51); // Mode → Modulator
        fm.set_param(0, 200.0); // Freq
        fm.set_param(1, 0.0); // Depth
        fm.set_param(2, 0.0); // Feedback
                              // Warm up the input-gate envelope follower (~5 attack TCs).
        for _ in 0..256 {
            fm.process_sample(1.0, 1.0);
        }
        let mut max_abs = 0.0_f32;
        for _ in 0..1024 {
            let (l, r) = fm.process_sample(1.0, 1.0);
            max_abs = max_abs.max(l.abs().max(r.abs()));
        }
        assert!(
            max_abs > 0.5,
            "Modulator mode with unity input + depth=0 must produce its carrier sine"
        );

        // Below the half-way threshold rounds to Carrier — silent input
        // produces silence (delay line is full of zeros).
        let mut fm2 = FmEffect::new();
        fm2.set_sample_rate(48_000.0);
        fm2.set_param(3, 0.3); // Mode → Carrier
        for _ in 0..1024 {
            let (l, _r) = fm2.process_sample(0.0, 0.0);
            assert_eq!(l, 0.0);
        }
    }

    #[test]
    fn fm_carrier_mode_with_depth_zero_passes_the_input_through_unchanged() {
        // Carrier mode at depth = 0 means the rotation angle θ stays at 0,
        // so the analytic signal is rotated by 0 cycles — i.e. the output
        // equals the (delay-matched real branch of the) input. After the
        // Hilbert FIR's warm-up, constant 0.5 input gives constant 0.5
        // output on both channels.
        let mut fm = FmEffect::new();
        fm.set_sample_rate(48_000.0);
        fm.set_param(3, 0.0); // Mode → Carrier
        fm.set_param(0, 5.0); // Freq
        fm.set_param(1, 0.0); // Depth = 0 — no modulation, identity rotation
        fm.set_param(2, 0.0); // Feedback
        let mut last = (0.0_f32, 0.0_f32);
        for _ in 0..1024 {
            last = fm.process_sample(0.5, 0.5);
        }
        assert!(
            (last.0 - 0.5).abs() < 1e-3,
            "after warm-up, output L should match input ({:?})",
            last
        );
        assert!((last.1 - 0.5).abs() < 1e-3);
    }

    #[test]
    fn fm_modulator_mode_feedback_stays_audibly_active_across_the_range() {
        // Modulator mode is now an input-gated PM operator. Driving with a
        // constant unity input warms the gate envelope to ~1.0, so the
        // carrier plays at full amplitude and feedback's timbral change
        // is observable. Three feedback settings must (a) preserve the
        // carrier at fb = 0, (b) stay bounded and audible at fb = 100 %,
        // and (c) measurably change the crest factor at intermediate
        // settings.
        let measure_at_fb = |fb_pct: f32| -> (f32, f32) {
            let mut fm = FmEffect::new();
            fm.set_sample_rate(48_000.0);
            fm.set_param(3, 1.0); // Mode → Modulator
            fm.set_param(0, 200.0); // Freq
            fm.set_param(1, 0.0); // Depth = 0
            fm.set_param(2, fb_pct);
            // Warm up the input-gate envelope follower with constant input.
            for _ in 0..2048 {
                fm.process_sample(1.0, 1.0);
            }
            let mut sum_sq = 0.0_f32;
            let mut peak = 0.0_f32;
            for _ in 0..2048 {
                let (l, _r) = fm.process_sample(1.0, 1.0);
                sum_sq += l * l;
                peak = peak.max(l.abs());
            }
            ((sum_sq / 2048.0).sqrt(), peak)
        };
        let (rms_0, peak_0) = measure_at_fb(0.0);
        let (rms_50, peak_50) = measure_at_fb(50.0);
        let (rms_100, peak_100) = measure_at_fb(100.0);
        // fb=0 with the gate fully open: a 200 Hz sine — RMS ≈ 1/√2 ≈ 0.707,
        // peak ≈ 1.
        assert!(
            (rms_0 - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.05,
            "fb=0 RMS should be ~0.707, got {rms_0}"
        );
        assert!(
            (peak_0 - 1.0).abs() < 0.05,
            "fb=0 peak should be ~1.0, got {peak_0}"
        );
        // fb=100%: still audibly present, still bounded.
        assert!(
            rms_100 > 0.1,
            "fb=100% should still produce audible output (RMS > 0.1), got {rms_100}"
        );
        assert!(
            peak_100 < 1.5,
            "fb=100% output should be bounded (peak < 1.5), got {peak_100}"
        );
        // Self-feedback enriches the carrier: the waveform drifts away from
        // a pure sine, so the crest factor (peak / RMS) changes measurably
        // between fb=0 and fb=50%.
        let crest_0 = peak_0 / rms_0;
        let crest_50 = peak_50 / rms_50;
        assert!(
            (crest_50 - crest_0).abs() > 0.05,
            "feedback should change the carrier's timbre \
             (crest@0={crest_0}, crest@50={crest_50})"
        );
    }

    #[test]
    fn fm_modulator_mode_silent_input_yields_silent_output() {
        // The input-gate keeps the carrier asleep until there's input. A
        // pristine silent input must yield exact-zero output forever — this
        // is the fix for "Modulator mode keeps playing while the transport
        // is stopped".
        let mut fm = FmEffect::new();
        fm.set_sample_rate(48_000.0);
        fm.set_param(3, 1.0); // Mode → Modulator
        fm.set_param(0, 100.0); // Freq
        fm.set_param(1, 0.0); // Depth
        fm.set_param(2, 0.0); // Feedback
        for _ in 0..4096 {
            let (l, r) = fm.process_sample(0.0, 0.0);
            assert_eq!(l, 0.0);
            assert_eq!(r, 0.0);
        }
    }

    #[test]
    fn fm_modulator_mode_with_constant_input_plays_a_pure_sine_at_the_carrier_freq() {
        // Drive with constant unity input so the input-gate envelope settles
        // to ~1.0. With depth=0 and fb=0 the output is then a clean 100 Hz
        // sine at the carrier frequency.
        let mut fm = FmEffect::new();
        fm.set_sample_rate(48_000.0);
        fm.set_param(3, 1.0); // Mode → Modulator
        fm.set_param(0, 100.0); // Freq
        fm.set_param(1, 0.0); // Depth
        fm.set_param(2, 0.0); // Feedback
                              // Settle the input-gate envelope follower.
        for _ in 0..2048 {
            fm.process_sample(1.0, 1.0);
        }
        // Measure period by finding zero-crossings.
        let mut zero_crossings = 0;
        let mut prev = 0.0_f32;
        for _ in 0..(48_000 / 10) {
            let (l, _r) = fm.process_sample(1.0, 1.0);
            if prev <= 0.0 && l > 0.0 {
                zero_crossings += 1;
            }
            prev = l;
        }
        // 0.1 s of a 100 Hz sine has exactly 10 positive-going zero crossings.
        assert!(
            (8..=12).contains(&zero_crossings),
            "expected ~10 positive zero crossings of a 100 Hz sine in 100 ms, got {zero_crossings}"
        );
    }

    #[test]
    fn fm_reset_clears_state_and_returns_silence() {
        let mut fm = FmEffect::new();
        fm.set_sample_rate(48_000.0);
        fm.set_param(3, 1.0); // Mode → Modulator (visible non-zero output)
        fm.set_param(0, 200.0); // Freq
                                // Drive it for a while to fill delay lines and advance phases.
        for _ in 0..1024 {
            fm.process_sample(0.4, 0.4);
        }
        fm.reset();
        // Switch to Carrier mode. Reset zeroed the delay line, so a silent
        // input produces exactly silence.
        fm.set_param(3, 0.0);
        let (l, r) = fm.process_sample(0.0, 0.0);
        assert_eq!(
            l, 0.0,
            "after reset Carrier mode on silent input is silence"
        );
        assert_eq!(r, 0.0);
    }

    #[test]
    fn fm_carrier_mode_feedback_changes_timbre_audibly() {
        // Carrier mode now routes the input through an analytic-signal
        // rotation, with feedback adding the previous output back into the
        // rotation phase (DX7-style operator self-modulation). Different
        // feedback settings should produce audibly different output
        // sequences on the same input. The output stays bounded at the
        // upper end — no runaway.
        let render = |fb_pct: f32, topology: f32| -> (Vec<f32>, f32) {
            let mut fm = FmEffect::new();
            fm.set_sample_rate(48_000.0);
            fm.set_param(3, 0.0); // Carrier
            fm.set_param(4, topology); // Topology
            fm.set_param(0, 5.0); // Freq (LFO rate)
            fm.set_param(1, 50.0); // Depth = 50%
            fm.set_param(2, fb_pct);
            let two_pi = std::f32::consts::TAU;
            let input = |i: usize| (two_pi * 1000.0 * i as f32 / 48_000.0).sin() * 0.5;
            // Warm up the Hilbert FIR plus the LFO.
            for i in 0..1024 {
                fm.process_sample(input(i), input(i));
            }
            let mut out = Vec::with_capacity(2048);
            let mut peak = 0.0_f32;
            for i in 1024..(1024 + 2048) {
                let (l, _r) = fm.process_sample(input(i), input(i));
                out.push(l);
                peak = peak.max(l.abs());
            }
            (out, peak)
        };
        // PM and FM topologies both — feedback should change the output.
        for &topology in &[0.0_f32, 1.0_f32] {
            let (out_0, peak_0) = render(0.0, topology);
            let (out_50, peak_50) = render(50.0, topology);
            let (_out_90, peak_90) = render(90.0, topology);
            let mean_abs_diff = out_0
                .iter()
                .zip(&out_50)
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>()
                / out_0.len() as f32;
            assert!(
                mean_abs_diff > 0.01,
                "topology {topology}: feedback must change the output (mean diff {mean_abs_diff})"
            );
            // Bounded: no setting drives the output above a safety ceiling.
            assert!(
                peak_0 < 2.0 && peak_50 < 2.0 && peak_90 < 2.0,
                "topology {topology}: outputs must stay bounded \
                 (peaks {peak_0}, {peak_50}, {peak_90})"
            );
        }
    }

    #[test]
    fn effect_kind_all_includes_fm() {
        assert!(EffectKind::ALL.iter().any(|&k| k == EffectKind::Fm));
        assert_eq!(EffectKind::Fm.name(), "FM");
        assert_eq!(param_count(EffectKind::Fm), 5);
        let defaults = default_params_for_kind(EffectKind::Fm);
        assert_eq!(defaults[0], 100.0); // Freq: 100 Hz
        assert_eq!(defaults[3], 0.0); // Mode: Carrier
        assert_eq!(defaults[4], 0.0); // Topology: PM
    }

    #[test]
    fn fm_modulator_topology_changes_spectral_content_with_audio_rate_modulator() {
        // For the same depth knob value, FM's effective modulation index at
        // modulator frequency `f_m` is `depth · sr / (2π · f_m)` while PM's
        // is `depth · 2π`. At depth = 0.5 with `f_m` = 200 Hz / sr = 48 kHz,
        // β_FM ≈ 19 and β_PM ≈ 3.14 — FM has ~6× the modulation index and
        // its output is dramatically richer in upper harmonics. The
        // sum-of-absolute-differences between consecutive samples is a
        // crude but effective proxy for that high-frequency content.
        let measure = |topology: f32| -> f32 {
            let mut fm = FmEffect::new();
            fm.set_sample_rate(48_000.0);
            fm.set_param(3, 1.0); // Mode → Modulator
            fm.set_param(4, topology); // Topology
            fm.set_param(0, 1000.0); // Freq: 1 kHz carrier
            fm.set_param(1, 50.0); // Depth = 50 %
            fm.set_param(2, 0.0); // Feedback = 0
            let two_pi = std::f32::consts::TAU;
            let m = |i: usize| (two_pi * 200.0 * i as f32 / 48_000.0).sin() * 0.5;
            // Warm up the input-gate envelope follower.
            for i in 0..2048 {
                fm.process_sample(m(i), m(i));
            }
            let mut prev = 0.0_f32;
            let mut sum_abs_diff = 0.0_f32;
            for i in 2048..(2048 + 4096) {
                let (l, _r) = fm.process_sample(m(i), m(i));
                sum_abs_diff += (l - prev).abs();
                prev = l;
            }
            sum_abs_diff
        };
        let pm_swing = measure(0.0);
        let fm_swing = measure(1.0);
        // FM with ~6× the modulation index should have substantially more
        // high-frequency content than PM at the same depth.
        assert!(
            fm_swing > pm_swing * 1.5,
            "FM topology should produce more spectral content than PM at \
             the same depth knob (PM swing = {pm_swing}, FM swing = {fm_swing})"
        );
    }

    #[test]
    fn fm_modulator_mode_with_topology_fm_lets_input_bend_carrier_pitch() {
        // True FM (Topology = 1) adds `depth · input` to the phase
        // INCREMENT, so a constant positive input bias permanently raises
        // the carrier's instantaneous pitch. The same setup under PM
        // (Topology = 0) keeps the base pitch fixed and just adds a
        // constant phase offset. Detect the difference by counting
        // zero-crossings over a fixed window with a positive DC bias.
        let count_pos_zcs = |topology: f32| -> i32 {
            let mut fm = FmEffect::new();
            fm.set_sample_rate(48_000.0);
            fm.set_param(3, 1.0); // Modulator
            fm.set_param(4, topology); // Topology
            fm.set_param(0, 100.0); // Freq
            fm.set_param(1, 50.0); // Depth = 50 % so the FM contribution is sizeable
            fm.set_param(2, 0.0); // Feedback = 0
            for _ in 0..2048 {
                // Warm up the input-gate envelope follower.
                fm.process_sample(0.5, 0.5);
            }
            let mut zcs = 0;
            let mut prev = 0.0_f32;
            for _ in 0..48_000 / 10 {
                // 0.1 s window.
                let (l, _r) = fm.process_sample(0.5, 0.5);
                if prev <= 0.0 && l > 0.0 {
                    zcs += 1;
                }
                prev = l;
            }
            zcs
        };
        let pm_zcs = count_pos_zcs(0.0);
        let fm_zcs = count_pos_zcs(1.0);
        // PM with DC bias only shifts the phase by a constant — the carrier
        // still runs at exactly 100 Hz, giving ≈ 10 positive zero-crossings.
        assert!(
            (8..=12).contains(&pm_zcs),
            "PM with a constant input bias should run at the carrier rate (~10 ZCs), got {pm_zcs}"
        );
        // Under true FM, the +0.5 DC bias adds `depth · 0.5 = 0.25` cycles/
        // sample to the phase increment, so the instantaneous frequency
        // jumps by sr · 0.25 — orders of magnitude above 100 Hz. The
        // crossings count is dramatically higher.
        assert!(
            fm_zcs > pm_zcs * 5,
            "FM topology should bend the carrier pitch noticeably above PM \
             (PM={pm_zcs}, FM={fm_zcs})"
        );
    }

    #[test]
    fn delay_effect_lists_four_parameters_with_the_expected_specs() {
        let d = DelayEffect::new();
        let specs = d.parameters();
        assert_eq!(specs.len(), 4);
        // Free at slot 0 so a fresh delay uses the continuous ms knob,
        // and the default MSEG target (Some(0)) modulates a useful
        // continuous param rather than rhythmically switching sync
        // subdivisions.
        assert_eq!(specs[0].name, "Free");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert_eq!(specs[1].name, "Time");
        assert!(matches!(specs[1].format, ParamFormat::Enum { .. }));
        // Default Time = the trailing "Free" slot, so the dropdown
        // out of the box defers to the Free ms dial.
        let max_time_idx = (DELAY_TIME_LABELS.len() - 1) as f32;
        assert_eq!(specs[1].default, max_time_idx);
        assert_eq!(specs[2].name, "Feedback");
        assert_eq!(specs[3].name, "Duck");
    }

    #[test]
    fn delay_free_time_produces_a_delayed_echo_of_the_input() {
        // Free-mode at 100 ms, feedback = 0, duck = 0. Drive an impulse,
        // then expect a single echo ~100 ms (= 4800 samples at 48 kHz)
        // later as `output - input`. Additive output: out = dry + delayed.
        let mut d = DelayEffect::new();
        d.set_sample_rate(48_000.0);
        d.set_param(0, 100.0); // Free = 100 ms
        d.set_param(1, 14.0); // Time → Free
        d.set_param(2, 0.0); // Feedback = 0
        d.set_param(3, 0.0); // Duck = 0
                             // Impulse at sample 0, then silence — count where the echo lands.
        let (l, _r) = d.process_sample(1.0, 1.0);
        // At t=0 the input is dry + (whatever the delay tap reads from
        // the still-empty buffer), so just the dry pass-through.
        assert!(
            (l - 1.0).abs() < 1e-6,
            "t=0 output should be the dry impulse"
        );
        let mut echo_peak_idx = 0usize;
        let mut echo_peak_val = 0.0_f32;
        for i in 1..12_000 {
            let (l, _r) = d.process_sample(0.0, 0.0);
            if l.abs() > echo_peak_val {
                echo_peak_val = l.abs();
                echo_peak_idx = i;
            }
        }
        // 100 ms @ 48 kHz = 4 800 samples; with linear-interp reads
        // the peak lands within a sample of that.
        assert!(
            echo_peak_idx >= 4_795 && echo_peak_idx <= 4_805,
            "echo peak should land near sample 4800, got {echo_peak_idx}"
        );
        assert!(
            echo_peak_val > 0.95,
            "echo should preserve the impulse level at fb=0, got {echo_peak_val}"
        );
    }

    #[test]
    fn delay_feedback_extends_the_echo_train() {
        // Higher feedback = more decaying repeats. Drive an impulse and
        // sum |output| over a long window; total energy must grow
        // monotonically with feedback and stay bounded at the cap.
        let render_energy = |fb_pct: f32| -> f32 {
            let mut d = DelayEffect::new();
            d.set_sample_rate(48_000.0);
            d.set_param(0, 50.0); // Free = 50 ms — quick echoes for a short test
            d.set_param(1, 14.0); // Time → Free
            d.set_param(2, fb_pct);
            d.set_param(3, 0.0);
            let mut energy = 0.0_f32;
            let (l, _r) = d.process_sample(1.0, 1.0);
            energy += l.abs();
            for _ in 0..48_000 {
                // 1 second window
                let (l, _r) = d.process_sample(0.0, 0.0);
                energy += l.abs();
            }
            energy
        };
        let e0 = render_energy(0.0);
        let e50 = render_energy(50.0);
        let e90 = render_energy(90.0);
        assert!(
            e50 > e0 * 1.3,
            "50 % feedback should add tail energy beyond fb=0 (e0={e0}, e50={e50})"
        );
        assert!(
            e90 > e50 * 1.3,
            "90 % feedback should add yet more (e50={e50}, e90={e90})"
        );
        assert!(
            e90.is_finite() && e90 < 1e6,
            "feedback at the cap stays bounded (e90={e90})"
        );
    }

    #[test]
    fn delay_duck_attenuates_the_echo_while_input_is_loud() {
        // With Duck = 100 % and continuous loud input, the duck envelope
        // saturates near 1.0 and the echo factor collapses near 0 — so
        // the audible "wet" component is negligible. Comparing total
        // off-dry energy between Duck=0 and Duck=100 over the same
        // long-input window gives a clear monotone decrease.
        let measure_wet_energy = |duck_pct: f32| -> f32 {
            let mut d = DelayEffect::new();
            d.set_sample_rate(48_000.0);
            d.set_param(0, 100.0); // Free = 100 ms
            d.set_param(1, 14.0); // Time → Free
            d.set_param(2, 80.0); // Healthy feedback so an echo is audible
            d.set_param(3, duck_pct);
            // Drive 0.5-amplitude DC for 1 s so the duck envelope settles
            // at 0.5 (vs 0 with Duck=0). The Free-mode 100 ms tap will
            // produce an "echo" that's also at 0.5 — output = dry + echo
            // = 1.0 with Duck=0. With Duck=100 the echo's factor is
            // (1 - 1.0·0.5) = 0.5 — output = 0.5 + 0.5·0.5 = 0.75.
            // Sum (output - dry) so we isolate the wet contribution.
            let mut wet = 0.0_f32;
            // Warm-up: let the delay buffer fill and the duck envelope
            // settle.
            for _ in 0..12_000 {
                d.process_sample(0.5, 0.5);
            }
            for _ in 0..48_000 {
                let (l, _r) = d.process_sample(0.5, 0.5);
                wet += (l - 0.5).abs();
            }
            wet
        };
        let no_duck = measure_wet_energy(0.0);
        let full_duck = measure_wet_energy(100.0);
        assert!(
            full_duck < no_duck * 0.7,
            "full duck should reduce wet content well below Duck=0 \
             (no_duck={no_duck}, full_duck={full_duck})"
        );
    }

    #[test]
    fn delay_sync_time_tracks_bpm() {
        // Sync mode at 1/4 note (idx 8): at 120 BPM, a quarter is 500 ms;
        // at 60 BPM, 1000 ms. Drive an impulse, then look for the echo
        // peak in each case. The peak index = subdivision-in-samples
        // (within a sample of jitter from linear interp).
        let echo_idx_at_bpm = |bpm: f32| -> usize {
            let mut d = DelayEffect::new();
            d.set_sample_rate(48_000.0);
            d.set_bpm(bpm);
            d.set_param(1, 8.0); // Time → 1/4 note (Time is at slot 1)
            d.set_param(2, 0.0); // Feedback = 0
            d.set_param(3, 0.0); // Duck = 0
            d.process_sample(1.0, 1.0);
            let mut peak_idx = 0usize;
            let mut peak_val = 0.0_f32;
            for i in 1..60_000 {
                let (l, _r) = d.process_sample(0.0, 0.0);
                if l.abs() > peak_val {
                    peak_val = l.abs();
                    peak_idx = i;
                }
            }
            peak_idx
        };
        let at_120 = echo_idx_at_bpm(120.0);
        let at_60 = echo_idx_at_bpm(60.0);
        // 120 BPM: quarter = 500 ms = 24 000 samples at 48 kHz.
        assert!(
            (23_990..=24_010).contains(&at_120),
            "1/4 note at 120 BPM should echo at ~24000 samples, got {at_120}"
        );
        // 60 BPM: quarter = 1000 ms = 48 000 samples.
        assert!(
            (47_990..=48_010).contains(&at_60),
            "1/4 note at 60 BPM should echo at ~48000 samples, got {at_60}"
        );
    }

    #[test]
    fn delay_dims_the_free_dial_only_when_time_points_at_a_sync_subdivision() {
        // Default Time is the trailing "Free" slot → Free is active, no dim.
        let mut d = DelayEffect::new();
        assert!(
            !d.param_dimmed(0),
            "Free dial active at default (Time → Free)"
        );
        // Pick any sync subdivision (e.g. 1/4 note at idx 8) → Free is unused,
        // so the editor should dim its dial.
        d.set_param(1, 8.0);
        assert!(d.param_dimmed(0), "Free dial dimmed when Time = 1/4");
        // Switch back to the Free slot → un-dim.
        let free_slot = (DELAY_TIME_LABELS.len() - 1) as f32;
        d.set_param(1, free_slot);
        assert!(!d.param_dimmed(0), "Free dial active again at Time → Free");
        // Other parameter slots are never dimmed.
        d.set_param(1, 8.0);
        for i in 1..d.parameters().len() {
            assert!(!d.param_dimmed(i), "slot {i} should never dim");
        }
        // Out-of-range index: harmless.
        assert!(!d.param_dimmed(99));
    }

    #[test]
    fn effect_instance_param_dimmed_dispatches_to_the_inner_effect() {
        // Two effects dim a slot under specific conditions: Delay's Free
        // (slot 0) dims when Time is a sync subdivision, Repeat's Rate
        // (slot 0) dims when Snap is a sync subdivision. Every other
        // kind never dims any slot.
        let mut delay = EffectInstance::new(EffectKind::Delay);
        delay.set_param(1, 8.0); // Time → 1/4 note
        assert!(delay.param_dimmed(0));
        assert!(!delay.param_dimmed(1));

        // Repeat dims by default (default Snap is 1/8, a sync slot).
        let repeat = EffectInstance::new(EffectKind::Repeat);
        assert!(repeat.param_dimmed(0));
        for i in 1..MAX_EFFECT_PARAMS {
            assert!(!repeat.param_dimmed(i));
        }
        // Switching Snap to Free un-dims slot 0.
        let mut repeat = EffectInstance::new(EffectKind::Repeat);
        repeat.set_param(1, 14.0); // Snap → Free
        for i in 0..MAX_EFFECT_PARAMS {
            assert!(!repeat.param_dimmed(i));
        }

        for kind in [
            EffectKind::None,
            EffectKind::Svf,
            EffectKind::Bitcrush,
            EffectKind::Fm,
            EffectKind::Phaser,
            EffectKind::WarpZone,
            EffectKind::Satch,
            EffectKind::Stretch,
            EffectKind::Comb,
            EffectKind::Ring,
        ] {
            let e = EffectInstance::new(kind);
            for i in 0..MAX_EFFECT_PARAMS {
                assert!(!e.param_dimmed(i), "{:?} slot {i} should not dim", kind);
            }
        }
    }

    // ----- SatchEffect ---------------------------------------------------

    #[test]
    fn satch_lists_four_parameters_with_the_expected_specs() {
        let s = SatchEffect::new();
        let specs = s.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Gain");
        assert_eq!(specs[0].min, 0.0);
        assert_eq!(specs[0].max, 24.0);
        assert_eq!(specs[1].name, "Threshold");
        assert_eq!(specs[1].min, -24.0);
        assert_eq!(specs[1].max, 0.0);
        assert_eq!(specs[2].name, "Detail");
        assert_eq!(specs[2].max, 100.0);
        assert_eq!(specs[3].name, "Knee");
        assert_eq!(specs[3].max, 100.0);
    }

    #[test]
    fn satch_reports_fft_size_latency() {
        let s = SatchEffect::new();
        assert_eq!(s.latency_samples(), 2048);
    }

    #[test]
    fn satch_set_param_clamps_to_each_spec_range() {
        let mut s = SatchEffect::new();
        s.set_param(0, 100.0);
        assert_eq!(s.gain_db, 24.0);
        s.set_param(0, -5.0);
        assert_eq!(s.gain_db, 0.0);
        s.set_param(1, 10.0);
        assert_eq!(s.threshold_db, 0.0);
        s.set_param(1, -100.0);
        assert_eq!(s.threshold_db, -24.0);
        s.set_param(2, 999.0);
        assert_eq!(s.detail_pct, 100.0);
        s.set_param(3, -50.0);
        assert_eq!(s.knee_pct, 0.0);
    }

    #[test]
    fn satch_output_stays_bounded_under_aggressive_gain_and_detail() {
        // Max gain + min threshold + full detail + soft knee — every per-
        // sample dial is at its limit. The wet output must stay within the
        // ±threshold clamp the algorithm enforces (≈ -24 dB ≈ 0.063 linear)
        // for every sample.
        let mut s = SatchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_param(0, 24.0); // +24 dB gain
        s.set_param(1, -24.0); // -24 dB threshold
        s.set_param(2, 100.0); // full Detail
        s.set_param(3, 100.0); // full Knee (tanh soft clip)
        let threshold_linear = 10.0_f32.powf(-24.0 / 20.0);
        for i in 0..96_000 {
            // 2 s of program
            let t = i as f32 / 48_000.0;
            let dry = 0.7 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                + 0.3 * (2.0 * std::f32::consts::PI * 880.0 * t).sin();
            let (l, r) = s.process_sample(dry, dry);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            // Algorithm clamps the wet to ±threshold; ride a tiny epsilon
            // above to absorb f32 round-trip rounding.
            let cap = threshold_linear + 1e-4;
            assert!(
                l.abs() <= cap && r.abs() <= cap,
                "sample {i} ({l}, {r}) exceeds threshold {threshold_linear}"
            );
        }
    }

    #[test]
    fn satch_reset_zeroes_dry_delay_and_clipper_state() {
        let mut s = SatchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_param(0, 12.0);
        s.set_param(2, 50.0);
        // Pump signal through to fill the delay + STFT rings.
        for i in 0..4_096 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 1_000.0 * t).sin();
            let _ = s.process_sample(dry, dry);
        }
        s.reset();
        assert!(s.dry_delay_l.iter().all(|&v| v == 0.0));
        assert!(s.dry_delay_r.iter().all(|&v| v == 0.0));
        assert_eq!(s.dry_delay_pos, 0);
        // First post-reset sample is finite + L==R symmetric for mono input.
        let (l, r) = s.process_sample(0.5, 0.5);
        assert!(l.is_finite() && r.is_finite());
        assert!((l - r).abs() < 1e-6);
    }

    // ----- RepeatEffect --------------------------------------------------

    #[test]
    fn repeat_lists_four_parameters_with_the_expected_specs() {
        let r = RepeatEffect::new();
        let specs = r.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Rate");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert_eq!(specs[0].min, 0.5);
        assert_eq!(specs[0].max, 1000.0);
        assert_eq!(specs[1].name, "Snap");
        assert!(matches!(specs[1].format, ParamFormat::Enum { .. }));
        // Snap default = 1/8 (idx 6).
        assert_eq!(specs[1].default, 6.0);
        assert_eq!(specs[2].name, "Refresh");
        assert!(matches!(specs[2].format, ParamFormat::Enum { .. }));
        // Refresh default = 1/4 (idx 8).
        assert_eq!(specs[2].default, 8.0);
        assert_eq!(specs[3].name, "Smooth");
        assert_eq!(specs[3].max, 100.0);
    }

    #[test]
    fn repeat_set_param_clamps_each_slot() {
        let mut r = RepeatEffect::new();
        r.set_param(0, 9_999.0);
        assert_eq!(r.rate_hz, 1_000.0);
        r.set_param(0, 0.0);
        assert_eq!(r.rate_hz, 0.5);
        // Snap clamps to [0, SNAP_LABELS.len() - 1] = [0, 14].
        r.set_param(1, 99.0);
        assert_eq!(r.snap_idx, 14.0);
        r.set_param(1, -5.0);
        assert_eq!(r.snap_idx, 0.0);
        // Refresh clamps to [0, REFRESH_LABELS.len() - 1] = [0, 13].
        r.set_param(2, 99.0);
        assert_eq!(r.refresh_idx, 13.0);
        r.set_param(3, 200.0);
        assert_eq!(r.smooth_pct, 100.0);
    }

    #[test]
    fn repeat_outputs_dry_passthrough_until_primed() {
        // Before the ring has filled with at least one loop_length of
        // audio, output mirrors input so the user always hears
        // something. Use Free mode with a long loop (0.5 Hz → 2 s loop)
        // so the priming window is much longer than the test span.
        let mut r = RepeatEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(1, 14.0); // Snap → Free
        r.set_param(0, 0.5); // Rate → 0.5 Hz (2 s loop)
                             // First 100 samples should pass dry through verbatim.
        for i in 0..100 {
            let dry = 0.1 + i as f32 * 0.001;
            let (l, ri) = r.process_sample(dry, -dry);
            assert!((l - dry).abs() < 1e-6, "sample {i} L not dry: {l} vs {dry}");
            assert!(
                (ri + dry).abs() < 1e-6,
                "sample {i} R not dry: {ri} vs {}",
                -dry
            );
        }
    }

    #[test]
    fn repeat_loops_a_captured_slice_in_free_mode() {
        // Free-mode short loop. After priming, the output sample at
        // playback position p should equal the input sample captured
        // (write_pos − loop_length + p) samples ago. Verify by feeding
        // a counter signal and checking the loop wraps at the right
        // boundary.
        let mut r = RepeatEffect::new();
        r.set_sample_rate(48_000.0);
        // Snap = Free, Rate = 1 kHz → 48-sample loop at 48 kHz.
        r.set_param(1, 14.0);
        r.set_param(0, 1_000.0);
        // Refresh interval long enough that no early refresh interferes
        // (1/1. at 30 BPM ≈ 12 s).
        r.set_param(2, 13.0); // 1/1.
        r.set_bpm(30.0);
        r.set_param(3, 0.0); // Smooth = 0 so we can compare exact samples
                             // Feed a recognisable sequence (sample i = i as f32 * 0.001).
        let mut last_output = 0.0;
        let mut wrapped_at_least_once = false;
        let mut samples_at_wrap = 0usize;
        for i in 0..1_000 {
            let dry = i as f32 * 0.001;
            let (out, _) = r.process_sample(dry, dry);
            // Once we've definitely primed (after 48 samples), the
            // output should NOT keep growing with the input — it should
            // loop. Detect the first time output is less than the
            // previous output (loop wrap).
            if i > 100 && out < last_output - 0.01 {
                wrapped_at_least_once = true;
                samples_at_wrap = i;
                break;
            }
            last_output = out;
        }
        assert!(
            wrapped_at_least_once,
            "Repeat in Free mode at 1 kHz should wrap its loop visibly"
        );
        // The wrap happens within ~loop_length samples of priming.
        assert!(
            samples_at_wrap < 200,
            "Wrap should land soon after priming, got at sample {samples_at_wrap}"
        );
    }

    #[test]
    fn repeat_loop_length_clamps_to_refresh_interval() {
        // If Snap asks for a longer loop than Refresh, the effective
        // loop_length must clamp to the Refresh interval — otherwise
        // captures would interrupt mid-loop.
        let mut r = RepeatEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_bpm(120.0);
        // Snap = 1/2 (large), Refresh = 1/16 (small).
        r.set_param(1, 10.0); // Snap → 1/2 note
        r.set_param(2, 4.0); // Refresh → 1/16 note
                             // After enough samples to prime, the loop_length must be ≤
                             // the 1/16-note window (= 60/120/4 × 48000 ≈ 6000 samples) not
                             // the 1/2-note value (24000 samples).
        for _ in 0..24_000 {
            let _ = r.process_sample(0.5, 0.5);
        }
        assert!(
            r.loop_length <= 6_010,
            "loop_length should clamp to ~1/16-note (6000 samples), got {}",
            r.loop_length
        );
        assert!(
            r.loop_length >= 5_990,
            "loop_length should be ~1/16-note (6000 samples), got {}",
            r.loop_length
        );
    }

    #[test]
    fn repeat_reset_clears_state_and_returns_to_passthrough() {
        let mut r = RepeatEffect::new();
        r.set_sample_rate(48_000.0);
        // Drive enough audio for priming, then reset.
        for _ in 0..96_000 {
            let _ = r.process_sample(0.5, 0.5);
        }
        assert!(r.primed);
        r.reset();
        assert!(!r.primed);
        assert_eq!(r.samples_since_reset, 0);
        assert_eq!(r.write_idx, 0);
        assert_eq!(r.loop_pos, 0);
        assert!(r.ring_l.iter().all(|&s| s == 0.0));
        assert!(r.ring_r.iter().all(|&s| s == 0.0));
        // The very first post-reset sample is dry passthrough.
        let (l, ri) = r.process_sample(0.42, -0.42);
        assert!((l - 0.42).abs() < 1e-6);
        assert!((ri + 0.42).abs() < 1e-6);
    }

    #[test]
    fn repeat_reports_zero_pdc_latency() {
        // The loop effect inherently delays the audio it plays back
        // (the loop is the most-recent N samples), but that's the
        // *point* of the effect — not host-PDC-reportable latency
        // (PDC compensates fixed plugin delay; the loop delay is
        // semantically the user's effect). Should report zero.
        let r = RepeatEffect::new();
        assert_eq!(r.latency_samples(), 0);
    }

    // ----- StretchEffect -------------------------------------------------

    #[test]
    fn stretch_lists_four_parameters_with_the_expected_specs() {
        let s = StretchEffect::new();
        let specs = s.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Pace");
        assert_eq!(specs[0].min, 0.05);
        assert_eq!(specs[0].max, 1.0);
        assert_eq!(specs[1].name, "Refresh");
        assert!(matches!(specs[1].format, ParamFormat::Enum { .. }));
        assert_eq!(specs[2].name, "Grain");
        assert!(matches!(specs[2].scaling, ParamScaling::Log));
        assert!(matches!(specs[2].format, ParamFormat::Hertz));
        assert_eq!(specs[2].min, 5.0);
        assert_eq!(specs[2].max, 200.0);
        assert_eq!(specs[3].name, "Smooth");
        assert_eq!(specs[3].max, 100.0);
    }

    #[test]
    fn stretch_set_param_clamps_each_slot() {
        let mut s = StretchEffect::new();
        s.set_param(0, 5.0);
        assert_eq!(s.pace, 1.0);
        s.set_param(0, -1.0);
        assert_eq!(s.pace, 0.05);
        // Refresh clamps to [0, REFRESH_LABELS.len() - 1] = [0, 13].
        s.set_param(1, 99.0);
        assert_eq!(s.refresh_idx, 13.0);
        s.set_param(2, 9_999.0);
        assert_eq!(s.grain_hz, 200.0);
        s.set_param(2, 0.0);
        assert_eq!(s.grain_hz, 5.0);
        s.set_param(3, 999.0);
        assert_eq!(s.smooth_pct, 100.0);
    }

    #[test]
    fn stretch_outputs_dry_passthrough_until_primed() {
        // The capture window = capture_period × Pace. At 30 BPM,
        // Refresh = 1/1. (idx 13 = 6 beats), Pace = 1.0 → capture
        // window ≈ 12 s × 48 kHz = 576k samples. Test span of 100
        // samples is dwarfed by that priming threshold; output must
        // be exactly dry over the whole span.
        let mut s = StretchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_bpm(30.0);
        s.set_param(1, 13.0); // Refresh → 1/1.
        s.set_param(0, 1.0); // Pace → 1.0
        for i in 0..100 {
            let dry = 0.1 + i as f32 * 0.001;
            let (l, r) = s.process_sample(dry, -dry);
            assert!((l - dry).abs() < 1e-6, "sample {i} L not dry: {l}");
            assert!((r + dry).abs() < 1e-6, "sample {i} R not dry: {r}");
        }
    }

    #[test]
    fn stretch_capture_window_scales_with_pace() {
        // capture_window = capture_period × pace. Verify two different
        // Pace values produce proportional capture windows on the same
        // Refresh setting and BPM.
        let mut s = StretchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_bpm(120.0);
        s.set_param(1, 8.0); // Refresh → 1/4 note (= 24000 samples @ 120 BPM)
        s.set_param(0, 0.5); // Pace → 0.5
                             // Pump enough samples to fire several Refresh ticks at the
                             // smaller capture window (12000 samples) so primed flips.
        for _ in 0..48_000 {
            let _ = s.process_sample(0.5, 0.5);
        }
        let window_at_half = s.capture_window;
        assert!(
            (11_990.0..=12_010.0).contains(&window_at_half),
            "Pace=0.5 should give ~12000-sample window, got {}",
            window_at_half
        );

        // Reset and try Pace = 0.25.
        s.reset();
        s.set_param(0, 0.25);
        for _ in 0..48_000 {
            let _ = s.process_sample(0.5, 0.5);
        }
        let window_at_quarter = s.capture_window;
        assert!(
            (5_990.0..=6_010.0).contains(&window_at_quarter),
            "Pace=0.25 should give ~6000-sample window, got {}",
            window_at_quarter
        );
    }

    #[test]
    fn stretch_output_is_bounded_under_aggressive_settings() {
        // Worst-case: Pace at minimum, Refresh = 1/16, Grain at min
        // (long grains), Smooth = 0 (boxcar). The COLA correction
        // (1 / (2 - smooth)) keeps the output near unit amplitude
        // even at Smooth=0 where adjacent grains' boxcar windows
        // would otherwise double.
        let mut s = StretchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_bpm(120.0);
        s.set_param(0, 0.05); // Pace minimum
        s.set_param(1, 4.0); // Refresh → 1/16
        s.set_param(2, 5.0); // Grain minimum
        s.set_param(3, 0.0); // Smooth = 0 (boxcar)
        for i in 0..48_000 {
            let t = i as f32 / 48_000.0;
            let dry = 0.7 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, r) = s.process_sample(dry, dry);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            // 1.5× input amplitude headroom — even with 2 boxcar
            // grains briefly summing at scale 0.5 the peak shouldn't
            // exceed ~1.0.
            assert!(
                l.abs() < 1.5 && r.abs() < 1.5,
                "sample {i} blew up: ({l}, {r})"
            );
        }
    }

    #[test]
    fn stretch_reset_clears_state_and_returns_to_passthrough() {
        let mut s = StretchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_param(0, 0.5);
        // Pump enough audio to prime.
        for _ in 0..48_000 {
            let _ = s.process_sample(0.5, 0.5);
        }
        assert!(s.primed);
        s.reset();
        assert!(!s.primed);
        assert_eq!(s.samples_since_reset, 0);
        assert_eq!(s.write_idx, 0);
        assert_eq!(s.read_pos, 0.0);
        assert_eq!(s.capture_window, 0.0);
        assert!(s.grains.iter().all(|g| !g.active));
        assert!(s.ring_l.iter().all(|&v| v == 0.0));
        assert!(s.ring_r.iter().all(|&v| v == 0.0));
        // First post-reset sample is dry passthrough.
        let (l, r) = s.process_sample(0.42, -0.42);
        assert!((l - 0.42).abs() < 1e-6);
        assert!((r + 0.42).abs() < 1e-6);
    }

    #[test]
    fn stretch_reports_zero_pdc_latency() {
        // The granulation delay (one Refresh interval) IS the effect —
        // not a fixed plugin delay that PDC should compensate for.
        // Reports zero; the host sees Stretch as sample-aligned.
        let s = StretchEffect::new();
        assert_eq!(s.latency_samples(), 0);
    }

    // ----- CombEffect ----------------------------------------------------

    #[test]
    fn comb_lists_five_parameters_with_the_expected_specs() {
        let c = CombEffect::new();
        let specs = c.parameters();
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[0].name, "Pitch");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert_eq!(specs[0].min, 20.0);
        assert_eq!(specs[0].max, 5_000.0);
        assert_eq!(specs[1].name, "Mode");
        assert!(matches!(specs[1].format, ParamFormat::Enum { .. }));
        assert_eq!(specs[1].default, 0.0); // Resonant
        assert_eq!(specs[2].name, "Depth");
        assert_eq!(specs[2].min, -100.0);
        assert_eq!(specs[2].max, 100.0);
        assert_eq!(specs[3].name, "Damping");
        assert_eq!(specs[4].name, "Stereo");
    }

    #[test]
    fn comb_set_param_clamps_each_slot() {
        let mut c = CombEffect::new();
        c.set_param(0, 50_000.0);
        assert_eq!(c.pitch_hz, 5_000.0);
        c.set_param(0, 1.0);
        assert_eq!(c.pitch_hz, 20.0);
        // Mode clamps to [0, MODE_LABELS.len() - 1] = [0, 2].
        c.set_param(1, 99.0);
        assert_eq!(c.mode_idx, 2.0);
        c.set_param(1, -5.0);
        assert_eq!(c.mode_idx, 0.0);
        // Depth signed.
        c.set_param(2, 999.0);
        assert_eq!(c.depth_pct, 100.0);
        c.set_param(2, -999.0);
        assert_eq!(c.depth_pct, -100.0);
        c.set_param(3, 200.0);
        assert_eq!(c.damping_pct, 100.0);
        c.set_param(4, 200.0);
        assert_eq!(c.stereo_pct, 100.0);
    }

    #[test]
    fn comb_resonant_mode_rings_at_pitch_frequency() {
        // Feed a single impulse into the resonant comb and look for the
        // first echo at delay = sample_rate / pitch. The impulse should
        // recirculate through the feedback loop, giving a strong peak
        // ~D samples after the input.
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, 1_000.0); // Pitch = 1 kHz → D = 48 samples
        c.set_param(1, 0.0); // Mode = Resonant
        c.set_param(2, 80.0); // Depth = 80 % (alpha = 0.8)
        c.set_param(3, 0.0); // No damping
        c.set_param(4, 0.0); // No stereo offset
        let (l, _) = c.process_sample(1.0, 1.0);
        // First sample = input + alpha * 0 = 1.0.
        assert!((l - 1.0).abs() < 1e-5, "impulse sample should pass through");
        // Look for the recirculated peak around sample 48.
        let mut peak_idx = 0usize;
        let mut peak_val = 0.0_f32;
        for i in 1..200 {
            let (out, _) = c.process_sample(0.0, 0.0);
            if out.abs() > peak_val {
                peak_val = out.abs();
                peak_idx = i;
            }
        }
        assert!(
            peak_val > 0.5,
            "resonant feedback should produce a clear recirculated peak, got {peak_val}"
        );
        assert!(
            (40..=56).contains(&peak_idx),
            "peak should land near sample 48 (= 48000 / 1000), got {peak_idx}"
        );
    }

    #[test]
    fn comb_notch_mode_has_no_feedback_decay() {
        // Notch mode writes only dry to the delay → an impulse produces
        // exactly one delayed echo, no recirculation. Sum the absolute
        // output over a window much longer than the delay; should be
        // bounded near the input + one echo (~2.0) for any Depth.
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, 1_000.0); // Pitch = 1 kHz → D = 48 samples
        c.set_param(1, 1.0); // Mode = Notch
        c.set_param(2, 90.0); // Depth = 90 % (would resonate in FB mode)
                              // Drive impulse, sum |output| over a long tail.
        let (first, _) = c.process_sample(1.0, 1.0);
        let mut energy: f32 = first.abs();
        for _ in 0..2_000 {
            let (out, _) = c.process_sample(0.0, 0.0);
            energy += out.abs();
        }
        // ~1.0 from impulse + ~0.9 from one delayed echo = ~1.9; allow
        // headroom for fractional-delay interpolation spreading the
        // echo across adjacent samples.
        assert!(
            energy < 3.5,
            "Notch mode should not recirculate (sum |y| was {energy})"
        );
    }

    #[test]
    fn comb_allpass_mode_is_magnitude_flat_on_a_sustained_tone() {
        // Allpass mode preserves magnitude at every frequency. For a
        // sustained sine at any pitch, the wet output's RMS should
        // match the input's RMS after the loop settles. (Engine-level
        // Mix would reveal the phase shifts when summed with dry; we
        // test the wet output's RMS directly here.)
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, 800.0); // Pitch
        c.set_param(1, 2.0); // Mode = Allpass
        c.set_param(2, 70.0); // Depth = 70 %
        let test_hz = 440.0;
        let mut input_energy = 0.0_f32;
        let mut output_energy = 0.0_f32;
        // Warm-up: let the allpass loop settle.
        for i in 0..2_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * test_hz * t).sin();
            let _ = c.process_sample(dry, dry);
        }
        // Measure RMS in/out over a long window.
        for i in 0..48_000 {
            let t = (2_000 + i) as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * test_hz * t).sin();
            let (out, _) = c.process_sample(dry, dry);
            input_energy += dry * dry;
            output_energy += out * out;
        }
        let ratio = (output_energy / input_energy).sqrt();
        assert!(
            (0.9..=1.1).contains(&ratio),
            "Allpass RMS should match dry within 10 % (got {ratio})"
        );
    }

    #[test]
    fn comb_stereo_offset_separates_l_and_r() {
        // With Stereo > 0 the per-channel pitches differ → L ≠ R for
        // a mono-sum input through the comb.
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, 500.0);
        c.set_param(2, 60.0);
        c.set_param(4, 100.0); // Stereo = 100 %
        let mut diff_energy = 0.0_f32;
        for i in 0..4_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 700.0 * t).sin();
            let (l, r) = c.process_sample(dry, dry);
            diff_energy += (l - r) * (l - r);
        }
        assert!(
            diff_energy > 1.0,
            "Stereo = 100 must produce L \u{2260} R for a mono input (diff_energy = {diff_energy})"
        );
    }

    #[test]
    fn comb_stereo_zero_collapses_to_mono() {
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(4, 0.0); // Stereo = 0
        for i in 0..2_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 800.0 * t).sin();
            let (l, r) = c.process_sample(dry, dry);
            assert!(
                (l - r).abs() < 1e-5,
                "Stereo = 0 must be L == R, sample {i}: {l} vs {r}"
            );
        }
    }

    #[test]
    fn comb_stays_bounded_under_max_depth_and_stereo_sweep() {
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(1, 0.0); // Resonant mode (the one that can ring)
        c.set_param(2, 100.0); // Max positive depth (clamps internally to 0.95)
        c.set_param(4, 100.0); // Max stereo offset
        for i in 0..48_000 {
            // Sweep Pitch wildly via set_param every sample.
            let p = (i as f32 / 4096.0).fract();
            let pitch = 20.0 * (250.0_f32).powf(p); // 20..5000 Hz log
            c.set_param(0, pitch);
            let dry = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, r) = c.process_sample(dry, dry);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            assert!(
                l.abs() < 16.0 && r.abs() < 16.0,
                "sample {i} blew up: ({l}, {r})"
            );
        }
    }

    #[test]
    fn comb_reset_clears_delay_line_and_damping_state() {
        let mut c = CombEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(2, 90.0);
        c.set_param(3, 50.0);
        for _ in 0..4_000 {
            let _ = c.process_sample(0.5, 0.5);
        }
        c.reset();
        assert!(c.delay_l.iter().all(|&v| v == 0.0));
        assert!(c.delay_r.iter().all(|&v| v == 0.0));
        assert_eq!(c.write_idx, 0);
        assert_eq!(c.damp_state_l, 0.0);
        assert_eq!(c.damp_state_r, 0.0);
    }

    // ----- RingEffect ----------------------------------------------------

    #[test]
    fn ring_lists_four_parameters_with_the_expected_specs() {
        let r = RingEffect::new();
        let specs = r.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Freq");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert_eq!(specs[0].min, 0.1);
        assert_eq!(specs[0].max, 5_000.0);
        assert_eq!(specs[1].name, "Shape");
        assert!(matches!(specs[1].format, ParamFormat::Enum { .. }));
        assert_eq!(specs[1].default, 0.0); // Sine
        assert_eq!(specs[2].name, "Bias");
        assert_eq!(specs[2].min, -100.0);
        assert_eq!(specs[2].max, 100.0);
        assert_eq!(specs[3].name, "Stereo");
        assert_eq!(specs[3].min, 0.0);
        assert_eq!(specs[3].max, 100.0);
    }

    #[test]
    fn ring_set_param_clamps_each_slot() {
        let mut r = RingEffect::new();
        // Freq clamps to [0.1, 5000].
        r.set_param(0, 50_000.0);
        assert_eq!(r.freq_hz, 5_000.0);
        r.set_param(0, 0.0);
        assert_eq!(r.freq_hz, 0.1);
        // Shape clamps to [0, SHAPE_LABELS.len() - 1] = [0, 1].
        r.set_param(1, 99.0);
        assert_eq!(r.shape_idx, 1.0);
        r.set_param(1, -5.0);
        assert_eq!(r.shape_idx, 0.0);
        // Bias clamps to [-100, +100].
        r.set_param(2, 999.0);
        assert_eq!(r.bias_pct, 100.0);
        r.set_param(2, -999.0);
        assert_eq!(r.bias_pct, -100.0);
        // Stereo clamps to [0, 100].
        r.set_param(3, 999.0);
        assert_eq!(r.stereo_pct, 100.0);
        r.set_param(3, -10.0);
        assert_eq!(r.stereo_pct, 0.0);
    }

    #[test]
    fn ring_bias_full_positive_is_dry_passthrough() {
        // Bias = +100 % → carrier = +1 regardless of shape/phase →
        // output should equal input exactly, sample for sample.
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 1_000.0); // Any non-zero freq
        r.set_param(2, 100.0);
        for i in 0..1_024 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            let (l, ri) = r.process_sample(dry, dry);
            assert!(
                (l - dry).abs() < 1e-6 && (ri - dry).abs() < 1e-6,
                "bias=+100 must be dry passthrough; sample {i}: ({l},{ri}) vs {dry}"
            );
        }
    }

    #[test]
    fn ring_bias_full_negative_is_dry_inverted() {
        // Bias = -100 % → carrier = -1 → output = -input exactly.
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 1_000.0);
        r.set_param(2, -100.0);
        for i in 0..1_024 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            let (l, _) = r.process_sample(dry, dry);
            assert!(
                (l - (-dry)).abs() < 1e-6,
                "bias=-100 must invert; sample {i}: {l} vs {}",
                -dry
            );
        }
    }

    #[test]
    fn ring_pure_rm_on_dc_input_traces_the_carrier() {
        // Bias = 0 (pure RM) on dry=1 means output equals the carrier
        // wave itself. For a sine carrier at 100 Hz, output should
        // average to zero and peak near ±1 over a full cycle.
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 100.0); // 100 Hz carrier
        r.set_param(2, 0.0); // Pure RM
        let mut sum = 0.0_f32;
        let mut peak = 0.0_f32;
        // 480 samples = exactly one cycle at 100 Hz / 48 kHz.
        for _ in 0..480 {
            let (l, _) = r.process_sample(1.0, 1.0);
            sum += l;
            peak = peak.max(l.abs());
        }
        // Tolerance ≈ 0.01: 480 f32 accumulations plus a tiny phase-
        // increment quantization error (1/480 isn't finitely
        // representable in binary). The carrier itself is bit-exact;
        // this is just summation drift.
        assert!(
            sum.abs() < 1e-2,
            "DC × pure-RM sine carrier should integrate to 0 over a cycle, got {sum}"
        );
        assert!(
            (peak - 1.0).abs() < 1e-2,
            "carrier peak should be ±1 (got {peak})"
        );
    }

    #[test]
    fn ring_freq_determines_carrier_rate_via_zero_crossings() {
        // Pure RM on dry=1 puts a clean sine of frequency `f` at the
        // output. Count zero crossings over 1 s and verify it equals
        // 2·f within rounding.
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        let f_hz = 250.0;
        r.set_param(0, f_hz);
        r.set_param(2, 0.0); // Pure RM
        let mut prev = 0.0_f32;
        let mut crossings = 0usize;
        for i in 0..48_000 {
            let (l, _) = r.process_sample(1.0, 1.0);
            // Skip the very first sample (prev is the initial 0 by accident).
            if i > 0 && prev.signum() != l.signum() && (prev != 0.0 || l != 0.0) {
                crossings += 1;
            }
            prev = l;
        }
        let expected = (2.0 * f_hz) as isize;
        let diff = (crossings as isize - expected).abs();
        assert!(
            diff <= 2,
            "expected ~{expected} zero crossings at {f_hz} Hz, got {crossings}"
        );
    }

    #[test]
    fn ring_triangle_shape_has_triangular_peak_distribution() {
        // A sine carrier's instantaneous value is concentrated near ±1
        // (arcsine PDF); a triangle's is uniform over [-1, +1]. Use
        // that to verify the Shape selector actually switches waveforms:
        // count samples whose magnitude exceeds 0.7. For sine over one
        // cycle ~50 % are above 0.7; for triangle only ~30 %.
        let mut r_sine = RingEffect::new();
        r_sine.set_sample_rate(48_000.0);
        r_sine.set_param(0, 100.0);
        r_sine.set_param(1, 0.0); // Sine
        r_sine.set_param(2, 0.0); // Pure RM
        let mut r_tri = RingEffect::new();
        r_tri.set_sample_rate(48_000.0);
        r_tri.set_param(0, 100.0);
        r_tri.set_param(1, 1.0); // Triangle
        r_tri.set_param(2, 0.0);

        // 4800 samples = 10 full cycles at 100 Hz / 48 kHz.
        let mut sine_above = 0usize;
        let mut tri_above = 0usize;
        for _ in 0..4_800 {
            let (s, _) = r_sine.process_sample(1.0, 1.0);
            let (t, _) = r_tri.process_sample(1.0, 1.0);
            if s.abs() > 0.7 {
                sine_above += 1;
            }
            if t.abs() > 0.7 {
                tri_above += 1;
            }
        }
        // Sine: ~50 % above 0.7. Triangle: ~30 %. Demand a clear gap.
        assert!(
            sine_above > tri_above + 500,
            "sine should spend much more time near ±1 than triangle (sine={sine_above}, tri={tri_above})"
        );
    }

    #[test]
    fn ring_stereo_zero_collapses_to_mono() {
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 333.0);
        r.set_param(2, 0.0); // Pure RM
        r.set_param(3, 0.0); // Stereo = 0
        for i in 0..1_024 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 500.0 * t).sin();
            let (l, ri) = r.process_sample(dry, dry);
            assert!(
                (l - ri).abs() < 1e-6,
                "Stereo = 0 must give L == R, sample {i}: {l} vs {ri}"
            );
        }
    }

    #[test]
    fn ring_stereo_100_inverts_right_carrier() {
        // Stereo = 100 % → R carrier is offset by 0.5 cycle (180°) →
        // for a sine carrier, R = -L · (dry). With identical L/R dry,
        // output_r should equal -output_l on every sample.
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 500.0);
        r.set_param(2, 0.0); // Pure RM
        r.set_param(3, 100.0); // 180° offset
        for i in 0..1_024 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 800.0 * t).sin();
            let (l, ri) = r.process_sample(dry, dry);
            assert!(
                (l + ri).abs() < 1e-4,
                "Stereo = 100 should antiphase the channels, sample {i}: ({l},{ri})"
            );
        }
    }

    #[test]
    fn ring_stays_bounded_under_aggressive_freq_sweep() {
        // RM has no feedback path so output is at most |dry|·|carrier| ≤
        // |dry|·1, but we still want to make sure phase wrapping doesn't
        // produce NaNs at extreme rates.
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(2, 0.0);
        r.set_param(3, 100.0);
        for i in 0..48_000 {
            // Sweep Freq from 0.1 Hz to 5 kHz log over 1 second.
            let p = (i as f32 / 4096.0).fract();
            let freq = 0.1 * 50_000.0_f32.powf(p);
            r.set_param(0, freq);
            let dry = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, ri) = r.process_sample(dry, dry);
            assert!(l.is_finite() && ri.is_finite(), "sample {i} not finite");
            assert!(
                l.abs() <= 0.5 + 1e-6 && ri.abs() <= 0.5 + 1e-6,
                "sample {i} exceeded |dry|: ({l},{ri})"
            );
        }
    }

    #[test]
    fn ring_reset_clears_phase() {
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 1_000.0);
        for _ in 0..500 {
            let _ = r.process_sample(0.5, 0.5);
        }
        assert!(r.phase != 0.0, "phase should have advanced");
        r.reset();
        assert_eq!(r.phase, 0.0);
        // First sample after reset: phase = 0, sine carrier at 0 = 0,
        // pure-RM (bias=0) → output = dry · 0 = 0.
        let (l, ri) = r.process_sample(0.7, 0.7);
        assert!(
            l.abs() < 1e-6 && ri.abs() < 1e-6,
            "post-reset first sample should be 0 for sine RM at phase 0, got ({l},{ri})"
        );
    }

    // ----- WarpZoneEffect ------------------------------------------------

    #[test]
    fn warpzone_lists_five_parameters_with_the_expected_specs() {
        let w = WarpZoneEffect::new();
        let specs = w.parameters();
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[0].name, "Shift");
        assert_eq!(specs[0].min, -24.0);
        assert_eq!(specs[0].max, 24.0);
        assert_eq!(specs[1].name, "Stretch");
        assert_eq!(specs[1].min, 0.5);
        assert_eq!(specs[1].max, 2.0);
        assert_eq!(specs[2].name, "Feedback");
        assert_eq!(specs[2].max, 95.0);
        assert_eq!(specs[3].name, "Low");
        assert!(matches!(specs[3].scaling, ParamScaling::Log));
        assert!(matches!(specs[3].format, ParamFormat::Hertz));
        assert_eq!(specs[4].name, "High");
        assert!(matches!(specs[4].format, ParamFormat::Hertz));
    }

    #[test]
    fn warpzone_set_param_clamps_to_each_spec_range() {
        let mut w = WarpZoneEffect::new();
        // Shift past +24 → 24.0; below -24 → -24.0.
        w.set_param(0, 100.0);
        assert_eq!(w.shift_st, 24.0);
        w.set_param(0, -100.0);
        assert_eq!(w.shift_st, -24.0);
        // Stretch clamps to [0.5, 2.0].
        w.set_param(1, 5.0);
        assert_eq!(w.stretch, 2.0);
        w.set_param(1, 0.0);
        assert_eq!(w.stretch, 0.5);
        // Feedback caps at 95 even if a stray modulation overshoots.
        w.set_param(2, 200.0);
        assert_eq!(w.feedback_pct, 95.0);
        // Hz params clamp to [20, 20000].
        w.set_param(3, 1.0);
        assert_eq!(w.low_hz, 20.0);
        w.set_param(4, 999_999.0);
        assert_eq!(w.high_hz, 20_000.0);
    }

    #[test]
    fn warpzone_output_is_finite_under_aggressive_modulation_and_max_feedback() {
        // Drive the cascade hard: feedback at the cap and a sustained
        // wide-band input. Output must stay bounded indefinitely.
        let mut w = WarpZoneEffect::new();
        w.set_sample_rate(48_000.0);
        w.set_param(2, 95.0); // Feedback at cap
        w.set_param(0, 12.0); // +12 st shift (× 2 frequency)
        for i in 0..96_000 {
            // 2 s
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                + 0.3 * (2.0 * std::f32::consts::PI * 880.0 * t).sin();
            let (l, r) = w.process_sample(dry, dry);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            assert!(
                l.abs() < 16.0 && r.abs() < 16.0,
                "sample {i} blew up: ({l}, {r})"
            );
        }
    }

    #[test]
    fn warpzone_reset_clears_feedback_and_shifter_state() {
        // Drive some signal in, reset, then verify the first sample with
        // pure-dry input doesn't carry residue from before.
        let mut w = WarpZoneEffect::new();
        w.set_sample_rate(48_000.0);
        w.set_param(2, 80.0); // High feedback to load state heavily
        for _ in 0..6_000 {
            w.process_sample(0.5, 0.5);
        }
        w.reset();
        // After reset both feedback slots are zero.
        assert_eq!(w.fb_l, 0.0);
        assert_eq!(w.fb_r, 0.0);
        // And the next impulse pair produces finite, well-bounded output
        // (the first few samples are silence — STFT hasn't synthesised yet).
        let (l, r) = w.process_sample(1.0, 1.0);
        assert!(l.is_finite() && r.is_finite());
        assert!(l.abs() < 4.0 && r.abs() < 4.0);
    }

    #[test]
    fn warpzone_default_params_match_pass_through_intent() {
        // Default settings = identity (shift=0, stretch=1, fb=0, full band).
        // After the FFT's settling latency, output should track input
        // closely enough to be recognisable as the same signal. We don't
        // assert sample-level equality (the STFT pipeline imparts the
        // documented identity-path trim and a 4096-sample delay), but the
        // RMS-of-output should be a meaningful fraction of the RMS-of-input
        // once samples have propagated through.
        let mut w = WarpZoneEffect::new();
        w.set_sample_rate(48_000.0);
        // Feed 2× FFT-size samples of a 1 kHz sine; measure RMS of the
        // SECOND half (past the latency).
        let n = 8192;
        let mut out_rms = 0.0_f32;
        let mut count = 0usize;
        for i in 0..n {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 1_000.0 * t).sin();
            let (l, _r) = w.process_sample(dry, dry);
            if i >= n / 2 {
                out_rms += l * l;
                count += 1;
            }
        }
        let rms = (out_rms / count as f32).sqrt();
        // 1 kHz sine RMS = 1/√2 ≈ 0.707. Identity path trims by ~3 dB
        // (0.707 × 10^(-3/20) ≈ 0.5), so anywhere ≥ 0.2 confirms the
        // signal made it through.
        assert!(
            rms > 0.2,
            "default warpzone should pass signal, got rms={rms}"
        );
    }

    // ----- PhaserEffect --------------------------------------------------

    #[test]
    fn phaser_lists_three_parameters_with_the_expected_specs() {
        let p = PhaserEffect::new();
        let specs = p.parameters();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].name, "Center");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert_eq!(specs[0].min, 50.0);
        assert_eq!(specs[0].max, 8_000.0);
        assert_eq!(specs[1].name, "Feedback");
        assert_eq!(specs[1].min, 0.0);
        assert_eq!(specs[1].max, 95.0);
        assert_eq!(specs[2].name, "Stereo");
        assert_eq!(specs[2].min, 0.0);
        assert_eq!(specs[2].max, 100.0);
    }

    #[test]
    fn phaser_at_default_colours_the_signal_without_silencing_it() {
        // Default phaser (Center=500, Feedback=30, Stereo=0) should pass a
        // signal but with the cascade applied — output is non-zero and not
        // identical to the dry input. Even without modulation the all-pass
        // sections introduce phase shift; summed against dry, the comb
        // notches colour the spectrum.
        let mut p = PhaserEffect::new();
        p.set_sample_rate(48_000.0);
        // Drive a 1 kHz sine for 2048 samples (settled past the transient).
        let mut wet_energy = 0.0_f32;
        let mut diff_energy = 0.0_f32;
        for i in 0..2048 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            let (l, _r) = p.process_sample(dry, dry);
            wet_energy += l * l;
            diff_energy += (l - dry) * (l - dry);
        }
        assert!(wet_energy > 100.0, "phaser must produce non-trivial output");
        assert!(
            diff_energy > 10.0,
            "phaser output must differ from dry (got diff_energy={diff_energy})"
        );
    }

    #[test]
    fn phaser_feedback_raises_total_energy_for_a_static_centre() {
        // More feedback = more pronounced peaks between notches → more total
        // energy through the cascade for a broadband (impulse-train) input.
        let render_energy = |fb_pct: f32| -> f32 {
            let mut p = PhaserEffect::new();
            p.set_sample_rate(48_000.0);
            p.set_param(1, fb_pct);
            let mut energy = 0.0;
            // Impulse train every 64 samples — broadband excitation.
            for i in 0..8192 {
                let dry = if i % 64 == 0 { 1.0 } else { 0.0 };
                let (l, _r) = p.process_sample(dry, dry);
                energy += l * l;
            }
            energy
        };
        let e_low = render_energy(0.0);
        let e_high = render_energy(90.0);
        assert!(
            e_high > e_low * 1.5,
            "fb=90 should accumulate more energy than fb=0 \
             (low={e_low}, high={e_high})"
        );
    }

    #[test]
    fn phaser_stereo_offset_separates_l_and_r() {
        // Mono-sum input through Stereo=100 should produce L ≠ R because
        // the all-pass centre frequencies are offset per channel.
        let mut p = PhaserEffect::new();
        p.set_sample_rate(48_000.0);
        p.set_param(2, 100.0); // Stereo = 100 %
        let mut diff_energy = 0.0_f32;
        for i in 0..2048 {
            let t = i as f32 / 48_000.0;
            // Mid-band tone so both offsets land in audible territory.
            let dry = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            let (l, r) = p.process_sample(dry, dry);
            diff_energy += (l - r) * (l - r);
        }
        assert!(
            diff_energy > 1.0,
            "stereo=100 must produce L ≠ R for a mono input (diff={diff_energy})"
        );
    }

    #[test]
    fn phaser_stereo_zero_collapses_to_mono() {
        // Stereo=0 means identical L/R centre frequencies → identical
        // cascade outputs for a mono-sum input.
        let mut p = PhaserEffect::new();
        p.set_sample_rate(48_000.0);
        p.set_param(2, 0.0);
        for i in 0..2048 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            let (l, r) = p.process_sample(dry, dry);
            assert!(
                (l - r).abs() < 1e-5,
                "stereo=0 must be L==R, sample {i}: {l} vs {r}"
            );
        }
    }

    #[test]
    fn phaser_reset_zeroes_state() {
        // Drive the cascade, reset, then verify the first sample of an
        // impulse-into-silence isn't tainted by the prior state.
        let mut p = PhaserEffect::new();
        p.set_sample_rate(48_000.0);
        for _ in 0..1024 {
            p.process_sample(0.5, 0.5);
        }
        p.reset();
        // After reset, an impulse from silence: cascade input = 1.0 + fb*0
        // = 1.0; first all-pass step produces -a*1 + 0 = -a; subsequent
        // stages cascade. Output = dry + cascade_output = 1.0 + cascade.
        let (l, r) = p.process_sample(1.0, 1.0);
        // Reset clears feedback memory → no residual ringing from prior
        // input. The output of a fresh impulse equals the cascade's
        // impulse response added to dry; both channels match.
        assert!((l - r).abs() < 1e-6, "reset must leave L and R symmetric");
        assert!(l.is_finite(), "reset output must be finite");
    }

    #[test]
    fn phaser_stays_bounded_under_aggressive_modulation() {
        // Worst case: maximum feedback (caps at 95 %) and Centre sweeping
        // wildly via set_param every sample. Output magnitude must stay
        // finite and well below numerical saturation.
        let mut p = PhaserEffect::new();
        p.set_sample_rate(48_000.0);
        p.set_param(1, 95.0); // Feedback at cap
        for i in 0..48_000 {
            // Centre sweeps log-style from 50 Hz to 8 kHz every 4096 samples.
            let phase = ((i as f32 / 4096.0).fract() * 2.0 - 1.0).abs();
            let centre = 50.0 * (160.0_f32).powf(phase); // 50..8000 Hz log
            p.set_param(0, centre);
            let dry = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, r) = p.process_sample(dry, dry);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            assert!(
                l.abs() < 8.0 && r.abs() < 8.0,
                "sample {i} blew up: ({l}, {r})"
            );
        }
    }
}
