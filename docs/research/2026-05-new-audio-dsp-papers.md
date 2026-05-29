# New Audio DSP Papers — Build-Idea Survey (May 2026)

A deep-research survey of recent (~2022–2026) music/audio DSP papers, looking for
novel, real-time-capable, **buildable** algorithm families that fill gaps in the
tract-plugin-pack roster and respect its constraints (Rust/nih-plug, CPU-rendered
GUI, no allocations on the audio thread, no ML on the audio thread unless the
inference graph is lightweight).

**Method:** 5 search angles → 22 primary sources fetched → 107 falsifiable claims
extracted → 25 verified by 3-vote adversarial fact-check (0 killed) → synthesized
to the findings below. Confidence labels and caveats are carried through from that
verification.

**Existing pack coverage (what to avoid re-treading):** wavetable-filter (wavetable
FIR), miff (MSEG-drawn FIR), gs-meter (loudness), gain-brain (gain linking),
tinylimit (peak limiter), satch (FFT-domain spectral saturator), six-pack
(multiband distort-the-difference saturator), pope-scope (oscilloscope), warp-zone
(phase-vocoder shift/stretch), imagine (multiband stereo imager), multosis (grid
sequencer + 14-effect Spectral family). **No reverb. No modulation effects
(phaser/chorus/flanger). No time-domain waveshaper. No perceptual-masking
processing.**

---

## Tier 1 — no-ML, in-window, directly buildable

### 1. Velvet-noise reverb + stereo widener  *(strongest fit — selected for build)*

Velvet noise is a sparse pseudo-random impulse train whose convolution needs
**only sign flips and gains — no multiplications**, making it cheap and a natural
fit for the no-alloc audio-thread rule.

- **Dark Velvet Noise (DVN)** — a sparse reverb whose spectral color comes from
  per-pulse *width modulation* (implemented with recursive running-sum filters),
  and which decouples spectral evolution (a probability matrix `P`) from the
  energy-decay envelope `g(m)`, so arbitrary non-exponential / multi-slope decays
  are independent of coloration. Pulses are routed to a small dictionary of
  2nd-order all-pole coloration filters (so the backbone is sparse-FIR with tiny
  recursive sub-filters, not strictly non-recursive).
- **Binaural DVN (BDVN)** — synthesizes a target frequency-dependent interaural
  coherence by cross-mixing two incoherent sequences, or by jittering one
  channel's pulse locations. The jitter variant yields a parametric **width**
  control and **zero-cost time-varying coherence** (animate the jitter
  distribution) as an artistic effect.
- **Velvet-noise widener** — decorrelate via sparse velvet-noise convolution (or a
  randomized-phase allpass cascade), then sine-law crossfade by a mixing angle β.
  Output cross-correlation is bounded `ICC_out ≈ cos²(β)·ICC_in` (under an
  ideal-decorrelator assumption; the paper's own measurements show the bound is
  approached, not exactly met).

**Fit:** pack has **no reverb**; this is a *different* widener than imagine's M/S
law + Schroeder decorrelator; **reuses `tract_dsp::boxcar::RunningSumWindow`** (the
exact primitive DVN's pulse-width modulation needs); lowest new-infrastructure cost
of any candidate. Confidence: **high**. Origin: Aalto Acoustics Lab
(Fagerström / Schlecht / Välimäki), the velvet-noise originators.

- DAFx 2024 paper 63 (BDVN): <https://www.dafx.de/paper-archive/2024/papers/DAFx24_paper_63.pdf>
- arXiv 2403.20090 (DVN non-exponential reverb): <https://arxiv.org/html/2403.20090v1>
- DAFx 2024 paper 92 (stereo widener, Das/Sonos): <https://www.dafx.de/paper-archive/2024/papers/DAFx24_paper_92.pdf>

### 2. Circuit-level Wave Digital Filter virtual-analog  *(biggest category gap)*

A WDF emulation of the MXR Phase 90 phaser runs at **~0.5% CPU** (44.1 kHz, M1 Pro)
by modeling each JFET as a **time-varying resistor** (drain-source resistance
driven by the LFO-modulated gate voltage) rather than as an amplifier — the
resistor value is still computed from the nonlinear I–V each sample, but with no
iterative solver. The **VIOLA** framework shows circuit netlists (LTspice) can be
auto-compiled into runnable VA plugins (but outputs MATLAB/C++/JUCE, not Rust, and
currently supports only diode nonlinearities — feasibility evidence, not a drop-in).

**Fit:** pack has **zero modulation effects** and no circuit-level VA — a whole
missing category. Bigger lift (a small WDF engine in Rust). Caveat: the 0.5% figure
is a single self-reported M1 number with no buffer-size/instance methodology;
unverified at the pack's 100+-instance x86 target. Confidence: **high** (for the
mechanism; the CPU number is hedged). Public ref code: `polimi-ispl/mxrphase90`.

- DAFx 2024 paper 13: <https://www.dafx.de/paper-archive/2024/papers/DAFx24_paper_13.pdf>
- VIOLA: <https://polimi-ispl.github.io/viola/>

### 3. Cubic antiderivative-antialiasing waveshaper  *(fills the time-domain saturation gap)*

Cubic-interpolation ADAA (AA-IIR): a **memoryless** waveshaper that is **cheaper
than oversampling in FLOPs for oversampling factors M ≥ 3** (antialiasing filter
order K = 8) for identical alias reduction, with up to ~16 dB SNR improvement over
linear ADAA for input frequencies above ~2.5 kHz. Uses numerical integration
(composite midpoint quadrature) to sidestep the expensive transcendental
antiderivatives of classic ADAA. Requires finite look-ahead.

**Fit:** satch and six-pack are both *FFT-domain magnitude* saturators — there is no
clean **time-domain** waveshaper in the pack. Scope is strictly memoryless
nonlinearities (for stateful systems, linear interpolation remains preferred).
More best-in-class-implementation than novel-product, but a real gap. Confidence:
**high**. Zheleznov & Bilbao, Univ. Edinburgh.

- DAFx 2024 paper 33: <https://www.dafx.de/paper-archive/2024/papers/DAFx24_paper_33.pdf>

### 4. Bark-domain perceptual-masking EQ  *(novel concept, no ML)*

Compute the Bark/critical-band masking threshold via spreading-function convolution
+ absolute-threshold-of-hearing comparison (25 bands), then derive an FFT
equalization curve — e.g. boost music *just enough* to perceptually mask
environmental noise, or an anti-masking auto-EQ. Validated real-time on a SHARC
DSP. Separately, the ITU-R BS.1387 **PEAQ** model can serve as an objective
function optimized by a Harmony Search metaheuristic to jointly tune
level/EQ/compression/spatialization and minimize inter-track auditory masking
(that paper targets multi-track *speech*, not music, though the cross-track
masking-minimization mechanism is the same).

**Fit:** purely classical psychoacoustics, **reuses the pack's existing
STFT/FFT/window stack**; nothing in the pack does masking-threshold-driven
processing. Confidence: **high**.

- DAFx 2024 paper 70 (masking EQ): <https://www.dafx.de/paper-archive/2024/papers/DAFx24_paper_70.pdf>
- arXiv 2404.17821 (PEAQ auto-mixing): <https://arxiv.org/pdf/2404.17821>

---

## Tier 2 — needs an offline ML training pipeline

Differentiable-DSP analog modeling. Only the *frozen* DSP graph runs on the audio
thread; all require a PyTorch training pipeline as a build dependency, which is
heavier and in tension with the pack's "lightweight only" ethos. Confidence:
**high** for the well-benchmarked white-box-ish ones, **medium** for the framework
papers (which are training tools, not real-time Rust recipes).

- **Cascaded differentiable biquads** — BOSS MT-2 captured in ~210 params (vs ~23k
  for a WaveNet baseline); hyperconditioning learns an interpretable map from user
  controls to biquad/gain coefficients. (Nercessian et al., iZotope, ICASSP 2021 —
  *just outside the 2023–2026 window*.) <https://arxiv.org/abs/2103.08709>
- **Grey-box WDF + small MLP** — WDF keeps all linear/stateful parts; only the
  memoryless nonlinearity is a tiny MLP. Tube models at 25–40× real-time via
  RTNeural. (Darabundit et al., CCRMA, DAFx 2022.)
  <https://www.dafx.de/paper-archive/2022/papers/DAFx20in22_paper_13.pdf>
- **CONMOD** — one neural model emulating phaser + flanger frame-wise with
  controllable LFO-rate / feedback, generalizing to unseen settings. (DAFx 2024.)
  <https://arxiv.org/html/2406.13935v1>
- **NablAFx** — differentiable framework for nonlinear / nonlinear-time-varying /
  modulation effect modeling (overdrive, amps, fuzz, compressors, phasers,
  flangers). PyTorch training framework. (Comunità et al., QMUL, Feb 2025.)
  <https://arxiv.org/html/2502.11668v1>
- **Differentiable all-pole filters for time-varying systems** — validated on Small
  Stone phaser, TB-303, LA-2A. (arXiv 2404.07970.)
  <https://arxiv.org/abs/2404.07970>

---

## Tier 3 — leads that didn't make the verified top 9 (budget-dropped, worth a look)

From the "novel time-frequency representations" angle:

- **Real-time Constant-Q Transform** (`rt-cqt`) — log-frequency, musically-tuned
  bins; could power a pitch-/note-aware spectral effect or a new multosis spectral
  kind. <https://github.com/jmerkt/rt-cqt>
- **Sines + Transients + Noise (STN) decomposition** (SiTraNo) — split a signal
  into tonal / transient / noise layers and process each independently. Very
  musical, underexploited in commercial plugins; arguably the most distinctive
  *product* concept surfaced. <https://github.com/himynameisfuego/SiTraNo>

---

## Cross-cutting caveats

- Two of the most immediately buildable Tier-2 ideas (cascaded biquads 2021,
  grey-box WDF+MLP 2022) are *just outside* the requested 2023–2026 window —
  technically current, but not "new."
- The differentiable/neural family was held at **medium** confidence: the
  frameworks are research/training tools, not real-time Rust recipes, and a large
  trained MLP nonlinearity may not be audio-thread-cheap (only tanh/Padé/biquad
  graphs reliably are).
- The WDF Phase 90's ~0.5% CPU is one self-reported M1 figure, not independently
  benchmarked or multi-instance/x86-validated.
- The stereo-widener ICC bound is an approximation under an ideal-decorrelator
  assumption, not an unconditional proof.

## Decision

**Building first:** the velvet-noise reverb/widener (Tier 1 #1) — novel, fills the
pack's single biggest missing category (reverb), respects the audio-thread
discipline, and reuses existing `tract-dsp` primitives.
