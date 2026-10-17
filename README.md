# Tract Plugin Pack

A collection of audio effect plugins (VST3, CLAP, standalone) built with [nih-plug](https://github.com/robbert-vdh/nih-plug) in Rust.

## Plugins

### Gain Brain

A lightweight gain utility with cross-instance group linking. Multiple instances can be assigned to the same group (1-16), and changing gain on any grouped instance applies that change to all others.

- 16 groups with Absolute (identical values) and Relative (delta-based) link modes
- Invert toggle for mirrored gain movement (ducking, sidechain-style workflows)
- In-process shared state with cumulative canonical delta sync (lock-free atomics, zero overhead)
- CPU-rendered GUI with rotary dial (tiny-skia + softbuffer), freely resizable
- ~0.62 MB RSS and 0.03% CPU per instance (200 instances grouped @ 6.3% CPU, 123 MB total)
- Inspired by [BlueCat's Gain Suite](https://www.bluecataudio.com/Products/Product_GainSuite/)

### GS Meter

A lightweight loudness meter with integrated gain utility, purpose-built for [clip-to-zero](#what-is-clip-to-zero) workflows. Designed to run 100+ instances per project without significant CPU or memory impact.

- dB and LUFS modes with per-mode gain/reference and gain-match buttons
- Peak max, true peak (ITU-R BS.1770-4), RMS, EBU R128 (integrated, short-term, momentary, LRA)
- CPU-rendered GUI (tiny-skia + softbuffer), freely resizable -- no GPU drivers loaded
- SIMD-optimized metering (`f32x16`)
- ~1.8 MB RSS and 0.05% CPU per instance (300 instances @ 15% CPU, 560 MB total)
- Inspired by [TBProAudio dpMeter](https://www.tbproaudio.de/products/dpmeter)

### HD26

A unison-detune chorus and pseudo-stereo widener, inspired by Serum's Hyper/Dimension module. Two sections run in series, labelled `H____` and `D________` on the panel. The first stacks up to 7 modulated fractional-delay voices per channel for rich detuned unison; the second is a 4-tap pseudo-stereo widener with two modes — AM (antisymmetric Side injection, mono-safe) and Pitch (delay modulation with cross-feed). A transient-detector **Retrig** re-aligns voice phases on detected attacks.

- **Unison chorus** (`H____`): 0–7 voices (0 = off), Szabo-style asymmetric detune spread, per-voice modulated fractional delay (Catmull-Rom interp), detune/rate/width controls; a depth clamp keeps the modulation sweep within the delay line's range
- **Widener** (`D________`): 4-tap widener in AM mode (antisymmetric L/R side injection → mono sums to exact dry) or Pitch mode (modulated delay + cross-feed); a wet HPF keeps the low end out of the widened path
- **Retrig**: fast/slow-envelope onset detector with refractory period; fires when a transient is detected to reset voice phases (reduces smear on new notes)
- Zero latency; no FFT; no allocations on the audio thread
- CPU-rendered GUI with lock-free retrig LED and level-bar telemetry (tiny-skia + softbuffer), freely resizable

### Imagine

A multiband stereo imager modeled on iZotope Ozone Imager. Four fixed bands with switchable linear-phase FIR or Linkwitz-Riley IIR (Lipshitz/Vanderkooy compensated) crossovers. Per-band Ozone-style **Width** (S_gain = (width+100)/100, mid unchanged), two **Stereoize** modes — Mode I is a Haas mid-into-side delay (1–20 ms control), Mode II is a Schroeder/Gerzon all-pass decorrelator (0.5–2.0× delay-scale control) — and a global **Recover Sides** that folds a Hilbert-rotated residue of removed-side energy back into mid for perceptual width retention when narrowing.

- 4 fixed bands with 3 draggable crossover splits, log-frequency display
- Ozone-style Width law: scales the side channel from 0 (mono-fold) through 1 (unity) to 2× (max widening), mid unchanged
- Stereoize per band with on/off toggle — Mode I (Haas, 1–20 ms tap) or Mode II (real Schroeder/Gerzon decorrelator, 0.5–2.0× delay scale, xcorr < 0.3 on broadband noise)
- Global Recover Sides for narrowing-without-energy-loss workflows
- Four vectorscope modes — half-disc Polar dot cloud, Polar Level rays, Goniometer (45°-rotated dual-tone dots), traditional Lissajous (X=L, Y=R) — with correlation + balance bars and a per-channel L/R peak meter strip in the half-disc modes
- Spectrum + magnitude-squared coherence γ²(k) display via single complex M+jS FFT
- CPU-rendered GUI (tiny-skia + softbuffer), freely resizable, Cassiopeia A gold/teal palette
- Inspired by [iZotope Ozone Imager](https://www.izotope.com/en/products/ozone/features/imager.html)

### Miff

A convolution filter whose FIR kernel you draw by hand with an MSEG editor — a sibling of Wavetable Filter, but the kernel comes from a sketched curve instead of a wavetable file.

- The drawn curve **is** the FIR impulse response; a bipolar tap map (`2·value − 1`) places the editor's 0.5 midline at a zero tap, enabling highpass/bandpass/comb shapes by drawing above or below centre
- **Raw Mode**: direct time-domain SIMD convolution (zero latency)
- **Phaseless Mode**: fixed 4096-point STFT magnitude-only filtering (constant 4096-sample latency)
- Adjustable kernel **Length** (64–4096 taps); peak-magnitude normalization keeps loudness consistent and never boosts above 0 dB
- MSEG editor with grid snap, Alt-drag stepped-draw, and a styled randomizer; frequency-response view with live input-spectrum shadow
- A flat default curve bakes to an all-zero kernel — a fresh instance is clean dry passthrough
- CPU-rendered GUI (tiny-skia + softbuffer), freely resizable

### Minitosis

One continuously active Multosis effect with the same effect picker, parameter controls, and four MSEGs, without the sequencer.

- All 49 effects plus None passthrough, sharing the Multosis DSP and editor
- Three assignable MSEGs plus Amp; Time/Beat clocks, Free run/Free Hz/Transient triggers
- Per-effect settings recall and stable, named DAW automation parameters
- One Mix, Output gain, latency-aligned bypass, and optional transport retrigger
- CPU-rendered, freely resizable GUI; VST3, CLAP, and standalone

### Multosis

A 16-row grid sequencer where each row runs a different audio effect driven by per-row MSEG modulators. Effects process serially — row 1's output feeds row 2 and so on — so the grid composes a programmable chain whose parameters sweep in time with the host.

- 50 registry entries (a None passthrough plus 49 effects) across 9 families: Filter (SVF, Comb, Ladder, Diode, Sallen-Key, Phaser Filter), Distortion (Bitcrush, Distortion, Downsample, Wavefolder), Dynamics (Compressor, Gate, Limiter, Transient Shaper), Modulation (Auto Pan, Chorus, Flanger, FM, Phaser, Ring, Tremolo, Vibrato), Pitch (Frequency Shift, Pitch Shift, Varispeed), Spatial (Haas, Stereo Widener), Time (Delay, Plate, Repeat, Reverb, Stretch), Misc (Vocoder), and a 16-effect Spectral family — 14 effects sharing a switchable-FFT-size spectral engine, plus the fixed-FFT Satch and Warp Zone
- 16 rows × 32 columns, with a draggable loop region and free / beat-synced playback
- 4 MSEGs per row (3 assignable + always-on Amp) with Bipolar/Unipolar polarity, Beat-synced or Time-synced loop length, Snap-to-grid with Shift-to-bypass, six randomizer styles, and right-click Transform menu for compress/expand on values or times
- Trigger sources include free Hz, beat divisions, and audio-driven transient detection
- Wet-bus compressor between the chain output and the global dry/wet blend
- Per-row mute/solo and a PDC (plugin delay compensation) stripe that lights only on active rows carrying latency
- CPU-rendered GUI (tiny-skia + softbuffer), freely resizable

### Nap

A character reverb built on the Extended Dark Velvet Noise (EDVN) engine. The defining feature is that you **draw three curves** — Decay, Width, and Tone — over a shared tail-position axis, sculpting how the tail's loudness, stereo spread, and tonal colour each evolve along their length. You can draw exponential decays, gated shapes, reverse swells, plateus, or anything in between; the Width curve collapses the image to mono or fans it wide at any point in the tail; the Tone curve scripts a bright-to-dark air-absorption arc or any tonal trajectory.

- Three independent MSEG curve editors (Decay / Width / Tone) sharing a 0 – 100 % tail-position axis
- Dual convolution engine — **Zero Latency** (time-domain O(M) tapped-delay sparse convolution, no FFT, zero reported latency) and **Efficient** (UPOLS FFT convolution of the analytically baked IR, ~512-sample latency, lower CPU at large/dense settings; both engines produce identical output)
- Live tail visualization in the Decay pane shows the generated pulse field (height = gain, colour = tone/filter, L/R split = width jitter)
- Width curve drives per-pulse right-channel jitter for frequency-dependent interaural coherence (BDVN coherence-via-jitter theory)
- Design-time controls (Size, Density, Width amount, Seed) re-generate the velvet sequence off the audio thread; automatable controls (Mix, Pre-Delay, Input, Output) smooth continuously
- CPU-rendered GUI (tiny-skia + softbuffer), freely resizable

### Pope Scope

A multichannel real-time oscilloscope with beat sync. Multiple instances share audio data through a global store, allowing one window to display waveforms from up to 16 tracks simultaneously.

- Three display modes: Vertical (stacked), Overlay (superimposed), Sum (mixed)
- Three draw styles: Line, Filled, Both
- Beat sync mode with bar/beat grid alignment (1/4, 1/2, 1, 2, 4 bars)
- Free-running mode with adjustable timebase (1ms - 10s)
- dB-scaled amplitude mapping with configurable range
- 16 track groups with per-track solo/mute and color assignment
- Cursor tooltip on hover shows time (or bar position in beat-sync) plus a color-coded dB reading per track; in Vertical mode the tooltip and cursor line restrict to the hovered lane
- Peak hold with 2-second hold and 20 dB/s decay
- Hold mode for phase alignment (shows last complete bar, swaps at boundary)
- SIMD-optimized ring buffer with f32x16 mipmap reduction
- Waveform renderer bypasses tiny-skia's raster pipeline — direct pixel-write column fills with half-split envelope smoothing (~52% less GUI CPU than the original path-based rasterizer)
- CPU-rendered GUI with amber phosphor terminal theme (tiny-skia + softbuffer), freely resizable
- 16 instances @ 2% CPU headless (0.13% per instance)
- Per-track solo/mute/color controls with DAW track name via CLAP track-info
- Inspired by [PsyScope](https://fx23.net/free-vsts/) and [RusovDmitriy/oscilloscope](https://github.com/RusovDmitriy/oscilloscope)

### Satch

A detail-preserving spectral saturator. Uses FFT-based spectral analysis to preserve quiet frequency components through the clipping process, producing textured flat-top clipping instead of featureless flat tops.

- Independent **Gain** (input boost) and **Threshold** (clip ceiling) controls
- **Detail** knob preserves quiet harmonics through clipped regions via per-bin spectral magnitude saturation
- **Knee** crossfades between hard clip (0%) and soft tanh saturation (100%)
- Clip-aware detail blend — only affects clipped portions, unclipped material is unchanged
- CPU-rendered GUI (tiny-skia + softbuffer), freely resizable
- ~0.82 MB RSS and 0.14% CPU per instance (100 instances @ 13.7% CPU, 82 MB total)
- Inspired by [Newfangled Audio Saturate](https://www.newfangledaudio.com/saturate)

### Six Pack

A six-band parallel multiband saturator inspired by Wavesfactory Spectre. Each band's EQ boost is computed against dry as a difference, then run through a per-band saturation algorithm — so a band at 0 dB contributes silence and gain effectively becomes the per-band drive.

- 1 low-shelf + 4 peaks + 1 high-shelf, each with frequency / gain / Q controls
- Six saturation algorithms per band: **Tube**, **Tape**, **Diode**, **Digital**, **Class B**, **Wavefold**
- Per-band **Stereo / Mid / Side** routing applied to the diff before saturation
- Linear-phase polyphase oversampling: **Off / 4× / 8× / 16×**
- Global de-emphasis subtracts the linear EQ boost so only saturation harmonics remain audible
- CPU-rendered GUI with EQ curve display + live spectrum analyzer overlay (tiny-skia + softbuffer), freely resizable
- Inspired by [Wavesfactory Spectre](https://www.wavesfactory.com/audio-plugins/spectre/)

### Tinylimit

A low-latency wideband peak limiter for track-level use. Feed-forward topology with lookahead and dual-stage transient/dynamics handling.

- Individual attack, release, knee, and transient controls
- 7 built-in character presets (Transparent, Aggressive, Punchy, Smooth, Safe, Vocal, Loud)
- Optional ISP (true peak targeting via ITU-R BS.1770-4)
- Gain Link mode for auditioning limiting without loudness change
- CPU-rendered GUI with input/output meters and GR readout (tiny-skia + softbuffer), freely resizable
- ~1.0 MB RSS and 0.12% CPU per instance (50 instances @ 6.2% CPU, 50 MB total)
- Inspired by [DMG Audio TrackLimit](https://dmgaudio.com/products_tracklimit.php)

### Warp Zone

A psychedelic spectral shifter/stretcher that transforms audio in the frequency domain using a phase vocoder. Makes familiar sounds alien -- voices from another dimension, instruments with impossible harmonic structures.

- **Shift** (-24 to +24 semitones) for pitch shifting without time stretching
- **Stretch** (0.5x to 2.0x) warps harmonic spacing for inharmonic/metallic textures
- **Freeze** captures the current spectrum as a sustained drone
- **Feedback** compounds spectral shifts for rising/falling Shepard tone effects
- **Low/High** frequency range limits for selective processing
- Scrolling spectral waterfall display with psychedelic color palette
- CPU-rendered GUI (tiny-skia + softbuffer), freely resizable
- 4096-point FFT phase vocoder with linear interpolation and phase-coherent bin remapping

### Wavetable Filter

A wavetable-based audio filter that uses wavetable frames as FIR filter kernels. Load any `.wav` or `.wt` wavetable file and use its spectral content to shape your audio.

- **Raw Mode**: Direct time-domain convolution (zero latency)
- **Phaseless Mode**: STFT magnitude-only filtering (no pre-ringing)
- 3D/2D wavetable visualization (click to toggle), real-time filter response with live input spectrum shadow
- CPU-rendered GUI (tiny-skia + softbuffer), freely resizable; rotary dials show DAW modulation arcs and support right-click text entry
- SIMD-optimized convolution (`f32x16`) with silence fast-path — idle plugins use near-zero CPU
- Inspired by [Kilohearts FilterTable](https://kilohearts.com/products/filter_table) and [EB-FreakyTable](https://ewanbristow.gumroad.com/l/freakytable)

## Build Requirements

- Rust 2024 edition with the stable toolchain (automatically configured via `rust-toolchain.toml`)
- Linux system dependencies (see below)

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Ubuntu / Debian
sudo apt install libxcb1-dev libx11-xcb-dev libx11-dev libxcursor-dev \
  libasound2-dev libgl-dev libjack-jackd2-dev libwayland-dev
```

## Building

```bash
# Install or update the bundler
cargo install --force --locked --git https://github.com/xxx/nih-plug.git --rev 2882e1642946103f39a5e068c8c8ebc901a6599e cargo-nih-plug

# Build the entire collection (VST3 + CLAP + standalone, release mode)
cargo xtask bundle --redistributable

# Build the entire collection optimized for this machine
cargo xtask bundle --native

# Build one plugin or standalone individually
cargo nih-plug bundle gs-meter --release
cargo build --bin gs-meter --release
```

`cargo xtask bundle` defaults to `--redistributable`. It discovers all workspace crates with a `cdylib` target and passes them to one bundler invocation, so newly added plugins are included automatically. Both modes replace inherited `target-cpu` and `target-feature` settings; redistributable builds use the generic target CPU, including when your Cargo config enables host optimizations. Other flags from `RUSTFLAGS` or `CARGO_ENCODED_RUSTFLAGS` are preserved; Cargo config rustflags are overridden.

Additional Cargo build options are forwarded, for example `cargo xtask bundle --redistributable --locked` or `cargo xtask bundle --profile profiling`. Without an explicit profile, the command uses `--release`. On macOS, `cargo xtask bundle-universal` builds the whole collection for Intel and Apple Silicon; both Rust targets must be installed. Universal builds use redistributable mode.

### Host-optimized builds

For local builds that exploit your CPU's SIMD (tiny-skia's AVX2 raster paths, auto-vectorized DSP code), wrap any cargo invocation with `cargo xtask native`. It detects AVX2+FMA+BMI2 on the build host and, when present, sets `RUSTFLAGS=-C target-cpu=haswell` for the child cargo process. On non-x86_64 or older x86_64 hosts it falls back to the default target-cpu.

```bash
cargo xtask bundle --native
cargo xtask native nih-plug bundle wavetable-filter --release
cargo xtask native build --release --bin gs-meter
```

On supported x86_64 hosts, native binaries require Haswell-compatible CPUs. Use `cargo xtask bundle --redistributable` for the collection's portable builds.

The bundler outputs plugin bundles and standalone applications to `target/bundled/` (or `<target-dir>/bundled/` when overriding Cargo's target directory). The pinned bundler revision preserves hyphens in standalone executable names such as `gs-meter`. Standalone executables are also available in `target/release/`; a custom profile or cross-compilation target changes that build directory. CI uploads the complete bundled collection.

Copy either the `.vst3` or `.clap` file (you only need one -- use whichever your DAW supports) to your plugin directory:

- **Linux**: `~/.vst3/` or `~/.clap/`
- **macOS**: `~/Library/Audio/Plug-Ins/VST3/` or `~/Library/Audio/Plug-Ins/CLAP/`
- **Windows**: `C:\Program Files\Common Files\VST3\` or `C:\Program Files\Common Files\CLAP\`

## Workspace Structure

```
tract-plugin-pack/
├── gain-brain/             # Gain utility with group linking
├── gs-meter/               # Loudness meter + gain utility
├── hd26/                   # Unison-detune chorus + pseudo-stereo widener (Serum-inspired)
├── imagine/                # Multiband stereo imager modeled on Ozone Imager
├── miff/                   # MSEG hand-drawn FIR convolution filter
├── minitosis/              # Single-effect processor with four MSEGs
├── multosis/               # 16-row grid sequencer with per-row effects + MSEGs
├── nap/                    # EDVN velvet-noise character reverb with drawn curves
├── pope-scope/             # Multichannel real-time oscilloscope
├── satch/                  # Spectral saturator with detail preservation
├── six-pack/               # Six-band parallel multiband saturator
├── tinylimit/              # Wideband peak limiter
├── warp-zone/              # Spectral shifter/stretcher
├── wavetable-filter/       # Wavetable-based filter plugin
├── nih-plug-widgets/       # Legacy vizia widgets (no longer used; kept on disk for reference, excluded from the workspace build)
├── tiny-skia-widgets/      # Shared CPU-rendered widgets, editor base scaffolding
├── tract-dsp/              # Shared GUI-free DSP primitives (true-peak, FIR, STFT, ...)
├── tract-effects/          # Shared effects, modulation runtime, and effect/MSEG editor
├── docs/                   # Plugin manuals
│   ├── gain-brain/
│   ├── gs-meter/
│   ├── hd26/
│   ├── imagine/
│   ├── miff/
│   ├── minitosis/
│   ├── multosis/
│   ├── nap/
│   ├── pope-scope/
│   ├── satch/
│   ├── six-pack/
│   ├── tinylimit/
│   ├── warp-zone/
│   └── wavetable-filter/
└── xtask/                  # Build tooling
```

## Testing

```bash
cargo nextest run --workspace   # All tests -- parallel runner
cargo clippy --workspace        # Lint check
```

Install nextest via `cargo install cargo-nextest --locked`. Config lives in `.config/nextest.toml`.

### Mutation testing

[cargo-mutants](https://mutants.rs/) changes production code in a scratch copy
and checks whether the tests catch each change. Coverage expands one crate and
one reviewed scope at a time:

| Package | Registered mutation scope | Configuration |
|---|---|---|
| `tract-dsp` | All library modules and FFT features | `.cargo/mutants.toml` |
| `tract-effects` | Shared compressor, modulation, randomization, and undo | `.cargo/mutants-tract-effects.toml` |

Effect DSP/registry and editor code in `tract-effects` will follow in later passes.

```bash
cargo install cargo-mutants --version 27.1.0 --locked
cargo mutants -p tract-dsp --list    # Preview mutations without running tests
cargo mutants -p tract-dsp          # Full pass, all tract-dsp features enabled
cargo mutants -p tract-effects --config .cargo/mutants-tract-effects.toml

# Focus on one module while strengthening its tests
cargo mutants -p tract-dsp --file tract-dsp/src/boxcar.rs
cargo mutants -p tract-effects --config .cargo/mutants-tract-effects.toml \
  --file tract-effects/src/modulation.rs
```

Run these commands from the workspace root. Each configuration limits mutations
to its registered scope, enables all features, and runs nextest using the
`mutants` profile: fail fast, no retries, one test at a time within each scenario
so failures stop before expensive tests start. The Cargo
profile of the same name disables debug symbols to reduce rebuild cost. Use
`-j 2` to run two mutation jobs concurrently if resources permit.
Use `--file` to narrow a run; `--re` adds alternatives to the configured crate
regex rather than narrowing it.

Results are written to the ignored `mutants.out/` directory. Read `missed.txt`
for changes that survived, `timeout.txt` for tests that exceeded their time limit,
and `unviable.txt` for changes that could not compile. Per-mutation diffs and logs,
plus `outcomes.json`, are retained there. A timeout is not evidence that an
assertion caught the change. Review survivors against the intended behavior;
some mutations are equivalent within the documented numerical tolerance.

The **Mutation testing** GitHub Actions workflow has separate jobs for both
packages. On pull requests it checks out the actual PR head, computes a
zero-context diff from the PR's merge base, and passes that file to `--in-diff`.
Only mutations intersecting changed production code within a registered scope
run. A manual dispatch runs both full registered scopes without a diff filter.
Both modes fail on survivors or timeouts and upload per-package report artifacts.
PR artifacts include `pr.diff`; local CI reproductions can use the ignored
`mutation-reports/` directory.

To check the committed diff on a local branch:

```bash
git diff --no-ext-diff --no-textconv --no-color --unified=0 \
  origin/master...HEAD -- > /tmp/tract-mutants.diff
cargo mutants -p tract-effects --config .cargo/mutants-tract-effects.toml \
  --in-diff /tmp/tract-mutants.diff
```

The diff's new text must match the working tree. Test-only, dependency, config,
and out-of-scope changes can select zero mutations; these runs succeed without a
baseline build. This also means diff mode cannot detect reduced coverage of
unchanged production code. Use manual full runs when reviewing test changes or
coverage policy. Regular workspace tests still run in the normal CI workflow.

There are no blanket exclusions for surviving mutations. When expanding coverage,
update the configuration and workflow paths together, select the package with
`-p`, and review its full baseline before relying on diff checks.

The first rollout adds 14 tests and strengthens reset, signal-polarity, threshold,
fast-path, and sample-rate coverage. The SPSC concurrency test now bounds the
writer to one ring capacity: live snapshots may mix generations when overwritten,
so requiring a contiguous sequence after the writer laps the reader caused false
mutation detections. The corrected test passed 200 stress iterations.

Results for `tract-dsp` on 2026-10-16 with cargo-mutants 27.1.0 and all features:

| Pass | Tests | Examined | Caught | Survived | Timed out | Unbuildable |
|---|---|---|---|---|---|---|
| Initial baseline | 130 | 765 | 658 | 87 | 3 | 17 |
| After fixes | 147 | 737 | 720 | 0 | 0 | 17 |

The follow-up adds spectral phase/interpolation, collision, Nyquist, window,
convolution-gain, clipping, and FFT-hop regression tests. It fixes frequency-range
passthrough and corrects the STFT convolver's full-frame latency report, including
Miff and Wavetable Filter host compensation. Their manuals and PDFs reflect the
measured delays of 4096 and 2048 samples respectively.

Redundant shifter buffer writes were removed. Eleven exact equivalent mutations
are excluded individually by the documented `exclude_re` entries in
`.cargo/mutants.toml`; meaningful contribution-tie comparisons remain tested.
Review those explanations when changing the affected expressions. The full
verification (`cargo mutants -p tract-dsp -j 8`) completed in about 7 minutes
locally.

Results for the `tract-effects` shared runtime on 2026-10-17:

| Pass | Tests | Examined | Caught | Survived | Timed out | Unbuildable |
|---|---|---|---|---|---|---|
| Initial baseline | 457 | 317 | 201 | 109 | 0 | 7 |
| After test additions | 479 | 309 | 302 | 0 | 0 | 7 |

Twenty-two added tests and stronger Mix endpoint assertions cover the compressor's
physical attack/release timing and soft knee, transient detection, beat clocks,
logarithmic modulation, legacy state padding, RNG sequences, and enum choices.
Undo's existing tests already caught all of its mutations. Eight equivalent
mutations have individual explanations in its configuration. The full shared
runtime pass with `-j 8` completed in about 7 minutes locally.

## Documentation

- [Gain Brain Manual](docs/gain-brain/gain-brain-manual.md)
- [GS Meter Manual](docs/gs-meter/gs-meter-manual.md)
- [HD26 Manual](docs/hd26/hd26-manual.md)
- [Imagine Manual](docs/imagine/imagine-manual.md)
- [Miff Manual](docs/miff/miff-manual.md)
- [Minitosis Manual](docs/minitosis/minitosis-manual.md)
- [Multosis Manual](docs/multosis/multosis-manual.md)
- [Nap Manual](docs/nap/nap-manual.md)
- [Pope Scope Manual](docs/pope-scope/pope-scope-manual.md)
- [Satch Manual](docs/satch/satch-manual.md)
- [Six Pack Manual](docs/six-pack/six-pack-manual.md)
- [Tinylimit Manual](docs/tinylimit/tinylimit-manual.md)
- [Warp Zone Manual](docs/warp-zone/warp-zone-manual.md)
- [Wavetable Filter Manual](docs/wavetable-filter/wavetable-filter-manual.md)

## What is Clip-to-Zero?

- [Clip-to-Zero video series](https://www.youtube.com/playlist?list=PLxik-POfUXY6i_fP0f4qXNwdMxh3PXxJx) (YouTube)
- [Clip-to-Zero process document](https://docs.google.com/document/d/1Ogxa5-X_QdbtfLLQ_2mDEgPgHxNRLebQ7pps3rXewPM/edit?tab=t.0#heading=h.lwtkibvu0gr) (Google Docs)

## License

GPL-3.0-or-later

## Author

Michael Dungan
