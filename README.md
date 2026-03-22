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

### GS Meter

A lightweight loudness meter with integrated gain utility, purpose-built for clip-to-zero workflows. Designed to run 50+ instances per project without significant CPU or memory impact.

- Peak max, true peak (ITU-R BS.1770-4), RMS integrated/momentary, crest factor
- One-click gain-from-reading buttons for fast level matching
- CPU-rendered GUI (tiny-skia + softbuffer) -- no GPU drivers loaded
- SIMD-optimized metering (`f32x16`)
- 50 instances: ~16% CPU, 48 MB memory (GUI closed, 48 kHz)

## Build Requirements

- Rust nightly toolchain (automatically configured via `rust-toolchain.toml`)
- Linux: `libxcb`, `libxcb-util`, `libasound2-dev`

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
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

Plugins are installed to `~/.vst3/` and `~/.clap/`.

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

## License

GPL-3.0-or-later

## Author

Michael Dungan
