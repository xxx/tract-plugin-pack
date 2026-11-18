# Detector input audition

Continue `feat/mseg-power-tools` after `ef360b0`. The user authorized the next
comparison slice. Add filtered-input audition before further detector algorithms.
Extend the existing CPU editor; no mock supplied. Reference: ShaperBox 3 manual,
Audio Trigger Setup's Monitor Input and Envelope Follower filtering:
https://downloads.cableguys.com/Cableguys-ShaperBox-3-Manual.pdf

## Behavior

1. Multosis/Minitosis add **Listen** to the Audio detector and Follower panels.
   Clicking toggles a temporary monitor of that panel's selected Main/Sidechain
   input. The button visibly indicates the request. L toggles it while the panel
   has keyboard focus outside numeric edits, dropdowns and active drags.
2. Trigger audition folds stereo to mono before the same HP/LP filter law used
   by Transient (sample), then sends it to both outputs. Follower audition keeps
   stereo, filters channels separately and applies shared Gain. It works with
   follower routes Off. Threshold, Attack/Hold/Release and curve routing do not
   gate/shape what is heard; this exposes the input used for detection.
3. Monitor filters use independent history and advance once per actual sample.
   Listening never changes detector/follower history, retriggers, MSEG phases,
   smoothing, parameter cadence, Mix, waveform capture or normal effect state.
   Ordinary traces/backdrops retain their existing measurements. Missing aux is
   silence and mono aux mirrors as usual. Invalid/extreme detector inputs remain
   bounded; the monitor is limited to +/-1 before the existing Output gain.
4. Listen temporarily replaces the final effect/mix/compressor output, retaining
   master Output gain. Minitosis bypass/pending different-effect preparation
   takes priority and returns its ordinary dry output. Multosis can audition a
   selected muted/inactive/None row because detector input is original main/aux.
   Current plugin latency and reporting stay unchanged; monitor samples are
   delayed by that latency. A 5 ms linear fade enters/exits after delay warmup.
   Off with no pending fade preserves ordinary audio bit for bit.
5. Input/source/latency changes discard queued monitor samples before fading in
   fresh data; ordinary audio plays during this reset/warmup. Filter/Gain edits update while listening, retaining bounded filter
   history. Disabling Listen returns smoothly to the normally running processor.
   Bypass immediately suppresses monitoring and re-entry fades from dry.
6. Only one panel/row/effect can listen in an instance. Closing the panel, outside
   dismissal, focus loss, resize, row/effect/source/preset replacement, disconnect
   or editor destruction releases its request. Old owners cannot stop a newer
   request. Invalidated requests never resume by themselves; re-open/re-enable
   is explicit. Audio independently validates kind, source/settings and state
   generation, including while the GUI is not drawing.
   Filter/Gain mismatches suspend monitoring while a pending GUI configuration is
   adopted; they do not revoke its lease. Kind/source/generation mismatches revoke
   it. A publication in progress defers monitoring until the next bounded snapshot.
   Undo/redo/row swaps release the GUI lease, and native host-state initialize/reset
   invalidates it even when the editor is not drawing.
7. Listen is editor-only: no saved fields, host parameter IDs, automation writes,
   undo/bake steps, added latency or audio-thread allocation. Numeric outside-click
   commits retain their normal behavior. Empty blocks consume no monitor history.
   Host DSP reset clears monitor history and the temporary request.
   Rapid off/on for the same input identity reverses the current fade without
   restarting its queue. During bypass, enabled monitor filters keep consuming
   input while their audible blend is zero and their delay queue is cleared.
8. Both panels keep bounded usable Listen/Reset controls at 0.5x/1x/2x and small
   windows, with existing modal keyboard/pointer priority. Static Miff/Nap remain
   unchanged. Disconnected editors report unavailable monitoring without pretending
   it is active. Guide/manuals explain temporary ownership and pre-envelope sound.

## Technical approach

- Add a bounded atomic audition request to ModulationTelemetry: owner token,
  row/kind/source, normalized filters/gain and waveform generation. GUI publication
  uses a single-writer stamp; audio takes one bounded snapshot per block and a
  cheap live-token check per sample. Release uses token compare-and-swap.
- A shared preallocated monitor runtime reuses AudioFilter, with stereo history,
  latency ring, warmup and fade. Native construction supplies a fixed maximum
  latency (4096 in Minitosis, 16x4096 in Multosis). No actual detector is re-run.
- Native bridges validate a request against current row/effect modulation and
  generation before preparing. Process monitor replacement after normal DSP and
  waveform/trace capture, before master Output gain. Continue normal DSP throughout.
- EffectEditor's live source owns its temporary token. Common cleanup stops it;
  panel-local edits refresh normalized request settings and closing bindings stop
  stale owners. State invalidation also clears the atomic request.
- Independent read-only DSP/native and UI reviews per AGENTS.md; local implementation.

## Validation

- Filter responses/routing, trigger mono/follower stereo, gain/pre-threshold,
  disabled follower, invalid input, sidechain, request ownership/races and old state.
- Fade/warmup, source/latency changes, bypass/reset, host/grid/MIDI partitions,
  effect/detector/history parity, actual PDC impulses and allocation guards.
- Panel numeric/L/popup priority, cleanup/replacement/drop, no undo/state edits,
  actual native bindings and offscreen layout checks at multiple sizes.
- Six-crate focused nextest, workspace Clippy, format/whitespace and independent
  reviews. Keep this new slice uncommitted until separately requested.
