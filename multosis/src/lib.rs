//! `multosis` — a multi-FX step sequencer.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md`.
//! Milestone 1a is the headless step-sequencer model: the grid, the playhead
//! sequencer, and the step clock. No GUI, no audio, no nih-plug.

pub mod clock;
pub mod compressor;
pub mod editor;
pub mod effects;
pub mod engine;
pub mod grid;
pub mod handoff;
pub mod modulation;
pub mod playhead_display;
pub mod propagation;
pub mod randomize;
pub mod region;
pub mod seq_status;
pub mod undo;

use crate::clock::Speed;
use crate::engine::AudioEngine;
use crate::grid::Grid;
use crate::handoff::GridHandoff;
use nih_plug::prelude::*;
use std::sync::atomic::AtomicU16;
use std::sync::{Arc, Mutex};

/// The Multosis plugin's parameters and persisted state.
#[derive(Params)]
pub struct MultosisParams {
    /// Persisted editor window size.
    #[persist = "editor-state"]
    pub editor_state: std::sync::Arc<tiny_skia_widgets::EditorState>,

    /// The routing grid — persisted plugin state, edited by the GUI (Milestone
    /// 1b-ii). `Arc<Mutex<Grid>>` is nih-plug's `PersistentField` shape.
    #[persist = "grid"]
    pub grid: Arc<Mutex<Grid>>,

    /// Per-track effect configuration — persisted plugin state.
    #[persist = "track-effects"]
    pub track_effects: Arc<Mutex<[crate::effects::TrackEffect; 16]>>,

    /// Per-track modulation configuration — persisted plugin state.
    #[persist = "track-modulation"]
    pub track_modulation: Arc<Mutex<[crate::modulation::TrackModulation; 16]>>,

    /// Tempo-synced playhead advance rate.
    #[id = "speed"]
    pub speed: EnumParam<Speed>,

    /// Dry↔wet blend.
    #[id = "mix"]
    pub mix: FloatParam,

    /// Post-mix output gain.
    #[id = "output_gain"]
    pub output_gain: FloatParam,

    /// Wet-bus compressor threshold (dBFS). The compressor tames the +N×dry
    /// peaks produced by many parallel-row effect outputs summing.
    #[id = "comp_threshold"]
    pub comp_threshold: FloatParam,

    /// Wet-bus compressor ratio (≥ 1.0; 1.0 = bypass).
    #[id = "comp_ratio"]
    pub comp_ratio: FloatParam,
}

impl Default for MultosisParams {
    fn default() -> Self {
        Self {
            editor_state: tiny_skia_widgets::EditorState::from_size(1456, 758),
            grid: Arc::new(Mutex::new(Grid::default())),
            track_effects: Arc::new(Mutex::new(std::array::from_fn(
                crate::effects::TrackEffect::default_for_row,
            ))),
            track_modulation: Arc::new(Mutex::new(std::array::from_fn(
                crate::modulation::TrackModulation::default_for_row,
            ))),
            speed: EnumParam::new("Speed", Speed::Hold),
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
            comp_threshold: FloatParam::new(
                "Comp Threshold",
                -6.0,
                FloatRange::Linear {
                    min: -24.0,
                    max: 0.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            comp_ratio: FloatParam::new(
                "Comp Ratio",
                4.0,
                FloatRange::Skewed {
                    min: 1.0,
                    max: 20.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_unit(":1")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
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
    /// Audio→GUI playhead-column mirror, shared with the editor.
    playhead_display: Arc<crate::playhead_display::PlayheadDisplay>,
    /// Audio→GUI sequence-status mirror, shared with the editor.
    seq_status: Arc<crate::seq_status::SeqStatusDisplay>,
    /// Set by the editor's Reset button; consumed once per process block.
    reset_request: Arc<std::sync::atomic::AtomicBool>,
    /// Audio→GUI mirror of the engine's active-row mask, shared with the editor.
    active_rows: Arc<AtomicU16>,
    /// Audio→GUI mirror of every MSEG's free-running phase, as f32 bit-patterns.
    /// Slot `row*4 + k` holds phase of MSEG `k` on row `r` (4 MSEGs per row:
    /// 1 amplitude + 3 assignable). 64 stores per process block, allocation-free.
    mseg_phases: Arc<[std::sync::atomic::AtomicU32; 64]>,
    /// Set by the editor on any effect/modulation edit; consumed by `process`
    /// to re-bridge the persisted config into the engine.
    config_dirty: Arc<std::sync::atomic::AtomicBool>,
    /// Posted by the editor when a drag-and-drop reorder completes; consumed
    /// once per process block to swap the audio engine's per-row DSP state
    /// (effect instances, MSEG phases, amplitudes) alongside the persisted
    /// config that the editor already swapped through the mutex-backed
    /// `track_effects` / `track_modulation` / `grid`.
    ///
    /// Encoding: `((from + 1) << 8) | (to + 1)`. The sentinel `0` means "no
    /// swap pending"; the `+1` offsets keep that sentinel disjoint from any
    /// legitimate (from, to) pair (rows are 0..16, so the encoded value is
    /// always non-zero when set).
    pending_track_swap: Arc<std::sync::atomic::AtomicU32>,
    /// The latency value most-recently reported to the host via
    /// `set_latency_samples`. Cached so `process` only calls
    /// `set_latency_samples` when the chain's latency actually changes
    /// (avoids hosts re-aligning every single block).
    last_reported_latency: u32,
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
            playhead_display: Arc::new(crate::playhead_display::PlayheadDisplay::new()),
            seq_status: Arc::new(crate::seq_status::SeqStatusDisplay::new()),
            reset_request: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            active_rows: Arc::new(AtomicU16::new(0)),
            mseg_phases: Arc::new(std::array::from_fn(|_| {
                std::sync::atomic::AtomicU32::new(0)
            })),
            config_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pending_track_swap: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            last_reported_latency: 0,
        }
    }
}

impl Plugin for Multosis {
    const NAME: &'static str = "Multosis";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: std::num::NonZeroU32::new(2),
        main_output_channels: std::num::NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            self.params.clone(),
            self.playhead_display.clone(),
            self.seq_status.clone(),
            self.grid_handoff.clone(),
            self.reset_request.clone(),
            self.active_rows.clone(),
            self.mseg_phases.clone(),
            self.config_dirty.clone(),
            self.pending_track_swap.clone(),
        )
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.engine.set_sample_rate(self.sample_rate);
        // Bridge the persisted grid (possibly just restored from project
        // state) into the audio thread's working copy and the handoff.
        if let Ok(grid) = self.params.grid.lock() {
            self.grid = *grid;
            self.grid_handoff.publish(*grid);
        }
        // Bridge the persisted per-track effect config into the engine.
        if let Ok(cfg) = self.params.track_effects.lock() {
            self.engine.set_effects(&cfg);
        }
        if let Ok(m) = self.params.track_modulation.lock() {
            self.engine.set_modulation(&m);
        }
        self.was_playing = false;
        // Report the initial chain latency to the host (sum of every
        // effect that adds latency in non-muted rows — today: only
        // Warp Zone's FFT_SIZE per instance). `process` re-reports on
        // change so PDC tracks edits made after this point.
        let latency = self.engine.chain_latency_samples() as u32;
        context.set_latency_samples(latency);
        self.last_reported_latency = latency;
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.was_playing = false;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let transport = context.transport();
        let playing = transport.playing;
        let bpm = transport.tempo.unwrap_or(120.0);

        // Reset the sequence on the transport stopped→playing edge.
        if playing && !self.was_playing {
            self.engine.reset();
        }
        self.was_playing = playing;

        // A Reset request from the editor resets the sequence.
        if self
            .reset_request
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            self.engine.reset();
        }

        // Pick up the latest grid (non-blocking; keep the last on a miss).
        if let Some(grid) = self.grid_handoff.try_read() {
            self.grid = grid;
        }

        // Re-bridge edited config into the engine. Clear the dirty flag only
        // after a successful re-bridge so no edit is lost on lock contention.
        if self.config_dirty.load(std::sync::atomic::Ordering::Relaxed) {
            if let (Ok(eff), Ok(modu)) = (
                self.params.track_effects.try_lock(),
                self.params.track_modulation.try_lock(),
            ) {
                self.engine.set_effects(&eff);
                self.engine.set_modulation(&modu);
                self.config_dirty
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }

        // Drain any pending track-swap from the editor. Swapped AFTER the
        // config re-bridge so the engine's effect instances and modulation
        // runtime line up with the just-installed config, then the in-place
        // swap shuffles both together — preserving DSP state across the move.
        let pending = self
            .pending_track_swap
            .swap(0, std::sync::atomic::Ordering::AcqRel);
        if pending != 0 {
            let from = ((pending >> 8) & 0xFF) as usize - 1;
            let to = (pending & 0xFF) as usize - 1;
            self.engine.swap_tracks(from, to);
        }

        // Re-report latency if the chain's total has changed (e.g. the user
        // added a Warp Zone row or muted/un-muted one). Hosts that support
        // dynamic latency re-align their PDC; the call is a no-op when the
        // value matches what's already reported. Compared as an integer so
        // we only re-publish on actual change, never every block.
        let latency = self.engine.chain_latency_samples() as u32;
        if latency != self.last_reported_latency {
            context.set_latency_samples(latency);
            self.last_reported_latency = latency;
        }

        let sps =
            crate::clock::samples_per_step(self.params.speed.value(), bpm, self.sample_rate as f64);
        let mix = self.params.mix.value();
        // Bridge the wet-bus compressor's user-facing params each block.
        self.engine.set_compressor(
            self.params.comp_threshold.value(),
            self.params.comp_ratio.value(),
        );

        let n = buffer.samples();
        let channels = buffer.as_slice();
        let (first, rest) = channels.split_at_mut(1);
        let left = &mut first[0][..n];
        let right = &mut rest[0][..n];

        self.engine
            .process(&mut *left, &mut *right, playing, sps, bpm, mix, &self.grid);

        // Publish the playhead column for the editor to draw.
        self.playhead_display.publish(self.engine.playhead_column());
        self.seq_status.publish(self.engine.step());
        self.active_rows.store(
            self.engine.active_mask(),
            std::sync::atomic::Ordering::Relaxed,
        );
        // Publish the 16x4 MSEG phases for the editor's playhead overlay.
        for row in 0..16 {
            for k in 0..4 {
                let phase = self.engine.modulation_phase(row, k);
                self.mseg_phases[row * 4 + k]
                    .store(phase.to_bits(), std::sync::atomic::Ordering::Relaxed);
            }
        }

        // Post-mix output gain (smoothed per sample).
        for i in 0..n {
            let gain = self.params.output_gain.smoothed.next();
            left[i] *= gain;
            right[i] *= gain;
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for Multosis {
    const CLAP_ID: &'static str = "com.mpd.multosis";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A multi-FX step sequencer");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for Multosis {
    const VST3_CLASS_ID: [u8; 16] = *b"MultosisMpdPlg\0\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx];
}

nih_export_clap!(Multosis);
nih_export_vst3!(Multosis);
