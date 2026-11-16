# Project-wave automation

Continue `feat/mseg-power-tools` after `d8b6be1`. The user authorized the next
slice. Extend the existing CPU editor; no mock supplied.

## Behavior

1. Multosis exposes four stepped host parameters per row, and Minitosis exposes
   four per effect bank: **Amp Wave**, **1 Wave**, **2 Wave**, **3 Wave**. Values
   are **Drawn/MIDI** (default) and **Project 1–8**. Existing host parameter IDs
   and ordering stay intact; new parameters append. Values save with host state.
2. A Project selection overrides that curve's MIDI playback selection. MIDI
   continues tracking notes underneath; Drawn/MIDI resumes the latest MIDI wave
   when mapped, otherwise the authored curve. Other curves remain independent.
   An empty selected Project slot plays the authored geometry, still overriding
   MIDI. Populating/deleting/editing that slot updates playback at the next
   available bank refresh. Bank lock contention keeps the last ready snapshot.
3. The host's sample-accurate automation boundaries change geometry at the first
   sample of the supplied sub-block. Selection preserves phase, lag, trigger
   epoch and waveform capture clocks. Source smoothing applies to the changed
   geometry. An identical geometry selection does not disturb lag. Existing
   retrigger settings remain independent, including coincident MIDI/audio events.
   Zero-frame process calls defer wave preparation until actual audio resumes.
4. Project playback uses the destination's timing, polarity, depth and routing.
   Hold markers are ignored while Project automation is active, as in MIDI wave
   mode. Returning to an unmapped authored curve restores its saved hold markers.
   Reset clears runtime MIDI selection but reapplies host Wave values; transport
   retrigger keeps both. Same-effect sample-rate handoff retains ready bank data.
5. The existing wave setup panel gains a Wave selector for the displayed curve.
   The strip shows its Project slot while overridden. Changes use host parameter
   gestures and host undo; they never rewrite authored curves, slots, or local
   curve undo. Automation/recall updates the live curve preview and edit lock.
   View/Help and other controls remain usable. The panel identifies which curve
   its Wave field controls, and explains Project priority over MIDI.
6. Minitosis keeps each effect's Wave values independently. Multosis Wave lanes
   belong to fixed row numbers, including across row swaps; row configuration
   undo does not rewrite host automation. Legacy presets reset missing Wave
   parameters to Drawn/MIDI even when loaded into a populated instance. Miff/Nap
   retain manual palettes.

## Technical approach

- Append a separate nested host Wave parameter bank, avoiding changes to the
  existing Minitosis five-parameter effect banks. Read bounded `[u8; 4]` selections
  at each native process boundary, before refreshing the fixed palette snapshot.
  Both plugins already enable NIH-plug sample-accurate block splitting.
- Reuse the MIDI runtime with an independent preallocated four-curve automation
  overlay. Effective playback geometry resolves automation, then MIDI, then drawn.
  Mark changed active smoothing sources for immediate raw refresh without
  restarting the control clock. Shadowed MIDI notes update their slot cue without
  applying parameters or resampling the automated source. Entering/leaving a
  mapped Project override is staged until the first processed sample so bank
  edits and same-offset notes resolve before comparing the old and final geometry;
  identical final waves remain a no-op.
  Skip preparation on empty native/engine buffers, allowing selector changes
  before the next audible sample to resolve to their final value.
- Extend bounded atomic telemetry with applied Wave selectors. The editor binds
  current host values separately from authored configuration and returns explicit
  Wave edit requests to native ParamSetter bridges. Selector changes invalidate
  modulation measurements while preserving live/frozen waveform captures.
  Keep all audio work allocation
  free, including reset, slot refresh and sample-rate handoff.

## Validation

- Test all four independent selectors, defaults, bounds/format/parse, unique IDs,
  old parameter ordering, host-state migration and per-effect/row isolation.
- Exercise selection/release, empty/changed/deleted slots, MIDI priority and
  coincident retriggers, hold handling, phase/lag/Mix continuity, untouched rows,
  bypass/inactive modes, reset/sample-rate handoff and allocation guards.
- Test real native parameter ingestion at sub-block boundaries, modal input,
  host edit requests, live read-only preview and cancel/resize behavior. Render
  0.5x/1x/2x and short windows; reject layouts without usable field space.
- Run focused six-crate tests, workspace Clippy, formatting/whitespace and
  independent read-only DSP/UI reviews. DAW automation recording remains a
  manual host check. Leave this slice uncommitted until requested.
