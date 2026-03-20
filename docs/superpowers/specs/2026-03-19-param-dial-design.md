# ParamDial Widget Design

## Goal

Replace horizontal parameter sliders with arc-style rotary dials in a compact horizontal row, giving the plugin a more conventional audio-plugin look while preserving all parameter binding functionality.

## Overview

A new `ParamDial` custom Vizia view that renders a 270-degree arc knob with value text. Same constructor API as `ParamSlider` — drop-in replacement for continuous parameters. The editor layout changes from vertical slider rows to a horizontal row of dial columns.

## Widget: `ParamDial`

### File

`src/editor/param_dial.rs` (new module, added to `src/editor.rs` via `mod param_dial`)

### Constructor

```rust
ParamDial::new(cx, Data::params, |params| &params.frequency)
```

Same lens + closure pattern as `ParamSlider`: generic over `L: Lens<Target = Params>` and `FMap: Fn(&Params) -> &P`. Internally creates a `ParamWidgetBase` for parameter binding.

### Visual Structure

Vertical layout, top to bottom:

1. **Label** — parameter name (from `param.name()`), small centered text above the arc
2. **Arc area** — 70x70px square containing:
   - **Background track** — 270-degree arc (225° to -45°, gap at bottom), dim color (`#404040`)
   - **Value arc** — same path but filled from start to current normalized position, accent color (e.g. `#4fc3f7` or similar to match the existing UI blue tones)
   - **Indicator dot** — small filled circle (radius ~3px) at the endpoint of the value arc
3. **Value text** — formatted parameter value with unit (e.g. "1.2 kHz"), small centered text below the arc

Total widget size: approximately 70px wide x 110px tall (70px arc + ~20px label + ~20px value).

### Arc Geometry

- Center: middle of the 70x70 area
- Radius: ~28px (leaving room for the indicator dot)
- Start angle: 225° (7:30 clock position, bottom-left)
- End angle: -45° (4:30 clock position, bottom-right)
- Sweep: 270° clockwise
- Stroke width: ~3px for background track, ~3px for value arc
- Value arc endpoint: `start_angle + normalized_value * 270°` (in the sweep direction)

### Interaction

**Vertical drag (primary):**
- Mouse down on the dial area → call `begin_set_parameter()`, capture starting Y coordinate and current normalized value
- Mouse move → compute delta Y from start, map to normalized value change. Sensitivity: full 0.0→1.0 range over ~200px of vertical mouse movement (drag up = increase, drag down = decrease). Clamp result to [0.0, 1.0].
- Mouse up → call `end_set_parameter()`

**Shift+drag (fine-tune):**
- When shift is held during drag, reduce sensitivity to 0.1x (full range over ~2000px). Same granular drag pattern as `ParamSlider`.

**Double-click (reset to default):**
- On double-click, call `begin_set_parameter()`, `set_normalized_value(default)`, `end_set_parameter()`.

**Scroll wheel:**
- Each scroll tick changes the normalized value by a small step (e.g. 0.02, or 0.005 with shift held). Wrapped in begin/set/end calls.

### Parameter Binding

Uses `ParamWidgetBase` from `nih_plug_vizia::widgets::param_base`:

- `begin_set_parameter(cx)` — emits `RawParamEvent::BeginSetParameter`
- `set_normalized_value(cx, value)` — emits `RawParamEvent::SetParameterNormalized` (with plain→normalized roundtrip for stepped params)
- `end_set_parameter(cx)` — emits `RawParamEvent::EndSetParameter`

Value display uses a lens on `param.unmodulated_normalized_value()` piped through `param.normalized_value_to_string(value, true)` to get the live formatted string with unit.

### CSS Class

Element name: `"param-dial"`. Styled via `style.css` for background color consistency with the existing theme.

## Editor Layout Change

### Current Layout (before)

```
VStack {
  Label("Wavetable Filter")
  Status label
  HStack { Label("Wavetable Path:") + path display + Browse button }
  HStack { Label("Frequency") + ParamSlider }
  HStack { Label("Frame Position") + ParamSlider }
  HStack { Label("Resonance") + ParamSlider }
  HStack { Label("Mix") + ParamSlider }
  HStack { Label("Drive") + ParamSlider }
  HStack { Label("Mode") + ParamSlider }
  HStack { WavetableView | FilterResponseView }
}
```

### New Layout (after)

```
VStack {
  Label("Wavetable Filter")
  Status label
  HStack { Label("Wavetable Path:") + path display + Browse button }
  HStack {
    ParamDial(frequency)
    ParamDial(frame_position)
    ParamDial(resonance)
    ParamDial(mix)
    ParamDial(drive)
  }
  HStack { Label("Mode") + ParamSlider(mode) }
  HStack { WavetableView | FilterResponseView }
}
```

The five continuous parameters become dials in a single horizontal row, evenly spaced. Mode remains a `ParamSlider` since it's a discrete two-option enum (Raw / Phaseless) where a dial doesn't apply.

### Spacing

- Dials row: `HStack` with `col_between(Pixels(20.0))` and centered content
- Row height: ~120px to accommodate label + dial + value text
- The overall window dimensions may need minor adjustment depending on how the layout packs

## What's Not Included

- **Text input on alt+click** — can be added later if needed
- **Bipolar mode** — not needed; all current continuous params are unipolar (0 to max)
- **Custom colors per dial** — all dials use the same accent color for now

## Colors

Consistent with existing dark theme (`style.css`):
- Background track: `#404040` (matches border colors)
- Value arc: `#4fc3f7` (light blue accent) or similar
- Indicator dot: same as value arc color
- Label text: `#a0a0a0` (matches `.section-title`)
- Value text: `#ffffff` (matches main label color)
