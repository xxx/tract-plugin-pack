# Gated third-party notices

## Initial pattern bank

The eight starting rhythms in `src/pattern.rs` are adapted from the initial
Trance Gate step-count, enabled-step, and tied-step data in Sheldon Young's
independently developed **synthahol-phase-plant** project:

https://github.com/softdevca/synthahol-phase-plant/blob/12226ba38afdf887214ffad9a14c191ef3e3d0ca/src/effect/trance_gate.rs

The source project is distributed under the Apache License, Version 2.0; a copy
is included in `licenses/Apache-2.0.txt`. Gated converts those Boolean tables
into its own bitset representation. No reader implementation is incorporated.

The source's `init` test compares these patterns against saved Trance Gate
states from Phase Plant 1.8.14 and 2.0.16. This establishes the provenance of
this bank; it is not a claim that every subsequent Kilohearts release has the
same defaults. The named factory preset files (such as “Trance Gate 1”) can
contain different banks and are distinct from the initial pattern bank.

Trance Gate and Phase Plant are Kilohearts products. Gated is independent and
is not affiliated with or endorsed by Kilohearts.
