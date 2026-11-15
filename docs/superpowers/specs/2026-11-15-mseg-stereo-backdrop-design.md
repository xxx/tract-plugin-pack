# MSEG stereo waveform views

Continue `feat/mseg-power-tools` after audio-backdrop commit `02903cd`.
The user authorized committing that slice and proceeding. Reuse the existing
CPU UI; no mock supplied. Reference: [ShaperBox's Oscilloscope views](https://downloads.cableguys.com/Cableguys-ShaperBox-3-Manual.pdf).

## Behavior

1. Multosis/Minitosis add Magnitude, Combined, and Stereo display choices to the
   existing Audio menu. Magnitude remains the default and retains the current
   greater-absolute-channel peak view, with zero at the bottom.
2. Combined overlays signed left/right ranges around a shared zero at mid-height.
   Stereo shows left in the upper half and right in the lower half, each with
   its own zero line. Channels retain their signs and are never averaged or
   summed, so opposite-polarity audio remains visible. Zero-width DC ranges
   remain visible as a one-pixel stroke rather than disappearing.
3. The menu also offers display gain -12, 0 (default), and +12 dB. Gain affects
   only the backdrop. It applies before display clipping; each Stereo channel
   clips within its own fixed physical half-lane, including when zoomed/panned.
   Vertical zoom/pan maps signed levels inside each lane; the channel divider
   remains at the plot's physical midpoint.
   Raster clip bounds partition fractional dividers into disjoint pixel rows;
   boundary DC strokes move inward to remain visible within their channel.
4. Source Input/Output/Both and Freeze retain their behavior. Changing display
   mode or gain reuses captured measurements, including while frozen, and does
   not restart capture, clear waveform/trace history, or alter source epochs.
   Focus/resize retains these editor-session preferences. Existing source-change,
   state-replacement, Grid, pending-processor, and window cleanup still apply.
5. Input remains gray and output follows the selected envelope's color. Waveforms
   and zero guides stay behind grids, authored/ghost curves, nodes, and traces.
   A longer Audio menu keeps full keyboard/wheel/mouse priority and renders in
   the final popup pass at all supported scales.
6. Modes/gain are view-only: no DSP, latency, envelope geometry, host parameters,
   persistence formats, or undo changes. Miff/Nap retain their static views.

## Implementation and verification

- Preserve signed min/max for both channels of input and output in each phase
  bin. Aggregate every finite sample; initialize empty ranges without injecting
  zero, preserving DC offsets. The existing scalar peak accessor derives the
  original magnitude from these ranges.
- Keep the existing bounded ring/epochs/f64 clock. Pack four signed ranges into
  atomic u64 payloads under the existing Release/Acquire stamp protocol. No
  allocations, locks, unbounded retries, or sampling/trigger changes in audio.
- Share simple range/bin/style types with the pure MSEG background renderer.
  Only request-aware audio capture adds range comparisons; rendering/gain stays
  on the GUI thread. Off/Freeze/closed views perform no sample capture work.
- Test anti-phase, asymmetric channels, negative/positive/DC ranges, non-finite
  samples, partial bins/retriggers, coherent concurrent payloads, view/gain lane
  clipping, original magnitude parity, frozen switching without epoch changes,
  popup routing at 0.5x/1x/2x, and existing engine parity/allocation guards.
- Render all three views from actual captured DSP output. Update manuals, run
  the six-crate focused suite, workspace Clippy, formatting/whitespace, and
  independent read-only reviews. Leave this slice uncommitted unless requested.
