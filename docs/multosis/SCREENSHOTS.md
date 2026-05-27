# Screenshot Inventory — Multosis manual

The Multosis manual references the images below. Save each one in this
directory with the exact filename and the manual will pick them up on the
next `pandoc` build.

## Overview

| Filename | What it shows |
|---|---|
| `screenshot.png` | Full plugin window. Default size 1456 × 758. Show grid populated with a few active steps across different rows, the loop region overlay visible, and a track in Effect view (so the reader sees both the grid plumbing and the effect editor in one image). Save as the primary "marketing" image of the plugin. |

## Top bar

| Filename | What it shows |
|---|---|
| `toolbar.png` | **Both rows** of the top toolbar. Upper row: Speed selector dropdown, Mix dial, Output dial, Comp Threshold + Comp Ratio dials, Reset button. Lower row: Reinit Cells, Rnd Cells, Copy, Paste op buttons, plus the "Step N" status readout anchored in the middle. Tight crop, both rows fully in frame. |

## Grid sequencer

| Filename | What it shows |
|---|---|
| `grid.png` | The step grid with the playhead visible mid-traversal AND the loop region overlay visible. Show several rows with active steps and the playhead column highlighted. The loop region should be smaller than the full grid (e.g. resized to a sub-rectangle) so the overlay is obvious. The 16 rows on the left edge should all be labelled by their effect kind. |

## Track list (row controls)

| Filename | What it shows |
|---|---|
| `track-row.png` | A close-up of two or three adjacent track rows on the left of the window. The frame should cover: the row number ("1"-"16"), the family caption (e.g. "Filter") above the unique suffix ("Ladder") on at least one row, the stacked M and S buttons on the right of each row, and the sounding dot at the far right. **Include at least one row whose effect adds latency (any Spectral effect, Satch, or Warp Zone)** so the teal PDC stripe at the very left edge is visible. Optionally show one row with M lit (muted) and one with S lit (soloed) to demonstrate the colour treatment. |

## Effect editor

| Filename | What it shows |
|---|---|
| `effect-editor.png` | The Effect view's **EFFECT section** for a representative effect — use **Ladder** as the subject. The frame should show: the `< Grid` back button at the top-left, the "Editing Track N" title, the **Kind dropdown** (closed, showing "Ladder" + the family caption), the **Randomize button** below it, the parameter dials (Cutoff / Resonance / Drive) set to non-default values, and the **Mix dial** at the right of the dial row. Showing the live modulation arc on at least one dial (a coloured arc overlay tracking the current modulated position) is a nice plus if you have an MSEG running. |
| `effect-editor-spectral.png` | The editor for a spectral effect, showing the FFT-size selector at the right of the dial row — use **Spectral Bandpass** which has Freq / Width / FFT. The FFT selector should be in frame at the right end (rendered as a dropdown trigger, not visually distinct from other Enum params). |

## MSEG editor

| Filename | What it shows |
|---|---|
| `mseg.png` | The MSEG editor's curve canvas for one row, showing a multi-node curve drawn across it. The frame should include: the routing strip **above** the canvas (Trigger dropdown, Target dropdown, Depth dial, Sync mode Time/Beat tabs, Length slider) **and** the strip of mode controls **below** the canvas (Snap, Grid, Style, Polarity, Play Mode, Randomize). Tabs above-left should read **"1", "2", "3", "Amp"** left-to-right with one tab highlighted as active — Amp is the rightmost tab. |
| `mseg-strip.png` | Tight close-up of just the strip beneath the MSEG canvas, so the reader can match each button to the manual's description. Six controls in order: Snap On / Snap Off button, Grid `T/V` dropdown, Style `{name}` dropdown, Unipolar / Bipolar button, Cyclic / One-shot button, Randomize button. |
| `mseg-bipolar.png` | An MSEG canvas drawn in **Bipolar** mode with a non-trivial curve. The midline marker at value 0.5 should be clearly visible — that's the no-modulation reference the manual describes for the Bipolar polarity. |
| `mseg-assign.png` | The MSEG with its Target dropdown open, showing the user about to assign an assignable MSEG to a specific effect param. Optional — can skip if mid-interaction state is awkward to capture. |

## Optional / nice-to-have

| Filename | What it shows |
|---|---|
| `mseg-transform-menu.png` | The Transform context menu open over a selected MSEG node, showing the four options Compress values / Expand values / Compress times / Expand times. Optional — the manual's bulleted explanation usually suffices, but a screenshot can speed recognition. |
| `solo-mute.png` | A track row with the M button lit (muted) and another row with S lit (soloed), demonstrating the chain-bypass behaviour described in the manual. Can be folded into `track-row.png` if convenient. |

## Notes for collection

- All screenshots should be from the same theme/skin (Multosis only has one).
- Use a consistent zoom level (the default 1456 × 758 window) so the
  dials and grid cells look the same size across images.
- PNG, no compression artifacts.
- Crop tightly — the manual layout uses `width=60%` or `width=80%` for
  most embeds, and white space around the UI elements wastes vertical
  space.
- For close-ups of strips and buttons (`toolbar.png`, `track-row.png`,
  `mseg-strip.png`), crop tight enough that the per-button labels are
  legible at the embed size.
