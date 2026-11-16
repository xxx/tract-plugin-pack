# DAW-position MSEG playback

Continue `feat/mseg-power-tools` after `643899a`. The user authorized the next
slice. Extend the existing CPU editor; no mock supplied. Reference: ShaperBox 3
manual, LFO settings, pages 41–42:
https://downloads.cableguys.com/Cableguys-ShaperBox-3-Manual.pdf

## Behavior

1. Multosis/Minitosis append **DAW position** to Trigger. Each row/effect saves
   its choice and local undo restores it. Existing trigger values, defaults and
   host parameter IDs remain unchanged. Miff/Nap retain static curve editing.
2. In this mode every curve follows absolute song position. Beat uses quarter
   notes divided by that curve's beat length; Time uses seconds divided by its
   time length. Cyclic repeats, including before timeline zero. OneShot starts at
   timeline zero and clamps at its endpoint/sustain point. Saved hold loops are
   honored; MIDI/Project wave modes retain their existing hold-free behavior.
3. The first sample uses the supplied block-start position. Position advances
   per sample while playing, freezes while stopped, and follows seeks and tempo
   changes on the first sample of the new host block. Reported active loop ranges
   also wrap within a block. Missing/non-finite positions freeze the affected
   time domain at its last phase; invalid lengths/rates never produce NaNs.
   Transport restarts preserve those frozen phases and use the installed trigger
   mode when a mode change coincides with the restart.
4. Audio and phases are independent of host buffer/grid segmentation. Amp/Mix
   and source measurements update per sample, including Smooth Off. Existing
   target priority and polarity/depth mapping apply. Effect parameters use the
   fixed 1 kHz control clock, with immediate application on discontinuities or
   wave switches. Enum targets bypass source lag, as in other modes.
5. Positive Smooth retains its lag across transport jumps and mode changes;
   Off follows the current phase immediately. MIDI wave selection and host Wave
   automation keep their priority and never reset the host clock. Cell/MIDI/audio
   retriggers do not override DAW position. Reset/restart and processor adoption
   relock to the supplied position, including pending effect preparation.
6. The graph's playhead, measured trace and waveform capture follow the playing
   phase. Seeks/loop returns start fresh waveform aggregation at that position;
   stopped or unavailable clocks pause capture. Geometry and timing remain
   editable. Leaving the mode resumes free playback from the last phase.
   OneShot intro peaks, including clamped negative-time samples, remain aggregated
   until actual loop traversal; a large phase step alone does not reset a bin.
7. Zero-frame calls latch incoming host data without advancing clocks, changing
   lag, or causing intermediate refreshes. Inactive/muted/None rows and Minitosis
   bypass keep tracking. The mode adds no audio latency or audio-thread allocation.
8. Trigger captions/hints and the gesture guide explain timeline origin, Time vs
   Beat, OneShot and missing-position behavior. Dropdown/modal priority and
   layout remain usable at 0.5x/1x/2x and freely resized windows.

## Technical approach

- Add a host-independent fixed-size position record (playing, beats/seconds,
  tempo, optional active loop ranges) and bounded timeline clock to tract-effects.
  Native adapters use NIH-plug's transport accessors at every process/sub-block.
  Keep that record outside authored/persisted curve data.
- Reuse the existing per-sample modulation runtime with a continuous-source mask
  for DAW mode. Evaluate at the absolute sample position; zero lag has unity
  gain, supported positive lag preserves its state, and parameter setters retain
  the existing bounded control cadence. Legacy masks/paths stay unchanged.
- Detect transport discontinuities using the last consumed sample count; refresh
  sources/targets and capture epochs without clearing lag or wave selections.
  Handle loops from host ranges, seeks, paused positions, invalid/missing data,
  metadata edits and sample-rate changes. Defer preparation on empty buffers.
- Extend telemetry mode validation and waveform external-phase/epoch handling.
  Capture epochs track actual curve-loop iterations as well as host discontinuities.
  Keep native handoff, row swapping and pending wave refreshes coherent.
- Native position accessors may synthesize missing coordinates/loop ranges using
  the current tempo. Exact seconds across historical tempo changes require
  reliable host seconds/sample data; no extra conversion is invented here.
- NIH-plug's wrapper sub-block offsets are corrected separately in its custom fork:
  preserve stopped/origin positions, project known-rate domains and active loops,
  retain bar metadata origins, and use CLAP transport-event-relative offsets.
  This was merged in [fork PR #2](https://github.com/xxx/nih-plug/pull/2).
  Cargo.lock pins the merged revision `32f72ccc8194cb79ada455be6d0b8753ed638c6a`.

## Validation

- Unit tests for absolute phase math, negative/large positions, beat/time lengths,
  hold modes, loop boundaries, invalid inputs, missing domains and continuity.
- Shared/native engine probes for sample offsets, arbitrary segmentation, grid
  boundaries, tempo/seeks/loops/stop/start, smoothing/Mix/enums, Wave/MIDI priority,
  reset/handoff, paused capture and allocation guards. Test legacy no-op behavior.
- Input/serde/undo/caption coverage and real offscreen previews at multiple sizes.
- Focused six-crate nextest, workspace Clippy, format/whitespace and independent
  read-only DSP/native and UI reviews. DAW-specific transport behavior remains
  a manual host check. Leave this slice uncommitted until requested.
