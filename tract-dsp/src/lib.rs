//! `tract-dsp` — shared, GUI-free DSP primitives for the tract-plugin-pack
//! workspace.
//!
//! This crate contains pure signal-processing code only. It has no
//! `nih-plug`, `tiny-skia`, `softbuffer`, or editor dependency, and no
//! external crates at all — just `std` and `std::simd`.
//!
//! Requires nightly Rust for `std::simd`; the workspace already pins nightly
//! via `rust-toolchain.toml`.
#![feature(portable_simd)]

pub mod boxcar;
pub mod db;
pub mod fir;
pub mod spsc;
pub mod true_peak;
pub mod window;
