//! The effect abstraction — Phase 2 Milestone 2a. A standardized `Effect`
//! trait, the `EffectKind` registry, enum-dispatch `EffectInstance`, two
//! effects (`LowpassEffect`, `BitcrushEffect`), and the persisted per-track
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
}

/// A resonant lowpass — a TPT state-variable filter, lowpass output.
pub struct LowpassEffect {
    cutoff: f32,
    resonance: f32,
    sample_rate: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: [f32; 2],
    ic2: [f32; 2],
}

impl LowpassEffect {
    const PARAMS: [ParamSpec; 2] = [
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
    ];

    /// A `LowpassEffect` at its default parameters; call `set_sample_rate`
    /// before processing.
    pub fn new() -> Self {
        let mut lp = Self {
            cutoff: Self::PARAMS[0].default,
            resonance: Self::PARAMS[1].default,
            sample_rate: 48_000.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            ic1: [0.0; 2],
            ic2: [0.0; 2],
        };
        lp.recompute();
        lp
    }

    /// Recompute the TPT-SVF coefficients from cutoff / resonance / SR.
    fn recompute(&mut self) {
        let sr = self.sample_rate.max(1.0);
        let fc = self.cutoff.clamp(20.0, sr * 0.49);
        let g = (std::f32::consts::PI * fc / sr).tan();
        let q = 0.5 + self.resonance.clamp(0.0, 1.0) * 9.5;
        let k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    /// Process one sample for a single channel using the TPT-SVF integrator form.
    fn svf_step(&mut self, x: f32, ch: usize) -> f32 {
        let v3 = x - self.ic2[ch];
        let v1 = self.a1 * self.ic1[ch] + self.a2 * v3;
        let v2 = self.ic2[ch] + self.a2 * self.ic1[ch] + self.a3 * v3;
        self.ic1[ch] = 2.0 * v1 - self.ic1[ch];
        self.ic2[ch] = 2.0 * v2 - self.ic2[ch];
        v2
    }
}

impl Default for LowpassEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for LowpassEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        (self.svf_step(left, 0), self.svf_step(right, 1))
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute();
    }

    fn reset(&mut self) {
        self.ic1 = [0.0; 2];
        self.ic2 = [0.0; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.cutoff = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.resonance = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
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
    Lowpass,
    Bitcrush,
    Fm,
}

impl EffectKind {
    /// Every effect kind, in display / registry order.
    pub const ALL: [EffectKind; 4] = [
        EffectKind::None,
        EffectKind::Lowpass,
        EffectKind::Bitcrush,
        EffectKind::Fm,
    ];

    /// The kind's display name.
    pub fn name(self) -> &'static str {
        match self {
            EffectKind::None => "None",
            EffectKind::Lowpass => "Lowpass",
            EffectKind::Bitcrush => "Bitcrush",
            EffectKind::Fm => "FM",
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
    Lowpass(LowpassEffect),
    Bitcrush(BitcrushEffect),
    Fm(FmEffect),
}

impl EffectInstance {
    /// A fresh instance of `kind` at default parameters.
    pub fn new(kind: EffectKind) -> Self {
        match kind {
            EffectKind::None => EffectInstance::None(NoneEffect::new()),
            EffectKind::Lowpass => EffectInstance::Lowpass(LowpassEffect::new()),
            EffectKind::Bitcrush => EffectInstance::Bitcrush(BitcrushEffect::new()),
            EffectKind::Fm => EffectInstance::Fm(FmEffect::new()),
        }
    }

    /// Which kind this instance is.
    pub fn kind(&self) -> EffectKind {
        match self {
            EffectInstance::None(_) => EffectKind::None,
            EffectInstance::Lowpass(_) => EffectKind::Lowpass,
            EffectInstance::Bitcrush(_) => EffectKind::Bitcrush,
            EffectInstance::Fm(_) => EffectKind::Fm,
        }
    }
}

impl Effect for EffectInstance {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        match self {
            EffectInstance::None(e) => e.process_sample(left, right),
            EffectInstance::Lowpass(e) => e.process_sample(left, right),
            EffectInstance::Bitcrush(e) => e.process_sample(left, right),
            EffectInstance::Fm(e) => e.process_sample(left, right),
        }
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        match self {
            EffectInstance::None(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Lowpass(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Bitcrush(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Fm(e) => e.set_sample_rate(sample_rate),
        }
    }

    fn reset(&mut self) {
        match self {
            EffectInstance::None(e) => e.reset(),
            EffectInstance::Lowpass(e) => e.reset(),
            EffectInstance::Bitcrush(e) => e.reset(),
            EffectInstance::Fm(e) => e.reset(),
        }
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        match self {
            EffectInstance::None(e) => e.parameters(),
            EffectInstance::Lowpass(e) => e.parameters(),
            EffectInstance::Bitcrush(e) => e.parameters(),
            EffectInstance::Fm(e) => e.parameters(),
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match self {
            EffectInstance::None(e) => e.set_param(index, value),
            EffectInstance::Lowpass(e) => e.set_param(index, value),
            EffectInstance::Bitcrush(e) => e.set_param(index, value),
            EffectInstance::Fm(e) => e.set_param(index, value),
        }
    }
}

/// Maximum modulatable parameters any effect declares — fixes the
/// `TrackEffect::params` array length so the persisted config is stable as
/// effects are added (current max is 2; 4 leaves headroom).
pub const MAX_EFFECT_PARAMS: usize = 5;

/// One track row's persisted effect configuration: which effect, its
/// parameter values, and its dry/wet mix. `params[i]` is the value for the
/// kind's `parameters()[i]`; entries past the kind's parameter count are
/// unused.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrackEffect {
    pub kind: EffectKind,
    pub params: [f32; MAX_EFFECT_PARAMS],
    /// Per-track dry/wet blend, 0.0 (dry) .. 1.0 (full effect). Defaulted on
    /// deserialize so presets predating this field load as fully wet.
    #[serde(default = "default_track_mix")]
    pub mix: f32,
}

/// The serde default for `TrackEffect::mix` — fully wet, matching the
/// pre-`mix` behaviour of any older preset.
fn default_track_mix() -> f32 {
    1.0
}

impl TrackEffect {
    /// The default effect for a track row — no effect, fully wet. Audio
    /// passes through the track unchanged. Users assign an effect kind via
    /// the editor's dropdown.
    pub fn default_for_row(_row: usize) -> Self {
        TrackEffect {
            kind: EffectKind::None,
            params: [0.0; MAX_EFFECT_PARAMS],
            mix: 1.0,
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
    fn lowpass_effect_parameters_are_declared() {
        let lp = LowpassEffect::new();
        let specs = lp.parameters();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "Cutoff");
        assert_eq!(specs[1].name, "Resonance");
        assert!(specs[0].min < specs[0].max);
    }

    #[test]
    fn lowpass_effect_dark_cutoff_attenuates_highs() {
        let mut lp = LowpassEffect::new();
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
        let mut lp = LowpassEffect::new();
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
        let mut lp = LowpassEffect::new();
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
        let mut lp = LowpassEffect::new();
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
        assert_eq!(EffectKind::ALL.len(), 4);
        assert_eq!(EffectKind::None.name(), "None");
        assert_eq!(EffectKind::Lowpass.name(), "Lowpass");
        assert_eq!(EffectKind::Bitcrush.name(), "Bitcrush");
        assert_eq!(EffectKind::Fm.name(), "FM");
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
        let mut lp = EffectInstance::new(EffectKind::Lowpass);
        assert_eq!(lp.kind(), EffectKind::Lowpass);
        assert_eq!(lp.parameters().len(), 2);
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
        let mut e = EffectInstance::new(EffectKind::Lowpass);
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
        let legacy = r#"{"kind":"Lowpass","params":[0.0,0.0,0.0,0.0,0.0]}"#;
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
        let lp = default_params_for_kind(EffectKind::Lowpass);
        assert_eq!(lp[0], LowpassEffect::new().parameters()[0].default);
        assert_eq!(lp[1], LowpassEffect::new().parameters()[1].default);
        // Slots past the kind's parameter count are zero.
        assert_eq!(lp[2], 0.0);
        assert_eq!(lp[3], 0.0);
        let bc = default_params_for_kind(EffectKind::Bitcrush);
        assert_eq!(bc[0], BitcrushEffect::new().parameters()[0].default);
    }

    #[test]
    fn param_count_reports_each_kinds_arity() {
        assert_eq!(param_count(EffectKind::Lowpass), 2);
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
    fn lowpass_cutoff_is_log_hertz_and_resonance_is_linear_number() {
        let specs = LowpassEffect::new().parameters();
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert!(matches!(specs[1].scaling, ParamScaling::Linear));
        assert!(matches!(specs[1].format, ParamFormat::Number { .. }));
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
}
