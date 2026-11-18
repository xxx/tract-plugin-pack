# Curve-library categories implementation

Spec: `../specs/2026-11-16-mseg-categories-design.md`.

1. Complete: compatible category metadata and locked atomic updates with
   version-aware migration, stale-record protection and unchanged legacy files.
2. Complete: factory groups, cached filtered views and category selectors use the same
   mapping for navigation/cards/recall/paging and preserve stable IDs on refresh.
3. Complete: Save/Rename category editing/assignment, popup ownership, Unicode
   and scaled/minimum cleanup preserve Delete-name scrolling and drawing.
4. Complete: guide/manuals/example and native integration/offscreen previews.
5. Complete: all 2,074 six-crate tests, workspace Clippy and fmt/diff pass, including
   the minimum-layout regressions. Independent storage/UI reviews are clear.
   The user subsequently requested the compact-popup review fix and commit.

## Verification

- Full check: locked/offline nextest for tiny-skia-widgets, tract-effects,
  minitosis, multosis, miff and nap reports 2,074 passed. Workspace Clippy with
  `-D warnings`, formatting and whitespace checks pass.
- Five storage tests cover untouched version-1 reads/legacy wire output,
  permanent version-2 upgrades, metadata-only edits, global name uniqueness,
  category-stale actions and invalid version/metadata byte preservation. All
  twelve shared storage tests pass.
- Fourteen category UI tests cover factory mappings, sparse filtered pages/cards,
  recall/double-click, ID/sort/recategorization, empty filters, inherited category,
  special/custom label distinction, Unicode/Option input, child chooser priority,
  cached browsing, mouse form admission and readable compact headers/popups. All 50
  browser tests pass, including the prior long Delete-name raster regressions.
- The shared native test at 0.5x/1x/2x confirms filtering and metadata edits retain
  effect/modulation/frozen state until actual recall, which reports one Changed.
- Red minimum-layout probes caught a mouse-only hidden Save/Rename form and a
  clipped compact caption at 1x/180 px. Mouse actions now run the same form-space
  validation as keyboard entry; header admission reserves caption and chevron
  space while retaining 0.5x/124 px Factory/Project access.
- The post-review compact User regression reproduced a 54-pixel popup in a
  320x720 window. Popups now reserve font/label width independently of the
  trigger, capped to the full window; row height and the trigger stay unchanged.
  Two red-before-fix regressions verify complete labels and mouse hit testing at
  0.5x/1x/2x plus clamping in the admitted 124-pixel Factory layout.
  Independent public probes confirm full known labels at 320x720, selection past
  the original trigger edge and unchanged trigger/row geometry. No further
  findings; evidence: `/tmp/mseg-category-popup-width-independent.log`.
- Independent storage probes pass v1/v2 migration, permanent upgrades, old API
  wrappers, category-only ID/shape/next-ID preservation, full-record guards,
  exact-byte failure paths, four processes committing 32 category updates and a
  full 256-by-128-node categorized bank. Prior lock/death/collision probes remain
  green. Evidence: `/tmp/tract-category-migration-review.log` and
  `/tmp/tract-user-presets-category-storage-review.log`.
- Independent UI probes pass filtered actions/paging, stable IDs, typed special/
  custom groups, inherited categories, stale records, child-popup input/submit
  ownership and Unicode/Option entry. Long 64-character fields/choosers render
  and save exactly at 0.5x/1x/2x; small 1x/2x backing layouts retain browsing/
  Delete and reject unusable forms before writes. Native routing and frozen
  integration are consistent; no remaining actionable review findings.
- Actual previews were rendered and inspected: `/tmp/mseg-category-user.png`,
  `/tmp/mseg-category-filter.png`, `/tmp/mseg-category-save-half.png` and
  `/tmp/mseg-category-assign-half.png`. Native/HiDPI integration tests and prior
  minimum-layout tests pass. DSP, host IDs, Cargo.lock and dependencies are
  unchanged. Cargo still reports existing unused `atomic_float` manifest entries.

Category writes use the new User-bank version; older builds reject it instead of
discarding metadata. Plain uncategorized version-1 banks remain compatible. Search
is the next comparison slice.
