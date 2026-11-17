# Curve-library categories

Continue `feat/mseg-power-tools` after `e3a7bcc`. The user authorized the next
comparison slice: group factory shapes, assign categories to named User presets,
and filter browsing by category. Extend the existing CPU editor; no mock supplied.
Preset search remains a separate follow-up.

## Behavior

1. Factory curves have four groups: Basic (Flat high/center), Ramps (Ramp up/down),
   Modulation (Triangle/Sine), and Rhythmic (Square/Duck). Factory and User tabs
   expose a category selector in the browser header; other tabs retain their
   eight-slot layouts. Opening/switching tabs starts with All categories.
2. User categories are optional custom labels of at most 64 Unicode characters,
   trimmed and free of controls. Empty labels mean Uncategorized. Comparisons
   use Unicode lowercase, consistently with preset-name matching. Names remain
   unique across the complete library, independently of category.
3. Save and Rename dialogs include a Category field. It accepts printable
   Unicode/Option input and offers existing categories plus factory groups via
   its chooser (arrow, F4 or Alt+Down). Tab traverses Name, Category, apply and
   Cancel; Enter applies both metadata fields atomically. Rename keeps its
   initial name focus and can change only the category. New saves inherit the
   active User category. Category edits retain preset IDs, geometry and next ID.
4. Category filtering uses cached snapshots, changing only which thumbnails are
   shown. The selector offers All categories, Uncategorized on User, and sorted
   available categories; custom labels are unambiguous beside the special options.
   C opens it; arrows/Home/End/Page Up/Down/wheel navigate; Enter chooses and
   Escape cancels. It is drawn above browser cards and metadata dialogs.
5. Cards, arrow/Home/End navigation, double-click, paging and Load/Fit all address
   the filtered view. The selected record remains selected when it still matches;
   otherwise select the first match. Empty results explain how to choose All and
   disable recall/Rename/Delete. No hidden source-index actions may occur.
6. Successful Save/Rename follows the stable ID to its sorted page. If the edited
   category would hide that record, reveal its category. Delete/Refresh retain
   the filter and clamp selection within matching results, including an empty
   group. A selected empty category remains available so it can be cleared or
   populated again. Switching tabs resets the filter without editing a curve.
7. All category and metadata actions preserve authored curves, selection, viewport,
   pen state, frozen capture, Project/Favorites/clipboard and curve undo/baking.
   Load/Fit retain the established geometry-only recall and Changed/no-op rules.
   Authored-document replacement, source/preset/window/directory cleanup still
   cancel stale dialogs. Record category changes participate in full stale-record
   checks before recall or mutation; concurrent unrelated edits remain intact.
8. Category popups own pointer/wheel/key/release events. An outside popup press
   cancels only that popup and consumes the press; it cannot load a card, change
   a tab or submit a metadata dialog. Right-click retains browser dismissal.
   Resize/focus loss/binding changes close popups and drafts. Narrow/scaled/HiDPI
   layouts reserve readable header controls, previews and both fields, or show
   the existing enlargement message. Long labels fit/scroll within their bounds.
9. Older User records load as Uncategorized without writing on read. Plain
   uncategorized version-1 banks retain their compatible wire format. A category
   write upgrades the bank to version 2 so older readers reject unsupported
   metadata rather than erase it. Current readers accept both versions; writes
   retain the version-2 marker after upgrade. Existing curve files and persisted
   Project/Favorites formats remain unchanged. Corrupt/unknown versions and failed
   writes retain all bytes, with the existing retryable error behavior.
10. Category metadata stays in the GUI-side shared library. Card/page navigation,
    filtering and rendering perform no filesystem work. No DSP, host parameter,
    latency or audio-thread changes; user-library writes remain explicit actions.

## Technical approach

- Add default-omitted category metadata to UserPreset, bounded validation and
  version-aware bank checks. Keep old save/rename API wrappers; add categorized
  save and atomic name/category update under the existing nonblocking lock.
- Map FactoryShape groups without changing CurveShape serialization. Add a
  cached browser view mapping visible positions to original indices, retaining
  record IDs across refresh/sort/category edits. Apply it consistently to all
  card/actions/navigation/paging/rendering paths.
- Reuse DropdownState with a frozen label/choice snapshot for header filtering
  and category-field assignment. Keep popup state local to the browser, route it
  before underlying controls, and draw it last. Popup width reserves label space
  independently of the compact trigger and clamps within the native window.
  Reuse Unicode NameField editing
  for category input, preserving the Option-input regression fix.
- Add a second field to metadata dialogs while retaining the separate scrollable
  Delete confirmation. Validate actual metadata-dialog space on entry; ordinary
  User browsing/Delete need not inherit the larger form's minimum dimensions.
- Implementation remains local. Independent read-only storage/migration and
  UI/view/input reviews follow AGENTS.md. No commit until requested.

## Validation

- Version-1 fixtures, category round trips/normalization/bounds, version-2
  preservation, atomic/stale/concurrent category edits, failed-write bytes and
  legacy clipboard/Favorites/Project/curve compatibility.
- Filtered factory/User Load/Fit, sparse mappings across pages, empty categories,
  stable IDs after rename/recategorize/delete/refresh, chooser ownership and
  all Unicode/Option/form/cancellation behavior, real native integration and
  scaled/HiDPI/minimum renders. Retain long Delete-name raster regressions.
- Six affected crates' nextest suite, workspace Clippy, fmt/diff, independent
  reviews and actual offscreen previews. Leave this slice uncommitted.
