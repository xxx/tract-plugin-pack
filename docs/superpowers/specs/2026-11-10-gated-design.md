# Gated

## Summary

A compact rhythmic amplitude gate inspired by Kilohearts Trance Gate, with
eight patterns, a circular step editor, ADSR, swing, and random population.
The user approved the recommendations in interview questions 1–14 and asked
to see a working first version. No Figma mock was supplied; the reference is
https://kilohearts.com/products/trance_gate.

## Behavior

1. Build a VST3, CLAP, and standalone plugin called Gated, with the pack's
   CPU-rendered, freely resizable editor and zero reported latency.
2. Eight numbered slots select both the edited and sounding pattern. Selection
   is automatable and switches immediately at the incoming pattern's song
   position, with a short gain transition. New instances populate all eight slots
   with the initial Trance Gate rhythms documented by synthahol-phase-plant
   (source/attribution in `gated/THIRD_PARTY_NOTICES.md`). Slot lengths are
   16, 16, 32, 16, 16, 16, 16, and 64. Saved projects retain their stored bank.
3. Each slot stores 1–64 binary steps, outgoing ties, and its length. Length
   starts at the slot's factory value. Drag the large central count up to increase or down to decrease
   length, using 4 logical pixels per step and Shift for ten-times-finer movement.
   The count highlights and displays a hint while hovered or dragged. There are
   no +/− buttons. A drag edits the slot grabbed at its start and is one undo
   action on release or focus loss; external state recall cancels it. Shortening
   retains hidden steps for subsequent expansion.
   Storage leaves room for a later 128-step version, but v1 exposes only 64.
4. Clicking a step toggles it. Dragging clockwise or counterclockwise across
   steps enables and ties the traversed steps. Shift-drag paints steps off.
   Right-clicking a step toggles its outgoing tie when both endpoints are on.
   Ties may cross the end of the pattern. Each gesture is one undo action.
5. Untied on steps retrigger; tied on steps hold one envelope. Global Gate
   (1–100%, default 50%) shortens only the final step of each tied run.
6. Global Attack (0–2000 ms, default 2 ms), Decay (0–2000 ms, default 50 ms),
   Sustain (0–100%, default 100%), and Release (0–4000 ms, default 20 ms)
   shape a linear ADSR. Release starts at the level reached at note-off.
   A completely tied, entirely active circular pattern holds Sustain.
7. Global resolution offers straight, dotted, and triplet 1/1 through 1/64
   divisions; default 1/16. The clock follows the DAW's beat position, including
   seeks, loops, and negative pre-roll positions. Beat zero anchors step one.
8. Swing (0–100%, default zero) delays the second step of each pair, from
   equal lengths to 3:1. Every pair retains its duration. Pairing restarts at
   each pattern cycle; an unpaired final step in an odd-length pattern retains
   its ordinary duration. Swing also applies to dotted/triplet resolutions.
9. Stopped transport passes input through. Bypass passes input through while
   preserving clock position. Gain transitions have a 2 ms slew limit to avoid
   instantaneous gain changes. Both channels always receive the same gain.
10. Mix (default 100%) blends linearly with dry audio. Host automation exposes
    slot selection, resolution, ADSR, Gate, Swing, Mix, Density, Tie Probability,
    and bypass. Pattern content and length persist as document state.
11. Randomize changes only the selected slot's visible steps and ties, using
    Density (default 50%) and Tie Probability (default 25%). It preserves length,
    hidden steps, other slots, and timing/sound settings. Zero density may
    intentionally produce silence. Randomize is one undo action.
12. Copy/Paste shortcuts transfer a complete slot including length. Rotate left/right
    shifts visible steps and ties cyclically. Undo/redo restores pattern edits
    without changing automation controls or the selected slot. History is local
    to the open editor and clears when external document state replaces it.
    Copy/Paste and Undo/Redo are keyboard-only actions, without on-panel buttons.
13. Dials support vertical drag, Shift for fine adjustment, double-click reset,
    and right-click numeric entry. Enter commits; Escape cancels. Keyboard
    shortcuts are Ctrl/Cmd+Z, Ctrl/Cmd+Shift+Z, Ctrl/Cmd+Y, Ctrl/Cmd+C/V.
14. Use a tall circular layout: the title "gated" without a secondary brand label,
    slots, ring with centered length and transport status, two centered rotation
    buttons (68 × 27 logical pixels, 8-pixel gap), ADSR, resolution/Gate/Swing/Mix, and
    randomization controls. The ring shows step boundaries and ties, plus the
    currently sounding step when the DAW is playing.
15. When host timing is incomplete, use 120 BPM if tempo is absent and a local
    beat accumulator if position is absent. The local fallback restarts on
    transport start; the interface identifies missing song-position sync.

## Deferred

128 exposed steps and Euclidean population are future work. Independent stereo
patterns, MIDI triggering, and a user-selectable free-running clock are outside v1.
