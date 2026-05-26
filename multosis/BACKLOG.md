# multosis — effect backlog

Effects not yet implemented, organised by where they would slot in the
existing `EffectKind` registry. The "Recommended build order" at the
bottom is my opinionated read on highest musical value per implementation
hour, but treat the whole document as a menu, not a queue.

## Glaring omissions (basic effects most multi-FX have)

- **Tremolo** — amplitude LFO. Ring covers ring mod, but no plain trem.
- **AutoPan** — stereo position LFO.
- **Vibrato** — pitch LFO. Chorus at Center=0 can fake it; a dedicated
  effect is cleaner.
- **EQ** — parametric multi-band EQ. SVF gets close but isn't multi-band
  with shelves.
- **Limiter** — in-chain peak limiter. (Distinct from the standalone
  `tinylimit` plugin — this would be a multosis effect-row slot.)
- **Gate** — hard expander for cleanup.
- **Transient shaper** — attack/sustain rebalance.

## Character variants of existing effects

### Distortion family

- **Tape saturation** — soft asymmetric, different from current Soft shape.
- **Tube saturation** — even-harmonic-heavy, asymmetric.
- **Wavefolder** — Buchla-style; the current Distortion has a simpler
  triangle Fold shape, not a true wavefolder.
- **Octaver / Sub** — bass octave-down (Octavia-style).

### Filter family

- **Sallen-Key / MS-20** — distinct topology from Moog / Diode / SVF.
- **Formant filter** — vowel-style cascaded BPs.
- **Allpass filter** — phase manipulation exposed as an effect.
- **Notch** — SVF only exposes LP / BP / HP currently.

### Time family

- **Plate reverb** — bright, dense, no early reflections (Dattorro lattice).
  Very different from the current Schroeder-Moorer Freeverb.
- **Spring reverb** — twangy / boingy classic guitar-amp reverb.
- **Tape echo** — saturating delay with wow/flutter + LP feedback.
- **Reverse delay** — plays back the input reversed.
- **Gated reverb** — 80s drum-room reverb + gate.

### Pitch family

- Mostly covered (FreqShift / PitchShift / Varispeed).

### Spectral family

- Many possibilities exist (spectral inversion, sines/noise/transient
  separation, true spectral vocoder, etc.) but the 14 spectral kinds
  already cover the most useful ground.

## New families to consider

### Spatial (does not exist as a family today)

- **Stereo widener** — M/S width control. The `imagine` plugin has
  this; multosis does not.
- **Haas delay** — sub-30ms stereo image shift.
- **AutoPan** (also listed under Modulation; could live in Spatial instead).
- **Mid/Side encode/decode utility** — niche but a clean primitive.

## Niche / specialised

- **De-esser** — frequency-selective compressor.
- **Expander** — opposite of compressor.

## Recommended build order

1. **Tremolo + AutoPan + Vibrato** — three thin Modulation-family
   effects sharing a sine/triangle/saw LFO core; could ship as a batch.
2. **Wavefolder** — distinct sonic territory from the existing distortion
   shapes; single new Distortion-family effect.
3. **Plate reverb** — meaningfully different from Freeverb; slots next to
   Reverb in Time.
4. **Sallen-Key filter** — completes the "famous analog filter
   topologies" set alongside Moog / Diode.

After these, the next tier is probably Limiter / Gate / Transient Shaper
(Dynamics family, doesn't exist yet) and Stereo Widener / Haas (Spatial
family, doesn't exist yet).
