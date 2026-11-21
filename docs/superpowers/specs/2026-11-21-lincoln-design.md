# Lincoln

## Summary

Lincoln is a three-band upward and downward compressor modeled on the factory
sound and controls of Xfer OTT 1.37. It adds two automatable crossover frequencies
and a Tract-style editor. No Figma mock was provided.

## Behavior

1. Lincoln runs in mono or linked stereo as a CLAP, VST3, or standalone effect.
   It reports zero samples of latency. Bypass passes the input through, and
   engaging the effect again does not restart its crossover or detector state.
   The processor requests silent blocks until the detector finishes releasing.
2. The factory instance applies full upward and downward compression at the
   reference timing, with neutral user gains and thresholds. The parameter
   defaults and ranges below are visible in the editor and persist with the
   session. Each band has separate switches to turn its upward and downward
   compression on or off.
3. Depth scales the dynamics and automatic makeup. At 0% the crossover still
   affects phase, and manual gains still apply. Time changes the release. Its
   behavior in milliseconds follows the 48 kHz reference at all sample rates.
   When the combined Downward strength and Depth reach the maximum ratio,
   increasing the strength lowers the downward knee by up to 12 dB. The
   factory knee and gain curve use the measured reference settings.
4. Low/Mid and Mid/High start at 88.3 Hz and 2,500 Hz. Both are automatable and
   can be reset individually. Their effective values stay ordered and below
   Nyquist. A small minimum gap keeps the bands distinct. The editor limits the
   dragged split at the other split. Double-click restores the control's
   factory target, and the band map follows the applied splits during host
   automation. Close splits gain phase compensation to prevent cancellation
   when the bands are recombined.
5. The resizable CPU-rendered editor groups the six global controls above three
   color-coded band strips. Two frequency dials sit beside a logarithmic band
   map. Each strip shows gain change and exposes its threshold, gain, and two
   direction switches. Continuous controls support Shift-drag, double-click
   reset, and right-click numeric entry. Edits start from the parameter's base
   value under host modulation. The editor saves logical dimensions.

## OTT 1.37 factory reference

Defaults were read from the local Windows VST2 and VST3 on November 21, 2026.
The VST3 additionally exposes a wrapper Program parameter whose default is 1.

| Control | Factory | Range |
| --- | ---: | --- |
| Depth | 100% | 0–100% |
| Time | 100% | 0–1,000% |
| In Gain, Out Gain | 0 dB each | Down to silence, up to +19.1 dB |
| Upward, Downward | 100% each | 0–200% |
| Low, Mid, High Threshold | 100% each | 0–200% |
| Low, Mid, High Gain | 0 dB each | −∞ to +6.0 dB |
| Six band bypasses, global bypass | Off | Off/on |
| Clean XOV | Off | Off/on |
| Low/Mid, Mid/High | 88.3 Hz, 2.5 kHz | Fixed in OTT |

Warmed-up 48 kHz impulse tests at Depth 0% isolated the bands by muting both
other gains. They measured −6.02 dB at each split and matched a series LR4
network. The low output is taken directly from the first low-pass branch. The
first high-pass branch feeds both arms of the upper split. The measured factory
network delays two samples and its sum dips by about 0.014 dB. Both formats
rendered the same factory crossover. For Clean XOV, the measured lower split
was 88.33 Hz at 44.1 kHz, 96.15 Hz at 48 kHz, and 192.29 Hz at 96 kHz. Lincoln
uses a single calibrated crossover at the displayed frequencies.

At 48 kHz, neutral-threshold tones measured a 0.75 dB gain increase per dB of
falling input in the upward region. That gain increase caps at about 36 dB.
The downward region is near limiting and the automatic makeup at full Depth
measured +13.94 / +9.34 / +13.94 dB (Low / Mid / High). High-band tone steps
showed nearly identical attack with Time 100% and 550%. The initial release
recovered about 2.8 dB per 10 ms at 48 kHz and 4.3 dB per 10 ms at 96 kHz at
Time 100%. Fitting the initial 48 kHz release gives power-envelope time
constants of about 32.4 / 32.4 / 15.2 ms at factory Time. The release scales
with Time down to a floor of about 10 ms. These values describe the test
signals and observed response.

- [Xfer OTT 1.37 downloads and release notes](https://xferrecords.com/freeware)
- [Xfer's explanation of Depth](https://xferrecords.com/forums/general/how-is-the-depth-knob-different-from-the-dry-wet-on-the-ableton-ott)
- [Xfer's explanation of Clean XOV](https://xferrecords.com/forums/general/ott-preset)
- [David Braun's measured OTT 1.3.1 model](https://faustlibraries.grame.fr/libs/compressors/#coxfer_ott)

## Implementation and validation

Use two cascaded second-order Butterworth sections per split output with
trapezoidal state-variable integrators in double precision. Smooth split
frequencies in log space, keep the integrator state during changes, and update
the coefficient targets on a fixed sample cadence. Blend the low arm toward an
upper-split allpass as the cutoffs approach. Use the measured uncompensated
response at factory spacing.

Each band runs one linked mean-square detector and a bounded piecewise gain
curve with independent upward and downward contributions. Apply input gain
ahead of the crossover, band gain after dynamics, and output gain after the sum.
Use stack-allocated DSP state and lock-free meter updates. The editor uses
`EditorWindow`, `DragState`, and `TextEditState` for the pack's geometry and
input behavior.

Verify band response and phase against impulse measurements, neutral summation
at close splits, ordering under extreme automation, static and stepped gain
response, linked stereo, silence, reset, and sample-rate consistency. Run
workspace formatting and Clippy, Lincoln tests, and the redistributable bundle.

The Linux VST3 factory build matched OTT 1.37 to within 0.42 dB across 45
band-isolated steady-state tones at 48 kHz. Its 10 ms RMS deviation on the
level-step probes was under 0.76 dB at the 95th percentile in each band. A 5 ms
window 200 ms into the high-band release was 2.1 dB below the reference.
With the reference delay removed, the neutral 0.25-amplitude impulse differed
by at most 3 × 10⁻⁶. The same simultaneous split automation produced identical
samples at host block sizes of 127, 512, and 1,024.
