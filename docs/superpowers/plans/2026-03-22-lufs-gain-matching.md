# LUFS Gain Matching Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In LUFS mode, show gain in LU and reference in LUFS, and add gain-match buttons to LUFS readings (matching dpMeter5's EBU R128 gain matching behavior).

**Architecture:** Extract the gain-match calculation into a testable helper function (TDD first). Extend `GainSource` with LUFS variants and wire the complete match in one pass. Switch gain/reference slider labels by mode. Replace the LUFS readings section with gain-match buttons. The formula `gain = reference - reading` is identical for dB and LUFS since both are absolute dB-scale units.

**Tech Stack:** Rust, nih-plug, softbuffer + tiny-skia (CPU rendering)

---

### Task 1: Extract gain_match_db helper with tests (TDD)

**Files:**
- Modify: `gs-meter/src/editor.rs` (add helper function near format helpers at ~line 517, add test module at end of file)

- [ ] **Step 1: Write the gain_match_db tests**

Add a test module at the bottom of `editor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gain_match_reference_minus_reading() {
        // Reference -14 LUFS, reading -20 LUFS -> need +6 dB gain
        assert_eq!(gain_match_db(-14.0, -20.0), Some(6.0));
    }

    #[test]
    fn test_gain_match_negative_gain() {
        // Reference -23 LUFS, reading -14 LUFS -> need -9 dB gain (too loud)
        assert_eq!(gain_match_db(-23.0, -14.0), Some(-9.0));
    }

    #[test]
    fn test_gain_match_zero_when_matched() {
        // Already at target -> 0 dB gain
        assert_eq!(gain_match_db(-14.0, -14.0), Some(0.0));
    }

    #[test]
    fn test_gain_match_invalid_reading_returns_none() {
        // Reading at or below floor -> no valid measurement
        assert_eq!(gain_match_db(-14.0, -100.0), None);
        assert_eq!(gain_match_db(-14.0, -200.0), None);
    }

    #[test]
    fn test_gain_match_just_above_floor() {
        // Reading just above -100 dB floor -> valid measurement
        let result = gain_match_db(-14.0, -99.99);
        assert!(result.is_some());
        let gain = result.unwrap();
        assert!((gain - 85.99).abs() < 0.02);
    }

    #[test]
    fn test_gain_match_works_for_db_mode_too() {
        // dB mode: reference 0 dBFS, peak at -3 dB -> need +3 dB
        assert_eq!(gain_match_db(0.0, -3.0), Some(3.0));
    }

    #[test]
    fn test_gain_match_positive_reading() {
        // Reading above 0 (clipping) -> large negative gain
        assert_eq!(gain_match_db(-14.0, 2.0), Some(-16.0));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package gs-meter`
Expected: FAIL — `gain_match_db` not found

- [ ] **Step 3: Write the gain_match_db helper function**

Add near the format helpers at ~`editor.rs:517`:

```rust
/// Compute the gain adjustment (in dB) needed to match a meter reading to a reference level.
/// Returns None if the reading is below the -100 dB floor (no valid measurement).
/// Works identically for dB and LUFS modes since both are absolute dB-scale units.
/// Note: the returned value may exceed the gain parameter's range (-40..+40 dB);
/// nih-plug's parameter system will clamp it when applied.
fn gain_match_db(reference: f32, meter_reading: f32) -> Option<f32> {
    if meter_reading <= -100.0 {
        None
    } else {
        Some(reference - meter_reading)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package gs-meter`
Expected: all tests pass (75 total — 68 existing + 7 new)

- [ ] **Step 5: Refactor the existing GainFromReading handler to use gain_match_db**

Replace the gain matching logic at `editor.rs:619-636`. Change the `if meter_db > -100.0` block to use the new helper:

```rust
HitAction::Button(ButtonAction::GainFromReading(source)) => {
    let meter_db = match source {
        GainSource::PeakMax => MeterReadings::load_db(&self.readings.peak_max_db),
        GainSource::TruePeak => MeterReadings::load_db(&self.readings.true_peak_max_db),
        GainSource::RmsIntegrated => MeterReadings::load_db(&self.readings.rms_integrated_db),
        GainSource::RmsMomentary => MeterReadings::load_db(&self.readings.rms_momentary_db),
        GainSource::RmsMomentaryMax => MeterReadings::load_db(&self.readings.rms_momentary_max_db),
    };
    if let Some(target_gain_db) = gain_match_db(self.params.reference_level.value(), meter_db) {
        let target_gain_linear = nih_plug::util::db_to_gain(target_gain_db);
        let normalized = self.params.gain.preview_normalized(target_gain_linear);
        setter.begin_set_parameter(&self.params.gain);
        setter.set_parameter_normalized(&self.params.gain, normalized);
        setter.end_set_parameter(&self.params.gain);
    }
}
```

- [ ] **Step 6: Verify it compiles and tests pass**

Run: `cargo test --package gs-meter`
Expected: all 75 tests pass

- [ ] **Step 7: Commit**

```
feat: extract gain_match_db helper with tests
```

---

### Task 2: Switch gain/reference slider labels by meter mode

**Files:**
- Modify: `gs-meter/src/editor.rs:316-320` (gain/reference slider label formatting)

- [ ] **Step 1: Make gain slider label mode-aware**

Replace the gain_text and ref_text formatting at `editor.rs:316-320`:

```rust
let gain_db = nih_plug::util::gain_to_db(self.params.gain.value());
let gain_text = if meter_mode == crate::MeterMode::Lufs {
    format!("{:.1} LU", gain_db)
} else {
    format!("{:.1} dB", gain_db)
};
slider_row!("Gain", self.params.gain, ParamId::Gain, &gain_text);

let ref_val = self.params.reference_level.value();
let ref_text = if meter_mode == crate::MeterMode::Lufs {
    format!("{:.1} LUFS", ref_val)
} else {
    format!("{:.1} dB", ref_val)
};
slider_row!("Reference", self.params.reference_level, ParamId::Reference, &ref_text);
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check --package gs-meter`
Expected: compiles, no errors

- [ ] **Step 3: Commit**

```
feat: show gain in LU and reference in LUFS when in LUFS mode
```

---

### Task 3: Add LUFS variants to GainSource and wire gain-match buttons

**Files:**
- Modify: `gs-meter/src/editor.rs:101-108` (GainSource enum)
- Modify: `gs-meter/src/editor.rs:364-381` (LUFS mode readings in draw())
- Modify: `gs-meter/src/editor.rs:619-636` (GainFromReading event handler)

- [ ] **Step 1: Add LUFS variants to GainSource enum**

Extend the enum at `editor.rs:102-108`:

```rust
#[derive(Clone, Copy, PartialEq)]
enum GainSource {
    PeakMax,
    TruePeak,
    RmsIntegrated,
    RmsMomentary,
    RmsMomentaryMax,
    // LUFS mode sources
    LufsIntegrated,
    LufsShortTerm,
    LufsShortTermMax,
    LufsMomentary,
    LufsMomentaryMax,
    LufsTruePeak,
}
```

- [ ] **Step 2: Add LUFS match arms to GainFromReading handler**

Extend the `meter_db` match in the GainFromReading handler (the one refactored in Task 1) to include the new variants:

```rust
GainSource::LufsIntegrated => MeterReadings::load_db(&self.readings.lufs_integrated),
GainSource::LufsShortTerm => MeterReadings::load_db(&self.readings.lufs_short_term),
GainSource::LufsShortTermMax => MeterReadings::load_db(&self.readings.lufs_short_term_max),
GainSource::LufsMomentary => MeterReadings::load_db(&self.readings.lufs_momentary),
GainSource::LufsMomentaryMax => MeterReadings::load_db(&self.readings.lufs_momentary_max),
GainSource::LufsTruePeak => MeterReadings::load_db(&self.readings.true_peak_max_db),
```

- [ ] **Step 3: Replace LUFS readings with gain-match buttons**

Replace the LUFS readings block at `editor.rs:364-381`. Uses the same pattern as dB mode — an array of `(label, value, formatted, GainSource)` tuples with `-> Gain` buttons, plus LRA at the end with no button (LRA is a range, not an absolute level). True Peak uses `format_dbtp` because dBTP is the correct unit regardless of meter mode:

```rust
} else {
    // ── LUFS mode readings with gain-match buttons ──
    let lufs_integrated = MeterReadings::load_db(&self.readings.lufs_integrated);
    let lufs_short_term = MeterReadings::load_db(&self.readings.lufs_short_term);
    let lufs_short_term_max = MeterReadings::load_db(&self.readings.lufs_short_term_max);
    let lufs_momentary = MeterReadings::load_db(&self.readings.lufs_momentary);
    let lufs_momentary_max = MeterReadings::load_db(&self.readings.lufs_momentary_max);
    let lufs_true_peak = MeterReadings::load_db(&self.readings.true_peak_max_db);
    let lufs_range = MeterReadings::load_db(&self.readings.lufs_range);

    let lufs_gain_sources = [
        ("Integrated", lufs_integrated, format_lufs(lufs_integrated), GainSource::LufsIntegrated),
        ("Short-Term", lufs_short_term, format_lufs(lufs_short_term), GainSource::LufsShortTerm),
        ("ST Max", lufs_short_term_max, format_lufs(lufs_short_term_max), GainSource::LufsShortTermMax),
        ("Momentary", lufs_momentary, format_lufs(lufs_momentary), GainSource::LufsMomentary),
        ("Mom Max", lufs_momentary_max, format_lufs(lufs_momentary_max), GainSource::LufsMomentaryMax),
        ("True Peak", lufs_true_peak, format_dbtp(lufs_true_peak), GainSource::LufsTruePeak),
    ];

    for (label, _val, formatted, source) in &lufs_gain_sources {
        tr.draw_text(&mut self.pixmap, pad, y + font_size, label, font_size, widgets::color_muted());
        tr.draw_text(&mut self.pixmap, pad + label_w + gap, y + font_size, formatted, font_size, widgets::color_text());
        let bx = pad + label_w + gap + value_w + gap;
        let by = y + 2.0 * s;
        widgets::draw_button(
            &mut self.pixmap, tr, bx, by, btn_w, btn_h,
            "\u{2192} Gain", false, false,
        );
        self.hit_regions.push(HitRegion {
            x: bx, y: by, w: btn_w, h: btn_h,
            action: HitAction::Button(ButtonAction::GainFromReading(*source)),
        });
        y += row_h;
    }

    // LRA (no gain-match button — it's a range, not an absolute level)
    let lra_val = format_lu(lufs_range);
    tr.draw_text(&mut self.pixmap, pad, y + font_size, "LRA", font_size, widgets::color_muted());
    tr.draw_text(&mut self.pixmap, pad + label_w + gap, y + font_size, &lra_val, font_size, widgets::color_text());
    y += row_h;
}
```

- [ ] **Step 4: Verify it compiles and all tests pass**

Run: `cargo test --package gs-meter`
Expected: all 75 tests pass

- [ ] **Step 5: Commit**

```
feat: add gain-match buttons to LUFS mode readings
```

---

### Task 4: Final verification and review

- [ ] **Step 1: Run full workspace tests**

Run: `cargo test --workspace`
Expected: all tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Dispatch review agents**

Use `superpowers:code-reviewer` and `feature-dev:code-reviewer` agents in parallel to review all changes since the last commit. Focus areas:
- No allocations in draw() hot path (format strings are OK — they're GUI thread, not audio thread)
- Gain matching formula correctness
- All GainSource variants handled in every match (no non-exhaustive matches)
- No missing hit regions
