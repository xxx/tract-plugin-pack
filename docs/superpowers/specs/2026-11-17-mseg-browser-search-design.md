# Curve-browser search

Continue `feat/mseg-power-tools` after `63a9846`. The user authorized the final
comparison slice: search preset names and categories. Extend the existing CPU
browser; no mock supplied. Search is ephemeral GUI state, outside stored curves.

## Behavior

1. Factory and User offer **Find** in the browser header. Click it, press
   Ctrl/Cmd+F, or press `/` to open/focus a search row and select its current text.
   The row has a query field, Clear and X. Favorites and Project retain their
   fixed eight-slot browsing. Opening/reopening or changing tabs starts without
   a query; category filters keep their established tab-reset behavior.
2. Queries contain up to 256 Unicode characters. Matching uses Unicode lowercase
   and whitespace-separated terms: every term must occur in the preset name or
   category, in any order. Empty/whitespace queries show all entries allowed by
   the category filter. Uncategorized records match the displayed Uncategorized
   label. Matching is literal substring matching, without regex or fuzzy rules.
3. Editing filters the cached view immediately and performs no filesystem work.
   Search and category filters combine. Matching records retain name order;
   selection stays on the same record when it still matches, otherwise the first
   match. Cards, double-click, paging, arrow/Home/End navigation and all actions
   use the same visible-to-source mapping. A nonempty query reports match count.
4. Query input supports Unicode/printable Option text, Ctrl/Cmd+A, Shift-selection,
   caret Left/Right/Home/End, Backspace and Delete. Up/Down navigate result rows;
   Page Up/Down page User results. Tab leaves query editing for ordinary browser
   navigation. Enter loads the selected match; Shift+Enter fits it. Disabled/empty
   results cannot act on hidden records and keep search available for refinement.
   S/R/C and other characters enter query text while focused rather than running
   library commands. Command keys and key releases cannot edit the curve beneath.
5. Clear empties the query and keeps the row focused. X or Escape closes/clears
   search, retaining the category filter and selected match when possible; a
   subsequent Escape closes the browser. Clicking another browser control leaves
   query focus and follows its ordinary behavior. Find/shortcut refocus existing
   text without clearing it. An empty result explains clearing search/category;
   record actions are disabled while User Save remains available outside typing.
6. Category popups and metadata/Delete dialogs keep input priority above search.
   Escape first dismisses the highest active layer. Ctrl/Cmd+F or `/` inside a
   metadata field cannot steal its input. Search stays applied when entering or
   canceling a form. Successful Save/Rename follows the stable ID as before; if
   the edited record no longer matches the query, clear the query to reveal it.
   Refresh/Delete retain the query and clamp selection within matching results.
7. Browsing/searching/clearing preserve authored curves, viewport, point selection,
   pen repetition, frozen capture, curve undo, baking and all stored library
   records. Only explicit recall performs the established geometry-only edit.
   Stale User records are still verified before recall/mutation. Existing native
   resize/focus/source/preset/directory cleanup discards search and open layers.
8. The row fits native/scaled/HiDPI bounds with readable field/buttons and usable
   thumbnails, or the existing enlargement message closes the browser before
   invisible input can act. Both mouse and keyboard search entry validate the
   expanded layout. Ordinary closed-search browsing keeps its previous minimum.
   Long queries scroll horizontally to the caret; render text stays within bounds.
9. Search does not change User-bank versions, shape/Favorites/Project formats,
   host automation IDs, DSP, latency or audio-thread behavior. It uses snapshots;
   opening User/Refresh and explicit library actions retain their existing I/O.

## Technical approach

- Reuse the Unicode NameField editor with a configurable character limit, keeping
  64-character Name/Category defaults and their Option/command guards unchanged.
  Add a small browser-local search state with visibility, focus, field and cached
  lowercase terms. Do not persist it or add dependencies.
- Add search predicates to the cached Factory/User view builder. Retain source
  indices and stable User IDs, using the existing view for every action/page.
  Metadata success reveals its record by clearing a mismatching query before
  refresh; ordinary refresh/delete keep the predicate.
- Replace the existing Factory/User header title area with Find, preserving
  category/pager geometry. Reveal a bounded field/Clear/X row above thumbnails
  only while searching. Route chooser/dialog input first, then search, then
  browser commands. Validate row admission on mouse/keyboard entry.
- Bind an open search to its authored document and validate it before input or
  recall, including standalone Miff/Nap replacements. Consume the closing row's
  follow-up clicks so a reflowed thumbnail cannot receive that double-click.
- Implementation stays local; independent read-only state/action and input/render
  reviews follow AGENTS.md. No commit until requested.

## Validation

- Matching names/categories/terms/Unicode/Uncategorized and category intersections;
  sparse pages/cards/double-click/Load/Fit/actions, empty results, query clearing,
  stable IDs through refresh/rename/recategorize/delete and stale-record checks.
- Focus/Enter/Escape/Tab/command/Option/caret/release behavior, nested popup/form
  ownership and cleanup, no I/O during edits/navigation/drawing, native frozen/
  config/Changed parity, minimum/scaled/HiDPI renders and existing field limits.
- Six-crate nextest, workspace Clippy, fmt/diff, actual previews and independent
  reviews. Leave this final feature slice uncommitted for user review.
