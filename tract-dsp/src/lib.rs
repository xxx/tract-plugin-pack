//! `tract-dsp` — shared, GUI-free DSP primitives for the tract-plugin-pack
//! workspace.
//!
//! This crate contains pure signal-processing code only. It has no
//! `nih-plug`, `tiny-skia`, `softbuffer`, or editor dependency. By default it
//! pulls no external crates at all — just `std` and `std::simd`. The optional
//! `stft` feature adds `realfft`/`rustfft` and enables the `stft` module; the
//! optional `stft-analysis` feature adds `rustfft` and enables the
//! `stft_analysis` module.
//!
//! Requires nightly Rust for `std::simd`; the workspace already pins nightly
//! via `rust-toolchain.toml`.
#![feature(portable_simd)]

pub mod boxcar;
pub mod db;
pub mod fir;
pub mod spsc;
#[cfg(feature = "stft")]
pub mod stft;
#[cfg(feature = "stft-analysis")]
pub mod stft_analysis;
pub mod true_peak;
pub mod window;
