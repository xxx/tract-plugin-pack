# Workspace Restructuring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure the single-plugin repo into a `tract-plugin-pack` Cargo workspace with a shared `nih-plug-widgets` GUI crate.

**Architecture:** Create workspace root with members list + profiles. Extract ParamDial and CSS into `nih-plug-widgets` crate. Move existing plugin into `wavetable-filter/` subdirectory. All existing tests and builds must continue to work.

**Tech Stack:** Rust nightly, Cargo workspaces, nih-plug, nih_plug_vizia

---

### Task 1: Create the `nih-plug-widgets` crate

**Files:**
- Create: `nih-plug-widgets/Cargo.toml`
- Create: `nih-plug-widgets/src/lib.rs`
- Create: `nih-plug-widgets/src/param_dial.rs` (copy from `src/editor/param_dial.rs`)
- Create: `nih-plug-widgets/src/style.css` (copy from `src/style.css`)

- [ ] **Step 1: Create directory structure**

```bash
mkdir -p nih-plug-widgets/src
```

- [ ] **Step 2: Create `nih-plug-widgets/Cargo.toml`**

```toml
[package]
name = "nih-plug-widgets"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
nih_plug = { git = "https://github.com/robbert-vdh/nih-plug.git", features = ["simd"] }
nih_plug_vizia = { git = "https://github.com/robbert-vdh/nih-plug.git" }
```

Note: No `standalone` feature — that is plugin-specific.

- [ ] **Step 3: Copy `param_dial.rs` into the new crate**

```bash
cp src/editor/param_dial.rs nih-plug-widgets/src/param_dial.rs
```

No code changes needed — ParamDial's imports (`nih_plug::prelude::Param`, `nih_plug_vizia::vizia::prelude::*`, etc.) are all satisfied by the new crate's dependencies.

- [ ] **Step 4: Copy `style.css` into the new crate**

```bash
cp src/style.css nih-plug-widgets/src/style.css
```

- [ ] **Step 5: Create `nih-plug-widgets/src/lib.rs`**

```rust
pub mod param_dial;

pub use param_dial::ParamDial;

use nih_plug_vizia::vizia::prelude::*;

/// Load the shared dark theme CSS into a vizia context.
/// Uses `include_str!` for compile-time embedding — no runtime file access needed.
pub fn load_style(cx: &mut Context) {
    cx.add_stylesheet(include_str!("style.css"))
        .expect("Failed to load nih-plug-widgets stylesheet");
}
```

- [ ] **Step 6: Verify the new crate compiles standalone**

```bash
cd nih-plug-widgets && cargo check && cd ..
```

Expected: compiles with no errors.

---

### Task 2: Create workspace root `Cargo.toml`

**Files:**
- Modify: `Cargo.toml` (replace with workspace-only manifest)

- [ ] **Step 1: Replace root `Cargo.toml` with workspace manifest**

The current `Cargo.toml` has `[package]`, `[workspace]`, `[dependencies]`, `[profile.*]`, and `[package.metadata.bundler]`. Replace it entirely with:

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

No `[package]` section. Profiles must be here (Cargo ignores them in member manifests).

---

### Task 3: Move plugin files into `wavetable-filter/` subdirectory

**Files:**
- Create: `wavetable-filter/Cargo.toml`
- Move: `src/` → `wavetable-filter/src/`
- Move: `build.rs` → `wavetable-filter/build.rs`
- Move: `tests/` → `wavetable-filter/tests/`

- [ ] **Step 1: Create `wavetable-filter/` directory and its `Cargo.toml`**

```bash
mkdir -p wavetable-filter
```

Create `wavetable-filter/Cargo.toml`:

```toml
[package]
name = "wavetable-filter"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "lib"]

[[bin]]
name = "wavetable-filter"
path = "src/main.rs"

[dependencies]
nih-plug-widgets = { path = "../nih-plug-widgets" }
nih_plug = { git = "https://github.com/robbert-vdh/nih-plug.git", features = ["simd", "standalone"] }
nih_plug_vizia = { git = "https://github.com/robbert-vdh/nih-plug.git" }
hound = "3.5"
atomic_float = "1.0"
rfd = "0.17.2"
realfft = "3.3"
rustfft = "6.2"

[package.metadata.bundler]
name = "Wavetable Filter"
company = "mpd"
description = "A wavetable-based filter plugin"
license = "GPL-3.0-or-later"
version = "0.1.0"
```

No `[workspace]` section — that belongs to the root only.

- [ ] **Step 2: Move source files**

```bash
mv src/ wavetable-filter/src/
mv build.rs wavetable-filter/build.rs
mv tests/ wavetable-filter/tests/
```

- [ ] **Step 3: Remove the old `param_dial.rs` and `style.css` from the plugin**

These now live in `nih-plug-widgets`:

```bash
rm wavetable-filter/src/editor/param_dial.rs
rm wavetable-filter/src/style.css
```

---

### Task 4: Update `editor.rs` to use `nih-plug-widgets`

**Files:**
- Modify: `wavetable-filter/src/editor.rs`

- [ ] **Step 1: Update module declarations and imports**

In `wavetable-filter/src/editor.rs`, change the top of the file from:

```rust
mod filter_response_view;
mod param_dial;
mod wavetable_view;

use nih_plug::prelude::Editor;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::Arc;

use crate::WavetableFilterParams;
use filter_response_view::FilterResponseView;
use param_dial::ParamDial;
use wavetable_view::WavetableView;
```

To:

```rust
mod filter_response_view;
mod wavetable_view;

use nih_plug::prelude::Editor;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::Arc;

use crate::WavetableFilterParams;
use filter_response_view::FilterResponseView;
use nih_plug_widgets::ParamDial;
use wavetable_view::WavetableView;
```

Changes: removed `mod param_dial;`, replaced `use param_dial::ParamDial` with `use nih_plug_widgets::ParamDial`.

- [ ] **Step 2: Replace stylesheet loading**

In the same file, change:

```rust
        cx.add_stylesheet(include_str!("style.css"))
            .expect("Failed to load stylesheet");
```

To:

```rust
        nih_plug_widgets::load_style(cx);
```

---

### Task 5: Move docs to organized structure

**Files:**
- Move: `docs/manual/` → `docs/wavetable-filter/`

- [ ] **Step 1: Reorganize docs**

```bash
mkdir -p docs/wavetable-filter
mv docs/manual/* docs/wavetable-filter/
rmdir docs/manual
```

---

### Task 6: Verify everything builds and passes

- [ ] **Step 1: Run `cargo check --workspace`**

```bash
cargo check --workspace
```

Expected: all three crates compile with no errors.

- [ ] **Step 2: Run all tests**

```bash
cargo test -p wavetable-filter
```

Expected: all 30 tests pass.

- [ ] **Step 3: Run clippy**

```bash
cargo clippy --workspace
```

Expected: no warnings.

- [ ] **Step 4: Build release plugin bundles**

```bash
cargo nih-plug bundle wavetable-filter --release
```

Expected: produces `.vst3` and `.clap` bundles in `target/bundled/`.

---

### Task 7: Rename repo directory and clean up

- [ ] **Step 1: Rename the repository directory**

```bash
cd .. && mv wavetable-filter tract-plugin-pack && cd tract-plugin-pack
```

- [ ] **Step 2: Verify build still works after rename**

```bash
cargo test -p wavetable-filter
cargo nih-plug bundle wavetable-filter --release
```

- [ ] **Step 3: Clean up stale screenshots from root**

```bash
rm -f ss.png ss2.png ss3.png
```

- [ ] **Step 4: Update CLAUDE.md and README.md references**

Update any paths or descriptions that reference the old flat layout. The README should describe the workspace structure. CLAUDE.md should update the Architecture table to reflect the new crate layout.
