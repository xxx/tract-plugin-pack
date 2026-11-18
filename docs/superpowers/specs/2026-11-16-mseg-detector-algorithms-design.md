# Transient detector algorithms and detail

Continue `feat/mseg-power-tools` after `e38953b`. The user authorized the next
comparison slice. Extend the existing CPU editor; no mock supplied. Reference:
ShaperBox 3 manual, Audio Trigger Setup Algorithm/Detail, page 44:
https://downloads.cableguys.com/Cableguys-ShaperBox-3-Manual.pdf
These are Tract's own causal analysis modes.

## Behavior

1. Multosis/Minitosis Detect... adds **Algorithm** (Envelope / Peak rise /
   RMS rise) and **Detail** (0–100%, default 50%). Settings apply only to
   Transient (sample). Legacy block-aligned Transient remains unchanged.
2. **Envelope** at Detail 50% preserves the existing fast/slow rectified-envelope
   detector bit for bit, including event offsets, noise floor, Hold and reset.
   Detail scales its 0.5/5 ms fast attack/release and 30/200 ms slow attack/release
   windows by `2^((50-detail)/25)` (4x at 0%, 1x at 50%, 0.25x at 100%).
3. **Peak rise** compares the current absolute filtered sample to the previous
   decaying peak reference multiplied by the existing Sens threshold ratio.
   Its reference attacks immediately and decays with a 20 ms time constant at
   Detail 50%, scaled by the same Detail factor. It suits distinct sharp hits;
   higher Detail lets the reference recover between faster repeated hits.
4. **RMS rise** compares exponentially averaged fast and slow power, using 2 ms
   and 200 ms windows at Detail 50%, scaled by the same Detail factor. Sens compares
   their RMS amplitudes (equivalently power ratio against squared Sens). It
   responds to energy increases rather than individual sample peaks.
5. All algorithms analyze the existing filtered mono Main/Sidechain input, use
   the absolute Threshold before firing, and share Hold as the minimum interval
   between triggers. Peak and RMS retain a -60 dB amplitude noise floor. Their
   finite analysis input is bounded to +/-64; invalid samples feed silence.
   The audible signal and the independent Listen path remain unchanged.
6. Analysis advances exactly once per consumed sample and retriggers at that
   sample's existing boundary path. Muted/inactive/None rows and Minitosis bypass
   keep tracking. Host/grid/MIDI partitioning does not change results. There is
   no FFT, lookahead, added latency or audio-thread allocation.
7. Detail edits preserve analysis history and remaining Hold. Algorithm changes
   reset analysis history while preserving remaining Hold; they do not reset
   curves directly. Input changes retain their existing fresh-detector behavior.
   Sample-rate handoff retains history and rescales Hold while rebuilding cached
   coefficients. Response edits while inactive take effect when re-enabled.
   Empty blocks defer Algorithm/Detail changes and consume no analysis samples.
8. Settings save per row/effect and use local modulation undo. Old presets default
   to Envelope/50%; default response settings are omitted from serialized data.
   Detail clamps to 0–100, nonfinite values normalize to 50. No host parameter IDs
   or automation ordering change; Miff/Nap retain static editing.
9. The detector meter reflects the prepared algorithm's actual amplitude level:
   fast envelope for Envelope, current peak for Peak rise, fast RMS for RMS rise.
   Exact prepared Algorithm/Detail and filter settings validate meter ownership;
   stale readings are unavailable until the audio thread catches up.
10. Algorithm is a stepped dropdown with keyboard/wheel navigation and final
    popup/input priority. Detail supports fine drag, double-click default and
    right-click numeric percent entry; Enter commits, Escape cancels, and outside
    press/drag/focus loss auto-commit. Numeric fields/menus keep L priority.
    Changing these controls keeps Listen active without modifying its signature.
11. Reset restores Envelope/50% and default threshold/filters, preserving Input as
    before. Close/focus/resize/source/preset cleanup owns all new interactions.
    Controls remain bounded at 0.5x/1x/2x and narrow windows. Hints, guide and
    manuals explain the algorithms, Detail windows and unchanged Sens/Hold roles.
    Windows that cannot fit readable controls show an enlarge-window message
    instead; no parameter controls or Listen are active there. Click/Escape closes
    that message, and entering it cancels old detector interactions/monitoring.

## Technical approach

- Add normalized `TransientSettings { algorithm, detail }` as an optional/default-
  omitted TrackModulation field. Keep AudioTriggerSettings shared with the follower
  and audition, which do not use onset algorithms.
- Reuse PreciseTrigger's fixed-size detector state. Cache response coefficients;
  retain the legacy default path, add peak-reference and power analysis, and
  preserve refractory samples across response edits and SR handoff. RMS square
  roots are only needed for measured telemetry, not per-sample detection.
- Extend DetectorReading and bounded atomic telemetry with prepared response.
  Add Algorithm dropdown and Detail action inside the existing modal owner;
  validate response along with existing source/filter bindings. Leave native
  MIDI/grid segment machinery and independent audition processing unchanged.
- Implement locally; independent read-only DSP/native and UI reviews per AGENTS.md.

## Validation

- Legacy exact parity, peak-roll Detail behavior, RMS energy/level reference,
  gate/noise/Hold timing, extreme inputs and coefficient lifecycle transitions.
- Native Main/aux/mono, host/grid/MIDI partitioning, bypass/inactive/None, zero
  frames, reset/SR inheritance, actual telemetry and allocation guards.
- Serde/undo/recall, prepared meter ownership, dropdown/numeric/drag priority,
  Listen signature/history parity, cleanup and scaled offscreen renders.
- Six-crate nextest, workspace Clippy, format/whitespace and independent reviews.
  Keep this slice uncommitted until separately requested.
