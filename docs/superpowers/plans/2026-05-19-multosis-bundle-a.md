# Multosis Bundle A — Dial Polish — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Per-`ParamSpec` value formatting and log-scaling, plus right-click text entry on the effect-editor parameter dials.

**Architecture:** Extend `ParamSpec` with `scaling: ParamScaling` and `format: ParamFormat` enums; expose shared `value_to_norm` / `norm_to_value` / `format_value` / `parse_value` helpers in `effects.rs`. The editor swaps its local `normalize_param`/`format!("{value:.0}")` and the Phase 3 trigger-rate `norm_to_hz`/`hz_to_norm` to call the shared helpers. Text entry on `EffectHit::Dial(i)` wires `TextEditState<EffectHit>` (same pattern as gain-brain, miff, satch, six-pack, tinylimit) and adds a `Keyboard` event arm to `MultosisWindow::on_event`.

**Tech Stack:** Rust (nightly), nih-plug, `tiny-skia-widgets`, `cargo nextest`.

**Reference:** `docs/superpowers/specs/2026-05-19-multosis-bundle-a-design.md`.

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** every commit message ends with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` lines below omit it — add it to each.

---

## Pre-existing state (203 multosis tests after Phase 3)

- `multosis/src/effects.rs`:
  - `pub struct ParamSpec { pub name: &'static str, pub min: f32, pub max: f32, pub default: f32 }` — `Clone, Copy, Debug`.
  - `LowpassEffect::PARAMS: [ParamSpec; 2]` — `{name: "Cutoff", 20.0, 20_000.0, 2_000.0}`, `{name: "Resonance", 0.0, 1.0, 0.1}`.
  - `BitcrushEffect::PARAMS: [ParamSpec; 2]` — `{name: "Bit Depth", 1.0, 16.0, 16.0}`, `{name: "Rate Reduction", 1.0, 50.0, 1.0}`.
- `multosis/src/editor.rs`:
  - `fn normalize_param(value: f32, spec: ParamSpec) -> f32` — local helper, linear only.
  - `fn apply_effect_dial(&mut self, i: usize, norm: f32)` — body `let spec = self.param_spec(i)?; let v = spec.min + norm * (spec.max - spec.min); …`.
  - `MultosisWindow` holds `effect_dial_drag: widgets::DragState<EffectHit>`, `kind_dropdown: DropdownState<EffectAction>`, `mseg_last_click_time/_pos`, etc. There is **no** `text_edit` field yet, and **no `baseview::Event::Keyboard` arm** in `on_event`.
- `multosis/src/editor/effect_editor.rs`:
  - `pub const TRIGGER_RATE_MIN_HZ: f32 = 0.05; pub const TRIGGER_RATE_MAX_HZ: f32 = 20.0;`
  - `pub fn norm_to_hz(norm: f32) -> f32` / `pub fn hz_to_norm(hz: f32) -> f32` — log mapping.
  - `draw_effect_section` formats each param dial's value as `format!("{value:.0}")` and calls `widgets::param_dial::draw_dial(...)` (not `draw_dial_ex`).
  - `draw_trigger_controls` formats the rate as `format!("{hz:.2} Hz")` and calls `draw_dial`.
  - A test `hz_norm_round_trips_within_range` (in the file's `#[cfg(test)] mod tests`) covers the local log helpers — Bundle A retires this test along with the helpers.
- `tiny-skia-widgets`:
  - `TextEditState<A: Clone + PartialEq>`: `new()`, `begin(action: A, initial: &str)`, `active_for(&action) -> Option<&str>`, `cancel()`, `insert_char(c)`, `backspace()`, `commit() -> Option<(A, String)>`, `is_active() -> bool`, `caret_visible() -> bool`.
  - `param_dial::draw_dial_ex(pixmap, tr, cx, cy, radius, label, value_text, normalized, modulated_normalized: Option<f32>, editing_text: Option<&str>, caret_on: bool)` — already supports the edit overlay.
  - Reference consumer: `gain-brain/src/editor.rs` — search for `text_edit:` / `TextEditState` / `insert_char` / `Keyboard` to see the keyboard-event arm and the begin/commit flow.

---

### Task 1: `ParamSpec` extension + shared helpers

**Files:**
- Modify: `multosis/src/effects.rs`

This task introduces the new enums, helpers, and per-effect param-table updates as one cohesive change — all in `effects.rs`. No consumer code changes yet (Task 2 does that).

- [ ] **Step 1: Write the failing tests**

Append to `effects.rs`'s `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn value_to_norm_linear_round_trips() {
        // Linear: midpoint of 0..1 is 0.5; midpoint of 20..40 is 0.5.
        assert!((value_to_norm(0.5, 0.0, 1.0, ParamScaling::Linear) - 0.5).abs() < 1e-6);
        assert!((value_to_norm(30.0, 20.0, 40.0, ParamScaling::Linear) - 0.5).abs() < 1e-6);
        // Round trip.
        for v in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            let n = value_to_norm(v, 0.0, 1.0, ParamScaling::Linear);
            let back = norm_to_value(n, 0.0, 1.0, ParamScaling::Linear);
            assert!((back - v).abs() < 1e-6, "v={v} n={n} back={back}");
        }
        // Out of range clamps.
        assert_eq!(value_to_norm(-1.0, 0.0, 1.0, ParamScaling::Linear), 0.0);
        assert_eq!(value_to_norm(2.0, 0.0, 1.0, ParamScaling::Linear), 1.0);
    }

    #[test]
    fn value_to_norm_log_round_trips() {
        // Log: 20 -> 0.0, 20000 -> 1.0, midpoint (geometric mean) ≈ 632 Hz.
        assert!((value_to_norm(20.0, 20.0, 20_000.0, ParamScaling::Log) - 0.0).abs() < 1e-4);
        assert!((value_to_norm(20_000.0, 20.0, 20_000.0, ParamScaling::Log) - 1.0).abs() < 1e-4);
        // 20 * sqrt(1000) ≈ 632.4555
        let geo = 20.0_f32 * 1000.0_f32.sqrt();
        assert!((value_to_norm(geo, 20.0, 20_000.0, ParamScaling::Log) - 0.5).abs() < 1e-4);
        // Round trip.
        for v in [20.0_f32, 80.0, 200.0, 2_000.0, 20_000.0] {
            let n = value_to_norm(v, 20.0, 20_000.0, ParamScaling::Log);
            let back = norm_to_value(n, 20.0, 20_000.0, ParamScaling::Log);
            assert!((back - v).abs() / v < 1e-4, "v={v} n={n} back={back}");
        }
        // Out of range clamps; degenerate (min<=0) returns 0.
        assert_eq!(value_to_norm(1.0, 20.0, 20_000.0, ParamScaling::Log), 0.0);
        assert_eq!(value_to_norm(40_000.0, 20.0, 20_000.0, ParamScaling::Log), 1.0);
        assert_eq!(value_to_norm(5.0, 0.0, 100.0, ParamScaling::Log), 0.0);
    }

    #[test]
    fn format_value_number_with_and_without_unit() {
        assert_eq!(
            format_value(0.15, ParamFormat::Number { decimals: 2, unit: "" }),
            "0.15"
        );
        assert_eq!(
            format_value(8.0, ParamFormat::Number { decimals: 0, unit: "bits" }),
            "8 bits"
        );
        assert_eq!(
            format_value(4.0, ParamFormat::Number { decimals: 0, unit: "x" }),
            "4 x"
        );
    }

    #[test]
    fn format_value_hertz_auto_scales() {
        assert_eq!(format_value(0.05, ParamFormat::Hertz), "0.05 Hz");
        assert_eq!(format_value(80.0, ParamFormat::Hertz), "80 Hz");
        assert_eq!(format_value(2_000.0, ParamFormat::Hertz), "2.0 kHz");
        assert_eq!(format_value(18_500.0, ParamFormat::Hertz), "18.5 kHz");
    }

    #[test]
    fn parse_value_number_strips_unit() {
        let fmt = ParamFormat::Number { decimals: 0, unit: "bits" };
        assert_eq!(parse_value("8 bits", fmt), Some(8.0));
        assert_eq!(parse_value("8", fmt), Some(8.0));
        assert_eq!(parse_value("0.15", ParamFormat::Number { decimals: 2, unit: "" }), Some(0.15));
        assert_eq!(parse_value("", fmt), None);
        assert_eq!(parse_value("abc", fmt), None);
    }

    #[test]
    fn parse_value_hertz_handles_k_kHz_Hz() {
        let f = ParamFormat::Hertz;
        assert_eq!(parse_value("80", f), Some(80.0));
        assert_eq!(parse_value("80 Hz", f), Some(80.0));
        assert_eq!(parse_value("80hz", f), Some(80.0));
        assert_eq!(parse_value("2k", f), Some(2_000.0));
        assert_eq!(parse_value("2 kHz", f), Some(2_000.0));
        assert_eq!(parse_value("2.5kHz", f), Some(2_500.0));
        assert_eq!(parse_value("0.5", f), Some(0.5));
        assert_eq!(parse_value("", f), None);
        assert_eq!(parse_value("xyz", f), None);
    }

    #[test]
    fn format_then_parse_round_trips_each_format() {
        let cases: &[(f32, ParamFormat)] = &[
            (0.15, ParamFormat::Number { decimals: 2, unit: "" }),
            (8.0,  ParamFormat::Number { decimals: 0, unit: "bits" }),
            (0.05, ParamFormat::Hertz),
            (80.0, ParamFormat::Hertz),
            (2_000.0, ParamFormat::Hertz),
            (18_500.0, ParamFormat::Hertz),
        ];
        for &(v, f) in cases {
            let s = format_value(v, f);
            let back = parse_value(&s, f).unwrap_or_else(|| panic!("parse failed for {s:?}"));
            assert!((back - v).abs() / v.abs().max(1.0) < 0.05,
                "round-trip {v} -> {s} -> {back}");
        }
    }

    #[test]
    fn lowpass_cutoff_is_log_hertz_and_resonance_is_linear_number() {
        let specs = LowpassEffect::new().parameters();
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert!(matches!(specs[1].scaling, ParamScaling::Linear));
        assert!(matches!(specs[1].format, ParamFormat::Number { .. }));
    }

    #[test]
    fn bitcrush_param_formats_carry_their_units() {
        let specs = BitcrushEffect::new().parameters();
        if let ParamFormat::Number { unit, .. } = specs[0].format {
            assert_eq!(unit, "bits");
        } else {
            panic!("bit-depth format should be Number");
        }
        if let ParamFormat::Number { unit, .. } = specs[1].format {
            assert_eq!(unit, "x");
        } else {
            panic!("rate-reduction format should be Number");
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p multosis --lib -E 'test(value_to_norm) + test(format_value) + test(parse_value) + test(format_then_parse_round_trips) + test(lowpass_cutoff_is_log_hertz) + test(bitcrush_param_formats_carry)'`
Expected: build failure — `cannot find type ParamScaling` / `ParamFormat` / `cannot find function value_to_norm` etc.

- [ ] **Step 3: Write minimal implementation**

In `effects.rs`, add the enums above the `ParamSpec` struct:

```rust
/// How a dial's normalised 0..1 position maps to its parameter value range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamScaling {
    /// `value = min + norm * (max - min)`; norm `= (value - min) / (max - min)`.
    Linear,
    /// `value = min * (max / min).powf(norm)`; norm `= log_(max/min)(value / min)`.
    /// Requires `min > 0` and `max > min`; degenerate ranges fall back to 0/min.
    Log,
}

/// How a parameter value renders as a string on the dial and how a typed
/// string parses back to a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamFormat {
    /// Fixed-decimals number, optional unit suffix. An empty unit prints no
    /// suffix; a non-empty unit prints with a single space separator.
    Number {
        decimals: u8,
        unit: &'static str,
    },
    /// Auto Hz/kHz scaling: < 1 → `"0.05 Hz"` (2 dec); 1..1000 → `"80 Hz"`
    /// (0 dec); ≥ 1000 → `"2.0 kHz"` (1 dec).
    Hertz,
}
```

Extend `ParamSpec` with the two new fields:

```rust
#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub scaling: ParamScaling,
    pub format: ParamFormat,
}
```

Update the per-effect param tables:

```rust
impl LowpassEffect {
    const PARAMS: [ParamSpec; 2] = [
        ParamSpec {
            name: "Cutoff",
            min: 20.0,
            max: 20_000.0,
            default: 2_000.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Resonance",
            min: 0.0,
            max: 1.0,
            default: 0.1,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number { decimals: 2, unit: "" },
        },
    ];
    // ... rest unchanged
}

impl BitcrushEffect {
    const PARAMS: [ParamSpec; 2] = [
        ParamSpec {
            name: "Bit Depth",
            min: 1.0,
            max: 16.0,
            default: 16.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number { decimals: 0, unit: "bits" },
        },
        ParamSpec {
            name: "Rate Reduction",
            min: 1.0,
            max: 50.0,
            default: 1.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number { decimals: 0, unit: "x" },
        },
    ];
    // ... rest unchanged
}
```

Add the four shared helpers (place them after the `ParamSpec` struct, before the first `impl LowpassEffect`):

```rust
/// Map a parameter value to a `0..1` normalised dial position, given the
/// parameter's range and scaling. Clamps to `0..1`. Degenerate ranges
/// (`max <= min`, or `Log` with `min <= 0`) return `0.0`.
pub fn value_to_norm(value: f32, min: f32, max: f32, scaling: ParamScaling) -> f32 {
    if max <= min {
        return 0.0;
    }
    match scaling {
        ParamScaling::Linear => ((value - min) / (max - min)).clamp(0.0, 1.0),
        ParamScaling::Log => {
            if min <= 0.0 {
                return 0.0;
            }
            ((value / min).log(max / min)).clamp(0.0, 1.0)
        }
    }
}

/// Map a normalised dial position to a parameter value, given the
/// parameter's range and scaling. `norm` is clamped to `0..1`. Degenerate
/// ranges return `min`.
pub fn norm_to_value(norm: f32, min: f32, max: f32, scaling: ParamScaling) -> f32 {
    if max <= min {
        return min;
    }
    let n = norm.clamp(0.0, 1.0);
    match scaling {
        ParamScaling::Linear => min + n * (max - min),
        ParamScaling::Log => {
            if min <= 0.0 {
                return min;
            }
            min * (max / min).powf(n)
        }
    }
}

/// Format a parameter value as a display string.
pub fn format_value(value: f32, format: ParamFormat) -> String {
    match format {
        ParamFormat::Number { decimals, unit } => {
            let dec = decimals as usize;
            if unit.is_empty() {
                format!("{value:.dec$}")
            } else {
                format!("{value:.dec$} {unit}")
            }
        }
        ParamFormat::Hertz => {
            let v = value;
            if v.abs() < 1.0 {
                format!("{v:.2} Hz")
            } else if v.abs() < 1000.0 {
                format!("{v:.0} Hz")
            } else {
                format!("{:.1} kHz", v / 1000.0)
            }
        }
    }
}

/// Parse a user-typed string back to a parameter value. Returns `None` on
/// empty input or an unparseable number. The consumer should clamp the
/// result into the parameter's `[min, max]` range.
pub fn parse_value(text: &str, format: ParamFormat) -> Option<f32> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    match format {
        ParamFormat::Number { unit, .. } => {
            // Strip the unit suffix (case-insensitive) if present.
            let body = if !unit.is_empty() && t.to_ascii_lowercase().ends_with(&unit.to_ascii_lowercase()) {
                t[..t.len() - unit.len()].trim()
            } else {
                t
            };
            body.parse::<f32>().ok()
        }
        ParamFormat::Hertz => {
            let lower = t.to_ascii_lowercase();
            let (body, mult) = if lower.ends_with("khz") {
                (&t[..t.len() - 3], 1000.0)
            } else if lower.ends_with("hz") {
                (&t[..t.len() - 2], 1.0)
            } else if lower.ends_with('k') {
                (&t[..t.len() - 1], 1000.0)
            } else {
                (t, 1.0)
            };
            body.trim().parse::<f32>().ok().map(|v| v * mult)
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p multosis --lib -E 'test(value_to_norm) + test(format_value) + test(parse_value) + test(format_then_parse_round_trips) + test(lowpass_cutoff_is_log_hertz) + test(bitcrush_param_formats_carry)'`
Expected: PASS — 9 tests.
Run: `cargo nextest run -p multosis` — PASS, 212 tests (203 + 9 new).
Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo clippy -p multosis -- -D warnings` — clean.
Run: `cargo fmt -p multosis -- --check` — clean.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/effects.rs
git commit -m "feat(multosis): ParamSpec scaling + format + shared dial helpers"
```

---

### Task 2: Consumer cleanup — swap to shared helpers

**Files:**
- Modify: `multosis/src/editor.rs`
- Modify: `multosis/src/editor/effect_editor.rs`

Refactor; no new tests. The existing `hz_norm_round_trips_within_range` test in `effect_editor.rs` covers the old local helpers and is retired — the new shared helpers in Task 1 cover the same math more broadly.

- [ ] **Step 1: Sanity-run the suite**

Run: `cargo nextest run -p multosis` — PASS, 212 tests (Task 1 baseline).

- [ ] **Step 2: Delete the local `normalize_param` in `editor.rs`**

In `multosis/src/editor.rs`:

- Delete the function:
  ```rust
  /// Map a parameter `value` to `[0, 1]` against its spec range. Degenerate
  /// (max <= min) specs map to 0.
  fn normalize_param(value: f32, spec: ParamSpec) -> f32 { ... }
  ```
- For every call site of `normalize_param(value, spec)` (search `rg "normalize_param\(" multosis/src/editor.rs`), substitute:
  ```rust
  effects::value_to_norm(value, spec.min, spec.max, spec.scaling)
  ```
- In `apply_effect_dial`'s body, replace the linear mapping:
  ```rust
  // OLD:
  let v = spec.min + norm * (spec.max - spec.min);
  // NEW:
  let v = effects::norm_to_value(norm, spec.min, spec.max, spec.scaling);
  ```
- Add `use crate::effects;` to the top of `editor.rs` if it isn't already imported (the file currently uses `crate::effects::{EffectKind, ParamSpec}` — extend the `use crate::effects::{...}` line to include the new helpers/types via the `effects` module path, or import them individually). Pick the form that matches the file's existing convention; do not duplicate imports.

- [ ] **Step 3: Swap `draw_effect_section`'s value text in `effect_editor.rs`**

In `multosis/src/editor/effect_editor.rs::draw_effect_section`, the param-dial loop currently does:

```rust
widgets::param_dial::draw_dial(
    pixmap,
    tr,
    dx + dw / 2.0,
    dy + dh / 2.0,
    (dw.min(dh) / 2.0) - 8.0 * scale,
    spec.name,
    &format!("{value:.0}"),    // <-- replace
    norm,
);
```

Replace `&format!("{value:.0}")` with `&crate::effects::format_value(value, spec.format)`. Also replace the `norm` computation higher up — if it's currently a hand-rolled `((value - spec.min) / (spec.max - spec.min)).clamp(0.0, 1.0)` line, swap to `crate::effects::value_to_norm(value, spec.min, spec.max, spec.scaling)`.

(Search `rg "draw_dial\(" multosis/src/editor/effect_editor.rs` to find every dial draw; apply the same swap consistently.)

- [ ] **Step 4: Swap the trigger-rate dial in `effect_editor.rs`**

In `effect_editor.rs`:

- Delete the local helpers entirely:
  ```rust
  pub fn norm_to_hz(norm: f32) -> f32 { ... }
  pub fn hz_to_norm(hz: f32) -> f32 { ... }
  ```
  Keep the two constants:
  ```rust
  pub const TRIGGER_RATE_MIN_HZ: f32 = 0.05;
  pub const TRIGGER_RATE_MAX_HZ: f32 = 20.0;
  ```
- In `draw_trigger_controls`, the rate-dial draw currently uses `hz_to_norm(hz)` and `&format!("{hz:.2} Hz")`. Replace with:
  ```rust
  widgets::param_dial::draw_dial(
      pixmap,
      tr,
      rx + rw / 2.0,
      ry + rh / 2.0,
      (rw.min(rh) / 2.0) - 6.0 * scale,
      "Rate",
      &crate::effects::format_value(hz, crate::effects::ParamFormat::Hertz),
      crate::effects::value_to_norm(
          hz,
          TRIGGER_RATE_MIN_HZ,
          TRIGGER_RATE_MAX_HZ,
          crate::effects::ParamScaling::Log,
      ),
  );
  ```

- In `multosis/src/editor.rs`, every external call to `effect_editor::norm_to_hz`/`hz_to_norm` updates to the shared helpers. Run `rg "effect_editor::(norm_to_hz|hz_to_norm)" multosis/src/editor.rs` to find every call. Each `effect_editor::hz_to_norm(hz)` becomes:
  ```rust
  effects::value_to_norm(
      hz,
      effect_editor::TRIGGER_RATE_MIN_HZ,
      effect_editor::TRIGGER_RATE_MAX_HZ,
      effects::ParamScaling::Log,
  )
  ```
  Each `effect_editor::norm_to_hz(norm)` becomes:
  ```rust
  effects::norm_to_value(
      norm,
      effect_editor::TRIGGER_RATE_MIN_HZ,
      effect_editor::TRIGGER_RATE_MAX_HZ,
      effects::ParamScaling::Log,
  )
  ```

- [ ] **Step 5: Delete the obsolete `hz_norm_round_trips_within_range` test**

In `effect_editor.rs`'s `#[cfg(test)] mod tests`, delete the entire `hz_norm_round_trips_within_range` test — the shared helpers are now tested in `effects.rs`. Run `rg "hz_norm_round_trips_within_range" multosis` to confirm there are no other references.

- [ ] **Step 6: Run the suite and lint**

Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo nextest run -p multosis` — PASS, 211 tests (212 − the deleted test = 211).
Run: `cargo clippy -p multosis -- -D warnings` — clean.
Run: `cargo fmt -p multosis -- --check` — clean.

- [ ] **Step 7: Commit**

```bash
git add multosis/src/editor.rs multosis/src/editor/effect_editor.rs
git commit -m "refactor(multosis): swap dials to shared scaling/format helpers"
```

---

### Task 3: Right-click text entry on effect-param dials

**Files:**
- Modify: `multosis/src/editor.rs`
- Modify: `multosis/src/editor/effect_editor.rs`

Add `TextEditState<EffectHit>` to `MultosisWindow`; route right-click on `EffectHit::Dial(i)` into `begin`; add a `baseview::Event::Keyboard` arm; auto-commit on outside press; cancel on drag-start of the editing dial; draw via `draw_dial_ex` when a dial is being edited.

This task has no unit tests — the `TextEditState` widget has its own tests; integration is verified by the Task 4 smoke test. The plan-internal "test" is a manual rebuild + clippy + fmt cycle at each step.

- [ ] **Step 1: Add the `text_edit` field**

In `multosis/src/editor.rs`, add to `MultosisWindow` (place near `effect_dial_drag`):

```rust
    /// Right-click text-entry state for the effect-param dials. Only
    /// `EffectHit::Dial(i)` is ever begun on this; the depth and trigger-rate
    /// dials stay drag-only.
    text_edit: widgets::TextEditState<EffectHit>,
```

Initialise in `MultosisWindow::new`: `text_edit: widgets::TextEditState::new(),`.

- [ ] **Step 2: Route right-click on a dial into `text_edit.begin`**

`MultosisWindow::on_event`'s Right `ButtonPressed` arm currently runs `if self.view == View::Effect` → the MSEG pane's `on_right_click`. Before the MSEG pane forwarding, hit-test for a Dial: in the existing `if self.view == View::Effect` block, **first** call `effect_editor::effect_hit(px, py, scale, param_count, selected_mseg, is_free_hz)`. If the result is `Some(EffectHit::Dial(i))`:

```rust
let value = self
    .params
    .track_effects
    .lock()
    .ok()
    .map(|cfg| cfg[self.selected_track].params[i])
    .unwrap_or(0.0);
let spec = self.param_spec(i);
self.text_edit.begin(
    EffectHit::Dial(i),
    &effects::format_value(value, spec.format),
);
return baseview::EventStatus::Captured;
```

If the right-click hits any other `EffectHit` variant (or none), fall through to the existing MSEG-pane right-click handling unchanged.

Confirm `self.param_spec(i)` exists on `MultosisWindow` (it should — it was added in 2c Task 8). If for some reason it returns `Option<ParamSpec>`, gracefully short-circuit the begin on `None`.

- [ ] **Step 3: Add the `Keyboard` event arm to `on_event`**

In `MultosisWindow::on_event`, add a new arm to the `match &event` for keyboard input — place it near the existing mouse arms. Mirror gain-brain's pattern (read `gain-brain/src/editor.rs` around the `Keyboard` arm). Concretely:

```rust
baseview::Event::Keyboard(ke) => {
    use keyboard_types::{Key, KeyState};
    // Swallow key-ups while editing; only handle key-downs.
    if ke.state != KeyState::Down {
        if self.text_edit.is_active() {
            return baseview::EventStatus::Captured;
        }
        return baseview::EventStatus::Ignored;
    }
    if !self.text_edit.is_active() {
        return baseview::EventStatus::Ignored;
    }
    match &ke.key {
        Key::Character(s) => {
            for c in s.chars() {
                self.text_edit.insert_char(c);
            }
        }
        Key::Backspace => self.text_edit.backspace(),
        Key::Enter => {
            if let Some((EffectHit::Dial(i), text)) = self.text_edit.commit() {
                self.commit_dial_text_edit(i, &text);
            }
        }
        Key::Escape => self.text_edit.cancel(),
        _ => {}
    }
    baseview::EventStatus::Captured
}
```

Add the `commit_dial_text_edit` helper to `impl MultosisWindow`:

```rust
    /// Parse the typed text for dial `i`, clamp to the param's range, write
    /// to the persisted config, and mark dirty. Parse failure is a silent
    /// no-op — the dial keeps its previous value.
    fn commit_dial_text_edit(&mut self, i: usize, text: &str) {
        let spec = self.param_spec(i);
        let Some(v) = effects::parse_value(text, spec.format) else {
            return;
        };
        let clamped = v.clamp(spec.min, spec.max);
        if let Ok(mut cfg) = self.params.track_effects.lock() {
            cfg[self.selected_track].params[i] = clamped;
        }
        self.mark_config_dirty();
    }
```

(If `self.param_spec(i)` returns `Option<ParamSpec>`, early-return on `None` before the `Some(v)` line.)

If `baseview::Event::Keyboard` does not exist in the variant set, search the codebase (`rg "baseview::Event::Keyboard" --include='*.rs'`) for the working pattern used by other plugins and follow it. Other consumers (gain-brain, satch, etc.) use this seam — copy the call shape.

- [ ] **Step 4: Auto-commit on outside left press; cancel on drag-start of the editing dial**

In `MultosisWindow::on_event`'s Left `ButtonPressed` arm, **at the very top**, before any other dispatch (including toolbar/op/track-list/back-button/effect-press), check if a text edit is active. If so, decide:

- If the press resolves (via `effect_hit`) to **the same `EffectHit::Dial(i)` that text_edit is editing**: cancel the edit and proceed (a left-press on the active dial means the user wants to drag — `effect_dial_drag.begin_drag` will fire in the normal Effect-press path).
- Otherwise: commit the edit (parse and apply), then proceed normally.

Add a small helper to `impl MultosisWindow`:

```rust
    /// If a dial-text edit is active, decide whether to commit or cancel it
    /// based on the incoming press location. A press on the active dial
    /// cancels (the user wants to drag); a press anywhere else commits.
    fn finalize_dial_edit_for_press(&mut self, px: f32, py: f32) {
        let Some(active) = self.text_edit.active_for_any() else {
            return;
        };
        // Resolve the press to an effect-editor hit, if any.
        let trigger = self.selected_track_modulation().trigger;
        let is_free_hz = matches!(trigger, crate::modulation::TriggerSource::FreeHz { .. });
        let param_count = self.selected_track_param_count();
        let hit = effect_editor::effect_hit(
            px,
            py,
            self.scale_factor,
            param_count,
            self.selected_mseg,
            is_free_hz,
        );
        if hit == Some(active) {
            self.text_edit.cancel();
        } else if let Some((EffectHit::Dial(i), text)) = self.text_edit.commit() {
            self.commit_dial_text_edit(i, &text);
        } else {
            self.text_edit.cancel();
        }
    }
```

This helper depends on a small addition to `TextEditState` in `tiny-skia-widgets`: an `active_for_any(&self) -> Option<A>` method that returns the currently-editing action (or `None`). If it doesn't exist, add it — see Step 5.

Call `self.finalize_dial_edit_for_press(px, py);` at the top of the `Left ButtonPressed` arm, before any other branching.

- [ ] **Step 5: Add `TextEditState::active_for_any` if missing**

In `tiny-skia-widgets/src/text_edit.rs`, check whether the type already exposes the current action when active. If not, add (next to `active_for`):

```rust
/// The currently-editing action, or `None` when no edit is in progress.
pub fn active_for_any(&self) -> Option<A> {
    /* return self.action.clone() when active */
}
```

The exact body depends on the existing internal state — read the file and match its naming. If the type stores the active action in a field named `action` only when editing, the body is `self.action.clone()` (the field is already `Option<A>` shaped, or guarded). Search the file for `active_for` and write `active_for_any` next to it, returning the bare action (cloned, since `A: Clone`).

If the addition is genuinely intrusive (the internal state needs reshaping), an inferior fallback is to use `self.text_edit.is_active()` together with a `MultosisWindow` field `editing_dial: Option<usize>` that mirrors which dial is active. Avoid the fallback if a single getter works.

- [ ] **Step 6: Switch the dial draw to `draw_dial_ex` when editing**

In `effect_editor.rs::draw_effect_section`, extend the signature with an additional argument:

```rust
#[allow(clippy::too_many_arguments)]
pub fn draw_effect_section(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    track: &TrackEffect,
    track_index: usize,
    kind_dropdown_open: bool,
    editing_dial: Option<(usize, &str, bool)>,  // (slot index, buffer, caret_on)
    scale: f32,
) {
```

In the per-spec dial loop:

```rust
let value_text = crate::effects::format_value(value, spec.format);
let norm = crate::effects::value_to_norm(value, spec.min, spec.max, spec.scaling);
match editing_dial {
    Some((idx, buf, caret_on)) if idx == i => {
        widgets::param_dial::draw_dial_ex(
            pixmap,
            tr,
            dx + dw / 2.0,
            dy + dh / 2.0,
            (dw.min(dh) / 2.0) - 8.0 * scale,
            spec.name,
            &value_text,
            norm,
            None,
            Some(buf),
            caret_on,
        );
    }
    _ => {
        widgets::param_dial::draw_dial(
            pixmap,
            tr,
            dx + dw / 2.0,
            dy + dh / 2.0,
            (dw.min(dh) / 2.0) - 8.0 * scale,
            spec.name,
            &value_text,
            norm,
        );
    }
}
```

In `MultosisWindow::draw_effect_view`'s call to `draw_effect_section`, compute `editing_dial`:

```rust
let editing_dial = match self.text_edit.active_for_any() {
    Some(EffectHit::Dial(i)) => self
        .text_edit
        .active_for(&EffectHit::Dial(i))
        .map(|buf| (i, buf, self.text_edit.caret_visible())),
    _ => None,
};
effect_editor::draw_effect_section(
    &mut self.surface.pixmap,
    &mut self.text_renderer,
    &track,
    self.selected_track,
    self.kind_dropdown.is_open_for(EffectAction::Kind),
    editing_dial,
    self.scale_factor,
);
```

- [ ] **Step 7: Run the build and the suite**

Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo nextest run -p multosis` — PASS, 211 tests (no new tests in Task 3).
Run: `cargo nextest run -p tiny-skia-widgets` — PASS (the `active_for_any` addition shouldn't regress widget tests; if you added it, the existing TextEditState tests still pass).
Run: `cargo clippy -p multosis -- -D warnings` — clean.
Run: `cargo clippy -p tiny-skia-widgets -- -D warnings` — clean.
Run: `cargo fmt --check` — clean (run `cargo fmt` if not).

- [ ] **Step 8: Commit**

```bash
git add multosis/src/editor.rs multosis/src/editor/effect_editor.rs tiny-skia-widgets/src/text_edit.rs
git commit -m "feat(multosis): right-click text entry on effect-param dials"
```

(If `text_edit.rs` wasn't touched, omit it from `git add`.)

---

### Task 4: Verification

**Files:** none — checks and a manual smoke test.

- [ ] **Step 1: Full suite, lint, format**

Run: `cargo nextest run -p multosis` — PASS, all green (211 tests).
Run: `cargo nextest run --workspace` — PASS, all green.
Run: `cargo clippy --workspace -- -D warnings` — no warnings.
Run: `cargo fmt --check` — clean.

- [ ] **Step 2: Release build and bundle**

Run: `cargo xtask native build --bin multosis --release` — standalone binary builds.
Run: `cargo xtask native nih-plug bundle multosis --release` — VST3 + CLAP + standalone bundle, no errors.

- [ ] **Step 3: Manual smoke test**

Run `cargo run --bin multosis` (or load the bundle in a host). Confirm:

- Open a track's effect editor. Each parameter dial's value text now reflects its `ParamFormat`: Cutoff reads `"2.0 kHz"`, Resonance reads `"0.10"`, Bit Depth reads `"16 bits"`, Rate Reduction reads `"1 x"`. The trigger-rate dial reads `"1 Hz"` / `"0.50 Hz"` / `"20 Hz"` etc.
- Drag a Cutoff dial. The mapping is log-curved: a small drag near the bottom moves through audibly fewer Hz than the same drag at the top.
- **Right-click** a Cutoff dial. The dial's value text shows the buffer with the caret blinking. Type `1k`, press Enter — the cutoff snaps to 1 kHz audibly.
- **Right-click** a Resonance dial. Buffer pre-fills with the current value. Type `0.7`, Enter — resonance jumps.
- **Right-click** a Bit Depth dial. Type `4 bits`, Enter — bit-depth drops to 4.
- **Escape** during an edit cancels (no change).
- **Click outside the active dial** during an edit auto-commits (the typed value is applied).
- **Click ON the active dial** during an edit cancels the edit and starts a drag.
- **Parse failure** (e.g. type `"xyz"` then Enter) is silently dropped — the dial keeps its previous value, no error noise.
- The trigger-rate dial drag still works (no regression); its value text now uses the shared `Hertz` formatter (`"0.50 Hz"` / `"2.0 kHz"`).
- The grid editor, the effect editor's other controls (kind dropdown, MSEG selector, target/depth, MSEG pane), `< Grid`, the toolbar — all still work.

Report the smoke-test observations.

- [ ] **Step 4: Commit (only if Step 1 required formatting edits)**

```bash
git add -A
git commit -m "style(multosis): apply rustfmt for the Bundle A dial-polish work"
```

If Step 1 produced no edits, skip this commit.

---

## Definition of done

- `ParamSpec` declares per-parameter `scaling` and `format`; the shared `value_to_norm` / `norm_to_value` / `format_value` / `parse_value` helpers are the only source of dial-mapping math in the editor.
- The effect-editor parameter dials format values per-spec; Cutoff drags log-curved.
- Right-click on an effect-param dial opens a typed text-entry buffer; Enter commits via `parse_value` + clamp; Escape cancels; click-outside auto-commits; click-on-the-same-dial cancels and starts a drag.
- The trigger-rate dial's Phase 3 hand-coded log helpers are gone; its math runs through the shared helpers.
- `cargo nextest run -p multosis` is green (211 tests); `cargo clippy --workspace -- -D warnings` is clean; the plugin bundles and the smoke test confirms every bullet above.

## Spec coverage check (self-review)

- §1 `ParamSpec` extension — Task 1 adds `ParamScaling`, `ParamFormat`, the new fields on `ParamSpec`, and the updated effect param tables.
- §2 Shared helpers — Task 1 adds `value_to_norm` / `norm_to_value` / `format_value` / `parse_value` with the spec's exact algorithms.
- §3 Consumer cleanup — Task 2 deletes `normalize_param`, swaps `draw_effect_section`'s value text, retires `effect_editor.rs`'s `norm_to_hz`/`hz_to_norm` and routes the rate dial through the shared helpers.
- §4 Right-click text entry — Task 3 wires `TextEditState<EffectHit>`, the press dispatch, the new `Keyboard` arm, auto-commit / cancel rules, and the `draw_dial_ex` switch.
- §5 Defaults & persistence — Task 1's `ParamSpec` field additions are non-persisted constants; no `#[persist]` change. (Implicit.)
- §6 Out of scope — Tasks honour: no text entry on depth or trigger-rate dials; no log drag curves for non-Log params.
- §7 Testing — Task 1 covers every helper + format + parse path; Task 4 covers the editor behaviour via the smoke test.

## Note on task sequencing

Task 1 is pure data + tests, ships green on its own. Task 2 is a refactor — no behaviour change but renumbers `effect_editor.rs`'s test count by −1 (the `hz_norm_round_trips_within_range` retire). Task 3 introduces the keyboard-event arm and the `text_edit` field, the only Multosis-side place that needs `tiny-skia-widgets::TextEditState` exposure; if `active_for_any` is missing from the widget, Step 5 adds it. Task 4 is verification.
