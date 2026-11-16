# External MSEG sidechain input

Continue `feat/mseg-power-tools` after loop-seam commit `dd31334`. The user
authorized the next wishlist slice. Extend the existing detector and audio menus;
no mock supplied.

## Behavior

1. Multosis/Minitosis expose one stereo auxiliary input named **Sidechain**,
   alongside the existing stereo main input/output. Keep a stereo-only layout
   available for hosts selecting the previous configuration. Missing auxiliary
   data is silence; a mono buffer is duplicated to both detector/display channels.
   Short auxiliary buffers are zero-padded without allocating or truncating main
   audio. Never fall back to main audio when Sidechain is explicitly selected.
2. Transient (sample)'s Detect panel adds **Input: Main / Sidechain**, saved per
   row/effect with the detector settings and covered by local undo. Main remains
   the omitted default in old projects. Legacy block-rate Transient and other
   trigger sources retain their current behavior.
3. The chosen dry signal feeds the existing stereo-average detector, threshold,
   filters, Hold, and measured level. Routing changes clear private detector/filter
   history and refractory state while preserving envelope phases and smoothing.
   The new input must be used for first-sample preparation and interior samples
   across grid/MIDI/host partitions. No events are capped or delayed to a block.
4. Sidechain never enters the audible effect/dry path or changes reported latency.
   Main-source processing remains identical with arbitrary auxiliary input.
   Detection continues during bypass, muted/inactive/None rows, and pending
   Minitosis preparation, following existing processor ownership rules.
5. The Input button is stepped and non-editable. Clicking it commits pending
   detector numeric input first and keeps the modal panel open. Reset turns the
   threshold/filters Off while retaining the input choice. Source identity is
   carried in coherent detector telemetry so old-source readings are hidden.
6. Audio adds **Sidechain** as a view of the unfiltered auxiliary signal, using
   the existing Magnitude/Combined/Stereo and display-gain choices. It follows the
   selected MSEG's actual clock and preserves short peaks. Input/Output/Both keep
   their existing row/processor meaning; detector Input and waveform view are
   independent choices.
7. Capture still uses one bounded requested stream. Switching between the main
   capture family and Sidechain clears its bins and unfreezes to capture the
   chosen source. Input/Output/Both switches keep their current frozen-capture
   behavior. Request identity includes capture source; reject stale old-source
   samples. Closing, Off, and Freeze release capture as before.
8. No new host automation IDs or curve-file attributes. Shape copy/paste stays
   geometry-only. Static Miff/Nap remain unchanged. No audio-thread allocation,
   blocking synchronization, auxiliary writes, DSP construction, or extra latency.

## Implementation and validation

- Introduce a borrowed shared SidechainInput view with safe mono/missing/short
  sample reads and offset-preserving subviews. Native adapters borrow the first
  auxiliary input for the current process call only.
- Default-skip a detector input enum in AudioTriggerSettings; include it in
  coherent measured-frame payloads and cached filter identity. Keep existing
  public main-only processing entry points as silent-sidechain wrappers.
- Thread auxiliary samples through both engines, including pre-segment trigger
  checks. Waveform requests select main or sidechain capture in the existing
  AudioBin storage, avoiding another always-present waveform ring.
- Verify old serialization/default parity; route switching/refractory reset;
  conflicting main/sidechain impulses; missing/mono/short buffers; original input
  preservation; exact offsets under host/grid/MIDI partitions; inactive/bypass
  behavior; filter/threshold/telemetry identity; pending processor and SR/reset
  behavior; main output/Mix/PDC parity; no-allocation guards; UI modal priority,
  numeric auto-commit/undo/reset and source-aware freeze at 0.5x/1x/2x.
- Render actual sidechain-driven DSP, update manuals, run the six-crate focused
  suite, workspace Clippy, format/whitespace checks, and independent read-only
  reviews. Host-specific DAW bus routing remains a manual integration check.
- Leave the sidechain slice uncommitted unless separately requested.
