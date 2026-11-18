# MSEG envelope follower

Continue `feat/mseg-power-tools` after `d6f09c4`. The user authorized the next
slice. Extend the existing CPU editor; no mock supplied. Reference: ShaperBox 3
manual, Envelope Follower, pages 47–54:
https://downloads.cableguys.com/Cableguys-ShaperBox-3-Manual.pdf

## Behavior

1. Multosis/Minitosis gain one linked stereo envelope follower per row/effect.
   Each of Amp/1/2/3 can independently enable it and choose **Follow**, **Add**
   or **Multiply**, with its own Amount. All routes default Off, Follow, 100%.
   Response/input settings are shared across the four curves.
2. Input is Main (unprocessed plugin input) or external Sidechain; missing
   sidechain is silence. High-pass/Low-pass are independently Off or 20–20000 Hz.
   Gain is -24..+24 dB, default 0. Each channel is filtered separately, then
   linked using the larger absolute level, so antiphase stereo cannot cancel.
   Filtering/Gain affect detection only. Invalid samples are treated as silence;
   extreme detector inputs are bounded without changing the audible signal.
3. Attack 0–1000 ms, Hold 0–2000 ms and Release 0–5000 ms default to 5/0/100.
   Attack/Release are one-pole time constants; 0 is instantaneous. Rising/equal
   targets refresh Hold; falling targets hold the current envelope for the
   configured number of samples, then release. Threshold is Off or -60..0 dB:
   lower levels feed zero to the envelope, so existing Hold/Release still finish.
   The envelope follows the gained input amplitude, clamped to 0..1.
4. Follow outputs Amount × envelope. Add outputs curve + Amount × envelope.
   Multiply outputs curve × Amount × envelope. Results clamp to 0..1 before
   existing Smooth/polarity/Depth/Target mapping. Add allows -200..+200%; Follow
   and Multiply allow 0..200%. Existing curves, clocks and wave selection keep
   running; Follow leaves the drawn geometry available for later editing/use.
5. Amp, Mix and combined source measurements update each sample. Effect
   parameters retain the fixed 1 kHz control cadence, with immediate refresh on
   route/input changes. Enum targets bypass the existing Smooth lag but retain
   follower Attack/Hold/Release. Overlapping targets retain last-slot priority.
   Disabling a route restores its curve without restarting clocks or lag.
6. Retriggers and transport restarts preserve follower history. Inactive/muted/
   None rows and bypass continue following. Explicit modulation Reset clears
   follower history; ordinary DSP flush preserves it. Same-effect sample-rate
   adoption retains the envelope and rescales remaining hold time. Input changes
   start fresh detector history; response/filter edits retain bounded state.
   Enabling the first route starts a fresh follower; adding routes preserves it.
7. Settings/routes save per row/effect and participate in local modulation undo.
   Default follower state is omitted from legacy serialization. There are no
   new host parameter IDs, latency or audio-thread allocations. Zero-frame calls
   defer follower preparation and do not advance or reset its history.
8. **Follow** beside Waves opens a modal setup for the displayed curve. Enable,
   Mode and Amount control that curve; Input, Gain, Threshold, filters and
   response times are visibly shared. **Reset response** restores shared defaults
   while retaining every route. The panel shows actual post-filter/gain input and
   envelope meters, or unavailable state; ordinary Audio views remain raw.
9. The scrollable panel supports bounded scaled layout, keyboard/dropdown
   navigation, fine drags, double-click defaults and right-click numeric entry.
   Enter commits, Escape cancels a field before closing the panel, and outside
   press/drag start/focus loss auto-commit. Open overlays own all input and draw
   last. Resize/source/row/preset replacement closes stale interaction safely.
10. Measured trace/arcs represent the combined source. Trace identity includes
    follower combination mode and input, rejecting previous-source points.
    Exact prepared settings validate meter/frame ownership. Frozen waveform
    capture and curve geometry remain intact. Miff/Nap retain static editing.

## Technical approach

- Add fixed-size normalized follower config/routes to TrackModulation with
  default-compatible serde, and a preallocated boxed follower runtime in each
  ModulationState. Reuse two causal AudioFilters and cache gain/time coefficients.
- Extend the continuous-source mask with enabled routes. Native source processing
  feeds unmodified main/auxiliary samples once per sample before evaluating the
  combined source and existing lag/control path. Preserve legacy execution when
  no routes are active. Return changed-route masks for immediate source refresh.
- Extend bounded atomic telemetry with actual follower readings and prepared
  config. Include selected-route mode/input in the existing trace token key.
- Keep follower modal/drag/text/dropdown ownership in one shared editor module;
  native adapters only persist the ordinary config/undo results. Geometry helpers
  reserve strip space beside Waves, and a visible body viewport scrolls fields.

## Validation

- Exact step/hold/release and sample-rate tests; linked stereo/antiphase, gain,
  threshold, filters/Nyquist, missing/mono sidechain and invalid input safety.
- Default/legacy parity, route math/priority/Mix, clocks/Waves/retriggers, source
  lag/enums, mode/input changes, reset/flush/handoff, arbitrary host/grid splits
  and allocation guards in shared/native paths.
- Preset/undo, actual/absent meter readings, stale trace rejection, modal numeric/
  keyboard/scroll/resize ownership and 0.5x/1x/2x/short-window rendering.
- Focused six-crate nextest, workspace Clippy, formatting/whitespace and read-only
  DSP/native/UI reviews. Keep this slice uncommitted until requested.
