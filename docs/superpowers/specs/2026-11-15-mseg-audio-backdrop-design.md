# MSEG audio waveform backdrop

Continue `feat/mseg-power-tools` after smoothing/trace commit `f13eb34`.
User authorized the next slice in conversation. Reuse the existing CPU UI;
no mock supplied. Reference: Cableguys ShaperBox 3 manual, Shaper Oscilloscope.

## Behavior

1. Multosis/Minitosis add an Audio menu beside Smooth/Trace with Off (default),
   Input, Output, Both, and a Freeze toggle. These are editor-session preferences;
   they never change audio, saved envelopes, host parameters, or undo history.
2. The backdrop shows stereo magnitude peaks (the greater absolute channel),
   with input in quiet gray and output in the selected envelope's color. It
   remains beneath the authored curve, nodes, grids, ghosts, and measured trace.
   Magnitude 0 is the plot bottom and 1 is full height; larger peaks clip visually.
3. The time axis follows the selected envelope, including beat/time length,
   triggers, wrap, sustain/loop behavior, and zoom/pan. Capture examines every
   audio sample; short transients cannot disappear between telemetry ticks.
   Off smoothing retains its existing block/segment-rate audio behavior; a
   dedicated display cursor projects samples through that segment from its
   starting phase instead of using its already-advanced endpoint for the block.
4. Multosis captures immediately before/after the selected row's effect, Mix,
   and Amp, before the wet-bus compressor/master blend. Inactive, muted, and
   None rows show identical input/output. Minitosis captures processor input
   and returned output, including bypass/dry delay, before its Output gain.
   Effect latency is visible; no artificial alignment or audio processing is added.
5. Live capture replaces visited phase bins and retains the last observed pass
   elsewhere. Freeze retains the currently displayed bins and stops audio capture;
   unfreezing starts a fresh history. Input/Output/Both switch the visible layers
   without discarding captured data. Off clears history and stops capture.
6. Switching row, envelope, effect, or restored state clears the backdrop even
   while frozen. Existing pending-selection, reorder, Grid, and native-window
   cleanup release both measurements. Phase holds retain the completed display.
7. Closing the editor or using Off/Freeze adds no per-sample peak calculations.
   Capture has fixed storage, bounded GUI reads, no audio allocations/locks or
   retries, and no serialization-format changes. Miff/Nap retain static curves.

## Implementation and validation

- Add independent request epochs and a bounded 2048-record atomic waveform ring
  to the shared telemetry owner. Aggregate max input/output by 512 phase bins on
  the audio thread, retaining the pending bin across host block boundaries.
  Publish completed bins and the current partial bin at block end. Use stamped
  Release payload writes / Acquire reads, skipping torn/overwritten records.
- Share the existing phase-step/hold arithmetic for the display-only cursor;
  follow actual f64 per-sample phases for the selected slot's effective positive
  smoothing bit. Initialize the cursor
  before legacy segment advancement and after existing block/cell triggers.
- Reset pending peaks on a trigger epoch or unwrapped loop-boundary crossing,
  including loops shorter than one display bin/sample. Retain pending peaks
  across block-end partial publication within the same visit.
- Keep 512 latest GUI bins. Freeze releases its waveform request while retaining
  those bins. Source epochs reject stale data and preserve the modulation trace.
- An independent owner generation invalidates frozen histories on replacement.
  Focus/resize cancels gestures while retaining source history; state restoration
  explicitly invalidates it.
- Test peak retention, partial bins, block partitions, smoothing on/off phases,
  holds/loops/retriggers, epochs/overruns, concurrent reads, bypass/inactive/None,
  audio parity and allocation guards, view-only controls, overlay input priority,
  freezing/source switches, clipping, resize, and an actual DSP offscreen render.
- Run the six-crate focused suite, workspace Clippy, formatting/whitespace, and
  independent read-only reviews. Leave this slice uncommitted unless requested.
