# Multosis Editor Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Three small cleanups to the multosis effect editor — section headers, an honest field name, and a kind-switch integration test.

**Architecture:** EFFECT/MODULATION section headers are draw-only rects added to `EffectLayout`, with the section controls shifted down to make room. The `kind_dropdown` field is renamed. The kind-switch logic is extracted from the editor into a free function so it can be integration-tested.

**Tech Stack:** Rust (nightly), nih-plug plugin, `cargo nextest`. Workspace `/home/mpd/git-sources/tract-plugin-pack`, crate `multosis`, branch `multosis`.

**Spec:** `docs/superpowers/specs/2026-05-21-multosis-editor-polish-design.md`

**Conventions:**
- Build: `cargo build -p multosis`. Tests: `cargo nextest run -p multosis`. Lint: `cargo clippy -p multosis -- -D warnings`. Format: `cargo fmt -p multosis`.
- Never use `#[allow(...)]` to silence a warning without strong justification.
- Commit message trailer MUST be exactly:
  `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Editor diagnostics are often stale — verify with a real `cargo build` / `cargo nextest run`.
- Run all `cargo` commands from the workspace root `/home/mpd/git-sources/tract-plugin-pack`.

---

## Task 1: Rename `kind_dropdown` → `effect_dropdown`

The field `kind_dropdown: DropdownState<EffectAction>` in `editor.rs` backs three
dropdowns (Kind, Target, Trigger), not just Kind. Rename it to `effect_dropdown`.
The name appears only in `multosis/src/editor.rs` (~21 references including one
comment); no other file uses it.

**Files:**
- Modify: `multosis/src/editor.rs` — every `kind_dropdown` reference

- [ ] **Step 1: Confirm the name is confined to `editor.rs`**

Run: `rg -n 'kind_dropdown' multosis/src`
Expected: every hit is in `multosis/src/editor.rs` (declaration, initializer,
~18 use sites, one comment). If any other file appears, stop and report — the
plan assumed editor-only.

- [ ] **Step 2: Rename every occurrence**

Run: `sed -i 's/kind_dropdown/effect_dropdown/g' multosis/src/editor.rs`

This replaces the field declaration, its initializer, all method-call sites,
and the comment at the old line ~1147 (`shared `kind_dropdown` state handles
Kind, Target, and Trigger;` → `shared `effect_dropdown` state ...`).

- [ ] **Step 3: Verify the rename**

Run: `rg -n 'kind_dropdown' multosis/src`
Expected: NO matches.

- [ ] **Step 4: Build, lint, test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo nextest run -p multosis`
Expected: all PASS, no warnings. (A pure rename — behaviour unchanged.)

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor.rs
git commit -m "$(cat <<'EOF'
refactor(multosis): rename kind_dropdown to effect_dropdown

The field backs the Kind, Target, and Trigger dropdowns — one open at a
time, discriminated by EffectAction — so "kind" was misleading. It is
the effect editor's shared dropdown state. Pure rename, no behaviour
change.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Extract `switch_effect_kind` and integration-test it

`MultosisWindow::apply_kind_switch` (in `editor.rs`) composes three steps when
a track's effect kind changes — set kind, reset params to the kind's defaults,
clamp the modulation targets. It is a method on the editor window (owns a
rendering surface) so it cannot be tested directly. Extract the composable core
into a free function in `modulation.rs` and integration-test it.

**Files:**
- Modify: `multosis/src/modulation.rs` — new `switch_effect_kind` fn + test
- Modify: `multosis/src/editor.rs` — `apply_kind_switch` calls the new fn

- [ ] **Step 1: Write the failing test**

In `multosis/src/modulation.rs`, inside the `#[cfg(test)] mod tests` module, add:

```rust
    #[test]
    fn switch_effect_kind_resets_params_and_clamps_targets() {
        use crate::effects::{EffectKind, TrackEffect};

        // A Bitcrush track (2 params) whose first assignable MSEG targets
        // parameter index 1.
        let mut effect = TrackEffect {
            kind: EffectKind::Bitcrush,
            params: crate::effects::default_params_for_kind(EffectKind::Bitcrush),
            mix: 1.0,
        };
        let mut modulation = TrackModulation::default_for_row(0);
        modulation.targets[0] = Some(1);

        // Switch to None (0 parameters).
        switch_effect_kind(&mut effect, &mut modulation, EffectKind::None);

        assert_eq!(effect.kind, EffectKind::None, "kind switched");
        assert_eq!(
            effect.params,
            crate::effects::default_params_for_kind(EffectKind::None),
            "params reset to the new kind's defaults"
        );
        assert_eq!(
            modulation.targets[0], None,
            "out-of-range target cleared — None has 0 params"
        );
    }

    #[test]
    fn switch_effect_kind_keeps_an_in_range_target() {
        use crate::effects::{EffectKind, TrackEffect};

        // Switching between two kinds that both have parameter index 0.
        let mut effect = TrackEffect {
            kind: EffectKind::Lowpass,
            params: crate::effects::default_params_for_kind(EffectKind::Lowpass),
            mix: 1.0,
        };
        let mut modulation = TrackModulation::default_for_row(0);
        modulation.targets[0] = Some(0);

        switch_effect_kind(&mut effect, &mut modulation, EffectKind::Bitcrush);

        assert_eq!(effect.kind, EffectKind::Bitcrush);
        assert_eq!(
            modulation.targets[0],
            Some(0),
            "index 0 is in range for Bitcrush — target preserved"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p multosis switch_effect_kind`
Expected: FAIL — `switch_effect_kind` does not exist.

- [ ] **Step 3: Add `switch_effect_kind` to `modulation.rs`**

`modulation.rs` already has `use crate::effects::{...}`. Ensure `EffectKind`,
`TrackEffect`, `default_params_for_kind`, and `param_count` are in that `use`
list (add whichever are missing). Then add the function near the top-level
free functions of the module (e.g. next to `assignable_value`):

```rust
/// Switch one track to effect `kind`: set the kind, reset its parameters to
/// the kind's defaults, and clamp the track's assignable-MSEG targets to the
/// new kind's parameter count (so a target can never reference a parameter
/// the new effect lacks). The composable core of the editor's kind-switch.
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

If `EffectKind` / `TrackEffect` / `default_params_for_kind` / `param_count` are
referenced elsewhere in `modulation.rs` with a `crate::effects::` path rather
than imported, match that file's existing convention instead of adding imports.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p multosis switch_effect_kind`
Expected: PASS — both tests.

- [ ] **Step 5: Rewrite `apply_kind_switch` to call `switch_effect_kind`**

In `multosis/src/editor.rs`, `apply_kind_switch` currently locks
`track_effects` and `track_modulation` in two separate `if let Ok(...)` blocks
and inlines the three steps. Replace its body so it locks both and delegates:

```rust
    /// Apply a kind switch for the currently selected track: replace the kind,
    /// reset the params to its defaults, and clamp the track's modulation
    /// targets to the new arity. Marks config dirty.
    fn apply_kind_switch(&mut self, kind: EffectKind) {
        let row = self.selected_track;
        if let (Ok(mut eff), Ok(mut modu)) = (
            self.params.track_effects.lock(),
            self.params.track_modulation.lock(),
        ) {
            crate::modulation::switch_effect_kind(&mut eff[row], &mut modu[row], kind);
        }
        self.mark_config_dirty();
    }
```

(`mark_config_dirty()` stays unconditional, matching the original.)

- [ ] **Step 6: Build, lint, full test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo nextest run -p multosis`
Expected: all PASS, no warnings.

- [ ] **Step 7: Commit**

```bash
git add multosis/src/modulation.rs multosis/src/editor.rs
git commit -m "$(cat <<'EOF'
refactor(multosis): extract switch_effect_kind for testability

The kind-switch sequence (set kind, reset params to the kind's
defaults, clamp modulation targets) lived inline in the editor's
apply_kind_switch and could not be tested — the editor window owns a
render surface. Extract it as a free function in modulation.rs;
apply_kind_switch now locks both configs and delegates. Adds an
integration test covering the composed sequence, including the
out-of-range target clamp.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: EFFECT / MODULATION section headers

Add labelled headers above the two effect-editor sections. Each is a caption
(`"EFFECT"`, `"MODULATION"`) plus a thin divider rule. The section controls
shift down to make room; the MSEG pane shrinks to keep its bottom edge fixed.

**Files:**
- Modify: `multosis/src/editor/effect_editor.rs` — `EffectLayout`, `effect_layout`, new `draw_section_header`, layout test
- Modify: `multosis/src/editor.rs` — `draw_effect_view` calls `draw_section_header` twice

- [ ] **Step 1: Add the two header rects to `EffectLayout`**

In `multosis/src/editor/effect_editor.rs`, add two fields to the `EffectLayout`
struct — put `effect_header` right after `back`, and `modulation_header` right
after `mix`:

```rust
    /// EFFECT section header band (caption + divider rule). Draw-only.
    pub effect_header: (f32, f32, f32, f32),
```

```rust
    /// MODULATION section header band (caption + divider rule). Draw-only.
    pub modulation_header: (f32, f32, f32, f32),
```

- [ ] **Step 2: Shift the section layout and build the header rects**

In `effect_layout`, replace the block from `let back = ...` down to
`let mseg_pane = ...` with this (the EFFECT controls shift down 16 px, the
MODULATION controls down 32 px, the MSEG pane down 32 px and 32 px shorter):

```rust
    // Editor bar.
    let back = l(ox, oy + 4.0, 90.0, 30.0);
    // EFFECT section — a header band, then the controls shifted down to clear it.
    let effect_header = l(ox, oy + 36.0, mw - inset, 16.0);
    let kind = l(ox, oy + 66.0, 150.0, 34.0);
    let dials = std::array::from_fn(|i| l(ox + 180.0 + i as f32 * 96.0, oy + 60.0, 88.0, 88.0));
    // Per-track Mix dial: a fixed slot to the right of the four param-dial
    // slots (which end at ox+556), clearly set apart from them.
    let mix = l(ox + 580.0, oy + 60.0, 88.0, 88.0);
    // MODULATION section — its own header band, then the controls. The trigger
    // and rate are PER-TRACK (govern all 3 MSEGs).
    let modulation_header = l(ox, oy + 152.0, mw - inset, 16.0);
    let trigger = l(ox, oy + 200.0, 130.0, 34.0);
    let trigger_rate = l(ox + 146.0, oy + 192.0, 60.0, 42.0);
    let mseg_selector = l(ox + 222.0, oy + 200.0, 240.0, 34.0);
    let target = l(ox + 478.0, oy + 200.0, 170.0, 34.0);
    // Depth dial: raised so its value text doesn't fall into the MSEG pane below.
    let depth = l(ox + 664.0, oy + 172.0, 64.0, 64.0);
    // Active-MSEG sync + length, on the modulation row to the right of depth.
    let mseg_sync = l(ox + 740.0, oy + 200.0, 110.0, 34.0);
    let mseg_length = l(ox + 860.0, oy + 200.0, 140.0, 34.0);
    let mseg_pane = l(ox, oy + 240.0, mw - inset, 390.0);
```

Then add `effect_header` and `modulation_header` to the returned
`EffectLayout { ... }` struct literal.

- [ ] **Step 3: Write the failing layout test**

In `effect_editor.rs`'s `#[cfg(test)] mod tests` (the `rects_overlap` helper
already exists there), add:

```rust
    #[test]
    fn section_headers_are_disjoint_from_their_section_controls() {
        let lay = effect_layout(1.0);
        // EFFECT header clears the editor bar above and the EFFECT controls below.
        assert!(!rects_overlap(lay.effect_header, lay.back));
        assert!(!rects_overlap(lay.effect_header, lay.kind));
        for d in lay.dials {
            assert!(!rects_overlap(lay.effect_header, d));
        }
        assert!(!rects_overlap(lay.effect_header, lay.mix));
        // MODULATION header clears the EFFECT dials above and the MODULATION
        // controls below.
        for d in lay.dials {
            assert!(!rects_overlap(lay.modulation_header, d));
        }
        assert!(!rects_overlap(lay.modulation_header, lay.trigger));
        assert!(!rects_overlap(lay.modulation_header, lay.depth));
        assert!(!rects_overlap(lay.modulation_header, lay.mseg_pane));
        // The two headers do not overlap each other.
        assert!(!rects_overlap(lay.effect_header, lay.modulation_header));
        // The MSEG pane still sits below both sections.
        assert!(lay.mseg_pane.1 >= lay.modulation_header.1 + lay.modulation_header.3);
        assert!(lay.mseg_pane.1 >= lay.depth.1 + lay.depth.3);
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p multosis section_headers_are_disjoint`
Expected: PASS (Steps 1–2 produced the disjoint layout). If it FAILS, an
offset is wrong — adjust the `oy + ...` values in Step 2 so every band is
clear, keeping the EFFECT shift at 16 px and the MODULATION shift at 32 px.

- [ ] **Step 5: Add the `draw_section_header` helper**

In `effect_editor.rs`, add a public function (near `draw_effect_section`):

```rust
/// Draw a section header into `rect`: a left-aligned caption followed by a
/// thin divider rule running to the rect's right edge.
pub fn draw_section_header(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    rect: (f32, f32, f32, f32),
    label: &str,
    scale: f32,
) {
    let (x, y, w, h) = rect;
    let size = 13.0 * scale;
    let baseline = y + (h + size) * 0.5 - 2.0;
    tr.draw_text(pixmap, x, baseline, label, size, widgets::color_muted());
    // Divider rule, vertically centred, starting a gap past the caption.
    let caption_w = tr.text_width(label, size);
    let rule_x = x + caption_w + 8.0 * scale;
    let rule_w = (x + w) - rule_x;
    if rule_w > 0.0 {
        let rule_h = scale.max(1.0);
        widgets::draw_rect(
            pixmap,
            rule_x,
            y + (h - rule_h) * 0.5,
            rule_w,
            rule_h,
            widgets::color_border(),
        );
    }
}
```

If `widgets::color_muted` or `widgets::color_border` are not exported under
those paths, use the colour helpers the editor already uses for muted text and
hairlines (check how `draw_effect_section` colours the title / how `grid_view`
draws its hairlines) — match the existing palette.

- [ ] **Step 6: Draw the two headers from `draw_effect_view`**

In `multosis/src/editor.rs`, find `draw_effect_view` (the method that draws the
effect editor — it calls `effect_editor::draw_effect_section`, `draw_mseg`,
`draw_modulation_controls`, etc.). After it computes `lay` (the
`effect_layout` result) and draws `draw_effect_section`, add:

```rust
        effect_editor::draw_section_header(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            lay.effect_header,
            "EFFECT",
            self.scale_factor,
        );
        effect_editor::draw_section_header(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            lay.modulation_header,
            "MODULATION",
            self.scale_factor,
        );
```

Match the exact field names this method uses for the pixmap, the text
renderer, and the scale factor (read the surrounding `draw_effect_section`
call — use the identical receiver expressions). Placement among the other
draw calls does not matter visually (the header bands do not overlap any
control), but put them right after `draw_effect_section` for readability.

- [ ] **Step 7: Build, lint, full test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo nextest run -p multosis`
Expected: all PASS, no warnings. The existing layout tests
(`layout_rects_are_disjoint_for_the_main_controls`,
`mix_dial_is_hit_and_disjoint_from_other_effect_controls`) must still pass —
the shift preserves disjointness. If one fails, it asserts a specific rect
relationship the shift changed; re-read it and fix the assertion only if it
encoded an offset that legitimately moved (do not weaken a real invariant).

- [ ] **Step 8: Commit**

```bash
git add multosis/src/editor.rs multosis/src/editor/effect_editor.rs
git commit -m "$(cat <<'EOF'
feat(multosis): EFFECT / MODULATION section headers

Label the two effect-editor sections with a caption + divider rule.
The EFFECT controls shift down 16 px and the MODULATION controls 32 px
to give each header a clear band; the MSEG pane shifts down 32 px and
loses 32 px of height so its bottom edge stays put. The header rects
are draw-only — not hit-tested.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Final verification and bundle

- [ ] **Step 1: Full workspace check**

Run: `cargo build --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check && cargo nextest run -p multosis`
Expected: all PASS, no warnings.

- [ ] **Step 2: Bundle**

Run: `cargo xtask native nih-plug bundle multosis --release`
Expected: standalone + CLAP + VST3 bundles under `target/bundled/`.

- [ ] **Step 3: Manual smoke check**

Run the standalone and confirm: the effect editor shows an `EFFECT` header
above the kind dropdown / dials / Mix dial and a `MODULATION` header above the
trigger / MSEG row, each with a divider rule; the MSEG pane sits below both
with no overlap; every control still responds to clicks/drags at its new
position; switching a track's effect kind still resets its parameters.

---

## Self-Review

**Spec coverage:**
- §1 section headers — caption + rule, ~16 px bands, EFFECT shift 16 / MODULATION shift 32, MSEG pane shrinks 32 → Task 3. ✓
- §1 header rects in `EffectLayout`, draw-only (not in `effect_hit`) → Task 3 Steps 1–2 add fields; nothing adds them to `effect_hit`. ✓
- §1 EFFECT header drawn for the EFFECT section, MODULATION header for its section → Task 3 Step 6 draws both via `draw_section_header`. ✓
- §2 rename `kind_dropdown` → `effect_dropdown`, `editor.rs` only → Task 1. ✓
- §3 extract `switch_effect_kind(effect, modulation, kind)` into `modulation.rs`; `apply_kind_switch` delegates → Task 2. ✓
- §3 integration test composing kind + params + clamp → Task 2 Step 1 (two tests: the clamp case and the in-range-preserved case). ✓
- Testing: header layout disjointness test → Task 3 Step 3; `switch_effect_kind` test → Task 2. ✓

**Placeholder scan:** No TBD/TODO. Steps that may need codebase-specific
adjustment (colour helper names in Task 3 Step 5, receiver expressions in
Step 6, import style in Task 2 Step 3) state the fallback explicitly rather
than leaving a gap.

**Type consistency:** `switch_effect_kind(&mut TrackEffect, &mut TrackModulation,
EffectKind)` is defined in Task 2 Step 3 and called identically in Step 5 and
the tests in Step 1. `EffectLayout.effect_header` / `.modulation_header`
(Task 3 Step 1) are populated in Step 2, asserted in Step 3, and consumed in
Step 6. `draw_section_header(pixmap, tr, rect, label, scale)` is defined in
Step 5 and called with that arity in Step 6.
