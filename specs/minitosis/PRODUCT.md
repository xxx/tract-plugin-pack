# Minitosis

## Summary

Minitosis presents Multosis's effect editor as a standalone single-effect plugin. One selected effect processes incoming stereo audio continuously, with the same controls and four MSEGs, without the sequencer or track list.

## Design reference

Figma: none provided. The existing Multosis Effect view is the approved visual and interaction reference.

## Behavior

1. Minitosis is available as VST3, CLAP, and a standalone application, with its own identity and freely resizable, size-persisted CPU-rendered editor.
2. The effect picker offers all Multosis effect kinds, including None, grouped by the same families. Parameter names, ranges, defaults, formatting, enum menus, fine drags, double-click resets, and right-click text entry match Multosis.
3. Exactly one effect is selected and active. Audio processing continues when the transport is stopped; there is no grid, track list, step clock, mute/solo routing, or additional wet-bus compressor.
4. A fresh instance selects None, has Mix at 100%, Output at 0 dB, and bypass off. None passes audio through; the output control remains available.
5. The modulation section retains the 1, 2, 3, Amp tabs, target/depth routing, ghost curves, live dial arcs, curve editing, snapping, curve randomization, Time/Beat clocks, Cyclic/One-shot modes, and polarity controls.
6. Fresh Amp curves are flat at full level. All three assignable depths default to zero, so selecting an effect initially reveals its base sound.
7. Available trigger choices are Free run, Free Hz, and Transient. Cell light and Cell step do not appear.
8. All clocks continue while stopped. Beat clocks use host tempo with 120 BPM fallback; Time clocks retain their duration independently of tempo.
9. A global Retrig on play toggle optionally restarts the four modulation envelopes when the host starts playback. It defaults off. A Reset control restarts modulation without clearing delay/reverb tails.
10. Switching away and back recalls each effect's base parameters, MSEG curves, routing, depths, and trigger settings. Switching constructs fresh audio processing state rather than restoring an old delay/reverb tail. An effect's first visit uses its defaults and neutral modulation.
11. Switching is safe during playback. If preparation is pending, the plugin temporarily passes dry audio with the currently reported latency; stale preparation cannot install an earlier selection. The editor indicates a pending switch.
12. Every effect parameter has a stable, effect-specific DAW automation identity and readable effect/parameter name. Automation for one effect cannot alter another effect's parameters when the user changes selection. The effect selector itself is non-automatable; it remains editable in the plugin and host generic editor.
13. Automation changes are reflected in the editor, including when the editor was closed. MSEG modulation is applied around the current automated base value and does not overwrite that base value or record the MSEG motion as DAW automation.
14. There is one global Mix control. Output gain follows the blend; bypass produces latency-aligned dry audio without applying Output gain. Dry and wet are aligned for effects that report latency, including FFT-size changes.
15. Saving and reopening a project restores the selected effect, every effect's settings and modulation, globals, and window size. Loaded unsupported cell triggers fall back to Free run; out-of-range modulation targets are cleared.
16. Undo/redo covers effect selection and curve/routing edits. Continuous GUI parameter changes notify the host with one gesture per drag; parameter automation remains under the host's undo control. Randomizing effect parameters leaves Mix unchanged.
17. Instances remain independent. Audio processing, parameter changes, modulation, resets, and pending-effect adoption do not allocate, deallocate, or block on a mutex.
