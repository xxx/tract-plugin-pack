# Named user preset implementation

Spec: `../specs/2026-11-16-mseg-user-presets-design.md`.

1. Complete: bounded named-preset storage with stable identities, atomic locked
   mutations, stale-record guards and compatibility with existing curve files.
2. Complete: User browser snapshots, pagination, recall, Save/Rename/Delete dialogs,
   keyboard routing and cleanup without changing authored curves or old banks.
3. Complete: shared/native integration, small/scaled/HiDPI layouts and actual
   browser renders. Guide/manuals and the browser example are updated.
4. Complete: six-crate tests, workspace Clippy and formatting/whitespace pass.
   Independent storage/UI reviews are clear after the regression fixes. The user
   subsequently requested the Option-input review fix and commit of this slice.

## Verification

- All 2,054 tests pass across tiny-skia-widgets, tract-effects, minitosis,
  multosis, miff and nap with locked/offline nextest. Workspace Clippy with
  `-D warnings`, formatting and whitespace checks pass. Cargo still reports
  existing unused `atomic_float` manifest dependencies in Miff/Wavetable Filter;
  dependencies and Cargo.lock are unchanged.
- Seven permanent storage tests cover shared identities/legacy files, names/
  ordering/limits, stale records, invalid/oversized banks, lock contention and
  four concurrent writers. Fourteen User UI tests cover management, recall/Fit/
  no-op, pages, Unicode entry, errors, external changes, lifecycle, cached
  navigation/drawing and scaled/minimum layouts. Two shared effect tests verify
  management/config/frozen parity and exactly one Changed result for recall.
- Existing Project browser tests now expect the appended fourth tab and follow
  its keyboard cycle. Other tabs retain their old action counts/order. An
  existing native popup test caught initial geometry setup closing the browser;
  initial bounds adoption now retains it, while actual bounds changes cancel it.
- A red paging regression caught selection movement on a disabled boundary
  pager. Page changes now clamp page identity and retain card offset. A separate
  red ownership regression caught pending Save/Rename/Delete surviving authored
  curve replacement. Dialogs now validate a complete authored snapshot before
  key/button dispatch and consume replacement events without writing.
- Delete confirmation regressions reproduce overlap after a three-line name and
  hidden trailing Unicode characters after four lines. The layout now derives
  instruction placement from rendered name lines and reserves error/action space.
  All wrapped lines remain accessible through keyboard/wheel scrolling and a
  scrollbar. Three raster/geometry tests verify glyph preservation, minimum
  layouts and access to the last Unicode characters at 0.5x.
- Independent storage probes pass four writer processes with 32 committed
  saves, lock release after errors/process termination, malformed/oversized
  states, byte preservation and 512 forced temporary-file collisions. A full
  256-preset bank with 128-node curves and 64-character Unicode names loads and
  mutates; exhausted identities never wrap. Favorites/clipboard bytes remain
  unchanged. Evidence: `/tmp/tract-user-presets-storage-review.log`.
- Independent UI probes verify Save/Rename/Delete ownership across keyboard and
  mouse replacement, pager identity, Unicode caret/selection, stale drafts,
  native Miff/Nap Changed-only routing and frozen/config parity. Refreshed
  64-character W/‱ renders have complete names and clean instruction spacing;
  a 290x205/0.5x raster probe verifies End reveals the exact trailing Unicode
  substring and wheel restores the initial raster without mutation. No remaining
  actionable storage or UI findings.
- The Option-input review regression first saved `New curve` instead of the
  supplied Unicode name. Printable Option/Alt characters now reach name entry;
  noncharacter Alt events and Control/Command shortcuts retain their existing
  handling. Two red-before-fix regressions now pass Save/Rename with å/Å/é/ß€,
  Shift, key releases, dead/control characters and contained management commands.
  Independent public Save/Rename probes also pass mixed Option text and confirm
  retained IDs, geometry and command guards; no further findings.
- Actual full/static browser and naming/delete states render and were inspected
  at 0.5x/1x/2x, including `/tmp/mseg-user-browser.png`,
  `/tmp/mseg-user-save-half.png`, `/tmp/mseg-user-delete-half.png`,
  `/tmp/mseg-user-last.png` and `/tmp/mseg-user-rename-double.png`.

Named user presets are implemented. Categories and search remain follow-ups.
