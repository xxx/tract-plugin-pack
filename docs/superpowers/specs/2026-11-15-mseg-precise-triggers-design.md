# Sample-timed audio and Free Hz retriggers

Continue `feat/mseg-power-tools` after MIDI commit `4f39c26`; the user authorized
the next slice. Reuse the current editor (no mock supplied). Reference:
[ShaperBox Audio Trigger](https://downloads.cableguys.com/Cableguys-ShaperBox-3-Manual.pdf).

## Behavior

1. Add opt-in **Free Hz (sample)** and **Transient (sample)** choices. Existing
   Free Hz/Transient variants keep their block-aligned behavior and wire format.
   Old projects/defaults/automation order remain unchanged. New choices and their
   Rate or Sens/Hold values save per row/effect and share local undo.
2. Sample-timed Hz uses a persistent f64 repetition clock. Its first restart is
   after one full period; fractional periods do not restart at host/grid/MIDI
   boundaries. Rate/sample-rate changes retain normalized clock progress. Invalid
   rates freeze the clock. More than one period per sample coalesces to one fire.
3. Sample-timed Transient uses the existing dual-envelope threshold/noise-floor/
   Hold rule and dry stereo-average detector input. It checks each original input
   sample before effects mutate it and retains every accepted onset, including
   multiple onsets per block. Detector lag is unchanged; firing occurs on the
   actual detection sample. It adds no latency/lookahead or sidechain bus.
4. Restarts occur before the triggering sample's modulation/audio. All four
   envelopes on the row restart; Smooth retains its lag. Off retains existing
   endpoint evaluation. Unaffected rows keep their original host/grid endpoint
   values and smooth clocks. Matching rows reprepare only for the remainder of
   the original span, preserving the MIDI slice's row-local cadence rules.
5. The first sample of an original span is detected before any endpoint setup,
   preventing surplus enum/FFT setters and preserving inactive Mix bridging.
   Interior fires continue interrupted Mix from its consumed value. Triggers
   still run during inactive/muted/None/bypass paths. Grid/MIDI collisions do not
   double-process a detector sample or move other rows' clocks.
6. Explicit Reset/transport restart clear precise clock/detector timing;
   transport restart keeps smoothing. DSP flush preserves phases/trigger clocks
   while clearing smoothing seeds, as before. Switching between precise source
   families resets their private trigger timing. Same-family parameter edits
   retain the current clock/envelopes; sample-rate changes rescale refractory
   time. Trace/backdrop cursors follow the actual row reset epochs.
7. Reuse Rate or Sens/Hold dials. Menus retain host order, popup/input priority,
   readable sizing, keyboard/wheel behavior, and focus/resize cleanup. Miff/Nap
   retain static curves. No host parameters or serialized geometry changes.

## Implementation and verification

- Append explicit trigger variants. Add a fixed-size per-state precise runtime;
  cache detector coefficients on sample-rate changes and share the existing
  detector arithmetic without changing the legacy entry point's calculations.
- Native preparation handles first-sample fires before span advancement; later
  fires are processed in the sample loop before smoothing/effects. Cache a list
  of precise rows; legacy-only processing does no per-sample detector work.
- The Hz clock integrates a numerator against sample rate plus a pending-fire
  flag, preserving fractional progress when coalescing supra-sample frequencies.
  Same-effect Minitosis SR reinitialization copies only private precise timing
  into the new processor; other processor construction/reset policies stay intact.
- Keep event processing streaming: no event arrays/count caps, allocations,
  blocking locks, effect construction, or retries on the audio thread.
- Test 44.1/48/96/192 kHz fractional periods, partition-invariant event positions,
  multiple impulses/onsets, refractory/reset/rate changes, detector equivalence,
  invalid parameters, exact audio offsets, inactive/bypass/None, grid/MIDI
  collisions, unaffected-row parity, first-sample enum setup, Mix continuity,
  scope timing, saved-state/undo/menu behavior, and allocation guards.
- Render actual audio-triggered DSP offscreen; update manuals; run the six-crate
  focused suite, workspace Clippy, formatting/whitespace, and read-only independent
  reviews. Leave this slice uncommitted unless requested.
