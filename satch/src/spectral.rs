//! Detail-preserving spectral clipper has been promoted to
//! `tract_dsp::spectral_clipper` so it can be reused by multosis's Satch
//! effect. The module re-export keeps existing call sites + tests
//! (`satch::spectral::SpectralClipper`, `satch::spectral::saturate_td`,
//! etc.) building without changes.

pub use tract_dsp::spectral_clipper::{
    saturate_td, saturate_td_with_tanh, saturate_td_with_tanh_fast, SpectralClipper,
};
