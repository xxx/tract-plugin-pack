# MIDI project-wave switching

Continue `feat/mseg-power-tools` after palette commit `d293770`. The user
authorized the next slice. Extend the CPU editor; no mock supplied. This slice
adds MIDI switching in Multosis/Minitosis; host automation follows separately.

## Behavior

1. A **MIDI** control beside the full MSEG strip opens wave-switch setup. Each
   row/effect can enable one destination curve (Amp/1/2/3), a first MIDI note
   0..120 and Any/channel 1..16. Eight consecutive notes select Project slots
   1..8. Defaults are Off, selected curve, first note 36 and Any channel.
2. Valid positive note-ons switch the destination's playback geometry at their
   host sample offset. Empty slots, invalid messages, note-offs and zero-velocity
   note-ons do not switch. Same-offset events retain host order; the last valid
   selection wins. Inactive/muted/None/bypassed processors still accept switches.
3. Switching preserves all envelope phases, smoothing levels, waveform capture
   clocks and unaffected rows/curves. Existing trigger settings remain independent:
   if the same note matches MIDI retriggering, selection precedes that restart.
   Off smoothing retains endpoint-rate evaluation; positive smoothing retains
   its per-sample/fixed-control clock. Mix continues from its consumed blend.
   Identical geometry selections update the slot cue without resampling lag or
   calling parameter setters between existing control ticks.
4. Playback uses geometry only, with the destination's clock/play mode/polarity,
   routing, depth and smoothing. The mapped curve ignores saved hold markers
   for the whole enabled mode, including before the first note, so selection
   changes geometry without changing its clock. Saved markers remain intact.
   Until the first valid selection, the authored curve plays. Disabling or
   changing the mapping restores that authored curve. Removing an active slot
   falls back to it; editing an occupied slot refreshes playback at a block edge.
5. MIDI selection is runtime state and does not rewrite saved curves, slots or
   local undo history. Reset clears the selection; transport retrigger retains
   it. Settings and the bank save with the project; legacy states default Off.
6. The chosen curve displays the playing wave and its Project slot. Its geometry
   tools are read-only while wave switching is enabled; View navigation and Help
   remain available. Turning MIDI switching Off restores normal editing of the
   authored curve. Other curves and effect/modulation controls remain editable.
7. Setup has normal modal input/layering, keyboard/dropdown navigation, numeric
   outside-click finalization and focus/resize cleanup. Layout and captions stay
   bounded at 0.5x/1x/2x and short/narrow windows. Miff/Nap retain manual palettes.

## Technical approach

- Keep the persisted GUI bank outside audio configuration. Add a generation tag
  and a fixed-capacity, heap-free geometry snapshot; processing uses try_lock and
  bounded copies only when a configured MIDI bank changes. No Vec clones/drops,
  DSP construction or blocking locks on audio. Add no fixed latency or host
  parameters; existing modulated-effect latency changes retain correct reporting.
- Add an optional validated wave mapping to TrackModulation and a small runtime
  override in ModulationState. Reuse streaming MIDI boundaries. Wave-only events
  refresh the affected source at its existing endpoint/current lag phase without
  advancing clocks again; matching retriggers keep existing segmentation rules.
  Pending refreshes survive host/grid segment preparation and are consumed only
  after precise-trigger handling on the note's first processed sample.
- Extend atomic telemetry with mapping/active-slot readings. The GUI derives a
  read-only display copy; saved authored data remains available for undo/editing.
- Retain the ready fixed bank together with the selected wave across same-effect
  sample-rate handoff, including a first-block bank-lock miss.

## Validation

- Cover mapping/legacy serde, fixed snapshot generation and no-op updates, slot
  boundaries, all MIDI offsets/order/density, empty banks, phase/lag/Mix continuity,
  target overlap and unaffected source/row behavior. Test both engines and native
  MIDI ingestion, bank contention/refresh, reset/transport/sample-rate changes,
  inactive/bypass paths and allocation guards.
- Verify settings persistence/undo, read-only editing/navigation, live wave display,
  modal priority, focus/resize and native bounds. Render real states offscreen.
- Run focused six-crate tests, workspace Clippy, format/whitespace and independent
  read-only reviews. Leave this new slice uncommitted until requested.
