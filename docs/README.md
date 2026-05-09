# Plugin Manuals

Each plugin keeps its user manual under `docs/<plugin>/`:

| Plugin | Markdown | PDF |
|---|---|---|
| Gain Brain | [gain-brain-manual.md](gain-brain/gain-brain-manual.md) | [gain-brain-manual.pdf](gain-brain/gain-brain-manual.pdf) |
| GS Meter | [gs-meter-manual.md](gs-meter/gs-meter-manual.md) | [gs-meter-manual.pdf](gs-meter/gs-meter-manual.pdf) |
| Imagine | [imagine/imagine-manual.md](imagine/imagine-manual.md) | [imagine/imagine-manual.pdf](imagine/imagine-manual.pdf) |
| Pope Scope | [pope-scope-manual.md](pope-scope/pope-scope-manual.md) | [pope-scope-manual.pdf](pope-scope/pope-scope-manual.pdf) |
| Satch | [satch/satch-manual.md](satch/satch-manual.md) | [satch/satch-manual.pdf](satch/satch-manual.pdf) |
| Six Pack | [six-pack/six-pack-manual.md](six-pack/six-pack-manual.md) | [six-pack/six-pack-manual.pdf](six-pack/six-pack-manual.pdf) |
| Tinylimit | [tinylimit/tinylimit-manual.md](tinylimit/tinylimit-manual.md) | [tinylimit/tinylimit-manual.pdf](tinylimit/tinylimit-manual.pdf) |
| Warp Zone | [warp-zone/warp-zone-manual.md](warp-zone/warp-zone-manual.md) | [warp-zone/warp-zone-manual.pdf](warp-zone/warp-zone-manual.pdf) |
| Wavetable Filter | [wavetable-filter/wavetable-filter-manual.md](wavetable-filter/wavetable-filter-manual.md) | [wavetable-filter/wavetable-filter-manual.pdf](wavetable-filter/wavetable-filter-manual.pdf) |

## Regenerating a PDF

The markdown manuals use Unicode glyphs for math (−, ±, ×, ·, →, Σ, etc.) so the typography reads correctly. The default `pdflatex` engine pandoc reaches for cannot render arbitrary Unicode, so build the PDFs with **xelatex** plus a font that ships with broad Unicode coverage. **DejaVu Serif** + **DejaVu Sans Mono** is the project's chosen pair — installed by default on most Linux distros and on Homebrew (`brew install --cask font-dejavu`).

From a manual's directory (so the `screenshot.png` reference resolves):

```bash
cd docs/<plugin>/
pandoc --pdf-engine=xelatex \
       -V mainfont="DejaVu Serif" \
       -V monofont="DejaVu Sans Mono" \
       <plugin>-manual.md \
       -o <plugin>-manual.pdf
```

A successful run produces no warnings. If you see `Missing character: There is no <glyph>`, either:

1. The character is genuinely outside DejaVu Serif's coverage — pick a different font (e.g., `Noto Serif`, `Liberation Serif`) or use an ASCII fallback in the markdown.
2. Or pandoc is using a different (non-Unicode) font for that span — check the YAML front-matter in the manual.

## Style conventions for new manuals

- YAML front-matter at the top sets `title`, `subtitle: "User Manual"`, `author: "Michael Dungan"`, `geometry: margin=2.5cm`, `colorlinks: true`.
- Open with a screenshot at `width=80%` (matching `screenshot.png` in the same folder).
- Sections in roughly this order: *What is X?* / *Installation* / *Quick Start* / *Controls* (per-control reference) / *How It Works* (DSP details for the curious) / *Interaction* (mouse/keyboard gestures) / *Technical Notes* / *Formats* / *License*.
- Use Unicode minus (`−`, U+2212) for math expressions in prose. ASCII hyphen-minus (`-`) is fine inside backticked code spans and pseudocode blocks, since that's what the corresponding source code uses.
- Use `→` for prose arrows ("input → output → wet → mix"), `->` is fine inside code blocks.
- Em dashes (`—`, U+2014) for parenthetical asides; en dashes (`–`, U+2013) for ranges (e.g., `20 Hz – 20 kHz`).
