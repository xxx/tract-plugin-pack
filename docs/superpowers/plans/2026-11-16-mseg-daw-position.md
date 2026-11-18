# DAW-position playback implementation

Spec: `../specs/2026-11-16-mseg-daw-position-design.md`.

1. Add tested host-position/loop math and continuous clock state. Cover transport
   discontinuities, zero-frame deferral and malformed/missing input first.
2. Integrate absolute phases with the shared per-sample source/lag/control path;
   preserve legacy behavior, wave arbitration, Mix continuity and enum routing.
3. Bridge native transports in both plugins, including ready-processor adoption,
   fixed row clocks and mid-buffer grid/MIDI segments. Extend waveform epochs and
   telemetry mode validation. Exercise native seams under allocation guards.
4. Append the shared Trigger choice, update contextual help/manuals and render a
   real position-synced preview. Test keyboard/dropdown/undo and scaled bounds.
5. Run the focused/full gates and independent read-only DSP/native/UI reviews;
   fix findings with regressions and commit when requested.

## Verification

- Native/shared tests cover sample-zero phase, arbitrary host/grid partitions,
  seeks, stopped/missing domains, metadata/hold edits, smoothing/Mix/control
  cadence, enum bypass, reset, bypass, sample-rate handoff and allocation guards.
  UI tests cover scaled choice selection, editable geometry, undo, measured
  playheads and old-clock telemetry rejection.
- Independent review reproduced two clock edges and a trace race before fixes:
  loop returns exactly at a host boundary needed their pending domain epoch;
  missing tempo could otherwise wrap a stationary Beat clock at its loop end;
  trace requests now include clock mode so old-mode points are rejected as well
  as old-mode frames. Permanent regressions cover all three. Reviews are clear.
- Review follow-up covers transport restarts with missing/partial positions,
  simultaneous trigger-mode changes, and peak preservation through OneShot intros,
  negative pre-roll, short loops, host splits and tempo changes. Restart resolves
  current installed configuration; DAW capture epochs use actual curve iterations.
  Six permanent regressions reproduce the failures before their fixes; independent
  allocation-guarded probes pass 33,280 native waveform samples. Large Mix test
  fixtures now live on the heap to stay within the debug runner's stack limit.
- **1,922 focused tests passed** against merged NIH-plug revision `32f72ccc`,
  using `--locked --offline`. Workspace Clippy with `-D warnings`, formatting
  and whitespace checks passed. Only the NIH-plug/derive source revisions changed
  in Cargo.lock; other dependency choices remain unchanged. The compact **DAW**
  caption stays readable at .5x.
- The separate NIH-plug correction was merged in
  [fork PR #2](https://github.com/xxx/nih-plug/pull/2), revision
  `32f72ccc8194cb79ada455be6d0b8753ed638c6a`. Both wrappers use a shared transport projection:
  stopped/origin coordinates remain authoritative, known-rate positions and
  active loops advance coherently, bar origins survive, and CLAP offsets are
  relative to the latest transport event. **69 NIH core tests passed** using
  `/tmp/tract-nih-transport-tests/Cargo.toml`; this isolated manifest avoids
  resolving unrelated offline GUI workspace dependencies. Independent fork
  probes also cover all 256 loop partitions and partial-domain accessors.
- `mseg_position` renders real position-driven playback/trace/capture. Inspected
  `/tmp/mseg-daw-position.png`; independent UI probes rendered .5x/2x menus and
  stopped playback, including Minitosis's filtered seven-entry Trigger menu.
- The user requested committing this slice after the review corrections. Real
  DAW transport remains a manual host check; native accessors can synthesize
  missing domains with current tempo.
