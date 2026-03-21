# Workspace Restructuring: tract-plugin-pack

## Goal

Restructure the single-plugin `wavetable-filter` repo into a Cargo workspace called `tract-plugin-pack` with a shared GUI widget crate (`nih-plug-widgets`) so future plugins can reuse ParamDial and the CSS theme without code duplication.

## Motivation

The custom GUI work (ParamDial rotary knob with modulation display, dark theme CSS) is generic to any nih-plug plugin. Extracting it into a shared crate avoids duplicating this code across future plugins.

## Workspace Layout

```
tract-plugin-pack/
├── Cargo.toml                    # workspace root (members + profiles)
├── Cargo.lock                    # shared lockfile (ensures consistent dep versions)
├── rust-toolchain.toml
├── README.md
├── CLAUDE.md
├── nih-plug-widgets/             # shared GUI widget crate
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                # re-exports ParamDial, provides load_style()
│       ├── param_dial.rs         # moved from src/editor/param_dial.rs
│       └── style.css             # moved from src/style.css (common dark theme)
├── wavetable-filter/             # existing plugin in subdirectory
│   ├── Cargo.toml
│   ├── build.rs                  # mold linker detection (per-plugin)
│   ├── tests/
│   │   └── fixtures/
│   │       └── phaseless-bass.wt
│   └── src/
│       ├── lib.rs                # DSP code (unchanged)
│       ├── main.rs               # standalone entry point
│       ├── wavetable.rs          # wavetable I/O (unchanged)
│       ├── editor.rs             # layout (imports from nih-plug-widgets)
│       └── editor/
│           ├── wavetable_view.rs     # plugin-specific view
│           └── filter_response_view.rs  # plugin-specific view
├── docs/                         # documentation at root, organized by plugin
│   ├── wavetable-filter/         # plugin-specific docs (manual, screenshots)
│   └── superpowers/              # project-level specs and plans
└── xtask/                        # build tooling stays at root
```

## Root `Cargo.toml`

Workspace definition and shared profiles only — no `[package]` section:

```toml
[workspace]
members = ["nih-plug-widgets", "wavetable-filter", "xtask"]
resolver = "2"

[profile.release]
lto = true
strip = true

[profile.profiling]
inherits = "release"
debug = true
strip = false
```

Note: `[profile.*]` sections must be in the workspace root `Cargo.toml` — Cargo ignores profiles in member manifests. The root manifest has no `[package]` section, so no `build.rs` runs from the root.

## Crate: `nih-plug-widgets`

### Purpose

A workspace-local crate (not published) providing reusable nih-plug/vizia GUI widgets and a base CSS theme.

### Public API

- `ParamDial` — rotary knob widget with modulation indicator arc, vertical drag, shift fine-tune, double-click reset
- `load_style(cx: &mut Context)` — loads the shared dark theme CSS via `include_str!` (compile-time embedding, no runtime file access needed in DAW deployment)

### Dependencies

- `nih_plug` (same git source as wavetable-filter, no `standalone` feature — that is plugin-specific)
- `nih_plug_vizia` (same git source as wavetable-filter)

### What moves here

- `src/editor/param_dial.rs` → `nih-plug-widgets/src/param_dial.rs`
- `src/style.css` → `nih-plug-widgets/src/style.css`

### What stays plugin-specific

- WavetableView, FilterResponseView — specific to the wavetable filter
- `editor.rs` layout — each plugin defines its own layout
- Any plugin-specific CSS overrides

## Changes to `wavetable-filter`

1. All source files move into `wavetable-filter/` subdirectory
2. Gets its own `Cargo.toml` with `[package]` only (no `[workspace]` section — that belongs to the root)
3. `build.rs` moves into `wavetable-filter/build.rs` (build scripts are per-package)
4. `tests/fixtures/` moves into `wavetable-filter/tests/fixtures/` (tests use `CARGO_MANIFEST_DIR` for paths)
5. `editor.rs` changes:
   - Remove `mod param_dial`
   - Add `use nih_plug_widgets::ParamDial`
   - Replace `include_str!("style.css")` with `nih_plug_widgets::load_style(cx)`
   - Plugin-specific CSS (if any) loaded after the shared style
6. DSP code (`lib.rs`, `wavetable.rs`) — unchanged
7. `PendingReload` and other internal types — unchanged

## Dependency Flow

```
nih-plug-widgets
  └── depends on: nih_plug (no standalone feature), nih_plug_vizia

wavetable-filter
  └── depends on: nih-plug-widgets (path), nih_plug (simd + standalone), nih_plug_vizia,
                  hound, rfd, realfft, rustfft, atomic_float
```

The shared `Cargo.lock` at the workspace root ensures all crates resolve to the same git revision for `nih_plug` and `nih_plug_vizia`.

## Future Plugin Pattern

A new plugin crate would:

1. Create `new-plugin/Cargo.toml` depending on `nih-plug-widgets = { path = "../nih-plug-widgets" }`
2. Copy `wavetable-filter/build.rs` if mold linker detection is desired
3. Add to workspace members list
4. Use `nih_plug_widgets::load_style(cx)` + `nih_plug_widgets::ParamDial` in its editor
5. Define its own views and DSP

## Repo Rename

The repository directory is renamed from `wavetable-filter` to `tract-plugin-pack`. This is a local directory rename; any remote (GitHub) rename is a separate manual step.

## Testing

- All existing tests in `wavetable-filter` continue to pass (`cargo test -p wavetable-filter`)
- `cargo clippy --workspace` passes clean
- `cargo nih-plug bundle wavetable-filter --release` produces working VST3/CLAP bundles
- ParamDial renders identically after extraction (visual verification)
