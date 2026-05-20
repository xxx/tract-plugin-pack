# Multosis Bundle A — Dial Polish — Design

**Date:** 2026-05-19
**Status:** Approved (brainstorming)
**Branch:** `multosis`

## Overview

The effect-editor's parameter dials are functional but rough: every value text renders as `"{value:.0}"` (so a 0..1 Resonance shows as `0` or `1`); Cutoff drags linearly across 20..20000 Hz (mostly useless); right-click does nothing on a dial (where every other plugin in the pack lets you type a value). Phase 3 already hand-rolled a log-scaled trigger-rate dial with its own `norm_to_hz` / `hz_to_norm` — that duplication wants to fold into a shared scheme.

Bundle A unifies the three concerns:
1. **`ParamSpec` gains scaling + format fields** so each parameter declares how its dial maps and renders.
2. **Shared `value_to_norm` / `norm_to_value` / `format_value` / `parse_value` helpers** replace the editor's local `normalize_param` and the trigger-rate dial's hand-coded log math.
3. **Right-click text entry** on effect-param dials, via `tiny-skia-widgets::TextEditState` (the pattern already used by gain-brain, miff, satch, six-pack, tinylimit).

Depth and trigger-rate dials stay drag-only — text entry is scoped to `EffectHit::Dial(i)` only.

## Reference

- Current `ParamSpec` and effect param tables — `multosis/src/effects.rs` (the `LowpassEffect::PARAMS` and `BitcrushEffect::PARAMS` constants).
- Effect-editor dial wiring — `multosis/src/editor.rs::normalize_param`, `apply_effect_dial`; `multosis/src/editor/effect_editor.rs::draw_effect_section` (param dials), `draw_trigger_controls` (the rate dial).
- Phase 3's hand-coded trigger-rate log math — `multosis/src/editor/effect_editor.rs`: `TRIGGER_RATE_MIN_HZ`, `TRIGGER_RATE_MAX_HZ`, `norm_to_hz`, `hz_to_norm`. Spec `docs/superpowers/specs/2026-05-19-multosis-phase-3-design.md` §4.
- `TextEditState` widget — `tiny-skia-widgets/src/text_edit.rs`: `begin(action, initial)` / `insert_char(c)` / `backspace()` / `commit() -> Option<(A, String)>` / `cancel()` / `is_active()` / `active_for(&action) -> Option<&str>` / `caret_visible()`.
- `param_dial::draw_dial_ex(... editing_text: Option<&str>, caret_on: bool)` — already draws the edit-overlay; `draw_dial` is the no-edit shorthand.
- Reference consumer of `TextEditState` for the integration pattern — `gain-brain/src/editor.rs` (the `text_edit: TextEditState<HitAction>` field and the surrounding key/mouse wiring).

## §1 `ParamSpec` — scaling + format fields

`multosis/src/effects.rs`:

```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamScaling {
    /// `value = min + norm * (max - min)`; norm is `(value - min) / (max - min)`.
    Linear,
    /// `value = min * (max / min).powf(norm)`; norm is `log_(max/min)(value / min)`.
    /// Requires `min > 0`.
    Log,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamFormat {
    /// Fixed-decimals number, optional unit suffix. Empty unit prints no
    /// suffix; "bits", "x", etc. print with a single space.
    Number { decimals: u8, unit: &'static str },
    /// Auto Hz/kHz scaling: < 1 → `"0.05 Hz"` (2 dec), 1..1000 → `"80 Hz"`
    /// (0 dec), ≥ 1000 → `"2.0 kHz"` (1 dec).
    Hertz,
}

pub struct ParamSpec {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub scaling: ParamScaling,
    pub format: ParamFormat,
}
```

`ParamSpec` stays `Copy` (every new field is `Copy`). Adding fields breaks no existing API — all `ParamSpec` constructors live in this crate, in the two effect-param tables; both get updated.

**Effect param tables:**

- `LowpassEffect::PARAMS`:
  - `Cutoff` — `min=20.0, max=20_000.0, default=2_000.0, scaling=Log, format=Hertz`.
  - `Resonance` — `min=0.0, max=1.0, default=0.1, scaling=Linear, format=Number{decimals:2, unit:""}`.
- `BitcrushEffect::PARAMS`:
  - `Bit Depth` — `min=1.0, max=16.0, default=16.0, scaling=Linear, format=Number{decimals:0, unit:"bits"}`.
  - `Rate Reduction` — `min=1.0, max=50.0, default=1.0, scaling=Linear, format=Number{decimals:0, unit:"x"}`.

## §2 Shared helpers

In `multosis/src/effects.rs` (pub free functions, called by the editor and tests):

```
pub fn value_to_norm(value: f32, min: f32, max: f32, scaling: ParamScaling) -> f32;
pub fn norm_to_value(norm: f32, min: f32, max: f32, scaling: ParamScaling) -> f32;
pub fn format_value(value: f32, format: ParamFormat) -> String;
pub fn parse_value(text: &str, format: ParamFormat) -> Option<f32>;
```

**`value_to_norm`** — Linear: `((value - min) / (max - min)).clamp(0.0, 1.0)`. Log: `((value / min).log(max / min)).clamp(0.0, 1.0)`. Degenerate (`max <= min`, or Log with `min <= 0`) → 0.0.

**`norm_to_value`** — Linear: `min + norm.clamp(0.0, 1.0) * (max - min)`. Log: `min * (max / min).powf(norm.clamp(0.0, 1.0))`. Degenerate → `min`.

**`format_value`** —
- `Number { decimals, unit }`: `if unit.is_empty() { format!("{value:.dec$}") } else { format!("{value:.dec$} {unit}") }` where `dec = decimals as usize`.
- `Hertz` (auto-scale):
  - `value.abs() < 1.0` → `format!("{value:.2} Hz")` (e.g. `"0.05 Hz"`)
  - `value.abs() < 1000.0` → `format!("{value:.0} Hz")` (e.g. `"80 Hz"`)
  - `value.abs() >= 1000.0` → `format!("{:.1} kHz", value / 1000.0)` (e.g. `"2.0 kHz"`)

**`parse_value`** —
- `Number { unit, .. }`: trim whitespace; if the trimmed text ends in (case-insensitive) `unit` and `unit` is non-empty, strip it; parse the remainder as `f32`.
- `Hertz`: trim; case-insensitive — if the text ends in `khz`, strip and multiply the parsed number by 1000; else if it ends in `hz` or `k`, strip the `hz`/`k` and (for `k`) multiply by 1000; else parse as-is.
- Returns `None` on empty input or `f32::from_str` failure. The consumer clamps the result into `[min, max]`.

Examples (Hertz): `"2k"` → 2000, `"2 kHz"` → 2000, `"2.5kHz"` → 2500, `"80"` → 80, `"80 Hz"` → 80, `"0.5"` → 0.5, `""` → `None`, `"abc"` → `None`.

## §3 Consumer cleanup

**`multosis/src/editor.rs`.** Delete the local `normalize_param` helper. Every call to it (e.g. in the press handler reading a dial's current normalized value, and in the dial-drag CursorMoved branches) becomes `effects::value_to_norm(value, spec.min, spec.max, spec.scaling)`. `apply_effect_dial(i, norm)`'s body, currently `let v = spec.min + norm * (spec.max - spec.min);`, becomes `let v = effects::norm_to_value(norm, spec.min, spec.max, spec.scaling);`.

**`multosis/src/editor/effect_editor.rs`.**
- In `draw_effect_section`, the dial value text uses `effects::format_value(value, spec.format)` instead of `format!("{value:.0}")`.
- The trigger-rate dial's existing `TRIGGER_RATE_MIN_HZ` / `TRIGGER_RATE_MAX_HZ` constants remain as plain module-level constants (used by Phase 3 wiring) but the local `norm_to_hz` / `hz_to_norm` functions are **deleted**. Every caller switches to `effects::value_to_norm(hz, TRIGGER_RATE_MIN_HZ, TRIGGER_RATE_MAX_HZ, ParamScaling::Log)` / `effects::norm_to_value(...)`. `draw_trigger_controls`'s `"{hz:.2} Hz"` becomes `effects::format_value(hz, ParamFormat::Hertz)`.

After §3, `value_to_norm` / `norm_to_value` / `format_value` are the single source of dial-mapping math for every dial in the editor.

## §4 Right-click text entry — effect-param dials only

`MultosisWindow` gains:

```
text_edit: widgets::TextEditState<EffectHit>,
```

initialised `TextEditState::new()`. Only `EffectHit::Dial(i)` is ever begun on it (the existing `EffectHit::Depth`/`TriggerRate` paths are not affected).

### Press flow

In `MultosisWindow::on_effect_press`, a **right-button** press that resolves via `effect_hit` to `EffectHit::Dial(i)` calls:

```
let value = self.params.track_effects.lock().ok()
    .map(|cfg| cfg[self.selected_track].params[i]).unwrap_or(0.0);
let spec = self.param_spec(i);   // existing helper from 2c
self.text_edit.begin(
    EffectHit::Dial(i),
    &effects::format_value(value, spec.format),
);
```

The buffer is seeded with the current formatted value so the user is in edit-in-place mode rather than retype-from-scratch.

For dial hits other than `Dial(i)` (`MsegSelector`, `Target`, `Depth`, `MsegPane`, `Trigger`, `TriggerRate`), the right-click handling is unchanged from Phase 3 — the MSEG pane's `on_right_click` runs in `View::Effect`.

If `text_edit.is_active()` and the user **left-presses** in `View::Effect`, the existing edit is committed first (per the existing `TextEditState` consumer pattern), then the left press is processed normally. A press inside the active dial (i.e. the press resolves to the same `EffectHit::Dial(i)` that text_edit is editing) starts a drag and cancels the edit instead — drag-and-type are mutually exclusive.

### Keyboard

`MultosisWindow::on_event` grows a new `baseview::Event::Keyboard` arm — this is the first time multosis routes key events. When `text_edit.is_active()`:

- `keyboard_types::Key::Character(s)` (text input) → for each char in `s`, `text_edit.insert_char(c)`.
- `Backspace` → `text_edit.backspace()`.
- `Enter` → commit (see below).
- `Escape` → `text_edit.cancel()`.
- Any other key (including modifiers, navigation keys) is swallowed but does not commit.

Key-up events are always swallowed while editing (consistent with the gain-brain pattern).

On commit:

```
if let Some((EffectHit::Dial(i), text)) = self.text_edit.commit() {
    let spec = self.param_spec(i);
    if let Some(v) = effects::parse_value(&text, spec.format) {
        let clamped = v.clamp(spec.min, spec.max);
        if let Ok(mut cfg) = self.params.track_effects.lock() {
            cfg[self.selected_track].params[i] = clamped;
        }
        self.mark_config_dirty();
    }
    // Parse failure: drop the edit silently — the dial's value is unchanged.
}
```

Parse failure on Enter is a silent no-op (matches user expectation: the dial stays where it was, the edit overlay closes).

### Draw

`MultosisWindow::draw_effect_view` already loops per slot calling `draw_dial`. The new version checks the text-edit state per slot:

```
if let Some(buf) = self.text_edit.active_for(&EffectHit::Dial(i)) {
    widgets::param_dial::draw_dial_ex(
        pixmap, tr, cx, cy, radius,
        spec.name, &effects::format_value(value, spec.format), norm,
        None, Some(buf), self.text_edit.caret_visible(),
    );
} else {
    widgets::param_dial::draw_dial(
        pixmap, tr, cx, cy, radius,
        spec.name, &effects::format_value(value, spec.format), norm,
    );
}
```

(In practice the existing `draw_effect_section` does the loop, so it gains an `editing_buf: Option<(usize, &str)>` argument from the window and routes accordingly. Concrete signature decided in the plan.)

### Caret blink

`TextEditState::caret_visible()` flips on a half-second period based on `Instant::now()` (its internal blink helper). The editor calls it each frame and passes the result to `draw_dial_ex`. No new timer plumbing needed.

## §5 Defaults & persistence

- No persisted state changes. `TrackEffect::params: [f32; 4]` already holds parameter values; this milestone only changes how those values map to dial normalized positions and how they render. A project saved before Bundle A reloads with the same numeric `params` array; the new dial display will render them with the new format strings.
- `ParamSpec`'s new fields are `const`-initialised in the effect param tables; they are not part of persisted state.

## §6 Out of scope (future polish)

- Text entry on the depth dial and trigger-rate dial (those stay drag-only).
- Mouse-wheel value step (a separate UX feature).
- Log drag curves for non-Log params, or fine-drag (Shift) curves.
- Host parameter automation of effect params.
- Sticky-decimals or engineering-notation formats beyond Number / Hertz.

## §7 Testing

Per CLAUDE.md — TDD, inline `#[cfg(test)]`, `cargo nextest`.

- **`value_to_norm` / `norm_to_value`** round-trip in Linear (e.g. midpoint of 0..1 is 0.5; 20000 in 20..20000 Linear maps to 1.0). Log round-trip (20 → 0.0, 20000 → 1.0, midpoint of 20..20000 Log ≈ 632 Hz which is `20 * (1000)^0.5`). Out-of-range clamps to 0.0/1.0. Degenerate ranges return 0.0 / `min`.
- **`format_value`**:
  - `Number{decimals:2, unit:""}` of 0.15 → `"0.15"`.
  - `Number{decimals:0, unit:"bits"}` of 8.0 → `"8 bits"`.
  - `Number{decimals:0, unit:"x"}` of 4.0 → `"4 x"`.
  - `Hertz` at 0.05 → `"0.05 Hz"`; at 80 → `"80 Hz"`; at 2000 → `"2.0 kHz"`; at 18_500 → `"18.5 kHz"`.
- **`parse_value`** round-trip with `format_value` for every (format, sample-value) pair listed above. Bad inputs (`""`, `"abc"`, `"1.5.2"`, `"2 kgu"`) return `None`. Hertz parsing: `"2k"` / `"2 kHz"` / `"2.5kHz"` / `"80 Hz"` / `"80"` all parse to the expected Hz.
- **`TextEditState` integration** — purely behaviour-level via the existing widget's own tests; no new unit tests in multosis (the editor wiring is verified by the smoke test).
- **Smoke test** — right-click a Cutoff dial, the buffer pre-fills with `"2.0 kHz"`; type `1k`, hit Enter; the cutoff audibly drops; right-click a Resonance dial → `"0.10"`; type `0.7`, Enter; resonance climbs. Escape cancels. Click outside auto-commits. Cutoff drag is now log-curved (a small drag near the bottom moves through audibly fewer Hz than the same drag at the top). The trigger-rate dial's behaviour is unchanged (no visible regression).
