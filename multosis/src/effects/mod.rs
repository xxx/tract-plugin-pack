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

mod auto_pan;
mod bitcrush;
mod chorus;
mod comb;
mod compressor;
mod delay;
mod diode;
mod distortion;
mod downsample;
mod flanger;
mod fm;
mod frequency_shift;
mod ladder;
mod none;
mod phaser;
mod phaser_filter;
mod pitch_shift;
mod plate;
mod repeat;
mod reverb;
mod ring;
mod satch;
mod spectral_bandpass;
mod spectral_cascade;
mod spectral_compress;
mod spectral_corrupt;
mod spectral_lofi;
mod spectral_mirror;
mod spectral_reverb;
mod spectral_rotate;
mod spectral_scatter;
mod spectral_shift;
mod spectral_smear;
mod spectral_spread;
mod spectral_stretch;
mod spectral_twist;
mod stretch;
mod svf;
mod tremolo;
mod varispeed;
mod vibrato;
mod vocoder;
mod warp_zone;
mod wavefolder;

pub use auto_pan::AutoPanEffect;
pub use bitcrush::BitcrushEffect;
pub use chorus::ChorusEffect;
pub use comb::CombEffect;
pub use compressor::CompressorEffect;
pub use delay::DelayEffect;
pub use diode::DiodeEffect;
pub use distortion::DistortionEffect;
pub use downsample::DownsampleEffect;
pub use flanger::FlangerEffect;
pub use fm::FmEffect;
pub use frequency_shift::FrequencyShiftEffect;
pub use ladder::LadderEffect;
pub use none::NoneEffect;
pub use phaser::PhaserEffect;
pub use phaser_filter::PhaserFilterEffect;
pub use pitch_shift::PitchShiftEffect;
pub use plate::PlateEffect;
pub use repeat::RepeatEffect;
pub use reverb::ReverbEffect;
pub use ring::RingEffect;
pub use satch::SatchEffect;
pub use spectral_bandpass::SpectralBandpassEffect;
pub use spectral_cascade::SpectralCascadeEffect;
pub use spectral_compress::SpectralCompressEffect;
pub use spectral_corrupt::SpectralCorruptEffect;
pub use spectral_lofi::SpectralLofiEffect;
pub use spectral_mirror::SpectralMirrorEffect;
pub use spectral_reverb::SpectralReverbEffect;
pub use spectral_rotate::SpectralRotateEffect;
pub use spectral_scatter::SpectralScatterEffect;
pub use spectral_shift::SpectralShiftEffect;
pub use spectral_smear::SpectralSmearEffect;
pub use spectral_spread::SpectralSpreadEffect;
pub use spectral_stretch::SpectralStretchEffect;
pub use spectral_twist::SpectralTwistEffect;
pub use stretch::StretchEffect;
pub use svf::SvfEffect;
pub use tremolo::TremoloEffect;
pub use varispeed::VarispeedEffect;
pub use vibrato::VibratoEffect;
pub use vocoder::VocoderEffect;
pub use warp_zone::WarpZoneEffect;
pub use wavefolder::WavefolderEffect;

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
    /// Schroeder–Moorer "Freeverb" with per-comb LFO modulation,
    /// pre-delay, and stereo width.
    Reverb,
    /// Latency-free time-domain distortion / waveshaper. Five clip
    /// shapes (Hard / Soft / Cubic / Sine / Fold), Bias for
    /// asymmetric harmonics, post tilt EQ, and output trim.
    Distortion,
    /// CE-1-style stereo chorus: modulated delay tap per channel
    /// with L/R LFO phase offset, capable of flanger, chorus, or
    /// doubler depending on Center.
    Chorus,
    /// Granular pitch shifter spanning +/-24 semitones with grain
    /// frequency, size, feedback, and stereo detune.
    PitchShift,
    /// Soft-knee peak compressor (Threshold + Ratio). Wraps the
    /// same engine as the master-bus compressor: stereo-linked, fixed
    /// 5 ms attack / 50 ms release / 6 dB knee.
    Compressor,
    /// Classic stereo flanger with triangle LFO. Shorter delay
    /// range and hotter feedback defaults than Chorus, tuned for
    /// flanging out of the box.
    Flanger,
    /// Latency-free single-sideband frequency shifter (Bode-style
    /// IIR-Hilbert SSB modulator). Adds a constant Hz offset to
    /// every spectral component, breaking harmonic relationships
    /// for inharmonic / bell / clang sounds.
    FrequencyShift,
    /// Sample-rate reduction with smoothing and timing-jitter
    /// character knobs. Distinct from Bitcrush in that it doesn't
    /// touch bit depth and adds LP-follow smoothing + wow/flutter.
    Downsample,
    /// Tape-style varispeed: signed playback rate (negative =
    /// reverse, 0 = freeze), with drift LFO for tape wobble.
    /// Pitch and time scale together, unlike PitchShift (time-
    /// preserving) and Stretch (pitch-preserving).
    Varispeed,
    /// 16-band channel vocoder: input is the modulator, an
    /// internally-generated Saw / Square / Noise carrier is shaped
    /// by the modulator's per-band amplitude envelope.
    Vocoder,
    /// Circular spectrum rotation -- wraps the positive-half spectrum
    /// modulo N/2 so no energy is discarded. Switchable FFT size
    /// (512 / 1024 / 2048 / 4096); first effect in the Infiltrator-
    /// inspired spectral family.
    SpectralRotate,
    /// Brickwall FFT bandpass -- zeros every bin outside the band
    /// [Freq * 2^(-Width/2), Freq * 2^(Width/2)]. Switchable FFT
    /// size (512 / 1024 / 2048 / 4096); second of the Infiltrator-
    /// inspired spectral effects.
    SpectralBandpass,
    /// In-band spectrum flip -- mirrors bins within +/- Width/2 octaves
    /// of Freq around the centre bin (conjugate swap so phase reflects
    /// rather than just copies). Switchable FFT size (512 / 1024 / 2048
    /// / 4096); third of the Infiltrator-inspired spectral effects.
    SpectralMirror,
    /// Per-bin frequency translate -- moves the spectrum up or down by
    /// +/- 100% of Nyquist; out-of-range bins are zeroed (unlike
    /// SpectralRotate, which wraps). Scale parameter is declared but
    /// the transform is currently Translate-only; Task 14 will fill in
    /// the Scale path. Switchable FFT size (512 / 1024 / 2048 / 4096);
    /// fourth of the Infiltrator-inspired spectral effects.
    SpectralShift,
    /// Per-bin magnitude envelope hold with instant attack and a release
    /// tau equal to Length ms -- transients pass straight through, but the
    /// magnitude lingers after the source drops. Chaos randomises each bin's
    /// phase per hop, breaking inter-bin coherence as the smear runs. Switchable
    /// FFT size (512 / 1024 / 2048 / 4096); sixth of the Infiltrator-inspired
    /// spectral effects.
    SpectralSmear,
    /// Magnitude box-blur across bins -- each bin's magnitude is replaced
    /// by the mean within +/- radius bins (radius scales linearly with
    /// Amount, 0-100% -> 0-16 bins). Phase preserved per-bin. Switchable
    /// FFT size (512 / 1024 / 2048 / 4096); fifth of the Infiltrator-
    /// inspired spectral effects, and the first to use per-channel scratch.
    SpectralSpread,
    /// Bin decimation with optional randomisation. A keep-bitmask refreshed
    /// every Slow hops zeroes a Factor% fraction of bins; Random interpolates
    /// between regular (every Nth bin) and independent random keep. Switchable
    /// FFT size (512 / 1024 / 2048 / 4096); sixth of the Infiltrator-inspired
    /// spectral effects.
    SpectralLofi,
    /// Magnitude-ranked gate -- Amount > 0 zeros the |Amount|% quietest bins
    /// (creative noise-gate), Amount < 0 zeros the loudest (emphasises the
    /// noise floor). Decay carries the previous frame's per-bin gain toward
    /// the new target so cuts/restores fade instead of clicking. Switchable
    /// FFT size (512 / 1024 / 2048 / 4096); seventh of the Infiltrator-
    /// inspired spectral effects.
    SpectralCorrupt,
    /// Per-bin compression toward a target spectrum. Each bin's magnitude
    /// is tracked by a ~50 ms one-pole follower; the target is a power-law
    /// curve f^tone normalised at 1 kHz (tone=-1 pink, 0 flat, +1 white).
    /// Per-bin ratio = (target / current)^Amount preserves phase. Switchable
    /// FFT size (512 / 1024 / 2048 / 4096); eighth of the Infiltrator-
    /// inspired spectral effects.
    SpectralCompress,
    /// Per-bin delay ramped linearly around a Centre frequency. Bins above
    /// Centre are delayed up to Length ms (capped at 128 hops); bins at or
    /// below Centre read "now". Centre low -> upward slide; Centre high ->
    /// downward slide. Feedback recycles delayed values through a per-bin
    /// ring, building cascading echoes. Switchable FFT size (512 / 1024 /
    /// 2048 / 4096); ninth of the Infiltrator-inspired spectral effects,
    /// and the first to carry a multi-frame complex delay line.
    SpectralCascade,
    /// Per-bin one-pole feedback reverb with frequency-dependent T60. Each
    /// bin maintains a complex running tail; the per-bin decay coefficient
    /// is shaped by Tone, which lerps between dark (HF damped, T60 = 0.1 *
    /// Time) and bright (LF damped). Tone = 50 % is roughly uniform decay
    /// across the spectrum at the Time setting. Switchable FFT size (512 /
    /// 1024 / 2048 / 4096); tenth of the Infiltrator-inspired spectral
    /// effects.
    SpectralReverb,
    /// Per-bin random delays in [0, Length ms), reassigned every 1 / Rate
    /// seconds. xorshift32 RNG keeps the audio thread allocation-free.
    /// Feedback recycles delayed values through the per-bin ring so the
    /// random taps re-fire as the assignment refreshes. Switchable FFT
    /// size (512 / 1024 / 2048 / 4096); eleventh of the Infiltrator-
    /// inspired spectral effects, and the second to carry a multi-frame
    /// complex delay line.
    SpectralScatter,
    /// In-band bin folding around a centre frequency. Within +/- Width/2
    /// octaves of Freq, each source bin's offset-from-centre is scaled by
    /// (1 - Twist): Twist=+1 collapses the band onto Freq, Twist=-1 doubles
    /// the spread, Twist=0 is identity. Sources that land on the same
    /// destination accumulate so energy isn't lost on collapse. Switchable
    /// FFT size (512 / 1024 / 2048 / 4096); twelfth of the Infiltrator-
    /// inspired spectral effects.
    SpectralTwist,
    /// Phase-vocoder time stretching with chaos. Unlike the other spectral
    /// effects this owns its own analyzer per FFT size at 75% overlap
    /// (`hop = fft_size / 4`) rather than the shared SpectralEngine's 50%.
    /// Phase A scaffolding (this commit) is an identity passthrough; Phase B
    /// adds the per-bin phase advance scaled by Speed, Tempo throttle on
    /// re-analyze, and Chaos-injected random phase. Switchable FFT size
    /// (512 / 1024 / 2048 / 4096); thirteenth of the Infiltrator-inspired
    /// spectral effects.
    SpectralStretch,
    /// Static notch-comb filter -- cascade of 12 one-pole allpasses summed
    /// with the dry signal. Sits in the Filter family because sweep motion
    /// comes from external modulation of Cutoff (LFO/MSEG), like any other
    /// filter; the Phaser variant in the Modulation family bakes its own
    /// LFO sweep. Mirrors Xfer Serum's filter-section "Phaser" and Matt
    /// Tytel's Vital PhaserFilter.
    PhaserFilter,
    /// 24 dB/oct Moog ladder lowpass with transistor saturation -- the
    /// Huovilainen (2004) model with internal 2x oversampling via
    /// double-tick (zero added latency). Self-oscillates near unity
    /// resonance; Drive pushes harder into the tanh nonlinearity for
    /// the classic "growl".
    Ladder,
    /// 4-pole diode ladder lowpass with TB-303-style HP in the feedback
    /// path. Same parameter surface as `Ladder` but a different topology
    /// and a sharper, squelchier character; sibling effect, port of
    /// Matt Tytel's Vital DiodeFilter.
    Diode,
    /// Amplitude-modulation tremolo. Single LFO multiplies both channels'
    /// gain in unison; Depth = 0 is bypass, Depth = 100 is full chop.
    Tremolo,
    /// Stereo auto-panner: LFO drives a balance between L and R. Depth
    /// = 0 leaves both channels untouched (transparent); Depth = 100
    /// sweeps to silence on each side at the LFO extremes.
    AutoPan,
    /// Pitch-modulating vibrato via a short modulated delay line.
    /// Output is the delayed (pitch-wobbled) signal only; no dry
    /// blend, distinguishing it from `Chorus` / `Flanger`.
    Vibrato,
    /// Buchla 259-style west-coast wavefolder with ADAA1 anti-aliasing.
    /// Five-cell Lockhart-style cascade produces bell-like harmonic
    /// redistribution rather than the odd-harmonic emphasis of the
    /// existing Distortion Fold shape. Port of ChowDSP's
    /// WestCoastWavefolder.
    Wavefolder,
    /// Dattorro 1997 plate reverb: bright, dense, no early reflections.
    /// Distinct from the Schroeder-Moorer `Reverb` -- different topology
    /// (4 input diffusers + cross-coupled figure-8 tank with modulated
    /// allpasses), different character (EMT 140 / Lexicon 224 family).
    Plate,
}

impl EffectKind {
    /// Every effect kind, in display / registry order. All kinds whose
    /// `family()` returns `Some("Spectral")` are grouped contiguously at the
    /// end so the kind-picker can render a single "Spectral" section header
    /// above them.
    pub const ALL: [EffectKind; 44] = [
        // `None` stays first as the only un-categorised kind. The remaining
        // families are alphabetised by family name, with each block sorted
        // alphabetically by display name. Spectral is kept last as the big
        // FFT-based block.
        EffectKind::None,
        // Distortion
        EffectKind::Bitcrush,
        EffectKind::Distortion,
        EffectKind::Downsample,
        EffectKind::Wavefolder,
        // Filter
        EffectKind::Comb,
        EffectKind::Diode,
        EffectKind::Ladder,
        EffectKind::PhaserFilter,
        EffectKind::Svf,
        // Misc
        EffectKind::Compressor,
        EffectKind::Vocoder,
        // Modulation
        EffectKind::AutoPan,
        EffectKind::Chorus,
        EffectKind::Flanger,
        EffectKind::Fm,
        EffectKind::Phaser,
        EffectKind::Ring,
        EffectKind::Tremolo,
        EffectKind::Vibrato,
        // Pitch
        EffectKind::FrequencyShift,
        EffectKind::PitchShift,
        EffectKind::Varispeed,
        // Time
        EffectKind::Delay,
        EffectKind::Plate,
        EffectKind::Repeat,
        EffectKind::Reverb,
        EffectKind::Stretch,
        // Spectral
        EffectKind::SpectralBandpass,
        EffectKind::SpectralCascade,
        EffectKind::SpectralCompress,
        EffectKind::SpectralCorrupt,
        EffectKind::SpectralLofi,
        EffectKind::SpectralMirror,
        EffectKind::SpectralReverb,
        EffectKind::SpectralRotate,
        EffectKind::Satch,
        EffectKind::SpectralScatter,
        EffectKind::SpectralShift,
        EffectKind::SpectralSmear,
        EffectKind::SpectralSpread,
        EffectKind::SpectralStretch,
        EffectKind::SpectralTwist,
        EffectKind::WarpZone,
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
            EffectKind::Reverb => "Reverb",
            EffectKind::Distortion => "Distortion",
            EffectKind::Chorus => "Chorus",
            EffectKind::PitchShift => "Pitch Shift",
            EffectKind::Compressor => "Compressor",
            EffectKind::Flanger => "Flanger",
            EffectKind::FrequencyShift => "Freq Shift",
            EffectKind::Downsample => "Downsample",
            EffectKind::Varispeed => "Varispeed",
            EffectKind::Vocoder => "Vocoder",
            EffectKind::SpectralRotate => "Rotate",
            EffectKind::SpectralBandpass => "Bandpass",
            EffectKind::SpectralMirror => "Mirror",
            EffectKind::SpectralShift => "Shift",
            EffectKind::SpectralSmear => "Smear",
            EffectKind::SpectralSpread => "Spread",
            EffectKind::SpectralLofi => "Lofi",
            EffectKind::SpectralCorrupt => "Corrupt",
            EffectKind::SpectralCompress => "Compress",
            EffectKind::SpectralCascade => "Cascade",
            EffectKind::SpectralReverb => "Reverb",
            EffectKind::SpectralScatter => "Scatter",
            EffectKind::SpectralTwist => "Twist",
            EffectKind::SpectralStretch => "Stretch",
            EffectKind::PhaserFilter => "Phaser Filter",
            EffectKind::Ladder => "Ladder",
            EffectKind::Diode => "Diode",
            EffectKind::Tremolo => "Tremolo",
            EffectKind::AutoPan => "Auto Pan",
            EffectKind::Vibrato => "Vibrato",
            EffectKind::Wavefolder => "Wavefolder",
            EffectKind::Plate => "Plate",
        }
    }

    /// The family this kind belongs to, or `None` for `EffectKind::None`
    /// (the only un-categorised kind). Drives the two-line track-listing
    /// label (family caption above `name()`) and groups kinds in the
    /// effect-kind picker. `name()` returns just the unique suffix for
    /// spectral kinds; family kinds elsewhere keep their natural name.
    pub fn family(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Bitcrush | Self::Distortion | Self::Downsample | Self::Wavefolder => {
                Some("Distortion")
            }
            Self::Comb | Self::Diode | Self::Ladder | Self::PhaserFilter | Self::Svf => {
                Some("Filter")
            }
            Self::Compressor | Self::Vocoder => Some("Misc"),
            Self::AutoPan
            | Self::Chorus
            | Self::Flanger
            | Self::Fm
            | Self::Phaser
            | Self::Ring
            | Self::Tremolo
            | Self::Vibrato => Some("Modulation"),
            Self::FrequencyShift | Self::PitchShift | Self::Varispeed => Some("Pitch"),
            Self::Delay | Self::Plate | Self::Repeat | Self::Reverb | Self::Stretch => {
                Some("Time")
            }
            Self::Satch
            | Self::WarpZone
            | Self::SpectralRotate
            | Self::SpectralBandpass
            | Self::SpectralMirror
            | Self::SpectralShift
            | Self::SpectralSmear
            | Self::SpectralSpread
            | Self::SpectralLofi
            | Self::SpectralCorrupt
            | Self::SpectralCompress
            | Self::SpectralCascade
            | Self::SpectralReverb
            | Self::SpectralScatter
            | Self::SpectralTwist
            | Self::SpectralStretch => Some("Spectral"),
        }
    }

    /// True iff a default-parameter instance of this kind reports
    /// nonzero `latency_samples()` to the host. Used by the editor
    /// to draw a "this row adds PDC" badge on the track listing.
    ///
    /// Today this is purely a function of kind (Satch and WarpZone
    /// have fixed FFT-sized latency; everything else is zero-latency);
    /// if a future kind grows a state-dependent latency the editor
    /// will need a per-instance hook here instead. The
    /// `effect_kind_reports_latency_matches_instance_latency` test
    /// keeps this in sync with the trait impls.
    pub fn reports_latency(self) -> bool {
        matches!(
            self,
            Self::Satch
                | Self::WarpZone
                | Self::SpectralRotate
                | Self::SpectralBandpass
                | Self::SpectralMirror
                | Self::SpectralShift
                | Self::SpectralSmear
                | Self::SpectralSpread
                | Self::SpectralLofi
                | Self::SpectralCorrupt
                | Self::SpectralCompress
                | Self::SpectralCascade
                | Self::SpectralReverb
                | Self::SpectralScatter
                | Self::SpectralTwist
                | Self::SpectralStretch
        )
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
    // Boxed: ReverbEffect carries 8 stereo comb lines + 4 stereo
    // allpasses + pre-delay + per-comb LFO state — the struct itself
    // (just the metadata, not the Vec heap data) is ~1 KB, well past
    // clippy's `large-enum-variant` threshold. Box the variant so
    // every EffectInstance slot stays compact.
    Reverb(Box<ReverbEffect>),
    // Not boxed — DistortionEffect is tiny (10 f32s, no heap).
    Distortion(DistortionEffect),
    // Not boxed — ChorusEffect itself is small; the two Vec ring
    // buffers it holds are already heap-allocated by Vec.
    Chorus(ChorusEffect),
    // Boxed: PitchShiftEffect holds a [Grain; 16] pool plus the
    // standard ring buffers; the struct itself is ~420 B, past
    // clippy's large-enum-variant threshold.
    PitchShift(Box<PitchShiftEffect>),
    // Not boxed -- CompressorEffect is tiny (the inner Compressor
    // has 6 f32s plus the two cached param values).
    Compressor(CompressorEffect),
    // Not boxed -- FlangerEffect itself is small; the two Vec
    // ring buffers it holds are already heap-allocated by Vec.
    Flanger(FlangerEffect),
    // Boxed: the Signalsmith Hilbert IIR carries 6 inline `[f32; 12]`
    // arrays per channel (state + coefficients + poles) -- ~576 B for
    // the stereo pair alone, past clippy's `large-enum-variant`
    // threshold. The Box allocation happens once per kind-switch from
    // the GUI thread, never on the audio path.
    FrequencyShift(Box<FrequencyShiftEffect>),
    // Not boxed -- DownsampleEffect is tiny (~13 f32s + a u32, no heap).
    Downsample(DownsampleEffect),
    // Boxed: VarispeedEffect carries a [Grain; 16] pool (~320 B)
    // plus the ring buffer metadata; the struct itself is past
    // clippy's large-enum-variant threshold. Same Box-default
    // pattern as PitchShift.
    Varispeed(Box<VarispeedEffect>),
    // Boxed: VocoderEffect holds 48 SvfBp filter states (16 mod
    // x 2 channels + 16 shared carrier) plus 32 envelope-follower
    // floats; struct is ~940 B, past clippy's large-enum-variant
    // threshold.
    Vocoder(Box<VocoderEffect>),
    // Boxed: SpectralRotateEffect carries two SpectralEngine instances,
    // each pre-allocating all four FFT-size slots (analyzer ring + IFFT
    // scratch + output ring per slot) -- the struct is large enough to
    // trip clippy's large-enum-variant threshold. Allocation happens
    // once per kind-switch from the GUI thread, never on the audio path.
    SpectralRotate(Box<SpectralRotateEffect>),
    // Boxed for the same reason as SpectralRotate: two SpectralEngine
    // instances with all four FFT-size slots pre-allocated.
    SpectralBandpass(Box<SpectralBandpassEffect>),
    // Boxed for the same reason as SpectralRotate: two SpectralEngine
    // instances with all four FFT-size slots pre-allocated.
    SpectralMirror(Box<SpectralMirrorEffect>),
    // Boxed for the same reason as SpectralRotate: two SpectralEngine
    // instances with all four FFT-size slots pre-allocated.
    SpectralShift(Box<SpectralShiftEffect>),
    // Boxed for the same reason as SpectralRotate: two SpectralEngine
    // instances with all four FFT-size slots pre-allocated (plus the
    // per-channel last-magnitude scratch and RNG state).
    SpectralSmear(Box<SpectralSmearEffect>),
    // Boxed for the same reason as SpectralRotate: two SpectralEngine
    // instances with all four FFT-size slots pre-allocated (plus the
    // per-channel magnitude scratch).
    SpectralSpread(Box<SpectralSpreadEffect>),
    // Boxed for the same reason as SpectralRotate: two SpectralEngine
    // instances with all four FFT-size slots pre-allocated (plus the
    // per-channel keep_mask scratch and RNG state).
    SpectralLofi(Box<SpectralLofiEffect>),
    // Boxed for the same reason as SpectralRotate: two SpectralEngine
    // instances with all four FFT-size slots pre-allocated (plus the
    // per-channel bin-index, bin-gain, and target scratch buffers).
    SpectralCorrupt(Box<SpectralCorruptEffect>),
    // Boxed for the same reason as SpectralRotate: two SpectralEngine
    // instances with all four FFT-size slots pre-allocated (plus the
    // per-channel magnitude envelope scratch).
    SpectralCompress(Box<SpectralCompressEffect>),
    // Boxed for the same reason as SpectralRotate: two SpectralEngine
    // instances with all four FFT-size slots pre-allocated, plus the
    // per-channel complex delay ring (128 hops x 2049 bins ~= 2 MB per
    // channel). The struct metadata itself is past clippy's
    // large-enum-variant threshold; the ring's Vec heap data lives off-
    // enum, so the variant footprint stays compact.
    SpectralCascade(Box<SpectralCascadeEffect>),
    // Boxed for the same reason as SpectralRotate: two SpectralEngine
    // instances with all four FFT-size slots pre-allocated (plus the
    // per-channel complex tail scratch).
    SpectralReverb(Box<SpectralReverbEffect>),
    // Boxed for the same reason as SpectralCascade: two SpectralEngine
    // instances with all four FFT-size slots pre-allocated, plus the
    // per-channel complex delay ring (128 hops x 2049 bins ~= 2 MB per
    // channel). The struct metadata itself is past clippy's
    // large-enum-variant threshold; the ring's Vec heap data lives off-
    // enum, so the variant footprint stays compact.
    SpectralScatter(Box<SpectralScatterEffect>),
    // Boxed for the same reason as SpectralRotate: two SpectralEngine
    // instances with all four FFT-size slots pre-allocated.
    SpectralTwist(Box<SpectralTwistEffect>),
    // Boxed: SpectralStretchEffect carries its own per-channel four-slot
    // analyzer pool at 75% overlap (vs SpectralEngine's 50%), plus the
    // per-slot phase-tracker vectors that Phase B will use. The struct
    // metadata is past clippy's large-enum-variant threshold; the per-slot
    // Vec heap data lives off-enum, so the variant footprint stays compact.
    SpectralStretch(Box<SpectralStretchEffect>),
    // Not boxed -- PhaserFilterEffect is tiny: 12 allpass states + 2
    // conditioning states + 1 feedback sample per channel, all inline.
    PhaserFilter(PhaserFilterEffect),
    // Not boxed -- LadderEffect is tiny: 4 stages + 3 cached tanh + 4
    // delay + 2 phase-delay samples per channel, all inline (~22 floats).
    Ladder(LadderEffect),
    // Not boxed -- DiodeEffect is tiny: 4 unsaturated + 4 saturated stage
    // states + 1 HP feedback state per channel (~18 floats).
    Diode(DiodeEffect),
    // Not boxed -- TremoloEffect is tiny: one phase accumulator + params.
    Tremolo(TremoloEffect),
    // Not boxed -- AutoPanEffect is tiny: one phase accumulator + params.
    AutoPan(AutoPanEffect),
    // Not boxed -- VibratoEffect itself is small; the two Vec delay
    // buffers it holds are already heap-allocated by Vec.
    Vibrato(VibratoEffect),
    // Not boxed -- WavefolderEffect is tiny: previous-input + DC blocker
    // state per channel (~6 floats).
    Wavefolder(WavefolderEffect),
    // Boxed: PlateEffect carries 11 delay rings of 65 KB each (~720 KB
    // total) sized for 192 kHz operation. The struct metadata alone
    // (the Vec pointers/lens/caps and cached coefficient arrays) is
    // well past clippy's large-enum-variant threshold; boxing keeps
    // the EffectInstance enum compact. The Vec heap data lives off-
    // enum either way.
    Plate(Box<PlateEffect>),
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
            EffectKind::Reverb => EffectInstance::Reverb(Box::default()),
            EffectKind::Distortion => EffectInstance::Distortion(DistortionEffect::new()),
            EffectKind::Chorus => EffectInstance::Chorus(ChorusEffect::new()),
            EffectKind::PitchShift => EffectInstance::PitchShift(Box::default()),
            EffectKind::Compressor => EffectInstance::Compressor(CompressorEffect::new()),
            EffectKind::Flanger => EffectInstance::Flanger(FlangerEffect::new()),
            EffectKind::FrequencyShift => EffectInstance::FrequencyShift(Box::default()),
            EffectKind::Downsample => EffectInstance::Downsample(DownsampleEffect::new()),
            EffectKind::Varispeed => EffectInstance::Varispeed(Box::default()),
            EffectKind::Vocoder => EffectInstance::Vocoder(Box::default()),
            EffectKind::SpectralRotate => EffectInstance::SpectralRotate(Box::default()),
            EffectKind::SpectralBandpass => EffectInstance::SpectralBandpass(Box::default()),
            EffectKind::SpectralMirror => EffectInstance::SpectralMirror(Box::default()),
            EffectKind::SpectralShift => EffectInstance::SpectralShift(Box::default()),
            EffectKind::SpectralSmear => EffectInstance::SpectralSmear(Box::default()),
            EffectKind::SpectralSpread => EffectInstance::SpectralSpread(Box::default()),
            EffectKind::SpectralLofi => EffectInstance::SpectralLofi(Box::default()),
            EffectKind::SpectralCorrupt => EffectInstance::SpectralCorrupt(Box::default()),
            EffectKind::SpectralCompress => EffectInstance::SpectralCompress(Box::default()),
            EffectKind::SpectralCascade => EffectInstance::SpectralCascade(Box::default()),
            EffectKind::SpectralReverb => EffectInstance::SpectralReverb(Box::default()),
            EffectKind::SpectralScatter => EffectInstance::SpectralScatter(Box::default()),
            EffectKind::SpectralTwist => EffectInstance::SpectralTwist(Box::default()),
            EffectKind::SpectralStretch => EffectInstance::SpectralStretch(Box::default()),
            EffectKind::PhaserFilter => EffectInstance::PhaserFilter(PhaserFilterEffect::new()),
            EffectKind::Ladder => EffectInstance::Ladder(LadderEffect::new()),
            EffectKind::Diode => EffectInstance::Diode(DiodeEffect::new()),
            EffectKind::Tremolo => EffectInstance::Tremolo(TremoloEffect::new()),
            EffectKind::AutoPan => EffectInstance::AutoPan(AutoPanEffect::new()),
            EffectKind::Vibrato => EffectInstance::Vibrato(VibratoEffect::new()),
            EffectKind::Wavefolder => EffectInstance::Wavefolder(WavefolderEffect::new()),
            EffectKind::Plate => EffectInstance::Plate(Box::default()),
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
            EffectInstance::Reverb(_) => EffectKind::Reverb,
            EffectInstance::Distortion(_) => EffectKind::Distortion,
            EffectInstance::Chorus(_) => EffectKind::Chorus,
            EffectInstance::PitchShift(_) => EffectKind::PitchShift,
            EffectInstance::Compressor(_) => EffectKind::Compressor,
            EffectInstance::Flanger(_) => EffectKind::Flanger,
            EffectInstance::FrequencyShift(_) => EffectKind::FrequencyShift,
            EffectInstance::Downsample(_) => EffectKind::Downsample,
            EffectInstance::Varispeed(_) => EffectKind::Varispeed,
            EffectInstance::Vocoder(_) => EffectKind::Vocoder,
            EffectInstance::SpectralRotate(_) => EffectKind::SpectralRotate,
            EffectInstance::SpectralBandpass(_) => EffectKind::SpectralBandpass,
            EffectInstance::SpectralMirror(_) => EffectKind::SpectralMirror,
            EffectInstance::SpectralShift(_) => EffectKind::SpectralShift,
            EffectInstance::SpectralSmear(_) => EffectKind::SpectralSmear,
            EffectInstance::SpectralSpread(_) => EffectKind::SpectralSpread,
            EffectInstance::SpectralLofi(_) => EffectKind::SpectralLofi,
            EffectInstance::SpectralCorrupt(_) => EffectKind::SpectralCorrupt,
            EffectInstance::SpectralCompress(_) => EffectKind::SpectralCompress,
            EffectInstance::SpectralCascade(_) => EffectKind::SpectralCascade,
            EffectInstance::SpectralReverb(_) => EffectKind::SpectralReverb,
            EffectInstance::SpectralScatter(_) => EffectKind::SpectralScatter,
            EffectInstance::SpectralTwist(_) => EffectKind::SpectralTwist,
            EffectInstance::SpectralStretch(_) => EffectKind::SpectralStretch,
            EffectInstance::PhaserFilter(_) => EffectKind::PhaserFilter,
            EffectInstance::Ladder(_) => EffectKind::Ladder,
            EffectInstance::Diode(_) => EffectKind::Diode,
            EffectInstance::Tremolo(_) => EffectKind::Tremolo,
            EffectInstance::AutoPan(_) => EffectKind::AutoPan,
            EffectInstance::Vibrato(_) => EffectKind::Vibrato,
            EffectInstance::Wavefolder(_) => EffectKind::Wavefolder,
            EffectInstance::Plate(_) => EffectKind::Plate,
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
            EffectInstance::Reverb(e) => e.process_sample(left, right),
            EffectInstance::Distortion(e) => e.process_sample(left, right),
            EffectInstance::Chorus(e) => e.process_sample(left, right),
            EffectInstance::PitchShift(e) => e.process_sample(left, right),
            EffectInstance::Compressor(e) => e.process_sample(left, right),
            EffectInstance::Flanger(e) => e.process_sample(left, right),
            EffectInstance::FrequencyShift(e) => e.process_sample(left, right),
            EffectInstance::Downsample(e) => e.process_sample(left, right),
            EffectInstance::Varispeed(e) => e.process_sample(left, right),
            EffectInstance::Vocoder(e) => e.process_sample(left, right),
            EffectInstance::SpectralRotate(e) => e.process_sample(left, right),
            EffectInstance::SpectralBandpass(e) => e.process_sample(left, right),
            EffectInstance::SpectralMirror(e) => e.process_sample(left, right),
            EffectInstance::SpectralShift(e) => e.process_sample(left, right),
            EffectInstance::SpectralSmear(e) => e.process_sample(left, right),
            EffectInstance::SpectralSpread(e) => e.process_sample(left, right),
            EffectInstance::SpectralLofi(e) => e.process_sample(left, right),
            EffectInstance::SpectralCorrupt(e) => e.process_sample(left, right),
            EffectInstance::SpectralCompress(e) => e.process_sample(left, right),
            EffectInstance::SpectralCascade(e) => e.process_sample(left, right),
            EffectInstance::SpectralReverb(e) => e.process_sample(left, right),
            EffectInstance::SpectralScatter(e) => e.process_sample(left, right),
            EffectInstance::SpectralTwist(e) => e.process_sample(left, right),
            EffectInstance::SpectralStretch(e) => e.process_sample(left, right),
            EffectInstance::PhaserFilter(e) => e.process_sample(left, right),
            EffectInstance::Ladder(e) => e.process_sample(left, right),
            EffectInstance::Diode(e) => e.process_sample(left, right),
            EffectInstance::Tremolo(e) => e.process_sample(left, right),
            EffectInstance::AutoPan(e) => e.process_sample(left, right),
            EffectInstance::Vibrato(e) => e.process_sample(left, right),
            EffectInstance::Wavefolder(e) => e.process_sample(left, right),
            EffectInstance::Plate(e) => e.process_sample(left, right),
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
            EffectInstance::Reverb(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Distortion(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Chorus(e) => e.set_sample_rate(sample_rate),
            EffectInstance::PitchShift(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Compressor(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Flanger(e) => e.set_sample_rate(sample_rate),
            EffectInstance::FrequencyShift(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Downsample(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Varispeed(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Vocoder(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralRotate(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralBandpass(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralMirror(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralShift(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralSmear(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralSpread(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralLofi(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralCorrupt(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralCompress(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralCascade(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralReverb(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralScatter(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralTwist(e) => e.set_sample_rate(sample_rate),
            EffectInstance::SpectralStretch(e) => e.set_sample_rate(sample_rate),
            EffectInstance::PhaserFilter(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Ladder(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Diode(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Tremolo(e) => e.set_sample_rate(sample_rate),
            EffectInstance::AutoPan(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Vibrato(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Wavefolder(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Plate(e) => e.set_sample_rate(sample_rate),
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
            EffectInstance::Reverb(e) => e.reset(),
            EffectInstance::Distortion(e) => e.reset(),
            EffectInstance::Chorus(e) => e.reset(),
            EffectInstance::PitchShift(e) => e.reset(),
            EffectInstance::Compressor(e) => e.reset(),
            EffectInstance::Flanger(e) => e.reset(),
            EffectInstance::FrequencyShift(e) => e.reset(),
            EffectInstance::Downsample(e) => e.reset(),
            EffectInstance::Varispeed(e) => e.reset(),
            EffectInstance::Vocoder(e) => e.reset(),
            EffectInstance::SpectralRotate(e) => e.reset(),
            EffectInstance::SpectralBandpass(e) => e.reset(),
            EffectInstance::SpectralMirror(e) => e.reset(),
            EffectInstance::SpectralShift(e) => e.reset(),
            EffectInstance::SpectralSmear(e) => e.reset(),
            EffectInstance::SpectralSpread(e) => e.reset(),
            EffectInstance::SpectralLofi(e) => e.reset(),
            EffectInstance::SpectralCorrupt(e) => e.reset(),
            EffectInstance::SpectralCompress(e) => e.reset(),
            EffectInstance::SpectralCascade(e) => e.reset(),
            EffectInstance::SpectralReverb(e) => e.reset(),
            EffectInstance::SpectralScatter(e) => e.reset(),
            EffectInstance::SpectralTwist(e) => e.reset(),
            EffectInstance::SpectralStretch(e) => e.reset(),
            EffectInstance::PhaserFilter(e) => e.reset(),
            EffectInstance::Ladder(e) => e.reset(),
            EffectInstance::Diode(e) => e.reset(),
            EffectInstance::Tremolo(e) => e.reset(),
            EffectInstance::AutoPan(e) => e.reset(),
            EffectInstance::Vibrato(e) => e.reset(),
            EffectInstance::Wavefolder(e) => e.reset(),
            EffectInstance::Plate(e) => e.reset(),
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
            EffectInstance::Reverb(e) => e.parameters(),
            EffectInstance::Distortion(e) => e.parameters(),
            EffectInstance::Chorus(e) => e.parameters(),
            EffectInstance::PitchShift(e) => e.parameters(),
            EffectInstance::Compressor(e) => e.parameters(),
            EffectInstance::Flanger(e) => e.parameters(),
            EffectInstance::FrequencyShift(e) => e.parameters(),
            EffectInstance::Downsample(e) => e.parameters(),
            EffectInstance::Varispeed(e) => e.parameters(),
            EffectInstance::Vocoder(e) => e.parameters(),
            EffectInstance::SpectralRotate(e) => e.parameters(),
            EffectInstance::SpectralBandpass(e) => e.parameters(),
            EffectInstance::SpectralMirror(e) => e.parameters(),
            EffectInstance::SpectralShift(e) => e.parameters(),
            EffectInstance::SpectralSmear(e) => e.parameters(),
            EffectInstance::SpectralSpread(e) => e.parameters(),
            EffectInstance::SpectralLofi(e) => e.parameters(),
            EffectInstance::SpectralCorrupt(e) => e.parameters(),
            EffectInstance::SpectralCompress(e) => e.parameters(),
            EffectInstance::SpectralCascade(e) => e.parameters(),
            EffectInstance::SpectralReverb(e) => e.parameters(),
            EffectInstance::SpectralScatter(e) => e.parameters(),
            EffectInstance::SpectralTwist(e) => e.parameters(),
            EffectInstance::SpectralStretch(e) => e.parameters(),
            EffectInstance::PhaserFilter(e) => e.parameters(),
            EffectInstance::Ladder(e) => e.parameters(),
            EffectInstance::Diode(e) => e.parameters(),
            EffectInstance::Tremolo(e) => e.parameters(),
            EffectInstance::AutoPan(e) => e.parameters(),
            EffectInstance::Vibrato(e) => e.parameters(),
            EffectInstance::Wavefolder(e) => e.parameters(),
            EffectInstance::Plate(e) => e.parameters(),
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
            EffectInstance::Reverb(e) => e.set_param(index, value),
            EffectInstance::Distortion(e) => e.set_param(index, value),
            EffectInstance::Chorus(e) => e.set_param(index, value),
            EffectInstance::PitchShift(e) => e.set_param(index, value),
            EffectInstance::Compressor(e) => e.set_param(index, value),
            EffectInstance::Flanger(e) => e.set_param(index, value),
            EffectInstance::FrequencyShift(e) => e.set_param(index, value),
            EffectInstance::Downsample(e) => e.set_param(index, value),
            EffectInstance::Varispeed(e) => e.set_param(index, value),
            EffectInstance::Vocoder(e) => e.set_param(index, value),
            EffectInstance::SpectralRotate(e) => e.set_param(index, value),
            EffectInstance::SpectralBandpass(e) => e.set_param(index, value),
            EffectInstance::SpectralMirror(e) => e.set_param(index, value),
            EffectInstance::SpectralShift(e) => e.set_param(index, value),
            EffectInstance::SpectralSmear(e) => e.set_param(index, value),
            EffectInstance::SpectralSpread(e) => e.set_param(index, value),
            EffectInstance::SpectralLofi(e) => e.set_param(index, value),
            EffectInstance::SpectralCorrupt(e) => e.set_param(index, value),
            EffectInstance::SpectralCompress(e) => e.set_param(index, value),
            EffectInstance::SpectralCascade(e) => e.set_param(index, value),
            EffectInstance::SpectralReverb(e) => e.set_param(index, value),
            EffectInstance::SpectralScatter(e) => e.set_param(index, value),
            EffectInstance::SpectralTwist(e) => e.set_param(index, value),
            EffectInstance::SpectralStretch(e) => e.set_param(index, value),
            EffectInstance::PhaserFilter(e) => e.set_param(index, value),
            EffectInstance::Ladder(e) => e.set_param(index, value),
            EffectInstance::Diode(e) => e.set_param(index, value),
            EffectInstance::Tremolo(e) => e.set_param(index, value),
            EffectInstance::AutoPan(e) => e.set_param(index, value),
            EffectInstance::Vibrato(e) => e.set_param(index, value),
            EffectInstance::Wavefolder(e) => e.set_param(index, value),
            EffectInstance::Plate(e) => e.set_param(index, value),
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
            EffectInstance::Reverb(e) => e.set_bpm(bpm),
            EffectInstance::Distortion(e) => e.set_bpm(bpm),
            EffectInstance::Chorus(e) => e.set_bpm(bpm),
            EffectInstance::PitchShift(e) => e.set_bpm(bpm),
            EffectInstance::Compressor(e) => e.set_bpm(bpm),
            EffectInstance::Flanger(e) => e.set_bpm(bpm),
            EffectInstance::FrequencyShift(e) => e.set_bpm(bpm),
            EffectInstance::Downsample(e) => e.set_bpm(bpm),
            EffectInstance::Varispeed(e) => e.set_bpm(bpm),
            EffectInstance::Vocoder(e) => e.set_bpm(bpm),
            EffectInstance::SpectralRotate(e) => e.set_bpm(bpm),
            EffectInstance::SpectralBandpass(e) => e.set_bpm(bpm),
            EffectInstance::SpectralMirror(e) => e.set_bpm(bpm),
            EffectInstance::SpectralShift(e) => e.set_bpm(bpm),
            EffectInstance::SpectralSmear(e) => e.set_bpm(bpm),
            EffectInstance::SpectralSpread(e) => e.set_bpm(bpm),
            EffectInstance::SpectralLofi(e) => e.set_bpm(bpm),
            EffectInstance::SpectralCorrupt(e) => e.set_bpm(bpm),
            EffectInstance::SpectralCompress(e) => e.set_bpm(bpm),
            EffectInstance::SpectralCascade(e) => e.set_bpm(bpm),
            EffectInstance::SpectralReverb(e) => e.set_bpm(bpm),
            EffectInstance::SpectralScatter(e) => e.set_bpm(bpm),
            EffectInstance::SpectralTwist(e) => e.set_bpm(bpm),
            EffectInstance::SpectralStretch(e) => e.set_bpm(bpm),
            EffectInstance::PhaserFilter(e) => e.set_bpm(bpm),
            EffectInstance::Ladder(e) => e.set_bpm(bpm),
            EffectInstance::Diode(e) => e.set_bpm(bpm),
            EffectInstance::Tremolo(e) => e.set_bpm(bpm),
            EffectInstance::AutoPan(e) => e.set_bpm(bpm),
            EffectInstance::Vibrato(e) => e.set_bpm(bpm),
            EffectInstance::Wavefolder(e) => e.set_bpm(bpm),
            EffectInstance::Plate(e) => e.set_bpm(bpm),
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
            EffectInstance::Reverb(e) => e.param_dimmed(index),
            EffectInstance::Distortion(e) => e.param_dimmed(index),
            EffectInstance::Chorus(e) => e.param_dimmed(index),
            EffectInstance::PitchShift(e) => e.param_dimmed(index),
            EffectInstance::Compressor(e) => e.param_dimmed(index),
            EffectInstance::Flanger(e) => e.param_dimmed(index),
            EffectInstance::FrequencyShift(e) => e.param_dimmed(index),
            EffectInstance::Downsample(e) => e.param_dimmed(index),
            EffectInstance::Varispeed(e) => e.param_dimmed(index),
            EffectInstance::Vocoder(e) => e.param_dimmed(index),
            EffectInstance::SpectralRotate(e) => e.param_dimmed(index),
            EffectInstance::SpectralBandpass(e) => e.param_dimmed(index),
            EffectInstance::SpectralMirror(e) => e.param_dimmed(index),
            EffectInstance::SpectralShift(e) => e.param_dimmed(index),
            EffectInstance::SpectralSmear(e) => e.param_dimmed(index),
            EffectInstance::SpectralSpread(e) => e.param_dimmed(index),
            EffectInstance::SpectralLofi(e) => e.param_dimmed(index),
            EffectInstance::SpectralCorrupt(e) => e.param_dimmed(index),
            EffectInstance::SpectralCompress(e) => e.param_dimmed(index),
            EffectInstance::SpectralCascade(e) => e.param_dimmed(index),
            EffectInstance::SpectralReverb(e) => e.param_dimmed(index),
            EffectInstance::SpectralScatter(e) => e.param_dimmed(index),
            EffectInstance::SpectralTwist(e) => e.param_dimmed(index),
            EffectInstance::SpectralStretch(e) => e.param_dimmed(index),
            EffectInstance::PhaserFilter(e) => e.param_dimmed(index),
            EffectInstance::Ladder(e) => e.param_dimmed(index),
            EffectInstance::Diode(e) => e.param_dimmed(index),
            EffectInstance::Tremolo(e) => e.param_dimmed(index),
            EffectInstance::AutoPan(e) => e.param_dimmed(index),
            EffectInstance::Vibrato(e) => e.param_dimmed(index),
            EffectInstance::Wavefolder(e) => e.param_dimmed(index),
            EffectInstance::Plate(e) => e.param_dimmed(index),
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
            EffectInstance::Reverb(e) => e.latency_samples(),
            EffectInstance::Distortion(e) => e.latency_samples(),
            EffectInstance::Chorus(e) => e.latency_samples(),
            EffectInstance::PitchShift(e) => e.latency_samples(),
            EffectInstance::Compressor(e) => e.latency_samples(),
            EffectInstance::Flanger(e) => e.latency_samples(),
            EffectInstance::FrequencyShift(e) => e.latency_samples(),
            EffectInstance::Downsample(e) => e.latency_samples(),
            EffectInstance::Varispeed(e) => e.latency_samples(),
            EffectInstance::Vocoder(e) => e.latency_samples(),
            EffectInstance::SpectralRotate(e) => e.latency_samples(),
            EffectInstance::SpectralBandpass(e) => e.latency_samples(),
            EffectInstance::SpectralMirror(e) => e.latency_samples(),
            EffectInstance::SpectralShift(e) => e.latency_samples(),
            EffectInstance::SpectralSmear(e) => e.latency_samples(),
            EffectInstance::SpectralSpread(e) => e.latency_samples(),
            EffectInstance::SpectralLofi(e) => e.latency_samples(),
            EffectInstance::SpectralCorrupt(e) => e.latency_samples(),
            EffectInstance::SpectralCompress(e) => e.latency_samples(),
            EffectInstance::SpectralCascade(e) => e.latency_samples(),
            EffectInstance::SpectralReverb(e) => e.latency_samples(),
            EffectInstance::SpectralScatter(e) => e.latency_samples(),
            EffectInstance::SpectralTwist(e) => e.latency_samples(),
            EffectInstance::SpectralStretch(e) => e.latency_samples(),
            EffectInstance::PhaserFilter(e) => e.latency_samples(),
            EffectInstance::Ladder(e) => e.latency_samples(),
            EffectInstance::Diode(e) => e.latency_samples(),
            EffectInstance::Tremolo(e) => e.latency_samples(),
            EffectInstance::AutoPan(e) => e.latency_samples(),
            EffectInstance::Vibrato(e) => e.latency_samples(),
            EffectInstance::Wavefolder(e) => e.latency_samples(),
            EffectInstance::Plate(e) => e.latency_samples(),
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
    fn effect_kind_registry() {
        assert_eq!(EffectKind::ALL.len(), 44);
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
        assert_eq!(EffectKind::Reverb.name(), "Reverb");
        assert_eq!(EffectKind::Distortion.name(), "Distortion");
        assert_eq!(EffectKind::Chorus.name(), "Chorus");
        assert_eq!(EffectKind::PitchShift.name(), "Pitch Shift");
        assert_eq!(EffectKind::Compressor.name(), "Compressor");
        assert_eq!(EffectKind::Flanger.name(), "Flanger");
        assert_eq!(EffectKind::FrequencyShift.name(), "Freq Shift");
        assert_eq!(EffectKind::Downsample.name(), "Downsample");
        assert_eq!(EffectKind::Varispeed.name(), "Varispeed");
        assert_eq!(EffectKind::Vocoder.name(), "Vocoder");
        // Spectral family kinds drop the "Spectral " prefix; the family is
        // surfaced separately via `EffectKind::family()` so the unique suffix
        // is what shows up in the UI.
        assert_eq!(EffectKind::SpectralRotate.name(), "Rotate");
        assert_eq!(EffectKind::SpectralBandpass.name(), "Bandpass");
        assert_eq!(EffectKind::SpectralMirror.name(), "Mirror");
        assert_eq!(EffectKind::SpectralShift.name(), "Shift");
        assert_eq!(EffectKind::SpectralSmear.name(), "Smear");
        assert_eq!(EffectKind::SpectralSpread.name(), "Spread");
        assert_eq!(EffectKind::SpectralLofi.name(), "Lofi");
        assert_eq!(EffectKind::SpectralCorrupt.name(), "Corrupt");
        assert_eq!(EffectKind::SpectralCompress.name(), "Compress");
        assert_eq!(EffectKind::SpectralCascade.name(), "Cascade");
        assert_eq!(EffectKind::SpectralReverb.name(), "Reverb");
        assert_eq!(EffectKind::SpectralScatter.name(), "Scatter");
        assert_eq!(EffectKind::SpectralTwist.name(), "Twist");
        assert_eq!(EffectKind::SpectralStretch.name(), "Stretch");
        assert_eq!(EffectKind::PhaserFilter.name(), "Phaser Filter");
        assert_eq!(EffectKind::Ladder.name(), "Ladder");
        assert_eq!(EffectKind::Diode.name(), "Diode");
        assert_eq!(EffectKind::Tremolo.name(), "Tremolo");
        assert_eq!(EffectKind::AutoPan.name(), "Auto Pan");
        assert_eq!(EffectKind::Vibrato.name(), "Vibrato");
        assert_eq!(EffectKind::Wavefolder.name(), "Wavefolder");
        assert_eq!(EffectKind::Plate.name(), "Plate");
    }

    #[test]
    fn effect_kind_reports_latency_matches_instance_latency() {
        // The editor's PDC badge consults `EffectKind::reports_latency()`
        // without instantiating the effect. Verify that flag stays in
        // lock-step with what a fresh instance actually reports via
        // `latency_samples()` -- if a future effect grows latency,
        // this test fires until both sides are updated together.
        for &kind in &EffectKind::ALL {
            let instance = EffectInstance::new(kind);
            let actual = instance.latency_samples() > 0;
            assert_eq!(
                kind.reports_latency(),
                actual,
                "{:?}: reports_latency()={} but a fresh instance reports latency_samples()={}",
                kind,
                kind.reports_latency(),
                instance.latency_samples()
            );
        }
        // Sanity: at least Satch and WarpZone DO report latency, so
        // the badge has something to draw.
        assert!(EffectKind::Satch.reports_latency());
        assert!(EffectKind::WarpZone.reports_latency());
        // ...and at least one common one doesn't, so the badge is
        // actually selective.
        assert!(!EffectKind::Svf.reports_latency());
        assert!(!EffectKind::None.reports_latency());
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
            EffectKind::Reverb,
            EffectKind::Distortion,
            EffectKind::Chorus,
            EffectKind::PitchShift,
            EffectKind::Compressor,
            EffectKind::Flanger,
            EffectKind::FrequencyShift,
            EffectKind::Downsample,
            EffectKind::Varispeed,
            EffectKind::Vocoder,
            EffectKind::SpectralRotate,
            EffectKind::SpectralBandpass,
            EffectKind::SpectralMirror,
            EffectKind::SpectralShift,
            EffectKind::SpectralSmear,
            EffectKind::SpectralSpread,
            EffectKind::SpectralLofi,
            EffectKind::SpectralCorrupt,
            EffectKind::SpectralCompress,
            EffectKind::SpectralCascade,
            EffectKind::SpectralReverb,
            EffectKind::SpectralScatter,
            EffectKind::SpectralTwist,
            EffectKind::SpectralStretch,
            EffectKind::PhaserFilter,
            EffectKind::Ladder,
            EffectKind::Diode,
            EffectKind::Tremolo,
            EffectKind::AutoPan,
            EffectKind::Vibrato,
            EffectKind::Wavefolder,
            EffectKind::Plate,
        ] {
            let e = EffectInstance::new(kind);
            for i in 0..MAX_EFFECT_PARAMS {
                assert!(!e.param_dimmed(i), "{:?} slot {i} should not dim", kind);
            }
        }
    }
}
