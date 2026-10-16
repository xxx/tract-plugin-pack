# Minitosis implementation

## Context

Behavior is approved in PRODUCT.md and the conversation. The preceding refactor is committed as f4f7a41 on feature/minitosis. `tract-effects/src/effects/mod.rs`, `tract-effects/src/modulation.rs`, and `tract-effects/src/editor/mod.rs` supply the registry/DSP, single-effect modulation state, and shared controller. `multosis/src/editor.rs` demonstrates nih-plug window integration. Minitosis must depend on the shared library rather than the Multosis plugin.

## Proposed changes

- Add the minitosis workspace crate, standalone entry point, plugin identifiers, bundle metadata, documentation, and build examples.
- Expose static shared parameter specifications without constructing DSP engines. Keep specification values and DSP algorithms unchanged.
- Derive nih-plug Params for fixed parameter records nested in an immutable per-effect bank. Each record has five normalized controls, using shared scaling/format/parse rules; unused slots are hidden. Effect-prefixed names keep the automation list understandable. Preserve a fixed append-only bank order independent of the family-sorted picker; a checked-in identity manifest guards changes.
- Keep effect selection non-automatable and store every effect's modulation in a persistent bank. Host parameter banks provide base-value recall; the shared editor receives a temporary selected-effect view and writes through ParamSetter. GUI MSEG changes update only the selected persistent bank entry. The GUI keeps pending/confirmed selection explicitly so deferred CLAP/VST3 setter echoes do not lose local undo or prepare an earlier kind.
- A boxed single-effect processor owns its effect, modulation state/config cache, and preallocated stereo dry-delay ring. Initialize creates the selected engine off the audio thread. Each block restores current automated base values, analyzes transient input, advances shared modulation, applies the wet gain, and mixes latency-aligned dry/wet. Bypass keeps the dry delay and engine running but ignores Output gain. Reset restarts modulation only; host reset flushes DSP/dry history while preserving modulation phases and transport-edge history.
- Use nih-plug background tasks for subsequent effect preparation. Requests carry selection generation, kind, and sample rate. Process deduplicates requests and adopts only the matching prepared engine with try_lock and mem::swap; the old processor stays in the mailbox for destruction outside process. This also covers host generic-editor changes with no plugin editor open. Store at most active, retired/pending, and currently preparing engines rather than all effect engines.
- Return KeepAlive so hosts keep silent-input delay/reverb tails and running clocks alive.
- Keep one global Mix, Output, bypass, Retrig on play, and Reset toolbar. Use the shared EffectEditor with supplied geometry, a zero-sized navigation hit rect, and the three allowed trigger choices. Retain existing window-size/resize helpers and supply the same shared font asset.
- Keep host automation gestures and plugin undo snapshots separate. GUI drag changes bracket host gestures; local undo snapshots cover selection and modulation edits. Never send MSEG output to host parameter setters.

## Testing and validation

- PRODUCT 1–3: debug standalone and CLAP/VST3 bundle builds; render a representative editor frame and exercise the existing shared interaction tests.
- PRODUCT 4–10: inline processor/state tests for neutral defaults, None identity, all effect kinds, continuous clocks while stopped, tempo, one-shots/retrigger, modulation-only reset, per-effect recall, and fresh DSP on switches.
- PRODUCT 11, 17: test mailbox generations, contention, rapid switches, retired-engine ownership, and processing under the existing allocation guard.
- PRODUCT 12–13: regression manifest for unique stable parameter IDs, parameter normalization/formatting, host-state save/load, and host base values remaining untouched by modulation.
- PRODUCT 14: impulse tests for dry/wet latency alignment and bypass at several FFT sizes, plus output gain/Mix behavior.
- PRODUCT 15–16: state round trips, sanitization, undo/redo, shared input/host gesture tests, and pending-switch state restoration.
- Run cargo nextest run --workspace, cargo clippy --workspace -- -D warnings, cargo fmt --check, and native release bundle/standalone builds. Dispatch a read-only implementation review before completion.
