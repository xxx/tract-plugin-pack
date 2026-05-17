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
- **Phaseless Mode**: fixed 4096-point STFT magnitude-only filtering (constant 2048-sample latency)
- Adjustable kernel **Length** (64–4096 taps); peak-magnitude normalization keeps loudness consistent and never boosts above 0 dB
- MSEG editor with grid snap, Alt-drag stepped-draw, and a styled randomizer; frequency-response view with live input-spectrum shadow
- A flat default curve bakes to an all-zero kernel — a fresh instance is clean dry passthrough
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

- Rust nightly toolchain (automatically configured via `rust-toolchain.toml`)
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
# Build all plugins (VST3 + CLAP)
cargo nih-plug bundle gain-brain --release
cargo nih-plug bundle gs-meter --release
cargo nih-plug bundle imagine --release
cargo nih-plug bundle miff --release
cargo nih-plug bundle pope-scope --release
cargo nih-plug bundle satch --release
cargo nih-plug bundle six-pack --release
cargo nih-plug bundle tinylimit --release
cargo nih-plug bundle warp-zone --release
cargo nih-plug bundle wavetable-filter --release

# Standalone binaries
cargo build --bin gain-brain --release
cargo build --bin gs-meter --release
cargo build --bin imagine --release
cargo build --bin miff --release
cargo build --bin pope-scope --release
cargo build --bin satch --release
cargo build --bin six-pack --release
cargo build --bin tinylimit --release
cargo build --bin warp-zone --release
cargo build --bin wavetable-filter --release
```

### Host-optimized builds

For local builds that exploit your CPU's SIMD (tiny-skia's AVX2 raster paths, auto-vectorized DSP code), wrap any cargo invocation with `cargo xtask native`. It detects AVX2+FMA+BMI2 on the build host and, when present, sets `RUSTFLAGS=-C target-cpu=haswell` for the child cargo process. On non-x86_64 or older x86_64 hosts it falls back to the default target-cpu.

```bash
cargo xtask native nih-plug bundle wavetable-filter --release
cargo xtask native build --release --bin gs-meter
```

The resulting binaries are NOT portable to pre-Haswell machines -- use plain `cargo nih-plug bundle ...` for distributable bundles.

The bundler outputs to `target/bundled/`. Copy either the `.vst3` or `.clap` file (you only need one -- use whichever your DAW supports) to your plugin directory:

- **Linux**: `~/.vst3/` or `~/.clap/`
- **macOS**: `~/Library/Audio/Plug-Ins/VST3/` or `~/Library/Audio/Plug-Ins/CLAP/`
- **Windows**: `C:\Program Files\Common Files\VST3\` or `C:\Program Files\Common Files\CLAP\`

## Workspace Structure

```
tract-plugin-pack/
├── gain-brain/             # Gain utility with group linking
├── gs-meter/               # Loudness meter + gain utility
├── imagine/                # Multiband stereo imager modeled on Ozone Imager
├── miff/                   # MSEG hand-drawn FIR convolution filter
├── pope-scope/             # Multichannel real-time oscilloscope
├── satch/                  # Spectral saturator with detail preservation
├── six-pack/               # Six-band parallel multiband saturator
├── tinylimit/              # Wideband peak limiter
├── warp-zone/              # Spectral shifter/stretcher
├── wavetable-filter/       # Wavetable-based filter plugin
├── nih-plug-widgets/       # Legacy vizia widgets (no longer used; kept on disk for reference, excluded from the workspace build)
├── tiny-skia-widgets/      # Shared CPU-rendered widgets, editor base scaffolding
├── tract-dsp/              # Shared GUI-free DSP primitives (true-peak, FIR, STFT, ...)
├── docs/                   # Plugin manuals
│   ├── gain-brain/
│   ├── gs-meter/
│   ├── imagine/
│   ├── miff/
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

## Documentation

- [Gain Brain Manual](docs/gain-brain/gain-brain-manual.md)
- [GS Meter Manual](docs/gs-meter/gs-meter-manual.md)
- [Imagine Manual](docs/imagine/imagine-manual.md)
- [Miff Manual](docs/miff/miff-manual.md)
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
