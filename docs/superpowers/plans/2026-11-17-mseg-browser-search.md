# Curve-browser search implementation

Spec: `../specs/2026-11-17-mseg-browser-search-design.md`.

1. Complete: browser-local query matching/state and configurable shared text
   limits without changing Name/Category behavior or any persistence format.
2. Complete: cached result mapping, stable IDs and recall/actions/refresh/edit
   semantics with category filters and empty results.
3. Complete: Find/query/Clear/X UI, keyboard/mouse focus and nested-layer priority,
   admission checks and scaled/HiDPI drawing/cleanup.
4. Complete: help/manuals/example, native integration and actual previews.
5. Complete: all 2,092 focused tests, workspace Clippy and fmt/diff pass.
   Independent state/UI reviews are clear. The user approved committing after a
   clean review of the final slice.

## Verification

- Locked/offline nextest passes all 2,092 tests across tiny-skia-widgets,
  tract-effects, minitosis, multosis, miff and nap. Workspace Clippy with
  `-D warnings`, formatting and whitespace checks pass. Cargo still reports
  existing unused `atomic_float` manifest entries in Miff/Wavetable Filter.
- Sixteen search tests cover literal every-term name/category/Unicode matching,
  category intersections, sparse source mappings, paging/double-click/Load/Fit,
  disabled empty results, query retention/selective metadata reveal, nested
  Escape/focus/shortcut ownership, Option/caret/256-character resets, cached I/O,
  authored replacement and mouse/keyboard minimum-layout admission.
- Native tests at 0.5x/1x/2x retain effect/modulation/frozen state and library
  bytes while typing; Enter reports one actual selected-curve recall. A native
  source-replacement test prevents search Enter from overwriting the new curve.
- A red close-row regression showed a double-click loading a card reflowing
  beneath X. Follow-up clicks on that control are consumed through the existing
  double-click interval, preserving the document and selected record.
- Independent input review caught modified Enter recalling a curve. A red
  Ctrl/Cmd/Alt(+Shift) matrix now passes after non-text command guards precede
  search dispatch. Ordinary/Shift Enter, Ctrl/Cmd+A and printable Option input
  retain their intended behavior.
- Independent UI probes pass Unicode/Option/slash/caret/256-limit, Tab/Escape
  layering, source cleanup, native routing and .5/1/2/narrow/HiDPI layouts.
  Evidence: `/tmp/mseg-search-independent-command-fixed.log` and
  `/tmp/mseg-search-independent-input-fixed.log`.
- Independent state/action probes pass 144 Load/Fit mappings, category
  intersections, sparse-page double-clicks, stale records, query retention/reveal,
  limit resets and authored replacement. Raster comparisons with unavailable
  storage confirm cached browsing; library, Favorites and clipboard bytes stay
  unchanged. Evidence: `/tmp/tract-browser-search-state-review.log`. No remaining
  actionable state or UI findings.
- Actual examples render `/tmp/mseg-search-user.png`, `/tmp/mseg-search-half.png`,
  `/tmp/mseg-search-empty.png` and `/tmp/mseg-search-double.png`. Matching and
  empty results, controls and thumbnail space were visually inspected.
- No storage schema/version, host IDs, Cargo.lock/dependencies, DSP or latency
  changes. Search resets when the browser/tab/context closes and remains GUI-only.

This implements the final feature from the original comparison list; the user
approved committing the reviewed changes.
