# Screenshot Inventory — Multosis manual

The Multosis manual references the following images. Save each one in
this directory with the exact filename and the manual will pick them up
on the next `pandoc` build.

## Overview

| Filename | What it shows |
|---|---|
| `screenshot.png` | Full plugin window. Default size 1456 x 758. Show grid populated with a few active steps across different rows and one row's effect editor visible. Save as the primary "marketing" image of the plugin. |

## Top bar

| Filename | What it shows |
|---|---|
| `toolbar.png` | The transport / master strip across the top of the plugin: Speed selector dropdown, Mix dial, Output dial, Comp Threshold + Comp Ratio dials, Reset button. Tight crop, just the bar. |

## Grid sequencer

| Filename | What it shows |
|---|---|
| `grid.png` | The step grid with the playhead visible mid-traversal. Show several rows with active steps and the playhead column highlighted. The 16 rows on the left edge should all be labelled by their effect kind. |

## Track list (row controls)

| Filename | What it shows |
|---|---|
| `track-row.png` | A close-up of one or two adjacent track rows on the left of the window. Show: the effect-kind picker (closed, showing the current selection with the family caption above the name), the Mix dial, the stacked M and S buttons. |
| `effect-dropdown.png` | The effect-kind picker dropdown OPEN with the section headers visible (None at top, then Distortion / Dynamics / Filter / Misc / Modulation / Pitch / Spatial / Time / Spectral). Scroll position should show several family headers at once -- the Modulation block is a good middle of the list. Type into the search box optional but illustrative. |

## Effect editor (parameter panel)

| Filename | What it shows |
|---|---|
| `effect-editor.png` | The per-effect parameter editor for a representative effect. Use **Ladder** as the subject -- its 3 dials (Cutoff/Resonance/Drive) are a clean image. Show the dials with non-default settings. |
| `effect-editor-stepped.png` | Same panel, but for an effect with a stepped/enum parameter -- use **Distortion** so you see the Type selector cycling through Hard/Soft/Cubic/Sine/Fold alongside the continuous dials. |
| `effect-editor-spectral.png` | The editor for a spectral effect, showing the FFT-size selector at the end of the dial row -- use **Spectral Bandpass** which has Freq / Width / FFT. The FFT selector should be visible at the right end. |

## MSEG editor

| Filename | What it shows |
|---|---|
| `mseg.png` | The MSEG editor for one row showing a multi-node curve drawn across the canvas. Include the four MSEG slot tabs (Amp, Mod1, Mod2, Mod3) visible. Pick a row where the user has actually shaped a curve so it's not flat. |
| `mseg-assign.png` | The MSEG with its assignment menu open (the target-param dropdown), showing the user about to assign Mod2 to a specific effect param. Optional -- can skip if the UI is hard to capture mid-interaction. |

## Optional / nice-to-have

| Filename | What it shows |
|---|---|
| `transport-stopped.png` | The plugin while the host transport is stopped (playhead frozen). Useful for the "playback" section but the main screenshot already implies this. |
| `solo-mute.png` | A track row with the M button lit (muted) and another row with S lit (soloed) -- demonstrates the chain-bypass behaviour described in the manual. |

## Notes for collection

- All screenshots should be from the same theme/skin (Multosis only has one).
- Use a consistent zoom level (the default 1456 x 758 window) so the
  dials and grid cells look the same size across images.
- PNG, no compression artifacts.
- Crop tightly -- the manual layout uses `width=80%` or `width=50%` for
  most embeds, and white space around the UI elements wastes vertical space.
