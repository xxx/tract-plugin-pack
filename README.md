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

# Standalone binaries
cargo build --bin wavetable-filter --release
cargo build --bin gs-meter --release
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
├── nih-plug-widgets/       # Shared GUI widgets (ParamDial, CSS theme)
├── docs/                   # Plugin manuals
│   ├── wavetable-filter/
│   └── gs-meter/
└── xtask/                  # Build tooling
```

## Testing

```bash
cargo test --workspace       # All tests (80+)
cargo clippy --workspace     # Lint check
```

## Documentation

- [Wavetable Filter Manual](docs/wavetable-filter/wavetable-filter-manual.md)
- [GS Meter Manual](docs/gs-meter/gs-meter-manual.md)

## What is Clip-to-Zero?

- [Clip-to-Zero video series](https://www.youtube.com/playlist?list=PLxik-POfUXY6i_fP0f4qXNwdMxh3PXxJx) (YouTube)
- [Clip-to-Zero process document](https://docs.google.com/document/d/1Ogxa5-X_QdbtfLLQ_2mDEgPgHxNRLebQ7pps3rXewPM/edit?tab=t.0#heading=h.lwtkibvu0gr) (Google Docs)

## License

GPL-3.0-or-later

## Author

Michael Dungan
