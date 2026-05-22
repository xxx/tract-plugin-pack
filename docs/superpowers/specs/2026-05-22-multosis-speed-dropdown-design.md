# Multosis Speed Dropdown — Design

**Date:** 2026-05-22
**Status:** Approved

## Summary

Replace the multosis toolbar **Speed** control — currently a click-to-cycle
button — with a dropdown that lists the six speed divisions. A pure UI
change: it reuses the editor's existing dropdown widget, touches no
parameter or persisted state, and changes no audio behaviour.

## Motivation

The Speed control cycles to the next division on every click (`1/32` →
`1/16` → … → `1/1` → wrap). To reach a specific division the user may have
to click through several, and there is no at-a-glance view of the options. A
dropdown shows all six and selects one directly — and matches the dropdowns
already used for the per-track effect Kind / Target / Trigger.

## Current model (for reference)

`MultosisParams.speed` is an `EnumParam<Speed>` (a host-automatable
parameter). `Speed` has six variants — `Div32, Div16, Div8, Div4, Div2,
Div1` — exposed as `Speed::ALL`, with `speed_label` giving each a short
string (`"1/32"` … `"1/1"`).

In the toolbar (`editor/toolbar.rs`), `ToolbarControl::Speed` is drawn with
`widgets::draw_button` showing `"Speed: <label>"`. A left click is routed by
`on_event` to `handle_toolbar_button`, whose `Speed` arm advances to the next
`Speed::ALL` entry and writes it via `ParamSetter` (begin/set/end).

The editor already uses the shared dropdown widget `widgets::dropdown`
(`DropdownState`, `draw_dropdown_trigger`, the popup render + hit-test) for
the effect editor's Kind / Target / Trigger dropdowns — `MultosisWindow`
holds an `effect_dropdown: DropdownState<…>` for those.

## Design

### 1. Speed dropdown state

`MultosisWindow` gains a dedicated `speed_dropdown` `DropdownState`, separate
from `effect_dropdown` — the Speed control is a different control and the
toolbar is visible in both the Grid and the Effect view, whereas the effect
dropdowns belong to the effect editor. It is initialised closed in
`MultosisWindow::new`.

### 2. Drawing

`toolbar.rs`'s `ToolbarControl::Speed` arm draws the control with
`widgets::dropdown::draw_dropdown_trigger` (the same trigger widget the
effect dropdowns use) instead of `widgets::draw_button`. The trigger's label
is the current division from `speed_label(params.speed.value())`. The
control's toolbar rect is unchanged — a dropdown trigger occupies the same
footprint as the button it replaces.

The dropdown popup (the open list) is drawn on top of everything else, after
the toolbar, opening downward over the editor area — the same layering the
effect dropdowns use.

### 3. Interaction

In `on_event`:

- A left click on the Speed control toggles `speed_dropdown` open/closed.
- While `speed_dropdown` is open, the popup owns every click and is
  hit-tested **before** any other control (so a click on the open list never
  falls through to a control behind it) — mirroring how the open
  `effect_dropdown` is handled.
- Selecting item `i` from the list sets the parameter to `Speed::ALL[i]` via
  `ParamSetter` (`begin_set_parameter` / `set_parameter` / `end_set_parameter`
  on `params.speed`), then closes the dropdown. A click outside the open list
  closes it without changing the value.
- The cycle logic in `handle_toolbar_button`'s `ToolbarControl::Speed` arm is
  removed; Speed is no longer a plain toolbar button.

The dropdown's item list is the six `speed_label` strings in `Speed::ALL`
order, so item index `i` maps to `Speed::ALL[i]`.

### 4. Unchanged

`speed` remains an `EnumParam<Speed>` — host-automatable, owned by the DAW's
automation and undo. Only the editor's input affordance changes. No change to
`Speed`, the clock, the audio engine, persisted state, or the toolbar layout.

## Testing

- `toolbar.rs` — the Speed control still hit-tests at its rect (the existing
  `toolbar_hit` round-trip test continues to cover it).
- The dropdown item ↔ `Speed` mapping: item index `i` corresponds to
  `Speed::ALL[i]`, and `speed_label` order matches `Speed::ALL` order — a
  small test asserting the list length and the index↔variant correspondence.
- Existing toolbar and editor tests still pass — the change is the Speed
  control's widget and click handling only.

`cargo build -p multosis`, `cargo clippy -p multosis -- -D warnings`,
`cargo fmt --check`, and `cargo nextest run -p multosis` all clean.

## Out of scope

- No change to any parameter, the `Speed` enum, the clock, or audio behaviour.
- No change to the other toolbar controls (Reset, the sliders, the grid-op
  buttons).
- No toolbar layout/rect changes.
