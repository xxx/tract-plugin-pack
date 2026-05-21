# Multosis Editor Polish — Design

**Date:** 2026-05-21
**Status:** Approved

## Summary

Three small, independent cleanups to the multosis per-track effect editor,
grouped as one bundle:

1. **Section headers** — `"EFFECT"` and `"MODULATION"` labels above the two
   sections of the effect editor.
2. **Rename `kind_dropdown`** — the field now backs three dropdowns; give it
   an honest name.
3. **Kind-switch integration test** — extract the kind-switch logic into a
   testable function and cover the composed sequence.

These are deferred items from the Phase 2c review. None changes audio
behaviour.

## 1. Section headers

The effect editor has two control sections — an EFFECT section (kind
dropdown, parameter dials, the per-track Mix dial) and a MODULATION section
(trigger, rate, MSEG selector, target, depth, sync, length, and the MSEG
pane) — with no visual labels. Today they sit ~8–10 px apart and read as one
undifferentiated block.

Add a labelled header above each section: a small left-aligned caption
(`"EFFECT"`, `"MODULATION"`) followed by a thin horizontal divider rule
running to the right edge of the editor's main area. Each header occupies a
~16 px band.

### Layout changes (`effect_editor.rs::effect_layout`)

- The EFFECT controls (`kind`, `dials`, `mix`) shift **down ~16 px** to clear
  the EFFECT header band.
- The MODULATION controls (`trigger`, `trigger_rate`, `mseg_selector`,
  `target`, `depth`, `mseg_sync`, `mseg_length`) shift **down ~32 px** — the
  EFFECT shift plus their own header band.
- The `mseg_pane` shifts **down ~32 px** and loses **~32 px of height**, so
  its bottom edge stays where it is.
- Two header rects are added to `EffectLayout` (`effect_header`,
  `modulation_header`) so layout stays centralized. They are **draw-only** —
  not returned by `effect_hit`, not interactive.

The exact pixel offsets are an implementation detail for the plan; the
constraint is: each header band is clear of the controls above and below it,
and no rect overlaps another.

### Drawing

`draw_effect_section` renders the EFFECT header (caption + rule). The
MODULATION header is rendered by whichever function draws the MODULATION
section (e.g. `draw_modulation_controls` / the modulation draw path). The
caption uses the editor's existing text rendering; the rule is a 1 px
`draw_rect` line in a muted colour consistent with the editor's palette.

## 2. Rename `kind_dropdown` → `effect_dropdown`

`editor.rs` has a field `kind_dropdown: DropdownState<EffectAction>`. It is no
longer kind-specific — it backs the Kind, Target, and Trigger dropdowns (one
open at a time, discriminated by `EffectAction`). Rename the field to
`effect_dropdown` — it is the effect editor's shared dropdown state.

This is a pure mechanical rename of every reference in `editor.rs` (the field
declaration, its initializer, and all use sites). No behaviour change, no
other file affected.

## 3. Kind-switch integration test

`MultosisWindow::apply_kind_switch` composes three steps inline when a track's
effect kind changes:

1. set `TrackEffect.kind`,
2. reset `TrackEffect.params` to the new kind's defaults
   (`default_params_for_kind`),
3. clamp the track's modulation targets to the new kind's parameter count
   (`TrackModulation::clamp_targets`).

The three pieces are unit-tested individually but the composition is not.
`apply_kind_switch` is a method on the editor window (which owns a rendering
surface), so it cannot be exercised directly from a test.

### Extraction

Add a free function to `modulation.rs` (which already depends on `effects.rs`):

```rust
/// Switch one track to effect `kind`: set the kind, reset its parameters to
/// the kind's defaults, and clamp the track's assignable-MSEG targets to the
/// new kind's parameter count (so a target can never reference a parameter
/// the new effect lacks).
pub fn switch_effect_kind(
    effect: &mut TrackEffect,
    modulation: &mut TrackModulation,
    kind: EffectKind,
) {
    effect.kind = kind;
    effect.params = default_params_for_kind(kind);
    modulation.clamp_targets(param_count(kind));
}
```

`MultosisWindow::apply_kind_switch` becomes: lock `track_effects` and
`track_modulation`, call `switch_effect_kind` for the selected row, then
`mark_config_dirty()`. Its observable behaviour is unchanged.

### The test

In `modulation.rs`'s `#[cfg(test)]` module, an integration test that composes
the full sequence: start with a `TrackEffect` on a kind with parameters and a
`TrackModulation` whose assignable MSEG targets a parameter index that the
*destination* kind does not have; call `switch_effect_kind`; assert:

- `effect.kind` is the new kind,
- `effect.params` equals `default_params_for_kind(new_kind)`,
- the out-of-range modulation target was cleared to `None`,
- an in-range target (if any) is preserved.

## Testing

- `switch_effect_kind` integration test (above).
- A layout test (`effect_editor.rs`) — the two header rects are disjoint from
  every control in their section and from each other, and the shifted
  `mseg_pane` still sits below both the EFFECT and MODULATION sections.
- Existing effect-editor and engine tests must still pass — the header shift
  is layout-only, the rename is mechanical, and `switch_effect_kind` preserves
  `apply_kind_switch`'s behaviour.
- `cargo build`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`,
  `cargo nextest run -p multosis` all clean.

## Out of scope

- No audio/DSP change.
- No new interactive controls — the section headers are decorative.
- No change to the dropdown behaviour, only the field name.
- The other backlog items (more triggers, more effect kinds, undo/redo,
  presets) are untouched.
