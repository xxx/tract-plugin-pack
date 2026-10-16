# MSEG Mix Modulation

- **Date:** 2026-10-16
- **Scope:** Multosis and Minitosis; Mix only.

## Behavior

1. Each assignable MSEG can target **Mix**, listed after the effect parameters. Multosis targets the selected row's Mix; Minitosis targets its global Mix. Depth controls remain outside the target list.
2. Modulation uses the existing bipolar/unipolar and signed-depth rules around the current base Mix, clamped to 0–100%. A unipolar MSEG with base Mix 0% and Depth +100% follows the curve across the full dry/wet range.
3. The base Mix and its host automation value remain unchanged. The Mix dial shows a colored modulation arc, and routed node tooltips show percentages. Multiple assignments retain the existing last-slot-wins rule.
4. Mix values ramp between segment endpoints to avoid abrupt block-boundary blend changes. Bypass and pending Minitosis effect switches remain latency-aligned dry; Amp and transport behavior retain their current semantics.
5. Existing projects retain their parameter assignments. Parameter targets retain their numeric serialized representation; Mix is serialized as `"mix"`. Mix assignments survive effect-kind changes, including None, whose audio remains passthrough.
6. The target metadata, evaluation, and smoothing live in `tract-effects` and allocate no memory during processing. Both engines consume the resolved blend without changing effect algorithms.

## Validation

- State compatibility, Mix target round trips, kind switches, polarity, signed depth, clamping, and assignment precedence.
- Sample-by-sample ramp continuity, including unassigning Mix and inactive Multosis rows.
- Audio-output tests for both plugins; unchanged automated base values and Minitosis bypass/pending behavior.
- Shared editor selection, Mix arc and tooltip checks; native GUI verification and updated Markdown/PDF manuals.
- Workspace tests, Clippy, formatting, and comparative engine benchmarks.
