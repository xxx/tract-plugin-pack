# Audio-trigger lookahead and signed shift

Continue `feat/mseg-power-tools` after `efc5e6c`. The user authorized the next
comparison item. Extend the CPU editor; no mock supplied. Reference: ShaperBox 3
manual, Audio Trigger Setup Trigger Shift and latency/lookahead sections:
https://downloads.cableguys.com/Cableguys-ShaperBox-3-Manual.pdf

## Behavior

1. Timing... adds **Lookahead: Off / 4 ms**. It is effective for Transient
   (sample) only and defaults Off. The existing Audio delay control becomes
   **Audio shift**: 0..+4 ms with Lookahead Off, -4..+4 ms with it On. The old
   audio_delay_ms field retains its wire name and meaning for legacy positive
   settings. Turning Lookahead Off clamps a saved negative shift to zero.
2. An effective lookahead delays that row/effect's input by 4 ms rounded up to
   whole samples. The transient detector still analyzes live original Main/aux
   before the delay. Its accepted cues are scheduled after the lookahead path
   plus the signed shift rounded to samples. Negative shifts therefore restart
   curves earlier relative to the detected cue on delayed input, without
   predicting input; the algorithm's own causal detection lag remains.
   All four curves reset together and retain Retrigger blend/Smooth behavior.
3. In Multosis, unmuted lookahead rows delay the through-path even while inactive
   or None, so sequencer grid changes do not alter lookahead latency. Muting or
   excluding a row via Solo bypasses its lookahead. Audio-trigger cue compensation
   includes upstream lookahead windows, preserving each audio row's signed shift
   relative to the added lookahead path and original Main/aux detection. Base
   effect latency retains its existing semantics. Other trigger sources and MIDI
   wave selection keep their host-clock
   offsets, as with existing latency-adding effects.
   In particular, existing upstream/own FFT latency is not compensated by the
   signed shift; it keeps its prior relationship to the modulation timeline.
4. Minitosis dry/Mix/bypass align to effect latency plus lookahead. Multosis's
   final dry path receives the total lookahead delay before global Mix, while
   retaining existing effect-latency behavior. Output gain remains post-mix.
   Host latency reporting includes the added samples (sum across unmuted rows)
   and is independent of active grid cells. No new host parameter IDs are added.
   While its adopted path contains lookahead, Multosis reports a finite tail equal
   to its full declared latency, protecting queued audio and E+L Listen during
   silent callbacks. CLAP therefore continues processing while lookahead is
   enabled; the all-Off path retains Normal suspension behavior.
5. Follower Main/aux use original input delayed by the lookahead path to the row.
   Capture Input remains that row's delayed series input; Sidechain capture uses
   delayed original aux, and Output remains the actual row result.
   Actual phases/traces/arcs remain measured. DAW/MIDI/free clocks retain their
   existing process-time definitions. Listen remains independent and aligns its
   live detector input to the new declared total latency; changing latency clears
   its old queue/request. Frozen captures remain available during ordinary edits.
6. Lookahead window/topology changes clear queued audio and pending reset cues
   before adopting fresh data; zeros fill the audio delay during warmup. Changing
   detector input clears its pending cues without flushing the audio delay.
   Processing remains
   allocation-free, including inactive/None/bypass, MIDI collisions and FFT-size
   latency changes. Raw input and aux buffers are read-only for detection.
   Host reset clears delay history; explicit Reset/transport retain their clock
   semantics and clear pending cues. SR setup preallocates buffers; same-effect
   SR handoff preserves modulation history while audio queues start fresh.
7. Empty calls do not consume audio/detector/delay history or adopt new lookahead
   paths. Explicit reset/transport commands retain their existing exception.
   Explicit transport restart may adopt the current path before restarting its
   clocks even on an empty callback; ordinary empty callbacks never adopt it.
   Legacy Lookahead Off/positive shift/default timing retains exact audio,
   detector events, state serialization and automation order.
8. The toggle and signed entry save per row/effect and participate in local undo.
   Numeric input commits before toggling; bounds/fine drags/defaults adapt to
   signed shift. Modal/source/preset/resize cleanup and scaled/HiDPI enlargement
   remain safe. The dialog and manuals state the added latency and audio-only
   scope; static Miff/Nap remain unchanged.

## Technical approach

- Add a default-omitted TrackModulation lookahead bool and normalize the timing
  shift range with that preference.
  A shared preallocated LookaheadDelay carries stereo series input, original
  Main and aux, with exact zero-delay direct branches and bounded warmup.
- Configure per-row lookahead windows/path compensation before block preparation.
  Extend the fixed timing ring with externally supplied window identity and
  derive the delay at the current sample rate; rescale on SR changes and clear
  on path changes. Runtime continuous
  masks cover compensated audio rows even when their local timing prefs are Off.
- Minitosis delays effect/follower/capture input while its dry buffer uses total
  latency. Multi processes source/follower updates per row after its input delay
  on the opt-in path, carrying delayed original Main/aux through the chain and
  using the final delayed original Main for global dry Mix. Retain the old path
  literally when every lookahead is Off.
- Grow audio/audition reserves only during construction/SR setup. Fixed 4 ms
  windows cap pathological rates at 4096 samples; the existing 65,536-sample cue
  ring covers conventional audio rates through 768 kHz with all 16 windows.
  Audition reserve includes the maximum effect plus lookahead latency.
- Independent read-only DSP/native and UI reviews per AGENTS.md; implementation
  stays local.

## Validation

- Exact delay/warmup/zero branches, negative/zero/positive cue shifts, source/input/
  mute/solo/None/inactive changes, paths/SR/reset/empty handling and bounds.
- Native input/output impulses, dry/Mix/bypass/master gain, actual host latency,
  downstream audio-row cue compensation, follower/capture/Listen alignment,
  MIDI-wave/grid/host partitions, dynamic FFT latency and allocation guards.
- Legacy wire/audio/IDs, signed numeric/toggle/undo/cleanup, frozen canvas and
  real scaled offscreen preview. Six-crate nextest, workspace Clippy, fmt/diff,
  independent reviews. Leave this new slice uncommitted until requested.
