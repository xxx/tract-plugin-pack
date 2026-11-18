# Audio trigger detector controls

Continue `feat/mseg-power-tools` after sample-timed trigger commit `68e7a33`.
The user authorized the next slice. Reuse the existing CPU UI (no mock supplied).
Reference: [ShaperBox Audio Trigger Setup](https://downloads.cableguys.com/Cableguys-ShaperBox-3-Manual.pdf).

## Behavior

1. Transient (sample) gains a Detect panel with Threshold, High-pass, Low-pass,
   Reset, and a live detector-level meter. Settings save per row/effect and share
   local undo. They are separate from MsegData/shape files. Legacy Transient and
   every other source ignore them. Missing settings default to all Off, retaining
   previous serialized state and sample-timed detection exactly.
2. Threshold is Off or -60..0 dB, applied to the filtered fast detector envelope.
   Envelope/refractory time always advances, including below the threshold;
   rejected quiet candidates do not arm Hold and block a subsequent loud hit.
   Existing Sens ratio, noise floor, attack/release, and Hold still apply.
3. High-pass/Low-pass are independently Off or 20..20000 Hz, using causal one-pole
   bilinear filters only in the detector path. Coefficients respect sample
   rate/Nyquist and retain the labeled -3 dB corner for every valid cutoff below
   Nyquist. Values at or above Nyquist use explicit unity low-pass/zero high-pass
   limits. Bounded input/output history prevents a hidden state spike on retuning.
   Both may be enabled, including crossed cutoffs (a more attenuated band).
   They never filter the audible signal or add audio latency/lookahead. Off
   bypasses their arithmetic. Enabling seeds from the preceding input; cutoff/SR
   edits retain input/output history. Reset/family changes clear private history.
4. The meter shows the actual DSP fast-envelope level after detector filtering,
   with the configured threshold marker; it does not simulate a detector in GUI.
   Pending/unavailable processor measurements show no invented level. Ordinary
   audio backdrops retain their existing input/output meaning.
5. Sliders support fine drag, double-click Off, and right-click numeric entry
   (including Off, dB, Hz/kHz). Enter commits, Escape cancels a field, and outside
   click/drag start/focus loss auto-commit. Reset restores all three Off as one
   undo action. A subsequent Escape/outside click closes the panel. Popup input
   and rendering take priority over underlying dials/curves/host controls.
6. Closing/resizing/replacing the view ends gestures and closes the panel while
   retaining saved settings and frozen/audio-view preferences. Switching rows
   or effects cannot leave edits applied to an old binding. Other MSEG tools,
   trigger timing, MIDI filtering, latency, and automation IDs remain intact.

## Implementation and verification

- Add default-skipped AudioTriggerSettings to TrackModulation and its legacy
  deserializer. Normalize malformed/non-finite settings in runtime/GUI; store
  options explicitly so Off is distinct from a real cutoff/threshold.
- Extend precise audio runtime with fixed filter history/coefficient caches and
  cached linear level floor. Preserve the old detector entry point by passing
  an Off floor, and gate before arming refractory. No audio allocations, locks,
  effect construction, per-sample coefficient recalculation, or event caps.
- Add optional detector level to coherent request-aware ModulationFrame payloads
  under the existing Release/Acquire stamps, including actual normalized settings
  identity. Reject incompatible cached readings and distinguish measured zero
  from unavailable data. Publish only actual state.
- Reuse shared slider/drag/text machinery inside a bounded modal overlay, drawn
  in the final popup pass and sized against the native window. Bind transient
  state/undo in the existing native adapters; static Miff/Nap stay unchanged.
- Track panel ownership by kind, source parameters, settings, row, telemetry owner
  and generation. Finalize focus/Drop before releasing native bindings. Scroll
  the visible text tail within its edit box while keeping the full bounded
  numeric buffer for parsing. Reserve a separate Off detent from valid minima.
- Test default/legacy parity and state round trips, quiet rejection then loud
  acceptance, filter response/DC continuity and SR/reset/family transitions,
  meter coherence/absence, native sample offsets/Mix/latency/allocation guards,
  panel numeric/drag/reset/undo, focus/resize/source ownership, popup rendering
  and input at 0.5x/1x/2x. Render actual filtered detector DSP offscreen.
- Run the six-crate focused suite, workspace Clippy, formatting/whitespace, and
  independent read-only reviews. Leave this slice uncommitted unless requested.
