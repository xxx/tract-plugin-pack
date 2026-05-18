//! `multosis` — a multi-FX routing sequencer.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md`.
//! Milestone 1a is the headless routing model: the grid, the wavefront
//! propagation engine, and the step clock. No GUI, no audio, no nih-plug.

pub mod clock;
pub mod effects;
pub mod engine;
pub mod grid;
pub mod handoff;
pub mod propagation;
pub mod randomize;
pub mod region;

use crate::clock::Speed;
use crate::effects::EffectBank;
use crate::engine::AudioEngine;
use crate::grid::Grid;
use crate::handoff::GridHandoff;
use nih_plug::prelude::*;
use std::sync::{Arc, Mutex};

/// The Multosis plugin's parameters and persisted state.
#[derive(Params)]
pub struct MultosisParams {
    /// The routing grid — persisted plugin state, edited by the GUI (Milestone
    /// 1b-ii). `Arc<Mutex<Grid>>` is nih-plug's `PersistentField` shape.
    #[persist = "grid"]
    pub grid: Arc<Mutex<Grid>>,

    /// Tempo-synced wavefront advance rate.
    #[id = "speed"]
    pub speed: EnumParam<Speed>,

    /// Dry↔wet blend.
    #[id = "mix"]
    pub mix: FloatParam,

    /// Post-mix output gain.
    #[id = "output_gain"]
    pub output_gain: FloatParam,

    /// Which throwaway effect every row uses.
    #[id = "effect_bank"]
    pub effect_bank: EnumParam<EffectBank>,

    /// When on, a dead-ended wavefront re-arms the start cells.
    #[id = "auto_restart"]
    pub auto_restart: BoolParam,
}

impl Default for MultosisParams {
    fn default() -> Self {
        Self {
            grid: Arc::new(Mutex::new(Grid::default())),
            speed: EnumParam::new("Speed", Speed::Div16),
            mix: FloatParam::new("Mix", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            output_gain: FloatParam::new(
                "Output",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-30.0),
                    max: util::db_to_gain(12.0),
                    factor: FloatRange::gain_skew_factor(-30.0, 12.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            effect_bank: EnumParam::new("Effect", EffectBank::Lowpass),
            auto_restart: BoolParam::new("Auto Restart", true),
        }
    }
}

/// The Multosis plugin.
pub struct Multosis {
    params: Arc<MultosisParams>,
    /// GUI→audio handoff of the grid (used by the Milestone 1b-ii editor).
    grid_handoff: Arc<GridHandoff>,
    /// The audio thread's working copy of the grid.
    grid: Grid,
    engine: AudioEngine,
    sample_rate: f32,
    /// Previous block's transport state, for stopped→playing edge detection.
    was_playing: bool,
}

impl Default for Multosis {
    fn default() -> Self {
        Self {
            params: Arc::new(MultosisParams::default()),
            grid_handoff: Arc::new(GridHandoff::new(Grid::default())),
            grid: Grid::default(),
            engine: AudioEngine::new(),
            sample_rate: 44_100.0,
            was_playing: false,
        }
    }
}
