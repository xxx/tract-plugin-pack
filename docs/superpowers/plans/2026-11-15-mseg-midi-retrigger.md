# MSEG MIDI retrigger implementation

- [x] Commit reviewed stereo views and record MIDI source behavior.
- [x] Add MIDI source/filter state and tested streaming event validation.
- [x] Merge MIDI boundaries into Multosis and split Minitosis preparation/segments.
- [x] Enable native MIDI input and verify offsets, parity, and allocation guards.
- [x] Add Note/Channel selectors with undo/recall and popup/focus coverage.
- [x] Update manuals and render an offscreen MIDI-source preview.
- [x] Resolve independent reviews; run focused tests and lint/format checks.

Verification:

- 1,707 tests passed across widgets, shared effects, Multosis, Minitosis, Miff,
  and Nap, including allocation guards, dense events, offset/filter/bypass tests,
  pending/adopted effect filters, and unaffected-row audio/clock/scope parity.
- Workspace Clippy (`--offline -- -D warnings`), formatting, and whitespace pass.
- `cargo run --offline -p tract-effects --example mseg_audio -- /tmp/mseg-midi.png stereo 0 midi`
  renders actual MIDI-triggered DSP, note/channel controls, and measured scopes;
  the preview was inspected.
- Independent core/native and UI/telemetry reviews are clear. Regressions cover
  interrupted Mix ramps, inactive-row activation at host/grid span starts,
  meaningful wet/dry comparisons, scrollbar-aware popup width, and shared/native
  focus cleanup with viewport/frozen history preservation and local undo capture.

Native/DAW MIDI routing and listening checks remain manual.
