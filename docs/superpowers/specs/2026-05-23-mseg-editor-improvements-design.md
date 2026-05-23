# MSEG Editor Improvements — Design

**Date:** 2026-05-23
**Crate:** `multosis` (with shared changes in `tiny-skia-widgets`)
**Status:** approved 2026-05-23

## Goal

Four coordinated improvements to the multosis MSEG editor: per-MSEG colour
identity that propagates from the selector tab into the editor pane (active and
ghost curves) and onto the param-dial modulation arc; a hover tooltip on nodes
that shows the parameter value the node would produce; and a right-click
transform menu on selected nodes with four compress/expand actions.

## Background

multosis's modulation row hosts three MSEGs per track — Amp (always modulates
per-track audio gain) and two assignable MSEGs that each target one effect
parameter via a dropdown + depth dial. Today every MSEG draws its curve and
nodes in the same `color_accent()` sky blue, ghost curves for the inactive
MSEGs use a single hard-coded `0x5A504060` brownish-grey, and the param-dial
modulation arc is a single `color_modulation()` orange regardless of which
MSEG is driving it. Together these make it hard to know at a glance which
MSEG is which, what a node will do to its target dial, and where the
modulation arc on a dial came from.

## Architecture

One palette, four insertion points. A new `mseg_color(slot: usize) -> Color`
function in multosis is the single source of truth for the three MSEG hues.
That function feeds:

1. The selector tab's active-segment fill (Section 1).
2. The active MSEG's curve / nodes / hold markers / selection outline in the
   editor pane (Section 1).
3. The ghost curves for the inactive MSEGs (Section 1).
4. The modulation arc on a param dial — coloured by whichever MSEG is driving
   that param (Section 4).

The tooltip (Section 2) and the right-click transform menu (Section 3) live
inside the MSEG widget; they don't depend on the colour subsystem but ship in
the same milestone since they're all MSEG-editor work.

### Palette

- **Amp** — `#4fc3f7` (sky blue — the existing accent, kept)
- **MSEG 1** — `#ffc858` (amber)
- **MSEG 2** — `#c378ff` (purple)

Chosen for maximum hue separation against the dark theme and against each
other; amber matches the existing "Editing Track N" caption colour `#E8C98A`
so the editor reads as one coordinated theme.

## Section 1 — Per-MSEG colour identity

### Where the colour shows up

- **Selector tab** (the `Amp / MSEG 1 / MSEG 2` stepped selector on the
  modulation row): the active segment fills with its own MSEG colour;
  inactive segments fall back to the existing muted swatch.
- **Active MSEG curve** in the editor pane: the polyline, node fill, node
  selection outline, hold-mode marker, and marquee outline all switch from
  `color_accent()` to the slot's colour.
- **Ghost curves** for the two inactive MSEGs: drawn at the existing ~38 %
  alpha but in each ghost's *own* colour rather than the shared
  `0x5A504060` brownish-grey.
- **MSEG playhead overlay** (the vertical line over the active MSEG in
  `draw_mseg_playhead`): switches to a desaturated tint of the active MSEG's
  colour.

### Mechanism

- `multosis::editor::mseg_color(slot: usize) -> tiny_skia::Color` — the
  central palette lookup. Three colours, indexed by `slot.min(2)`.
- `widgets::mseg::draw_mseg` gains a `value_color: Color` parameter that
  replaces every hard-coded `color_accent()` use inside the function (curve
  stroke, node fill, selection outline, hold marker).
- `widgets::mseg::draw_mseg_ghost` already takes a `color: u32` param —
  signature stays, but callers feed it from `mseg_color`.
- `widgets::controls::draw_stepped_selector` gains an optional
  `active_color: Option<Color>` parameter. `None` keeps the existing
  accent behaviour (so the MSEG sync-mode selector and the modulation
  trigger selector are unchanged); `Some(color)` overrides the active
  segment fill — the MSEG selector uses this with `mseg_color(active_idx)`.

## Section 2 — Hover-node tooltip

### Trigger

Cursor over a node circle (using the existing `MsegEditState::hover` hit
target — no new state). Only the *active* MSEG's nodes are interactive;
ghost MSEGs stay non-interactive (consistent with today).

### Content

The tooltip shows the parameter value the dial would read if the modulator
were *parked* at this node's level. It does **not** factor in the live
modulator output / current playhead position — that would jitter unhelpfully.

- **Amp MSEG (slot 0)**: dB readout, computed as
  `20 · log10(max(node.value, 1e-4))`. Floored at `-80 dB` (anything below
  renders as `-∞ dB`). Format: `"-6.0 dB"`, one decimal place.
- **Assignable MSEG with target set**: calls the existing
  `crate::modulation::assignable_value(node.value, base, depth, spec,
  polarity)`, then formats with the existing
  `crate::effects::format_value(value, spec.format)`. Identical math + format
  to what the dial's modulation arc already uses; only the modulator output
  is replaced with the node's static value.
- **Assignable MSEG with target = None**: the curve isn't routed, so no
  param-mapped value exists. Show just `"0.74"` — the raw node level to
  three significant figures.

### Layout

- A small rounded rect drawn above the node by default, with the value text
  centred horizontally on the node.
- Flip below the node when the node sits in the top quarter of the canvas
  so the tooltip stays inside the editor's bounds.
- Background `color_control_bg()`, 1px border `color_border()`, text
  `color_text()` at the same size as strip captions — visually consistent
  with existing widgets.
- Layout-only; no animation, no fade-in.

### Files

- `tiny-skia-widgets/src/mseg/render.rs` — a new `draw_node_tooltip`
  helper called from the end of `draw_mseg` (after the curve + nodes, so
  the tooltip sits on top). Takes the hovered node index, the formatted
  text, the canvas rect, and the node centre.
- `multosis/src/editor.rs` — pre-formats the tooltip text per the
  Amp / assignable / no-target rules above, and passes it (or `None`) to
  `draw_mseg`. multosis owns the text because the formatting depends on
  the target's `ParamSpec`, which the widget crate doesn't know about.

## Section 3 — Right-click transform menu on selected nodes

### Trigger

Right mouse button down lands on a node that is `is_node_selected(i)`,
while `selection_count() >= 1`. All other right-clicks keep their
existing behaviour:

- Right-click on a non-selected node → existing segment-stepped toggle.
- Right-click on a segment (between nodes) → existing segment-stepped toggle.
- Right-click on empty marker-lane / strip → no-op.

This preserves the entire existing right-click flow; the new menu only
opens when the click target overlaps the active selection.

### Menu UI

Reuses the existing `DropdownState<StripId>` widget the strip already
uses for Style and Grid — same theming, same dismiss logic, same
keyboard handling. A new `StripId::Transform` discriminant routes its
events. Anchored at the click point, opens below by default and flips
above when the click is near the bottom edge.

### Items (4)

1. **Compress values** — pull each selected node's `value` 25 % toward
   the selection's mean value.
2. **Expand values** — push each selected node's `value` 25 % away from
   the selection's mean value, clamped to `[0, 1]`.
3. **Compress times** — pull each selected node's `time` 25 % toward
   the selection's mean time. Anchor nodes (node 0 at `time = 0.0` and
   the last node at `time = 1.0`) are never moved.
4. **Expand times** — push each selected node's `time` 25 % away from
   the selection's mean time, clamped to `(0, 1)` exclusive of the
   anchors. Anchor nodes never move.

### Maths

For each selected node, `new = mean + (old - mean) · k`, where:

- `k = 0.75` for **Compress** (any axis).
- `k = 1.25` for **Expand** (any axis).

Per-axis: `mean` is computed across the selected nodes' `value` (for
value transforms) or `time` (for time transforms). The first/last node
of the MSEG are anchor nodes — their `time` is locked, so they're
excluded from the time-mean and from time mutation, but their `value`
is mutated normally if selected.

Time transforms can re-order nodes (a selected node may pass through
an unselected neighbour). The implementation routes through the
existing node-move codepath which sorts and reindexes; selection
follows the moved node (so a node selected before the transform is
still selected after).

Stacking: four compresses collapse selected values toward the mean (each
step is multiplicative by 0.75); two expands push them outward by 1.5625×;
this is the documented behaviour and falls out of the linear-step maths.

### Undo

Each menu invocation produces one `MsegEdit::Changed` return value —
the multosis undo system already snapshots full track config on this
event, so each menu pick is one undo step. No new undo plumbing needed.

### Files

- `tiny-skia-widgets/src/mseg/editor.rs` — extend `MsegHit` with a new
  `MsegHit::SelectedNode(i)` variant that the existing hit-tester emits
  when the click lands on a selected node (in addition to the existing
  `Node(i)` for unselected nodes). Right-click handler branches on it.
- New `MsegEditState` methods:
  `compress_values(&mut self, data: &mut MsegData) -> MsegEdit`,
  `expand_values(...)`, `compress_times(...)`, `expand_times(...)`.
  Each reads `self.selection`, computes the mean over its axis, applies
  the linear step, clamps, and calls the existing node-edit path.

## Section 4 — Modulation arc colour coordination

### Where it shows up

Param dials carry an orange "modulation arc" between the static value
position and the live modulated position. Today it's the same orange
on every dial; with this change it takes on the colour of the MSEG
driving the modulation:

- A dial modulated by MSEG 1 → amber arc + amber endpoint dot.
- A dial modulated by MSEG 2 → purple arc + purple endpoint dot.
- A dial not modulated → no arc (unchanged).

Amp never produces a param-dial arc (it targets per-track gain, not a
dial), so slot 0 doesn't participate.

### Mechanism

- `multosis::editor::MultosisWindow::compute_modulated_norms()` —
  return type changes from `[Option<f32>; MAX_EFFECT_PARAMS]` to
  `[Option<(f32, u8)>; MAX_EFFECT_PARAMS]`. The `u8` is the MSEG slot
  (1 or 2) whose contribution is being reported; last-MSEG-wins
  semantics preserved (slot 2 takes precedence when both target the
  same dial).
- `multosis/src/editor/effect_editor.rs` — per-dial loop reads the
  tagged slot, looks up `mseg_color(slot)`, passes as the new
  `mod_color: Color` arg into `draw_dial_ex` / `draw_dial_dimmed_ex`.
- `tiny-skia-widgets/src/param_dial.rs` — `draw_dial_ex`,
  `draw_dial_dimmed_ex`, and the shared `draw_dial_inner` all gain a
  `mod_color: Color` parameter. The existing
  `color_modulation()` / `color_modulation_dot()` constants become
  pure helpers that take a base hue and emit the arc colour at α=150
  / dot at α=200; the call site picks the hue per dial.
- The plain `draw_dial` entry point (no modulation indicator) is
  unchanged — only the `_ex` variants gain the parameter.

### Out of scope here

When both MSEG 1 and MSEG 2 target the same param, the arc shows only
MSEG 2's contribution and colour. That's the existing last-wins
display semantics; the colour matching surfaces the issue but does not
fix it. A multi-modulator visual is its own design problem.

## Testing strategy

### Unit tests

- **`mseg_color` palette**: `mseg_color(0)` returns the sky blue
  constant; `mseg_color(1)` returns amber; `mseg_color(2)` returns
  purple; `mseg_color(99)` clamps to slot 2 (purple).
- **Amp tooltip dB conversion**: `value = 1.0 → "0.0 dB"`,
  `value = 0.5 → "-6.0 dB"` (within 0.1 dB), `value = 0.0 → "-∞ dB"`,
  `value = 1e-5 → "-80.0 dB"` (floor).
- **Assignable tooltip formatting**: round-trips through
  `assignable_value` + `format_value` for one `ParamScaling::Log` case
  (Hz) and one `ParamScaling::Linear` case (percentage).
- **Compress / expand maths**: 4-node fixture per axis, verify
  resulting values match `mean + (old - mean) · k` for both
  `k = 0.75` and `k = 1.25`. Anchor times never move (assert exact
  equality on first/last node `time`). Expand clamps to `[0, 1]` /
  `(0, 1)` as appropriate.
- **Transform repeated apply**: two `compress_values` in a row produce
  values matching `mean + (old - mean) · 0.75 · 0.75`.

### Integration tests

- **Right-click hit-test routing**:
  - `right_click_on_selected_node_opens_transform_menu` — populate a
    selection, send a right-click on a node in it, assert the dropdown
    is open with `StripId::Transform`.
  - `right_click_on_unselected_node_still_toggles_stepped` — existing
    behaviour preserved.
  - `right_click_on_segment_still_toggles_stepped` — existing
    `right_click_toggles_segment_stepped` test stays green.
- **Colour identity rendering**:
  - `draw_mseg_paints_value_color_on_the_curve` — render with
    `value_color = magenta`, probe a pixel on a polyline segment,
    assert the pixel reads magenta-ish.
  - `dial_modulation_arc_uses_mseg_1_colour_when_mseg_1_drives` —
    draw a dial with a fresh modulation report from MSEG 1, probe a
    pixel on the arc, assert amber-ish RGB.
  - `dial_modulation_arc_uses_mseg_2_colour_when_both_target` —
    both MSEGs targeting the same dial, probe pixel reads purple-ish
    (last-MSEG-wins).

### Manual verification

After implementation, build the standalone multosis bundle and
visually verify on the Delay row:

1. Switch between Amp / MSEG 1 / MSEG 2 — selector tab colour and
   active curve colour stay in lockstep with the palette.
2. Hover a node on each MSEG — tooltip shows the expected mapped
   value (dB for Amp; Hz / % / ms / dB depending on the target for
   assignable).
3. Marquee-select two nodes, right-click on one of them — menu
   opens with four items. Each pick visibly compresses or expands
   the selection. Stacks across repeated picks.
4. Set MSEG 1 to target the Free dial; check the dial's modulation
   arc is amber. Switch to MSEG 2 instead; arc becomes purple.

## Files touched

| File | Change |
|---|---|
| `multosis/src/editor.rs` | `mseg_color(slot)`; tagged `compute_modulated_norms`; pass `value_color` into `draw_mseg` and `draw_mseg_ghost`; pass `mseg_color` into the selector + dial draw calls. |
| `multosis/src/editor/effect_editor.rs` | Pre-format tooltip text per-MSEG; pass `mod_color` to dial draws; switch selector active fill via the new `draw_stepped_selector` param. |
| `tiny-skia-widgets/src/mseg/render.rs` | `draw_mseg` gains `value_color`; new `draw_node_tooltip` helper; ghost colour wiring already in place. |
| `tiny-skia-widgets/src/mseg/editor.rs` | Hit-test emits `SelectedNode(i)`; right-click branches by it; new transform-state methods; new `StripId::Transform` for the menu's dropdown events. |
| `tiny-skia-widgets/src/param_dial.rs` | `mod_color` parameter on the `_ex` entry points; `color_modulation()` becomes a hue→arc-colour helper. |
| `tiny-skia-widgets/src/controls.rs` | `draw_stepped_selector` gains `active_color: Option<Color>`. |
| `multosis/src/modulation.rs` | No code change — the colour palette lives in `editor` since slots map 1:1 to the editor view. |

## Risks

- **`draw_dial_ex` signature change** — every workspace call site (only
  in `multosis::editor::effect_editor`) needs the new `mod_color` arg.
  Plain `draw_dial` callers (widget tests, other plugins' simple
  dials) unaffected.
- **Ghost colour at α=38 % may read too saturated for amber + purple
  on the dark canvas** — confirmed via the palette mockup that the
  same alpha works visually; tuneable in the const if the build looks
  off.
- **Transform menu on a single-node selection** — `mean` equals that
  one node's coordinate, so compress/expand are no-ops. Acceptable
  (the user can see nothing changed); no special-case needed.

## Out of scope

- Multi-modulator display (showing both MSEGs' contributions when both
  target the same dial). Pre-existing single-arc design retained.
- Secondary transforms beyond the four compress/expand ops (invert,
  reverse, normalize, quantize, smooth, jitter). Easy follow-up if
  desired — the menu plumbing supports adding items.
- Per-instance MSEG colour customization. Palette is static per slot.
- Tooltip on ghost MSEGs. Ghosts stay non-interactive in this
  milestone.
- Re-coloring the dial's modulation arc when no MSEG is currently
  publishing (e.g. modulation is configured but the editor isn't open
  / the trigger hasn't fired). Same gate as today — no arc shown.
