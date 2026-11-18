# MIDI retriggering for MSEGs

Continue `feat/mseg-power-tools` after stereo-view commit `ff56a1e`. The user
authorized the next slice. Reuse the existing editor; no mock supplied.
Reference: [ShaperBox MIDI Trigger](https://downloads.cableguys.com/Cableguys-ShaperBox-3-Manual.pdf)
and its channel routing options.

## Behavior

1. Multosis/Minitosis receive basic MIDI note events and offer **MIDI note** in
   the Trigger menu. Existing trigger variants, defaults, persisted states,
   automation IDs, and parameter order retain their behavior.
2. This source restarts all four envelopes on each matching positive-velocity
   note-on. Filters select Any or one MIDI note number 0–127, and Any or channel
   1–16. Filters are saved per row/effect and share local undo. Other events,
   zero/non-finite velocity, malformed notes/channels, and out-of-block offsets
   are ignored. Note-off does not release/gate envelopes: this is retriggering,
   like the existing transient source, rather than a MIDI voice/ADSR mode.
3. Matching host event offsets split processing at the note boundary. Samples
   before a note never contribute to the restarted envelope clock. Offset zero,
   multiple notes, simultaneous notes, last-sample notes, and sequencer boundary
   collisions are handled without dropping events or buffering them in a Vec.
   MIDI/cell triggers preserve smoothing levels. Explicit Reset retains its
   existing clear behavior.
4. Existing clock evaluation remains: Smooth Off evaluates at segment endpoints;
   positive Smooth advances on its existing per-sample/fixed-control clock. The
   new source does not replace either sampler. Unaffected rows retain their
   original host/grid endpoint values and clocks. A matching row recomputes its
   endpoint for the remainder of that original span after reset; a future note
   cannot change the earlier prefix's audio.
   Interior MIDI interruptions restart Mix ramps from the last consumed blend.
   Notes at a new original span retain existing endpoint bridging (including
   row activation); other trigger sources retain their prior ramp behavior.
5. Muted/inactive/None rows and Minitosis bypass keep their clocks and accept
   matching retriggers. Minitosis uses the active processor's recalled filter
   while a different effect is preparing; adopted processors use their own bank.
   Trace/backdrop timing follows the same reset epoch and segment boundaries.
6. Note/Channel are stepped dropdowns in the existing trigger dial spaces.
   Numeric right-click entry is unavailable for these selectors. Menus retain
   final drawing/input priority, keyboard/wheel navigation, measured label width,
   and resize/focus cleanup. MIDI note numbers avoid octave-name ambiguity.

## Implementation and verification

- Append `TriggerSource::MidiNote { note: Option<u8>, channel: Option<u8> }` and
  the shared trigger-menu choice. Add GUI-independent `MidiNoteOn` validation and
  a streaming boundary cursor; native glue converts nih-plug NoteOn events.
- Store MIDI channels zero-based (0–15), matching host events; display/filter
  selectors use channel numbers 1–16 and map their index accordingly.
- Enable `MidiConfig::Basic` in both plugins, with no MIDI output or voice state.
  Drain host events once. Merge accepted MIDI boundaries with existing Multosis
  step boundaries; apply all same-offset events before rendering the next sample.
- Separate Minitosis block preparation/detector work from segment advancement.
  Configure bases/detectors once, and prepare the waveform cursor before each
  segment. Retain ready/retired processor handoff and allocation-free audio.
- Stream every event without an event-count cap. Never allocate, lock, build DSP,
  or retry on the audio thread. Ignore late/out-of-order events defensively;
  host event timing is otherwise sorted. Legacy all-unmatched processing uses
  the original segmentation.
- Test old state compatibility, filters/invalid events, exact offsets, repeated
  notes, clock/gain transitions, smoothing preservation, inactive/bypass/pending
  selection, dense events, sequencer collisions, empty buffers, ignored-event
  audio parity, native MIDI configuration, and allocation guards.
- Test selector editing/undo/recall and popup ownership at multiple scales.
  Render an offscreen MIDI editor preview, update manuals, and run the six-crate
  focused suite, workspace Clippy, formatting/whitespace, and independent reviews.
  Leave this slice uncommitted unless requested.
