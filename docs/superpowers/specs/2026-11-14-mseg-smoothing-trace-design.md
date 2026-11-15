# MSEG smoothing and measured trace

Date: 2026-11-14. Scope authorized after the native-shape slice; continue on
`feat/mseg-power-tools`. Reuse existing controls; no mock supplied.

## Behavior

1. Multosis/Minitosis offer a per-envelope Smooth time, 0 (Off) to 1000 ms,
   persisted per row/effect. Missing settings default to Off. Off retains the
   current phase evaluation, parameter writes, Amp behavior, and Mix ramp.
2. Positive Smooth filters the normalized MSEG output before Depth/Target mapping.
   It applies to Amp, Mix, continuous targets, and unrouted source previews.
   Selector targets bypass it and dim the control while retaining its setting.
3. Smoothed sources use a persistent 1 kHz control clock, with fractional sample
   remainder, independent of host block/sequencer segment boundaries. Lag runs
   per sample; Amp/Mix receive per-sample values. Effect parameter writes are
   limited to control ticks or required route/retrigger initialization.
4. Last assignable slot wins per target. Hidden sources still run, but cannot
   overwrite the winning source. Base parameter bridging cannot stomp a held
   smoothed target between ticks. Inactive/None/bypassed paths retain clocks.
5. Retriggers/wraps restart source evaluation while retaining lag. Explicit Reset
   and DSP flush clear lag seeding; DSP flush preserves existing phase/transport
   history. Bypass continues smoothing behind dry output. Turning Smooth Off
   immediately restores the legacy path. Retargeting initializes its new held
   destination, and removing a route restores its base through existing bridging.
6. Existing Free Hz/Transient detection remains block-aligned. Buffer-partition
   invariance applies to free-running smoothing and identical explicit trigger
   events, not a redesign of those legacy detectors. Sample-rate changes update
   lag coefficients/control timing while retaining valid phases.
7. Trace shows actual DSP MSEG output before Depth/Target mapping. It reflects
   the real source clock (including legacy held segment phases when Off).
   Dial arcs use actual delivered target values, including the consumed Mix
   ramp; the GUI does not simulate smoothing from phase.
8. Trace is a bounded measured history, enabled per editor view. It clears on
   row/slot/effect changes, reorder, or replacement, respects zoom/pan/clipping,
   and hides while a newly selected Minitosis processor is still preparing.
   Closing the native editor releases the request; audio never waits for GUI.
9. Smooth supports drag/fine/reset/right-click numeric entry and local undo;
   trace toggling is view-only. Old shape clipboard files remain geometry-only.

## Implementation and verification

- Add a default-zero four-value bank to TrackModulation, not MsegData. Share a
  fixed-capacity smoothing runtime across both engines, with f64 phase/lag state
  and cached exponential coefficients. Source sampling and expensive mappings
  occur on the control clock. Keep the all-Off implementation intact.
- Publish one requested row snapshot and a 512-cell trace ring through atomics.
  Sequence stamps bracket Release payload stores; readers use Acquire loads
  and skip incomplete/overwritten records. Request epochs reject stale histories.
  GUI phase bins retain last observed source values; arcs use target snapshots.
- Request-aware publication/recording uses preallocated storage. No allocations,
  blocking locks, effect construction, or unbounded retries on the audio thread.
- Validate Off parity, block partitions at 44.1/48/96 kHz, time constants,
  selectors, overlapping routes, inactive rows, routing edits, trigger/wrap/reset/
  bypass behavior, allocation guards, telemetry accuracy/overrun/staleness, UI
  focus/undo, viewport clipping, and offscreen rendering. Run focused suites,
  workspace Clippy/format, and independent reviews. Leave changes uncommitted.
