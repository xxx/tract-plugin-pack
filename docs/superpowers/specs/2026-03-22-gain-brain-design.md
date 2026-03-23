# Gain Brain Design Spec

## Overview

Gain Brain is a lightweight gain utility plugin with cross-instance group linking. Multiple instances across a project can be assigned to the same group (1-16), and changing gain on any grouped instance applies that change to all others in the group. Designed for many instances per project, using the same CPU-rendered GUI stack as GS Meter.

Inspired by BlueCat's Gain Suite grouping feature.

## Plugin Parameters

| Parameter | Type | Range | Default | Notes |
|-----------|------|-------|---------|-------|
| `gain` | FloatParam | -60 to +60 dB (stored as linear gain) | 0.0 dB (unity) | Smoothed (50ms linear). Intentionally wider range than GS Meter's -40/+40 to match BlueCat. |
| `group` | IntParam | 0-16 | 0 | 0 = no group (standalone), 1-16 = group |
| `link_mode` | EnumParam | Off / Absolute / Relative | Off | Controls how group changes propagate |

**Latency:** Zero. No lookahead or convolution — just a gain multiplier.

## Link Modes

### Off (X)
Instance operates standalone. The group parameter value is persisted but inactive — no communication with other instances. This is the default mode.

### Absolute
All instances in the same group maintain identical gain values. When any instance's gain changes, all others in the group snap to that value. Use case: controlling the level of parallel bus sends identically.

### Relative
Gain *changes* (deltas) propagate to all instances in the group, but each instance keeps its own base gain. Use case: all instances gain-ride together while maintaining individual offsets (e.g., +3 dB on vocals, -2 dB on bass, but turning them both up by 1 dB simultaneously).

### Joining Behavior

**Changing group parameter (with link_mode Absolute or Relative):**
- Absolute: adopt the group's current gain value.
- Relative: keep current gain, baseline to the group's current value (future deltas are relative to that baseline).

**Changing link_mode (with group already set to 1-16):**
- Off -> Absolute: adopt the group's current gain value (same as joining).
- Off -> Relative: keep current gain, baseline to the group's current value.
- Absolute -> Relative: keep current gain (which is already the group's value), baseline to it.
- Relative -> Absolute: snap to the group's current gain value (may change local gain).

**Leaving a group (group set to 0, or link_mode set to Off):**
Keep current gain, stop reading/writing the group slot.

### Relative Mode — Delta Clamping
When a delta is applied, each instance clamps the result to the -60/+60 dB range. An instance at +58 dB receiving a +5 dB delta clamps to +60 dB. This does not affect other instances' deltas.

### Relative Mode — Known Limitation
With 3+ instances in a group, if two instances change gain within the same buffer cycle (~10-21ms at typical buffer sizes), one instance may miss an intermediate delta. This requires two humans physically turning two knobs simultaneously within 10ms, or conflicting automation lanes — both practically impossible in normal use. Accepted as a known limitation.

## Cross-Instance Communication

### Mechanism: Memory-Mapped File

A single memory-mapped file provides shared state across all instances, regardless of whether they run in the same process or are sandboxed into separate processes by the DAW. Direct memory reads/writes — no syscalls per buffer.

**File location:**
- Linux: `$XDG_RUNTIME_DIR/gain-brain-groups` (falls back to `/tmp/gain-brain-groups`)
- macOS: `$TMPDIR/gain-brain-groups` (falls back to `/tmp/gain-brain-groups`)
- Windows: `%LOCALAPPDATA%\Temp\gain-brain-groups`

**Crate:** `memmap2` for cross-platform memory mapping.

**Unsafe:** Only the `MmapMut::map_mut(&file)` constructor requires `unsafe` — this is an approved exception to the project's no-unsafe rule (see CLAUDE.md). The constructor is unsafe because the OS could theoretically modify the underlying file externally; this is safe in our case because only gain-brain instances access the file, and the layout is fixed. After construction, all reads/writes use safe byte-slice operations (`i32::from_le_bytes`, `i32::to_le_bytes`).

### Shared Memory Layout

The mmap file has a fixed size: a header followed by 16 group slots.

```
Header (16 bytes):
  magic: [u8; 4]       = b"GBRN"     — identifies the file
  version: u32          = 1           — layout version for forward compat (little-endian)
  _reserved: [u8; 8]    = [0; 8]     — future use

Per-group slot (16 bytes each, 16 groups = 256 bytes):
  gain_millibels: i32    — current gain in 0.01 dB units, little-endian (e.g., 350 = +3.50 dB)
  generation: u32        — incremented on every change, little-endian, for change detection
  _reserved: [u8; 8]     — future use

Total file size: 16 + (16 × 16) = 272 bytes
```

All values are little-endian. Reads and writes use `i32::from_le_bytes` / `i32::to_le_bytes` and `u32::from_le_bytes` / `u32::to_le_bytes` on the mmap byte slice. No pointer casting or transmuting.

### File Creation and Initialization

On first access, the plugin:
1. Attempts to open the file with read/write access.
2. If the file does not exist, creates it and writes the header + zeroed group slots.
3. Uses advisory file locking during creation to prevent races when multiple instances start simultaneously.
4. After creation, validates the magic bytes and version. If version != 1, falls back to standalone mode (forward compatibility with future layout changes).

### Stale Data / Session Boundaries

Group slots persist across DAW sessions. When a project is reopened, group slots contain gain values from the previous session. This is acceptable — instances joining a group will adopt (Absolute) or baseline from (Relative) whatever value is in the slot. If no instances are in a group, that slot's stale value is harmless.

The file is never deleted by the plugin. It is small (272 bytes) and located in a temp directory that the OS may clean on reboot.

### Instance Behavior

Each instance holds:
- `my_group: u8` — current group (0 = none, 1-16)
- `my_link_mode: LinkMode` — Off / Absolute / Relative
- `last_seen_generation: u32` — last generation observed from the group slot
- `last_sent_gain_millibels: i32` — last gain value this instance wrote

**On process() — every buffer:**
1. If `my_group == 0` or `my_link_mode == Off`, skip.
2. Read the group slot from the mmap (16 bytes at the appropriate offset).
3. If `generation == last_seen_generation`, no change — skip.
4. If `gain_millibels == last_sent_gain_millibels`, this is likely our own echo — update `last_seen_generation` and skip. (Edge case: another instance may have written the same value. In Absolute mode this is harmless; in Relative mode the delta is 0.)
5. Apply the change:
   - **Absolute:** set local gain to `gain_millibels`.
   - **Relative:** compute delta = `gain_millibels - last_sent_gain_millibels`, apply delta to local gain, clamp to -60/+60 dB range.
6. Update `last_seen_generation` and `last_sent_gain_millibels`.

**On gain change (user interaction or automation):**
1. If `my_group == 0` or `my_link_mode == Off`, skip.
2. Write new `gain_millibels` and incremented `generation` to the mmap group slot.
3. Update `last_sent_gain_millibels` and `last_seen_generation`.

### Graceful Degradation

If the shared file cannot be created or opened (permissions, unusual OS config), the plugin operates in standalone mode. No crash, no error dialog — grouping silently becomes unavailable. A log message is emitted via `nih_log!`.

## GUI

### Rendering Stack

CPU-rendered using softbuffer + tiny-skia + fontdue, same as GS Meter. No GPU dependencies.

### Layout

Compact vertical layout. The plugin should be small — it's a utility meant to sit on many tracks.

```
┌──────────────────────┐
│   Gain Brain    - +  │  ← title + scale buttons
├──────────────────────┤
│ Group   [X|1|2|..|16]│  ← stepped selector (scrollable or compact)
│ Link    [Off|Abs|Rel]│  ← stepped selector
│ Gain    [=========]  │  ← slider
│         +3.5 dB      │  ← gain readout
├──────────────────────┤
│      [ Reset ]       │  ← reset gain to 0 dB
└──────────────────────┘
```

### Window Size

Target: ~300 x 300 pixels at 1x scale. Scalable 75%-300% (same as GS Meter).

## Workspace Integration

New crate at `gain-brain/` in the workspace root:

```
gain-brain/
├── Cargo.toml
├── src/
│   ├── lib.rs          — plugin struct, params, process()
│   ├── main.rs         — standalone entry point
│   ├── editor.rs       — softbuffer GUI (same pattern as gs-meter)
│   ├── groups.rs       — file-based shared state, group IPC
│   └── widgets.rs      — copied from gs-meter (extract to shared crate later)
```

### Widget Reuse

GS Meter's `widgets.rs` contains general-purpose drawing primitives (draw_rect, draw_text, draw_button, draw_slider, draw_stepped_selector, TextRenderer). Copy for now, refactor into a shared crate when a third plugin needs them.

### Dependencies

- `nih-plug` (same fork as other plugins)
- `softbuffer`, `tiny-skia`, `fontdue`, `baseview`, `crossbeam` (same as GS Meter)
- `memmap2` (new — for cross-instance shared memory)
- `raw-window-handle` (same as GS Meter)

## Testing Strategy

### Unit Tests (groups.rs)

- Create shared file, verify header magic and version
- Write gain to a group slot, read it back
- Generation counter increments on write
- Two simulated instances in absolute mode stay synchronized
- Two simulated instances in relative mode apply deltas correctly
- Joining a group in absolute mode adopts current gain
- Joining a group in relative mode preserves local gain
- Leaving a group stops synchronization
- Changing link_mode from Off to Absolute triggers joining behavior
- Changing link_mode from Off to Relative triggers joining behavior
- Instance at gain limit receives delta — clamps correctly
- Self-echo detection (instance ignores its own writes)
- Graceful degradation when file cannot be created
- Version mismatch falls back to standalone mode

### Integration Tests

- Process() with grouped instances: verify gain changes propagate
- Mode switching mid-session
- Group switching mid-session

## Non-Goals (YAGNI)

- No MIDI control (can add later)
- No stereo/independent L-R gain (future enhancement)
- No mid/side mode
- No preset management beyond DAW presets
- No visual meters or level display beyond the gain readout
- No "reverse" link mode (BlueCat has this — not needed now)
