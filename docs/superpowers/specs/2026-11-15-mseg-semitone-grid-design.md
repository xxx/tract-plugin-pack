# MSEG semitone snapping

Continue `feat/mseg-power-tools` after sidechain commit `9743f54`. The user
authorized the next wishlist slice. Reuse the existing CPU editor; no mock supplied.
Reference: [ShaperBox pitch grid](https://downloads.cableguys.com/Cableguys-ShaperBox-3-Manual.pdf#page=13).

## Behavior

1. Multosis/Minitosis add **Semitones** after the existing Grid presets. This
   keeps the time division and remembered numeric value division, selecting a
   per-envelope musical value grid. A numeric grid preset restores the ordinary
   value grid. Choosing the mode alone leaves node geometry and playback intact;
   Snap On/Off remains independent. The preference saves and participates in undo.
2. Musical levels follow the selected assignable target's actual mapping,
   including base value, depth sign/magnitude, and unipolar/bipolar routing.
   Linear `st` parameters snap to integer semitone offsets. Logarithmic Hertz
   parameters snap to equal-tempered frequencies with A4 = 440 Hz. Only reachable
   levels within the target range and normalized curve domain are offered.
3. Amp, Mix, enums, other units/scalings, zero/invalid depth, and mappings with no
   reachable note have no musical grid. Choosing Semitones there reports why and
   leaves preferences/geometry intact. If routing changes after selecting it,
   retain the preference and show **st n/a**; time snapping continues and vertical
   interactive snapping pauses until a supported mapping returns. Explicit
   Quantize rejects the entire edit while the musical grid is unavailable.
4. Drawn grid lines and snap levels use the same bounded GUI-generated positions.
   Pointer insertion/drag, the group anchor, selection edges, pen endpoints,
   stepped-draw heights, mutation, Randomize, and explicit Quantize use those
   levels. Existing fine-drag paths bypass them with Shift. Group/handle edits
   retain their geometric relationships; they do not quantize every member.
   Imported/pasted/factory shapes, numeric inspector input, and exact geometric
   commands retain their authored values until explicitly quantized.
5. Semitone pens retain the same preview/commit boundaries and repeat behavior;
   fine dragging may leave points off-grid. Preserve pinned endpoint times,
   marker identity, neighbor constraints, capacity, and atomic collision errors.
6. Mode and mapping refresh never quantize existing nodes automatically or create
   audio/undo work. Routing/base/depth changes refresh the grid before the next
   input event and draw. Cache the GUI mapping to avoid rebuilding it on each
   pointer move. Grid lines clip with the viewport and may be visually thinned
   when too close, while all levels remain available for snapping.
   Mapping updates preserve numeric/free pen previews and repeat placement;
   only active Semitones with Snap On invalidates placement when levels change.
7. Static Miff/Nap retain their numeric grids and have no Semitones menu entry.
   The shared widget accepts optional pitched-grid context for animated hosts.
   A loaded preference without context is safe. Shape clipboard/files continue
   transferring geometry only, leaving the destination grid settings intact.
8. Old projects omit the new defaulted grid preference and retain their exact
   serialized defaults, samples, snap behavior, and randomizer results. No new
   DSP interpolation, quantizer, host parameter, or audio-thread allocation.

## Implementation and validation

- Default-skip a ValueGrid enum in the fixed Copy document and custom serde.
  Keep normalized pitch positions in a GUI-only PitchGrid, with finite sorted
  bounded levels and shared nearest-level snapping. Reuse existing operations.
- The effect editor derives pitch levels from parameter metadata and inverses of
  its existing mapping, cached by kind/slot/target/base/depth/polarity. The widget
  remains independent of effects metadata. Unavailable contexts clear old levels.
- Test legacy bits/serialization; mapped pitch/frequency under both depth signs
  and polarities; clipping, limited reachability, invalid/zero depth, ties and
  idempotence; pointer/group/pen/step/mutate/quantize/fine paths; routing refresh;
  selection/marker and collision preservation; menu/undo/viewport behavior at
  0.5x/1x/2x; geometry-only clipboard; and static-editor compatibility.
- Render an actual mapped-grid preview, update manuals, run the six-crate focused
  suite, workspace Clippy, formatting/whitespace, and independent read-only
  reviews. Leave this slice uncommitted unless separately requested.
