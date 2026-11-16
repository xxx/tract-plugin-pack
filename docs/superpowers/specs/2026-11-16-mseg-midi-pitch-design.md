# MIDI-pitch MSEG playback

Continue `feat/mseg-power-tools` after `a8948d1`. The user authorized the next
slice of the original ShaperBox comparison. Extend the existing CPU editor;
no mock supplied. Reference: ShaperBox 3 manual, LFO settings, pages 41–42:
https://downloads.cableguys.com/Cableguys-ShaperBox-3-Manual.pdf

## Behavior

1. Multosis/Minitosis append **MIDI pitch** to Trigger. A matching positive
   note-on sets all four curves' cycle rate to `440 × 2^((note-69+transpose)/12)`
   Hz and restarts them at that sample, whose phase is zero. Transpose is an
   integer -36..+36 semitones, default 0. Channel is Any or 1–16, default Any.
2. The last matching note wins, including simultaneous events in host order.
   Note-off, zero/non-finite velocity, malformed events and invalid offsets do
   not affect playback. Note-off does not gate or stop the clock. There is no
   voice stack or pitch-bend support in this slice.
3. Before the first note, curves hold at phase zero. Entering the mode or changing
   Channel clears the latched pitch and waits for another matching note. Editing
   Transpose updates the last note's rate without restarting phase. Leaving the
   mode resumes ordinary playback from the last phase. Original Time/Beat lengths
   and curve geometry are retained, and are ignored while MIDI pitch is active.
   Hz describes traversal of the full 0..1 phase; a shorter hold loop repeats
   faster according to its span.
4. Cyclic, OneShot and hold markers retain their phase traversal semantics. Each
   new note restarts even a completed OneShot. Project-wave automation and MIDI
   wave selection retain their priority; a note can select a wave and set pitch
   together. Rates above Nyquist remain finite and use normal sampled phase wrap.
5. Pitch phase, Amp, Mix and combined follower sources run per sample even with
   Smooth Off. Effect destinations retain the fixed 1 kHz control cadence and
   refresh immediately on note/route changes. Smooth keeps its current level
   across notes, mode changes and transpose edits; enum destinations bypass it.
   Playback is independent of host/grid partitioning and tempo.
6. Muted/inactive/None rows and Minitosis bypass keep tracking. Ordinary DSP flush,
   transport restart and same-effect sample-rate handoff preserve the latched
   note. Flush/handoff preserve phase; handoff also preserves source lag and any
   pending restart sample, recalculating the step for the new rate. Transport
   restart resets phase. Explicit modulation Reset clears the pitch and
   waits. Empty process calls defer mode/settings preparation and consume no
   events or phase. No latency, allocations or host parameter IDs are added.
   Pending different-effect selections continue feeding the old active processor;
   adopting a different kind starts waiting for a fresh matching note.
7. Per-row/effect settings save and use local modulation undo. Existing trigger
   variants, defaults, serialization and automation ordering remain intact.
   Miff/Nap retain static curve editing.
8. The editor offers Transpose and Channel stepped selectors beside Trigger.
   They use existing dropdown keyboard/wheel navigation and overlay priority.
   Saved length controls become a MIDI-pitch caption and a measured Hz/note
   readout, with waiting/unavailable states. They cannot edit inactive lengths.
   Hints and the gesture guide explain note priority, shared rate and note-off.
9. Measured traces, arcs, playheads and waveform capture use actual pitch phases.
   Waiting pauses waveform capture. Note resets and actual loop traversal clear
   waveform aggregation; one-shot intro peaks remain preserved. Source identity
   rejects late non-pitch frames/trace points after changing mode. Exact prepared
   Channel/Transpose settings validate measured readouts. Frozen audio remains
   available while editing settings. Layout remains bounded at 0.5x/1x/2x.

## Technical approach

- Append `TriggerSource::MidiPitch { transpose: i8, channel: Option<u8> }` with
  default fields. Add a small fixed-size runtime for active settings, latched
  note/rate, sample rate and the first-sample flag. Normalize transpose when used.
- Use existing streaming MIDI boundaries. A shared note handler selects pitch
  before triggering; both native engines call it. Prepare only on nonempty blocks.
  Reuse per-slot double-precision phase/lag state and its control clock, overriding
  steps in pitch mode and evaluating the restart sample before advancing.
- Track actual pitch wraps with per-slot capture epochs. Externally driven
  waveform clocks trust those phases/epochs rather than saved Time/Beat lengths.
- Add bounded atomic pitch telemetry and one trace-token mode bit. Keep the
  existing source request APIs as backward-compatible wrappers. The shared editor
  validates actual settings and draws the measured rate in the old length space.
- Keep implementation local. Independent read-only DSP/native and UI reviewers
  check the specification and resulting implementation per AGENTS.md.

## Validation

- Frequency/transpose/filter validation, exact first-note sample, simultaneous
  notes, pre-note wait, note-off/invalid events, high-rate wraps and hold modes.
- Shared/native host/grid partition probes, legacy audio parity, smoothing,
  control cadence, Mix, follower/Wave interactions, resets/flush/SR handoff,
  inactive/bypass/pending selection and allocation guards.
- Serde/undo/recall, full telemetry payload and late-source rejection, dropdown
  ownership, saved-length protection, scaled offscreen renders and frozen capture.
- Six-crate focused nextest, workspace Clippy, format/whitespace and read-only
  reviews. Keep this new slice uncommitted until separately requested.
