# Wavetable Filter

A wavetable-based audio filter plugin that uses wavetable frames as filter kernels. Supports both direct time-domain convolution (Raw mode) and magnitude-only STFT filtering (Phaseless mode).

## Features

- **Dual Filtering Modes:**
  - **Raw Mode**: Direct time-domain convolution using wavetable as FIR filter kernel
  - **Phaseless Mode**: STFT magnitude-only filtering — preserves magnitude spectrum without phase artifacts or pre-ringing
- **Wavetable Visualization**: Toggle between 3D overhead perspective and 2D single-frame view (click to switch)
- **Filter Response Graph**: Real-time frequency response with input spectrum shadow overlay
- **Rotary Knob Controls**: Custom dials with DAW modulation indicators (orange arc)
- **UI Scaling**: Adjustable from 100% to 300% in 25% steps, persisted across sessions
- **File Format Support**: Load `.wav` and `.wt` (Surge) wavetable files
- **SIMD Optimized**: Uses portable SIMD (`f32x16`) for vectorized convolution
- **Real-time Parameter Control**: Frequency, Frame Position, Resonance, Mix, and Gain
- **Multiple Plugin Formats**: VST3, CLAP, and Standalone

## Build Requirements

- Rust (nightly toolchain - automatically configured via `rust-toolchain.toml`)
- System dependencies:
  - Linux: `libxcb`, `libxcb-util`, `libasound2-dev`
  - macOS: Xcode Command Line Tools
  - Windows: Visual Studio 2019+ or MinGW

### Installing Rust

```bash
# Install rustup if you haven't already
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# The project will automatically use the nightly toolchain specified in rust-toolchain.toml
```

### Installing System Dependencies

**Arch Linux / Manjaro:**
```bash
sudo pacman -S base-devel libxcb xcb-util alsa-lib
```

**Ubuntu / Debian:**
```bash
sudo apt install build-essential libxcb1-dev libxcb-util-dev libasound2-dev
```

**Fedora:**
```bash
sudo dnf install gcc-c++ libxcb-devel xcb-util-devel alsa-lib-devel
```

## Building

**Note:** Plugins must be built separately on each target platform (Linux, macOS, Windows). Cross-compilation is possible but not recommended for audio plugins due to platform-specific dependencies.

### Plugin Formats (VST3 + CLAP)

Build and install the plugins using the bundler:

```bash
# Build optimized release version (recommended)
cargo nih-plug bundle wavetable-filter --release

# Build debug version (for development)
cargo nih-plug bundle wavetable-filter
```

The plugins will be automatically installed to:
- **Linux**: `~/.vst3/` and `~/.clap/`
- **macOS**: `~/Library/Audio/Plug-Ins/VST3/` and `~/Library/Audio/Plug-Ins/CLAP/`
- **Windows**: `C:\Program Files\Common Files\VST3\` and `C:\Program Files\Common Files\CLAP\`

### Standalone Application

Build the standalone GUI application:

```bash
# Debug build
cargo build --bin wavetable-filter

# Release build (recommended)
cargo build --bin wavetable-filter --release
```

The binary will be at: `./target/release/wavetable-filter`

## Running

### As a Plugin

Load `Wavetable Filter.vst3` or `Wavetable Filter.clap` in your DAW:
- Bitwig Studio
- Reaper
- Ardour
- Carla
- etc.

### Standalone Application

```bash
# Run directly
./target/release/wavetable-filter
```

#### Standalone Command-Line Options

```bash
# Use specific backend
./target/release/wavetable-filter --backend jack
./target/release/wavetable-filter --backend alsa

# Auto-connect JACK inputs (requires comma-separated port names)
./target/release/wavetable-filter --backend jack \
  --connect-jack-inputs "system:capture_1,system:capture_2"

# Set sample rate (ignored for JACK)
./target/release/wavetable-filter --sample-rate 48000

# See all options
./target/release/wavetable-filter --help
```

#### Connecting Audio with PipeWire/JACK

The standalone app creates JACK ports but doesn't auto-connect them by default. Use a patchbay tool:

**Graphical (Recommended):**
- `qpwgraph` - Qt-based PipeWire graph viewer
- `helvum` - GTK-based PipeWire patchbay
- `qjackctl` - Classic JACK control panel
- `carla-patchbay` - Part of Carla

**Command Line:**
```bash
# List all ports
pw-link -l
# Or: jack_lsp

# Connect manually
pw-link "Wavetable Filter:output_FL" "Built-in Audio Analog Stereo:playback_FL"
pw-link "Wavetable Filter:output_FR" "Built-in Audio Analog Stereo:playback_FR"
```

## Usage

1. **Load a Wavetable**: Click "Browse..." to load a `.wav` or `.wt` wavetable file
   - Compatible with Surge wavetables and standard WAV wavetables
   - Supports 256, 512, 1024, or 2048 samples per frame
   - Supports up to 2048 frames per wavetable

2. **Adjust Parameters**:
   - **Frequency**: Cutoff frequency control (20 Hz – 20 kHz). Determines where harmonic 24 of the wavetable maps in the spectrum, scaling the filter response up or down.
   - **Frame Position**: Select which wavetable frame to use (0.0–1.0). Blends between adjacent frames.
   - **Resonance**: Boost around the cutoff frequency (0–100%)
   - **Mix**: Dry/wet blend (0% = bypass, 100% = full effect)
   - **Gain**: Output gain (-20 dB to +20 dB)
   - **Mode**: Toggle between Raw and Phaseless filtering

3. **Visualize**:
   - Left panel shows wavetable frames (click to toggle 2D/3D view)
   - Right panel shows filter frequency response with input spectrum shadow
   - Current frame highlighted in orange; modulation shown as orange arc on dials

## Filtering Modes

### Raw Mode
Direct time-domain convolution using the wavetable frame as-is as an FIR filter kernel. Uses the wavetable exactly as provided, which can include arbitrary phase relationships. Fast and predictable.

### Phaseless Mode
STFT-based magnitude-only filtering. Applies the wavetable's magnitude spectrum to the input signal without altering its phase, eliminating pre-ringing and phase distortion. Uses 50% overlap-add with a Hann window for smooth, artifact-free output.

### Cutoff Frequency Behavior
The Frequency parameter controls where harmonic 24 of the wavetable maps to in the frequency spectrum:
- Lower cutoff values stretch the filter kernel, creating a lowpass effect
- Higher cutoff values compress the filter kernel, shifting the filter response upward
- At 1000 Hz cutoff, harmonic 24 appears at 1000 Hz
- The filter kernel is automatically resampled to achieve the target cutoff frequency

## File Formats

### Supported Wavetable Formats

**WAV Files** (`.wav`):
- Single-cycle waveforms concatenated
- Must be mono
- Frame size auto-detected (256/512/1024/2048)
- Total samples must be a multiple of frame size

**Surge WT Files** (`.wt`):
- Surge synthesizer wavetable format (`vawt` header)
- Supports both float32 and int16 sample data
- Header specifies frame count and frame size
- [Format specification](https://github.com/surge-synthesizer/surge/blob/main/resources/data/wavetables/WT%20fileformat.txt)

## Project Structure

```
wavetable-filter/
├── src/
│   ├── lib.rs                      # Plugin DSP: convolution, STFT, kernel synthesis
│   ├── main.rs                     # Standalone binary entry point
│   ├── wavetable.rs                # Wavetable I/O (.wav/.wt), frame interpolation
│   ├── editor.rs                   # Vizia UI layout, file browser, scaling
│   └── editor/
│       ├── param_dial.rs           # Custom rotary knob widget with modulation display
│       ├── wavetable_view.rs       # 2D/3D wavetable visualization
│       └── filter_response_view.rs # Filter response + input spectrum graph
├── tests/fixtures/                 # Test wavetable files
├── xtask/                          # Build tooling (nih-plug bundler)
└── rust-toolchain.toml             # Specifies nightly Rust
```

## Performance

- Uses Rust's portable SIMD (`f32x16`) for vectorized convolution
- Processes 16 samples per SIMD operation
- Typical CPU usage: ~1.5% audio thread at 48 kHz
- Zero-latency processing in Raw mode
- Silence detection clears filter state when input is idle

## Development

```bash
# Build for development (with debug symbols)
cargo build

# Run tests
cargo test

# Check code
cargo clippy

# Format code
cargo fmt

# Build documentation
cargo doc --open
```

## Troubleshooting

**Build errors about portable_simd:**
- Make sure you're using nightly Rust (handled automatically by `rust-toolchain.toml`)

**Standalone app doesn't connect to audio:**
- Manually connect with a patchbay tool
- For JACK: Use `qjackctl`, `Carla`, or `jack_connect`
- For PipeWire: Use `qpwgraph`, `helvum`, or `pw-link`

**Plugin not showing in DAW:**
- Make sure plugins are copied to the correct location
- Rescan plugins in your DAW
- Check DAW's plugin blacklist

**Cracking/popping audio:**
- Try adjusting your audio interface buffer size
- Default buffer: 256 samples (should be fine for most systems)

## Credits

- Built with [nih-plug](https://github.com/robbert-vdh/nih-plug) by Robbert van der Helm
- UI built with [Vizia](https://github.com/vizia/vizia)
- FFT processing via [RustFFT](https://github.com/ejmahler/RustFFT)
- Inspired by [Kilohearts FilterTable](https://kilohearts.com/products/filtertable)

## License

GPL-3.0-or-later

## Author

Michael Dungan
