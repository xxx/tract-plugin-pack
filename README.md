# Tract Plugin Pack

A collection of audio effect plugins (VST3, CLAP, standalone) built with [nih-plug](https://github.com/robbert-vdh/nih-plug) in Rust.

## Plugins

### Wavetable Filter

A wavetable-based audio filter that uses wavetable frames as FIR filter kernels. Load any `.wav` or `.wt` wavetable file and use its spectral content to shape your audio.

- **Raw Mode**: Direct time-domain convolution (zero latency)
- **Phaseless Mode**: STFT magnitude-only filtering (no pre-ringing)
- 3D/2D wavetable visualization, real-time filter response graph
- Custom rotary knobs with DAW modulation indicators
- SIMD-optimized convolution (`f32x16`)
- Inspired by [Kilohearts FilterTable](https://kilohearts.com/products/filtertable) and [FreakyTable](https://ewanbristow.gumroad.com/l/freakytable)

### GS Meter

A lightweight loudness meter with integrated gain utility, purpose-built for [clip-to-zero](#what-is-clip-to-zero) workflows. Designed to run 100+ instances per project without significant CPU or memory impact.

- dB and LUFS modes with per-mode gain/reference and gain-match buttons
- Peak max, true peak (ITU-R BS.1770-4), RMS, EBU R128 (integrated, short-term, momentary, LRA)
- CPU-rendered GUI (tiny-skia + softbuffer) -- no GPU drivers loaded
- SIMD-optimized metering (`f32x16`)
- ~1.8 MB RSS and 0.05% CPU per instance (300 instances @ 15% CPU, 560 MB total)
- Inspired by [TBProAudio dpMeter](https://www.tbproaudio.de/products/dpmeter)

### Gain Brain

A lightweight gain utility with cross-instance group linking. Multiple instances can be assigned to the same group (1-16), and changing gain on any grouped instance applies that change to all others.

- 16 groups with Absolute (identical values) and Relative (delta-based) link modes
- Invert toggle for mirrored gain movement (ducking, sidechain-style workflows)
- Cross-instance IPC via memory-mapped file (works across DAW sandbox boundaries)
- CPU-rendered GUI with rotary dial (tiny-skia + softbuffer)
- ~8 KB per instance headless, ~3 MB for 200 instances
- Inspired by [BlueCat's Gain Suite](https://www.bluecataudio.com/Products/Product_GainSuite/)

### tinylimit

A low-latency wideband peak limiter for track-level use. Feed-forward topology with lookahead and dual-stage transient/dynamics handling.

- Individual attack, release, knee, and transient controls (no opaque "style" presets)
- 7 built-in character presets (Transparent, Aggressive, Punchy, Smooth, Safe, Vocal, Loud)
- Optional ISP (true peak targeting via ITU-R BS.1770-4)
- Gain Link mode for auditioning limiting without loudness change
- CPU-rendered GUI with input/output meters and GR readout (tiny-skia + softbuffer)
- ~1.0 MB RSS and 0.12% CPU per instance (50 instances @ 6.2% CPU, 50 MB total)
- Inspired by [DMG Audio TrackLimit](https://dmgaudio.com/products_tracklimit.php)

### satch

A detail-preserving spectral saturator. Uses FFT-based spectral analysis to preserve quiet frequency components through the clipping process, producing textured flat-top clipping instead of featureless flat tops.

- Independent **Gain** (input boost) and **Threshold** (clip ceiling) controls
- **Detail** knob preserves quiet harmonics through clipped regions via per-bin spectral magnitude saturation
- **Knee** crossfades between hard clip (0%) and soft tanh saturation (100%)
- Clip-aware detail blend — only affects clipped portions, unclipped material is unchanged
- CPU-rendered GUI (tiny-skia + softbuffer)
- ~0.82 MB RSS and 0.14% CPU per instance (100 instances @ 13.7% CPU, 82 MB total)
- Inspired by [Newfangled Audio Saturate](https://www.newfangledaudio.com/saturate)

## Build Requirements

- Rust nightly toolchain (automatically configured via `rust-toolchain.toml`)
- Linux system dependencies (see below)

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Ubuntu / Debian
sudo apt install libxcb1-dev libxcb-icccm4-dev libxcb-dri2-0-dev libx11-xcb-dev \
  libx11-dev libxcursor-dev libasound2-dev libgl-dev libdrm-dev \
  libjack-jackd2-dev libwayland-dev libegl-dev
```

## Building

```bash
# Build all plugins (VST3 + CLAP)
cargo nih-plug bundle wavetable-filter --release
cargo nih-plug bundle gs-meter --release
cargo nih-plug bundle gain-brain --release
cargo nih-plug bundle tinylimit --release
cargo nih-plug bundle satch --release

# Standalone binaries
cargo build --bin wavetable-filter --release
cargo build --bin gs-meter --release
cargo build --bin gain-brain --release
cargo build --bin tinylimit --release
cargo build --bin satch --release
```

The bundler outputs to `target/bundled/`. Copy either the `.vst3` or `.clap` file (you only need one -- use whichever your DAW supports) to your plugin directory:

- **Linux**: `~/.vst3/` or `~/.clap/`
- **macOS**: `~/Library/Audio/Plug-Ins/VST3/` or `~/Library/Audio/Plug-Ins/CLAP/`
- **Windows**: `C:\Program Files\Common Files\VST3\` or `C:\Program Files\Common Files\CLAP\`

## Workspace Structure

```
tract-plugin-pack/
├── wavetable-filter/       # Wavetable-based filter plugin
├── gs-meter/               # Loudness meter + gain utility
├── gain-brain/             # Gain utility with group linking
├── tinylimit/              # Wideband peak limiter
├── satch/                  # Spectral saturator with detail preservation
├── nih-plug-widgets/       # Shared vizia widgets (ParamDial, CSS theme)
├── tiny-skia-widgets/      # Shared CPU-rendered widgets, editor base scaffolding
├── docs/                   # Plugin manuals
│   ├── wavetable-filter/
│   ├── gs-meter/
│   ├── gain-brain/
│   ├── tinylimit/
│   └── satch/
└── xtask/                  # Build tooling
```

## Testing

```bash
cargo test --workspace       # All tests (218+)
cargo clippy --workspace     # Lint check
```

## Documentation

- [Wavetable Filter Manual](docs/wavetable-filter/wavetable-filter-manual.md)
- [GS Meter Manual](docs/gs-meter/gs-meter-manual.md)
- [Gain Brain Manual](docs/gain-brain/gain-brain-manual.md)
- [tinylimit Manual](docs/tinylimit/tinylimit-manual.md)
- [satch Manual](docs/satch/satch-manual.md)

## What is Clip-to-Zero?

- [Clip-to-Zero video series](https://www.youtube.com/playlist?list=PLxik-POfUXY6i_fP0f4qXNwdMxh3PXxJx) (YouTube)
- [Clip-to-Zero process document](https://docs.google.com/document/d/1Ogxa5-X_QdbtfLLQ_2mDEgPgHxNRLebQ7pps3rXewPM/edit?tab=t.0#heading=h.lwtkibvu0gr) (Google Docs)

## License

GPL-3.0-or-later

## Author

Michael Dungan
