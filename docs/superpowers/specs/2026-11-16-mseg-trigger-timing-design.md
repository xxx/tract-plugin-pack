# Trigger delay and retrigger blending

Continue `feat/mseg-power-tools` after `81dcf6b`. The user authorized the next
comparison slice. Extend the CPU editor; no mock supplied. Reference: ShaperBox
3 manual, Audio Trigger Setup Trigger Shift and Anti-Click Trigger Smoothing:
https://downloads.cableguys.com/Cableguys-ShaperBox-3-Manual.pdf
This slice adds causal delay and retrigger blending. Negative shifts/lookahead
follow as a separate audio-pipeline/PDC change.

## Behavior

1. Multosis/Minitosis add **Timing...** at the end of the Trigger menu. It opens
   a modal without changing the selected source. **Audio delay** is 0–4 ms;
   **Retrigger blend** is 0–20 ms. Both default Off and save per row/effect with
   local modulation undo. Default settings are omitted from legacy serialization.
2. Audio delay affects Transient (sample) only. Detection, Threshold/Hold and
   its meter remain at the original sample; accepted onsets schedule a phase
   reset after the configured delay rounded to the nearest sample. Multiple
   distinct accepted onsets remain queued across host/grid boundaries; resets
   arriving at the same sample merge. No input/effect audio is delayed and no
   host latency or parameter IDs are added. MIDI waves still switch at their
   original offsets and other trigger sources retain their timing.
3. Queued resets keep their accepted delay when a positive preference changes.
   Turning delay Off, changing mode/input, invalidating the precise source,
   explicit Reset or transport restart clears old queued resets. Filter/Sens/
   Hold/Algorithm/Detail edits otherwise do not move accepted cues. Sample-rate
   handoff rescales pending sample intervals. Empty calls prepare/consume nothing.
   Explicit reset/transport commands still clear or restart immediately on an
   empty callback; their phase-zero/blend sample waits for real audio.
4. Retrigger blend crossfades each eligible source from its last delivered value
   to the newly running curve over the selected number of samples (rounded up).
   It applies after follower combination and existing Smooth, before polarity/
   Depth/Target mapping. The new curve and Smooth state keep advancing underneath.
   A retrigger during a blend starts from the currently delivered value.
5. Blending applies to explicit phase resets from MIDI, cells, free Hz, audio and
   transport restart. Follow-only follower routes and enum destinations bypass
   the blend; absolute DAW-position playback does not blend retriggers. A first
   trigger without a measured previous value does not invent an old source.
   Ordinary cyclic wraps/wave switches retain their existing behavior.
6. Enabling either effective timing option uses the shared per-sample source
   path, even with Smooth Off. A reset sample evaluates phase zero before the
   next phase step. Amp/Mix/measurements update per sample; effect parameters
   keep the fixed 1 kHz cadence plus immediate reset refresh. Enum sources remain
   sample-clocked but do not interpolate. Default Off retains legacy execution.
7. A positive blend edit affects later resets; a running blend finishes its
   captured interval. Turning blend Off cancels it. Same-effect SR handoff retains
   phase/base lag/delivered output and rescales pending blend time from the last
   delivered value. DSP flush clears audio/blend history without moving clocks;
   explicit modulation Reset also clears pending timing. Inactive/muted/None rows
   and bypass continue tracking; Listen remains independent.
8. Timing... supports fine drags, double-click Off and right-click milliseconds.
   Enter commits, Escape cancels a numeric field before closing the modal, and
   outside press/drag/focus loss auto-commit. Menus/numeric/drag ownership prevent
   hidden edits. Source/row/preset/resize replacement cancels stale interaction.
   Reset restores both Off in one undo. Miff/Nap retain static curve editing.
9. The dialog explains the audio-only delay and enum/Follow/DAW exclusions.
   Bounded layouts work at 0.5x/1x/2x and HiDPI; small windows show an enlargement
   message with inactive controls. Actual playheads/traces/arcs/capture follow
   delivered phases/source; frozen captures and authored geometry remain intact.

## Technical approach

- Add normalized optional/default-omitted TriggerTiming settings. A preallocated
  boxed runtime uses a 65,536-sample bit ring for pending reset samples plus four
  fixed blend records. This covers 4 ms at audio rates far beyond normal hosts;
  pathological sample counts saturate at 65,535 instead of allocating on process.
- Schedule delayed audio resets inside the existing once-per-sample precise
  trigger wrapper, preserving native preflight skip and boundary splitting.
  Keep zero-delay fast paths literal. Preserve/rescale pending bits on SR handoff.
- Reuse the continuous-source engine and actual output telemetry. Arm blends from
  delivered sources before phase reset, cancel unsupported slots in preparation,
  and interpolate outside the underlying one-pole lag state. Reset/SR adoption
  copy into existing fixed buffers, with no allocation or effect construction.
- Add Timing as a menu command, not a TriggerSource variant. Keep modal state/
  ownership in a shared editor module. Native adapters persist ordinary config.
- Implement locally; independent read-only DSP/native and UI reviews per AGENTS.md.

## Validation

- Exact delay offsets/queue collisions/host splits/changes/reset/SR/empty paths,
  legacy Off parity, blend interpolation/retrigger reversal/lag/enums/Follow,
  control cadence, Mix and first-sample phase. Bound invalid/extreme inputs.
- Native preflight/sidechain/grid/MIDI-wave collisions, bypass/inactive/None,
  sample-rate adoption, Listen/capture/history parity and allocation guards.
- Serde/undo/recall, command source preservation, modal numeric/drag/cleanup,
  frozen capture and real scaled offscreen previews.
- Six-crate nextest, workspace Clippy, format/whitespace and independent reviews.
  Leave the new slice uncommitted until separately requested.
