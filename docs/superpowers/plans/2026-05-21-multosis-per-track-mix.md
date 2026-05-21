# Multosis Per-Track Mix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give each multosis track an independent dry/wet Mix knob, applied to its effect lane before the amplitude MSEG and the wet-bus sum.

**Architecture:** A `mix: f32` field on the persisted `TrackEffect`. In the audio engine, each active row's effect output is blended with dry — `lane = dry + (effect − dry)·mix` — before the existing amplitude scaling and summation. A Mix dial in the effect editor's EFFECT section edits the field, reusing the param-dial drag / double-click / text-entry machinery.

**Tech Stack:** Rust (nightly), nih-plug plugin, `cargo nextest`. Workspace `/home/mpd/git-sources/tract-plugin-pack`, crate `multosis`, branch `multosis`.

**Spec:** `docs/superpowers/specs/2026-05-21-multosis-per-track-mix-design.md`

**Conventions:**
- Build: `cargo build -p multosis`. Tests: `cargo nextest run -p multosis`. Lint: `cargo clippy -p multosis -- -D warnings`. Format: `cargo fmt -p multosis`.
- Never use `#[allow(...)]` to silence a warning without strong justification.
- Commit message trailer MUST be exactly:
  `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Editor diagnostics are often stale — verify with a real `cargo build` / `cargo nextest run`.

---

## Task 1: Add the `mix` field to `TrackEffect`

`TrackEffect` (`multosis/src/effects.rs`) is the per-track persisted effect
config. Add a `mix: f32` dry/wet field, defaulting to `1.0` (fully wet), with
a serde default so presets predating the field load as fully wet.

**Files:**
- Modify: `multosis/src/effects.rs` — `TrackEffect` struct, `default_for_row`, a serde default fn, tests

- [ ] **Step 1: Write the failing tests**

In `multosis/src/effects.rs`, inside the existing `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn track_effect_default_is_fully_wet() {
        assert_eq!(TrackEffect::default_for_row(0).mix, 1.0);
        assert_eq!(TrackEffect::default().mix, 1.0);
    }

    #[test]
    fn track_effect_mix_round_trips_through_serde() {
        let mut te = TrackEffect::default_for_row(0);
        te.mix = 0.35;
        let json = serde_json::to_string(&te).unwrap();
        let back: TrackEffect = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mix, 0.35);
    }

    #[test]
    fn track_effect_legacy_blob_without_mix_loads_fully_wet() {
        // A TrackEffect JSON saved before the `mix` field existed.
        let legacy = r#"{"kind":"Lowpass","params":[0.0,0.0,0.0,0.0]}"#;
        let te: TrackEffect = serde_json::from_str(legacy).expect("legacy blob must load");
        assert_eq!(te.mix, 1.0);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p multosis track_effect_`
Expected: FAIL — `TrackEffect` has no `mix` field.

- [ ] **Step 3: Add the field, the serde default, and update `default_for_row`**

In `multosis/src/effects.rs`, replace the `TrackEffect` struct and its
`impl` block (currently the struct with `kind` + `params`, and
`default_for_row`) with:

```rust
/// One track row's persisted effect configuration: which effect, its
/// parameter values, and its dry/wet mix. `params[i]` is the value for the
/// kind's `parameters()[i]`; entries past the kind's parameter count are
/// unused.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrackEffect {
    pub kind: EffectKind,
    pub params: [f32; MAX_EFFECT_PARAMS],
    /// Per-track dry/wet blend, 0.0 (dry) .. 1.0 (full effect). Defaulted on
    /// deserialize so presets predating this field load as fully wet.
    #[serde(default = "default_track_mix")]
    pub mix: f32,
}

/// The serde default for `TrackEffect::mix` — fully wet, matching the
/// pre-`mix` behaviour of any older preset.
fn default_track_mix() -> f32 {
    1.0
}

impl TrackEffect {
    /// The default effect for a track row — no effect, fully wet. Audio
    /// passes through the track unchanged. Users assign an effect kind via
    /// the editor's dropdown.
    pub fn default_for_row(_row: usize) -> Self {
        TrackEffect {
            kind: EffectKind::None,
            params: [0.0; MAX_EFFECT_PARAMS],
            mix: 1.0,
        }
    }
}
```

Leave the `impl Default for TrackEffect` block (it delegates to
`default_for_row`) unchanged.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p multosis track_effect_`
Expected: PASS — all three tests.

- [ ] **Step 5: Build, lint, full test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo nextest run -p multosis`
Expected: all PASS, no warnings. If any other `TrackEffect { ... }` struct
literal in the crate now fails to compile (a missing `mix` field), fix it by
adding `mix: 1.0` — grep `rg -n 'TrackEffect \{' multosis/src` to check.

- [ ] **Step 6: Commit**

```bash
git add multosis/src/effects.rs
git commit -m "$(cat <<'EOF'
feat(multosis): add a per-track dry/wet mix field to TrackEffect

TrackEffect gains a `mix: f32` (0.0 dry .. 1.0 wet), defaulting to 1.0.
A serde default keeps presets saved before this field loading as fully
wet, so the change is sound-transparent until a Mix knob is moved.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Blend each track's lane in the audio engine

`AudioEngine::process_sample` (`multosis/src/engine.rs`) sums each active
row's effect output, scaled by the amplitude MSEG. Insert the per-track
dry/wet blend before the amplitude scaling. `AudioEngine` already stores
`track_effects: [TrackEffect; ROWS]` (bridged by `set_effects`).

**Files:**
- Modify: `multosis/src/engine.rs` — `process_sample`, tests

- [ ] **Step 1: Write the failing test**

In `multosis/src/engine.rs`, inside `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn per_track_mix_zero_makes_an_active_track_contribute_dry() {
        // Row 0 runs a Bitcrush effect (audibly alters the signal) at mix 0.0;
        // the lane must collapse to the dry input. Compared against the same
        // setup at mix 1.0, which alters it.
        use crate::effects::{EffectKind, TrackEffect, MAX_EFFECT_PARAMS};

        let mut wet = TrackEffect::default_for_row(0);
        wet.kind = EffectKind::Bitcrush;
        wet.params = crate::effects::default_params_for_kind(EffectKind::Bitcrush);
        wet.mix = 1.0;
        let mut dry_mix = wet;
        dry_mix.mix = 0.0;

        // Two engines, identical but for row 0's mix.
        let build = |te: TrackEffect| {
            let mut e = AudioEngine::new();
            e.set_sample_rate(48_000.0);
            let mut effects = [TrackEffect::default_for_row(0); ROWS];
            effects[0] = te;
            e.set_effects(&effects);
            e
        };
        let mut e_wet = build(wet);
        let mut e_dry = build(dry_mix);

        let grid = Grid::default();
        // A constant non-zero input; fully wet so the lane reaches the output.
        let input = [0.6_f32; 128];
        let (mut wl, mut wr) = (input, input);
        let (mut dl, mut dr) = (input, input);
        e_wet.process(&mut wl, &mut wr, true, 1000.0, 120.0, 1.0, &grid);
        e_dry.process(&mut dl, &mut dr, true, 1000.0, 120.0, 1.0, &grid);

        // At mix 0.0 row 0's lane is dry, so its output equals the dry input.
        assert!(
            dl.iter().all(|&s| (s - 0.6).abs() < 1e-4),
            "mix 0.0 active track should output dry"
        );
        // At mix 1.0 the Bitcrush alters the signal — outputs differ.
        assert!(
            wl.iter().zip(dl.iter()).any(|(&w, &d)| (w - d).abs() > 1e-4),
            "mix 1.0 should differ from mix 0.0"
        );
        let _ = (wr, dr, MAX_EFFECT_PARAMS);
    }
```

Note on the test setup: the default grid has every cell enabled, so on the
first step boundary every row is active. With only row 0 carrying a real
effect and the rest `EffectKind::None` (which contributes silence), the wet
bus is row 0's lane alone. `samples_per_step` is large (`1000.0`) so the
128-sample block crosses just the sample-0 boundary.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p multosis per_track_mix_zero`
Expected: FAIL — `process_sample` does not blend yet, so the mix-0 engine
still applies Bitcrush and its output is not the dry `0.6`.

- [ ] **Step 3: Blend the lane in `process_sample`**

In `multosis/src/engine.rs`, replace the body of `process_sample` with:

```rust
    /// Apply the active rows' effects to one dry stereo sample and sum them.
    /// Each active row's effect output is first blended with the dry input by
    /// that row's per-track `mix` (`lane = dry + (effect − dry)·mix`), then
    /// scaled by the row's amplitude MSEG. The sum is deliberately
    /// un-normalised — the wet-bus compressor and the global mix manage the
    /// parallel-row peak.
    fn process_sample(&mut self, dry_l: f32, dry_r: f32, active: u16) -> (f32, f32) {
        let mut wet_l = 0.0;
        let mut wet_r = 0.0;
        for r in 0..ROWS {
            if active & (1 << r) == 0 {
                continue;
            }
            let (eff_l, eff_r) = self.effects[r].process_sample(dry_l, dry_r);
            // Per-track dry/wet blend, before the amplitude MSEG.
            let mix = self.track_effects[r].mix;
            let lane_l = dry_l + (eff_l - dry_l) * mix;
            let lane_r = dry_r + (eff_r - dry_r) * mix;
            let amp = self.modulation.amplitude(r);
            wet_l += amp * lane_l;
            wet_r += amp * lane_r;
        }
        (wet_l, wet_r)
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p multosis per_track_mix_zero`
Expected: PASS.

- [ ] **Step 5: Build, lint, full test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo nextest run -p multosis`
Expected: all PASS, no warnings. Existing engine tests must still pass — the
default `mix == 1.0` makes `lane == effect`, byte-identical to before.

- [ ] **Step 6: Commit**

```bash
git add multosis/src/engine.rs
git commit -m "$(cat <<'EOF'
feat(multosis): apply the per-track mix in the audio engine

process_sample now blends each active row's effect output with the dry
input by that row's TrackEffect.mix before the amplitude MSEG:
lane = dry + (effect - dry)*mix. At mix 1.0 (the default) lane == effect,
so existing behaviour is unchanged; at mix 0.0 an active track's hit is
dry. One lerp per channel per active row, allocation-free.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Mix dial — layout, hit-test, drawing, drag, double-click reset

Add a Mix dial to the effect editor's EFFECT section: a fixed slot to the
right of the four effect-parameter dial slots. It reuses the shared
`param_dial` widget and the param-dial drag / double-click-reset machinery.
Text entry is added in Task 4.

**Files:**
- Modify: `multosis/src/editor/effect_editor.rs` — `EffectLayout`, `effect_layout`, `EffectHit`, `effect_hit`, `draw_effect_section`, tests
- Modify: `multosis/src/editor.rs` — press handler, `CursorMoved` drag handler, two helper methods

- [ ] **Step 1: Add the `mix` rect to `EffectLayout`**

In `multosis/src/editor/effect_editor.rs`, add a field to the `EffectLayout`
struct (after `dials`):

```rust
    /// Per-track Mix dial — a fixed slot right of the parameter dials.
    pub mix: (f32, f32, f32, f32),
```

In `effect_layout`, after the `dials` line, add:

```rust
    // Per-track Mix dial: a fixed slot to the right of the four param-dial
    // slots (which end at ox+556), clearly set apart from them.
    let mix = l(ox + 580.0, oy + 44.0, 88.0, 88.0);
```

and add `mix` to the returned `EffectLayout { ... }` literal.

- [ ] **Step 2: Add the `EffectHit::Mix` variant and hit-test**

In the same file, add a variant to `EffectHit`:

```rust
    /// The per-track Mix dial.
    Mix,
```

In `effect_hit`, immediately after the param-dial loop (the
`for i in 0..param_count.min(DIAL_SLOTS)` block), add:

```rust
    if in_rect(lay.mix, px, py) {
        return Some(EffectHit::Mix);
    }
```

- [ ] **Step 3: Write the failing layout test**

In `effect_editor.rs`'s `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn mix_dial_is_hit_and_disjoint_from_other_effect_controls() {
        let lay = effect_layout(1.0);
        // The Mix rect does not overlap the kind dropdown or any param dial.
        assert!(!rects_overlap(lay.mix, lay.kind));
        for d in lay.dials {
            assert!(!rects_overlap(lay.mix, d));
        }
        // A point inside the Mix rect hit-tests to EffectHit::Mix.
        let (mx, my, mw, mh) = lay.mix;
        let hit = effect_hit(mx + mw / 2.0, my + mh / 2.0, 1.0, 2, 0, false);
        assert_eq!(hit, Some(EffectHit::Mix));
    }
```

(The `rects_overlap` helper already exists in that test module.)

- [ ] **Step 4: Run the test to verify it fails, then passes**

Run: `cargo nextest run -p multosis mix_dial_is_hit`
Expected: it should now PASS (Steps 1–2 already added the layout + hit-test).
If it fails, the layout rect overlaps something — adjust the `ox + 580.0`
offset rightward until disjoint, keeping it within the main area.

- [ ] **Step 5: Draw the Mix dial**

In `draw_effect_section` (`effect_editor.rs`), after the parameter-dials
`for` loop, add:

```rust
    // Per-track Mix dial — value shown as a percentage.
    let (mx, my, mw, mh) = lay.mix;
    let mix_pct = format!("{}%", (track.mix * 100.0).round() as i32);
    widgets::param_dial::draw_dial(
        pixmap,
        tr,
        mx + mw / 2.0,
        my + mh / 2.0,
        (mw.min(mh) / 2.0) - 8.0 * scale,
        "Mix",
        &mix_pct,
        track.mix.clamp(0.0, 1.0),
    );
```

- [ ] **Step 6: Add the editor press handler for `EffectHit::Mix`**

In `multosis/src/editor.rs`, in the effect-editor press-handler `match` (the
one with arms `EffectHit::Dial(i)`, `EffectHit::Depth`, etc.), add an arm —
mirror the `EffectHit::Dial(i)` arm:

```rust
            EffectHit::Mix => {
                if self.effect_click.check_and_update(EffectHit::Mix) {
                    self.reset_mix_to_default();
                } else {
                    let norm = self.selected_track_effect().mix;
                    self.effect_dial_drag.begin_drag(EffectHit::Mix, norm, false);
                }
            }
```

- [ ] **Step 7: Add the `CursorMoved` drag handler for `EffectHit::Mix`**

In `editor.rs`, in the `match self.effect_dial_drag.active_action().copied()`
block (the one with `Some(EffectHit::Dial(i))`, `Some(EffectHit::Depth)`,
etc.), add an arm:

```rust
                    Some(EffectHit::Mix) => {
                        let current = self.selected_track_effect().mix;
                        if let Some(norm) = self.effect_dial_drag.update_drag(shift, current) {
                            self.apply_mix(norm);
                        }
                    }
```

- [ ] **Step 8: Add the `apply_mix` and `reset_mix_to_default` helpers**

In `editor.rs`, next to `apply_effect_dial` / `reset_effect_dial_to_default`,
add:

```rust
    /// Apply a Mix-dial drag's new normalized value (0..1) to the selected
    /// track's per-track mix, marking config dirty.
    fn apply_mix(&mut self, norm: f32) {
        let value = norm.clamp(0.0, 1.0);
        if let Ok(mut cfg) = self.params.track_effects.lock() {
            cfg[self.selected_track].mix = value;
        }
        self.mark_config_dirty();
    }

    /// Reset the selected track's per-track mix to fully wet (1.0). Marks
    /// dirty. Backs double-clicking the Mix dial.
    fn reset_mix_to_default(&mut self) {
        if let Ok(mut cfg) = self.params.track_effects.lock() {
            cfg[self.selected_track].mix = 1.0;
        }
        self.mark_config_dirty();
    }
```

The drag-release path (`self.effect_dial_drag.end_drag()`) already handles
any `EffectHit` generically — no change needed there.

- [ ] **Step 9: Build, lint, full test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo nextest run -p multosis`
Expected: all PASS, no warnings.

- [ ] **Step 10: Commit**

```bash
git add multosis/src/editor.rs multosis/src/editor/effect_editor.rs
git commit -m "$(cat <<'EOF'
feat(multosis): add the per-track Mix dial to the effect editor

A Mix dial sits in the EFFECT section, right of the effect-parameter
dials. Vertical drag sets the per-track dry/wet (0..100%); double-click
resets to 100%. Editing it writes TrackEffect.mix and marks config
dirty, so the audio thread re-bridges on the next block.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Right-click text entry on the Mix dial

The effect-parameter dials support right-click text entry via
`TextEditState<EffectHit>`. Extend it to the Mix dial: right-click opens an
entry seeded with the current percentage; Enter commits, parsing the typed
number back to a 0..1 mix.

**Files:**
- Modify: `multosis/src/editor.rs` — right-click handler, `text_edit.commit()` sites, a `commit_mix_text_edit` helper, the `draw_effect_section` call
- Modify: `multosis/src/editor/effect_editor.rs` — `draw_effect_section` renders the Mix dial's edit buffer

- [ ] **Step 1: Render an in-progress Mix edit in `draw_effect_section`**

In `multosis/src/editor/effect_editor.rs`, add a parameter to
`draw_effect_section` — after `editing_dial`:

```rust
    /// `Some((buffer, caret_on))` when a right-click text edit is active on
    /// the Mix dial; it renders the buffer + caret in place of the percentage.
    editing_mix: Option<(&str, bool)>,
```

Replace the Mix-dial draw block (added in Task 3 Step 5) with:

```rust
    // Per-track Mix dial — value shown as a percentage, or the edit buffer
    // when a text entry is active on it.
    let (mx, my, mw, mh) = lay.mix;
    let mix_cx = mx + mw / 2.0;
    let mix_cy = my + mh / 2.0;
    let mix_radius = (mw.min(mh) / 2.0) - 8.0 * scale;
    let mix_pct = format!("{}%", (track.mix * 100.0).round() as i32);
    match editing_mix {
        Some((buf, caret_on)) => {
            widgets::param_dial::draw_dial_ex(
                pixmap,
                tr,
                mix_cx,
                mix_cy,
                mix_radius,
                "Mix",
                &mix_pct,
                track.mix.clamp(0.0, 1.0),
                None,
                Some(buf),
                caret_on,
            );
        }
        None => {
            widgets::param_dial::draw_dial(
                pixmap,
                tr,
                mix_cx,
                mix_cy,
                mix_radius,
                "Mix",
                &mix_pct,
                track.mix.clamp(0.0, 1.0),
            );
        }
    }
```

- [ ] **Step 2: Pass `editing_mix` from the `draw_effect_section` call site**

In `multosis/src/editor.rs`, find the `draw_effect_section(...)` call (in the
effect-view draw method, near the `editing_dial` computation). The existing
code builds `editing_dial` from `self.text_edit.active_for_any()` matching
`EffectHit::Dial(i)`. Add, alongside it, a `editing_mix` binding:

```rust
        let editing_mix: Option<(&str, bool)> = match self.text_edit.active_for_any() {
            Some(EffectHit::Mix) => {
                let caret_on = self.text_edit.caret_visible();
                self.text_edit
                    .active_for(&EffectHit::Mix)
                    .map(|buf| (buf, caret_on))
            }
            _ => None,
        };
```

and pass `editing_mix` as the new argument to `draw_effect_section(...)`.

- [ ] **Step 3: Add the `commit_mix_text_edit` helper**

In `editor.rs`, next to `commit_dial_text_edit`, add:

```rust
    /// Parse a Mix-dial text entry (a percentage, e.g. `50` or `50%`), clamp
    /// to 0..100, and write it as a 0..1 mix. A parse failure is a silent
    /// no-op — the dial keeps its previous value.
    fn commit_mix_text_edit(&mut self, text: &str) {
        let cleaned = text.trim().trim_end_matches('%').trim();
        if let Ok(pct) = cleaned.parse::<f32>() {
            let value = (pct / 100.0).clamp(0.0, 1.0);
            if let Ok(mut cfg) = self.params.track_effects.lock() {
                cfg[self.selected_track].mix = value;
            }
            self.mark_config_dirty();
        }
    }
```

- [ ] **Step 4: Handle `EffectHit::Mix` at every `text_edit.commit()` site**

In `editor.rs` there are `text_edit.commit()` call sites that match only
`EffectHit::Dial`. Grep them: `rg -n 'text_edit.commit\(\)' multosis/src/editor.rs`.
Each currently reads like:

```rust
if let Some((EffectHit::Dial(i), text)) = self.text_edit.commit() {
    self.commit_dial_text_edit(i, &text);
}
```

`commit()` consumes the active edit; if the active edit is `EffectHit::Mix`
the `if let` would not match and the typed value would be silently dropped.
Change each of these sites to handle both:

```rust
match self.text_edit.commit() {
    Some((EffectHit::Dial(i), text)) => self.commit_dial_text_edit(i, &text),
    Some((EffectHit::Mix, text)) => self.commit_mix_text_edit(&text),
    _ => {}
}
```

Apply this at every `text_edit.commit()` site that currently special-cases
`EffectHit::Dial` (there are three: the auto-commit-on-click-outside path,
the right-click-a-different-dial path, and the Enter-key path). Where a site
also has an `else { self.text_edit.cancel(); }`, fold that into the `_ =>`
arm only if the original did — otherwise keep behaviour identical, just add
the `EffectHit::Mix` arm.

- [ ] **Step 5: Begin a Mix text edit on right-click**

In `editor.rs`, the right-click handler hit-tests for `EffectHit::Dial(i)`
and calls `text_edit.begin(...)`. After that `if let Some(EffectHit::Dial(i))`
block (and before the `mseg_pane` right-click check), add a parallel block
for the Mix dial:

```rust
                if let Some(EffectHit::Mix) = effect_editor::effect_hit(
                    px,
                    py,
                    self.scale_factor,
                    param_count,
                    self.selected_mseg,
                    is_free_hz,
                ) {
                    // Commit any prior edit before seeding a new one.
                    match self.text_edit.commit() {
                        Some((EffectHit::Dial(prev), text)) => {
                            self.commit_dial_text_edit(prev, &text)
                        }
                        Some((EffectHit::Mix, text)) => self.commit_mix_text_edit(&text),
                        _ => {}
                    }
                    let pct = (self.selected_track_effect().mix * 100.0).round() as i32;
                    self.text_edit.begin(EffectHit::Mix, &format!("{pct}"));
                    return baseview::EventStatus::Captured;
                }
```

- [ ] **Step 6: Build, lint, full test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo nextest run -p multosis`
Expected: all PASS, no warnings.

- [ ] **Step 7: Write a parse test**

In `editor.rs`'s `#[cfg(test)] mod tests` (if one exists; otherwise skip the
unit test and rely on the build — note which in the commit), a pure parse
check is hard to isolate from the editor struct. Instead verify by manual
reasoning in the commit message that `"50"` → `0.5`, `"50%"` → `0.5`,
`"150"` → clamped `1.0`, `"abc"` → no-op. If `editor.rs` has no test module,
do not add one solely for this — the `commit_mix_text_edit` logic is small
and covered by inspection.

- [ ] **Step 8: Commit**

```bash
git add multosis/src/editor.rs multosis/src/editor/effect_editor.rs
git commit -m "$(cat <<'EOF'
feat(multosis): right-click text entry on the per-track Mix dial

Right-clicking the Mix dial opens a text entry seeded with the current
percentage; Enter commits, parsing the number (with an optional %) back
to a 0..1 mix, clamped. Mirrors the effect-parameter dials' text-entry
path; every text_edit.commit() site now handles EffectHit::Mix so a
typed value is never silently dropped.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Final verification and bundle

- [ ] **Step 1: Full workspace check**

Run: `cargo build --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check && cargo nextest run -p multosis`
Expected: all PASS, no warnings.

- [ ] **Step 2: Bundle**

Run: `cargo xtask native nih-plug bundle multosis --release`
Expected: standalone + CLAP + VST3 bundles created under `target/bundled/`.

- [ ] **Step 3: Manual smoke check**

Run the standalone and confirm: each track's effect editor shows a Mix dial
right of the parameter dials; dragging it changes that track's dry/wet;
double-click resets it to 100%; right-click types a percentage; a track at
Mix 0% with an effect assigned sounds dry on its hits while a track at 100%
applies its effect; the global Mix still works as a master.

---

## Self-Review

**Spec coverage:**
- `TrackEffect.mix` field, range 0..1, default 1.0, serde back-compat default → Task 1. ✓
- Engine `lane = dry + (effect − dry)·mix` before amplitude MSEG → Task 2. ✓
- Mix dial in EFFECT section, fixed slot, set apart → Task 3 (layout `ox+580`). ✓
- Dial affordances: drag, double-click reset to 100% → Task 3; right-click text entry → Task 4. ✓
- Persisted config, marks dirty, audio re-bridges → Tasks 3 & 4 (`apply_mix`/`commit_mix_text_edit` call `mark_config_dirty`). ✓
- Mix dial drawn for every kind incl. `None` → Task 3 Step 5 draws unconditionally. ✓
- Not an MSEG target / no new nih-plug params → nothing in the plan adds either. ✓
- Tests: lane blend, engine mix-0-is-dry, default, serde round-trip + legacy, layout/hit → Tasks 1–3. ✓

**Placeholder scan:** No TBD/TODO. Task 4 Step 7 explicitly handles the "no
editor test module" case rather than leaving it vague.

**Type consistency:** `TrackEffect.mix: f32` (Task 1) is read in `engine.rs`
(Task 2), `effect_editor.rs` draw (Tasks 3–4), and `editor.rs`
`apply_mix`/`reset_mix_to_default`/`commit_mix_text_edit` (Tasks 3–4).
`EffectHit::Mix` (Task 3) is used in `effect_hit`, the press handler, the
drag handler (Task 3), and the text-edit sites (Task 4). `EffectLayout.mix`
(Task 3) is consumed by `effect_hit` and `draw_effect_section`. Consistent.
