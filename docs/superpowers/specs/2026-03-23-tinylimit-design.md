# tinylimit Design Spec

## Overview

tinylimit (stylized from μlimit / microlimit) is a low-latency wideband peak limiter for track-level use. Feed-forward topology with lookahead, dual-stage transient/dynamics handling, optional true peak (ISP) targeting. CPU-rendered GUI with input/output/GR metering.

Inspired by DMG Audio's TrackLimit. Instead of TrackLimit's proprietary "Style" preset system, tinylimit exposes individual controls (attack, knee, transient sensitivity) for equivalent configurability.

## Plugin Parameters

| Parameter | Type | Range | Default | Notes |
|-----------|------|-------|---------|-------|
| `input` | FloatParam | -60 to +18 dB | 0.0 dB | Pre-limiter input gain, smoothed |
| `threshold` | FloatParam | -60 to 0 dB | 0.0 dB | Gain boost before limiting. Lower = more limiting |
| `ceiling` | FloatParam | -30 to 0 dB | 0.0 dB | Maximum output level |
| `attack` | FloatParam | 0.1 to 10 ms | 5.0 ms | Lookahead time (also sets plugin latency) |
| `release` | FloatParam | 1 to 1000 ms | 200 ms | Release time for sustained (dynamics) stage |
| `knee` | FloatParam | 0 to 12 dB | 0 dB | Soft knee width (0 = hard knee) |
| `stereo_link` | FloatParam | 0 to 100% | 100% | Channel independence. 100% = fully linked |
| `transient` | FloatParam | 0 to 100% | 50% | Workload split: transient stage vs dynamics stage |
| `isp` | BoolParam | Off / On | Off | True peak targeting (ITU-1770 dBTP vs dBFS) |
| `gain_link` | BoolParam | Off / On | Off | Ceiling tracks threshold for auditioning |

**Latency:** Equal to the attack parameter. Reported to the host via `Plugin::LATENCY`. When attack changes, latency changes (host handles compensation).

## Signal Flow

```
input ──> [Input Gain] ──> [Threshold Boost] ──┬──> [Delay: attack ms] ──> [Apply GR] ──> [Ceiling Gain] ──> [Safety Clip] ──> output
                                                │
                                          SIDECHAIN
                                                │
                                                ├──> [Peak Detect (sample or ISP)]
                                                │         │
                                                │         v
                                                │    [Gain Computer (soft knee, ratio=inf)]
                                                │         │
                                                │         v
                                                │    ┌─────────────────────┐
                                                │    │ Dual-Stage Envelope │
                                                │    │                     │
                                                │    │ Transient: fast     │
                                                │    │ attack, auto-release│
                                                │    │ (= attack time)     │
                                                │    │                     │
                                                │    │ Dynamics: fast      │
                                                │    │ attack, user release│
                                                │    │                     │
                                                │    │ Mix: transient knob │
                                                │    └─────────────────────┘
                                                │         │
                                                │         v
                                                │    [Lookahead backward pass]
                                                │         │
                                                │         v
                                                └────── gain reduction signal
```

### Processing Order Per Buffer

1. **Input gain + threshold boost:** Apply `input + threshold` dB gain to the signal.
2. **Peak detection:** For each sample, detect peak level. If ISP=On, use 4x oversampled true peak detector (ITU polyphase FIR). If ISP=Off, use `|sample|`.
3. **Gain computer:** Map detected level through the static characteristic (soft knee, ratio=infinity, threshold=0 dBFS). Outputs gain reduction in dB (<= 0).
4. **Dual-stage envelope:**
   - Transient stage: very fast attack (instant or ~0.1ms), release = attack time. Catches brief peaks.
   - Dynamics stage: fast attack, release = user's release parameter. Handles sustained loud passages.
   - Output: `min(transient_gr, dynamics_gr)` weighted by the `transient` knob. At 100% transient, only the transient stage works. At 0%, only dynamics. At 50%, both contribute equally.
5. **Lookahead backward pass:** Iterate backwards through the buffer, linearly ramping (in dB) gain reduction toward each peak over the attack window. Deeper peaks override shallower ramps.
6. **Apply gain:** Multiply delayed audio by `10^(gr_dB / 20)`.
7. **Ceiling + safety clip:** Apply ceiling gain, then hard-clip at ceiling to catch any residual overshoots (soft knee overshoots, ISP residuals).
8. **Metering:** Update input peak, output peak, and GR meters.

### Gain Computer (Giannoulis et al.)

Hard knee (ratio = infinity):
```
y_dB = min(x_dB, 0)     // threshold is 0 dBFS after boost
gr_dB = y_dB - x_dB     // always <= 0
```

Soft knee (width W dB):
```
if x_dB < -W/2:
    gr_dB = 0                                        // below knee
elif x_dB <= W/2:
    gr_dB = -(x_dB + W/2)^2 / (2*W)                 // in knee region
else:
    gr_dB = -x_dB                                     // above knee, full limiting
```

### Dual-Stage Envelope

Each stage uses a branching one-pole IIR filter (Giannoulis):

```
alpha_A = exp(-1 / (fs * t_attack))
alpha_R = exp(-1 / (fs * t_release))

if gr_dB[n] <= gr_smooth[n-1]:    // attack (gain dropping)
    gr_smooth[n] = alpha_A * gr_smooth[n-1] + (1 - alpha_A) * gr_dB[n]
else:                              // release (gain recovering)
    gr_smooth[n] = alpha_R * gr_smooth[n-1] + (1 - alpha_R) * gr_dB[n]
```

Transient stage: `t_attack = 0.1ms`, `t_release = attack_time`
Dynamics stage: `t_attack = 0.1ms`, `t_release = user_release`

Combined: `gr_out = transient_mix * gr_transient + (1 - transient_mix) * gr_dynamics`, clamped to `min(gr_transient, gr_dynamics)` to ensure brickwall behavior.

### Lookahead Backward Pass

Process the gain reduction buffer backwards (per DanielRudrich's approach):

```
for i in (0..buffer_len).rev():
    target_gr = gr_envelope[i]
    // Linear ramp in dB over the lookahead window
    ramp_gr = gr_envelope[i + lookahead_samples] +
              (target_gr - gr_envelope[i + lookahead_samples]) *
              (lookahead_samples - remaining) / lookahead_samples
    gr_lookahead[i] = min(gr_lookahead[i], ramp_gr)  // deeper peaks override
```

### True Peak Detection (ISP Mode)

Reuse the ITU-R BS.1770-4 polyphase FIR from gs-meter:
- 48-tap, 4-phase filter
- 4x oversampling at <96kHz, 2x at 96-192kHz, bypass at >=192kHz
- Applied to the sidechain only (not the audio path)
- Double-buffered history for contiguous SIMD dot products

### Stereo Linking

```
gr_L = gain_computer(peak_L)
gr_R = gain_computer(peak_R)
gr_linked = min(gr_L, gr_R)
gr_L_out = link * gr_linked + (1 - link) * gr_L
gr_R_out = link * gr_linked + (1 - link) * gr_R
```

At 100% link (default), both channels get the same GR = min(L, R). Preserves stereo image. At 0%, channels are independent.

### Gain Link Mode

When gain_link is on, ceiling = threshold. This lets the user audition the effect of limiting without loudness change: the signal is boosted by threshold dB then limited back, with ceiling matching threshold so output level stays approximately the same.

When the user is satisfied with the limiting character, they turn off gain_link and set ceiling to their desired target (e.g., -0.1 dBFS for CD, -1 dBTP for streaming).

## GUI

### Rendering Stack

CPU-rendered using softbuffer + tiny-skia + fontdue (same as gs-meter, gain-brain).

### Layout

```
┌────────────────────────────────────────────────────┐
│  tinylimit                                   - 150% + │
├────────────────────────────────────────────────────┤
│                                                    │
│  ┌──────┐  ┌─────────────────────┐  ┌──────┐      │
│  │ IN   │  │     CONTROLS        │  │ OUT  │      │
│  │ L  R │  │                     │  │ L  R │      │
│  │ ▓  ▓ │  │  Input    [dial]    │  │ ▓  ▓ │      │
│  │ ▓  ▓ │  │  Thresh   [dial]    │  │ ▓  ▓ │      │
│  │ ▓  ▓ │  │  Ceiling  [dial]    │  │ ▓  ▓ │      │
│  │ ▓  ▓ │  │  Attack   [dial]    │  │ ▓  ▓ │      │
│  │ ▓  ▓ │  │  Release  [dial]    │  │ ▓  ▓ │      │
│  │ ▓  ▓ │  │  Knee     [dial]    │  │ ▓  ▓ │      │
│  │ ▓  ▓ │  │  Link%    [dial]    │  │ ▓  ▓ │      │
│  │ ▓  ▓ │  │  Transient[dial]    │  │ ▓  ▓ │      │
│  │ ▓  ▓ │  │                     │  │ ▓  ▓ │      │
│  │ ▓  ▓ │  │  [ISP] [GainLink]   │  │ ▓  ▓ │      │
│  │ dB dB│  │                     │  │ dB dB│      │
│  └──────┘  │  GR: -3.2 dB       │  └──────┘      │
│            └─────────────────────┘                 │
│                                                    │
└────────────────────────────────────────────────────┘
```

### Window Size

Target: ~500 x 500 pixels at 1x scale. Scalable 75%-300%.

### Meters

- **Input meter:** stereo PPM (L/R bars), peak hold with decay, numeric peak readout below
- **Output meter:** same as input meter, post-ceiling
- **GR display:** numeric readout of current gain reduction in dB, centered between the meters

Meters update every buffer from shared atomics (same pattern as gs-meter's MeterReadings).

## Workspace Integration

New crate at `tinylimit/` in the workspace root:

```
tinylimit/
├── Cargo.toml
├── src/
│   ├── lib.rs          — plugin struct, params, process()
│   ├── main.rs         — standalone entry point
│   ├── limiter.rs      — core DSP: gain computer, envelope, lookahead, dual-stage
│   ├── true_peak.rs    — ITU polyphase FIR (shared with gs-meter or extracted)
│   ├── editor.rs       — softbuffer GUI with meters
│   └── fonts/DejaVuSans.ttf
```

### Dependencies

- `nih-plug` (same fork)
- `tiny-skia-widgets` (shared widget crate)
- `softbuffer`, `tiny-skia`, `fontdue`, `baseview`, `crossbeam` (same as other plugins)

### True Peak Code Sharing

The ITU polyphase FIR is currently in `gs-meter/src/meter.rs`. Options:
1. **Copy** the true peak detector to `tinylimit/src/true_peak.rs` (simple, no dependency)
2. **Extract** into a shared crate (cleaner, but more refactoring)

Recommendation: copy for now, extract later (same approach as widgets before tiny-skia-widgets).

## Testing Strategy

### Unit Tests (limiter.rs)

- Gain computer: hard knee at various input levels
- Gain computer: soft knee transition region
- Envelope filter: attack coefficient computation
- Envelope filter: release coefficient computation
- Dual-stage: transient-only (100%) catches peaks, releases fast
- Dual-stage: dynamics-only (0%) has user release time
- Dual-stage: 50% mix doesn't exceed either stage
- Lookahead backward pass: gain reduction starts before peak
- Safety clip: output never exceeds ceiling
- Stereo link: 100% link produces identical GR for both channels
- Stereo link: 0% link produces independent GR
- Gain link: ceiling = threshold when enabled
- ISP mode: true peak detection catches inter-sample peaks

### Integration Tests

- Process a sine wave above threshold: output peak <= ceiling
- Process a transient: gain reduction recovers within release time
- Process stereo with one loud channel: linked GR preserves image
- Latency matches attack parameter

## Non-Goals (YAGNI)

- No multiband (this is a wideband limiter)
- No pre-limiter clipper/waveshaper
- No dither
- No sidechain input
- No MIDI control
- No preset system beyond DAW presets (individual controls replace "Style")
- No oversampling of the audio path (only sidechain for ISP)
