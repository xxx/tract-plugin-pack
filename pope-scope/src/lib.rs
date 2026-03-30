use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Debug-only logging. Compiles to nothing in release builds, avoiding
/// format!() heap allocations and stderr writes on the audio thread.
macro_rules! debug_log {
    ($($arg:tt)*) => {
        #[cfg(debug_assertions)]
        nih_log!($($arg)*);
    };
}

mod controls;
pub mod editor;
mod renderer;
pub mod ring_buffer;
pub mod snapshot;
pub mod store;
mod theme;
pub mod time_mapping;

// ── Enums ─────────────────────────────────────────────────────────────────────

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum DisplayMode {
    #[id = "vertical"]
    #[name = "Vertical"]
    Vertical,
    #[id = "overlay"]
    #[name = "Overlay"]
    Overlay,
    #[id = "sum"]
    #[name = "Sum"]
    Sum,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum DrawStyle {
    #[id = "line"]
    #[name = "Line"]
    Line,
    #[id = "filled"]
    #[name = "Filled"]
    Filled,
    #[id = "both"]
    #[name = "Both"]
    Both,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum SyncMode {
    #[id = "free"]
    #[name = "Free"]
    Free,
    #[id = "beat_sync"]
    #[name = "Beat Sync"]
    BeatSync,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum SyncUnit {
    #[id = "quarter"]
    #[name = "1/4 bar"]
    Quarter,
    #[id = "half"]
    #[name = "1/2 bar"]
    Half,
    #[id = "one"]
    #[name = "1 bar"]
    One,
    #[id = "two"]
    #[name = "2 bars"]
    Two,
    #[id = "four"]
    #[name = "4 bars"]
    Four,
}

impl SyncUnit {
    pub fn to_bars(self) -> f64 {
        match self {
            SyncUnit::Quarter => 0.25,
            SyncUnit::Half => 0.5,
            SyncUnit::One => 1.0,
            SyncUnit::Two => 2.0,
            SyncUnit::Four => 4.0,
        }
    }
}

// ── Plugin struct ──────────────────────────────────────────────────────────────

/// Monotonic counter for generating unique instance hashes.
/// Starts at 1 so that hash 0 is never issued (0 = free slot sentinel).
static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct PopeScope {
    params: Arc<PopeScopeParams>,
    /// Index into the global store (0-15), or None if no slot acquired.
    slot_index: Option<usize>,
    /// Unique hash for slot ownership.
    instance_hash: u64,
    /// Current sample rate.
    sample_rate: f32,
    /// Shared sample rate atomic (written by audio thread, read by editor).
    shared_sample_rate: Arc<AtomicU32>,
}

// ── Params ────────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct PopeScopeParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<editor::EditorState>,

    /// Timebase in milliseconds (Free mode only).
    #[id = "timebase"]
    pub timebase: FloatParam,

    /// Bottom of visible dB range.
    #[id = "min_db"]
    pub min_db: FloatParam,

    /// Top of visible dB range.
    #[id = "max_db"]
    pub max_db: FloatParam,

    /// Freeze display updates.
    #[id = "freeze"]
    pub freeze: BoolParam,

    /// Display mode: Vertical, Overlay, Sum.
    #[id = "display_mode"]
    pub display_mode: EnumParam<DisplayMode>,

    /// Draw style: Line, Filled, Both.
    #[id = "draw_style"]
    pub draw_style: EnumParam<DrawStyle>,

    /// Combine channels to mono for display.
    #[id = "mix_to_mono"]
    pub mix_to_mono: BoolParam,

    /// Max output data points for rendering.
    #[id = "decimation"]
    pub decimation: IntParam,

    /// Track group filter (0-15).
    #[id = "group"]
    pub group: IntParam,

    /// Sync mode: Free, BeatSync.
    #[id = "sync_mode"]
    pub sync_mode: EnumParam<SyncMode>,

    /// Sync unit (BeatSync mode only).
    #[id = "sync_unit"]
    pub sync_unit: EnumParam<SyncUnit>,

    /// Hold mode: show last complete bar instead of sweep (BeatSync mode only).
    #[id = "hold_mode"]
    pub hold_mode: BoolParam,
}

impl PopeScopeParams {
    fn new() -> Self {
        Self {
            editor_state: editor::default_editor_state(),

            timebase: FloatParam::new(
                "Timebase",
                2000.0,
                FloatRange::Skewed {
                    min: 1.0,
                    max: 10000.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_unit(" ms")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            min_db: FloatParam::new(
                "Min dB",
                -48.0,
                FloatRange::Linear {
                    min: -96.0,
                    max: -6.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            max_db: FloatParam::new(
                "Max dB",
                0.0,
                FloatRange::Linear {
                    min: -48.0,
                    max: 12.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            freeze: BoolParam::new("Freeze", false),

            display_mode: EnumParam::new("Display", DisplayMode::Vertical),

            draw_style: EnumParam::new("Style", DrawStyle::Both),

            mix_to_mono: BoolParam::new("Mono", true),

            decimation: IntParam::new("Decimation", 2048, IntRange::Linear { min: 128, max: 4096 }),

            group: IntParam::new("Group", 0, IntRange::Linear { min: 0, max: 15 }),

            sync_mode: EnumParam::new("Sync", SyncMode::BeatSync),

            sync_unit: EnumParam::new("Unit", SyncUnit::One),

            hold_mode: BoolParam::new("Hold", false),
        }
    }
}

impl Default for PopeScope {
    fn default() -> Self {
        let hash = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);

        debug_log!("pope-scope: Default::default() instance_hash={hash}");

        Self {
            params: Arc::new(PopeScopeParams::new()),
            slot_index: None,
            instance_hash: hash,
            sample_rate: 48000.0,
            shared_sample_rate: Arc::new(AtomicU32::new(48000)),
        }
    }
}

impl Plugin for PopeScope {
    const NAME: &'static str = "Pope Scope";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), Arc::clone(&self.shared_sample_rate))
    }

    fn initialize(
        &mut self,
        audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.shared_sample_rate
            .store(buffer_config.sample_rate as u32, Ordering::Relaxed);

        let num_channels = audio_io_layout
            .main_input_channels
            .map(|c| c.get() as usize)
            .unwrap_or(2);

        // Release any previously held slot (nih-plug may call initialize()
        // again without a preceding deactivate(), e.g. on sample rate change).
        if let Some(old_idx) = self.slot_index.take() {
            debug_log!(
                "pope-scope: initialize() releasing old slot {old_idx} (hash={})",
                self.instance_hash
            );
            store::release_slot(old_idx, self.instance_hash);
        }

        // Acquire a slot in the global store
        match store::acquire_slot(self.instance_hash) {
            Some(idx) => {
                store::init_buffers(idx, num_channels, self.sample_rate);
                // Set initial metadata
                let slot = store::slot(idx);
                slot.metadata
                    .display_color
                    .store(theme::channel_color(idx), Ordering::Relaxed);
                slot.metadata
                    .group
                    .store(self.params.group.value() as u32, Ordering::Relaxed);
                self.slot_index = Some(idx);
                debug_log!(
                    "pope-scope: initialize() acquired slot {idx} (hash={}, channels={num_channels}, sr={})",
                    self.instance_hash, self.sample_rate
                );
                true
            }
            None => {
                // All 16 slots are full
                debug_log!(
                    "pope-scope: initialize() FAILED to acquire slot (hash={}, all 16 full)",
                    self.instance_hash
                );
                self.slot_index = None;
                true // Still pass audio through
            }
        }
    }

    fn update_track_info(&mut self, info: TrackInfo) {
        if let Some(idx) = self.slot_index {
            let slot = store::slot(idx);
            if let Some(name) = &info.name {
                if let Ok(mut guard) = slot.metadata.track_name.lock() {
                    *guard = name.clone();
                }
            }
            if let Some((r, g, b, _a)) = info.color {
                let argb = 0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                slot.metadata.display_color.store(argb, Ordering::Relaxed);
            }
        }
    }

    fn deactivate(&mut self) {
        if let Some(idx) = self.slot_index.take() {
            debug_log!(
                "pope-scope: deactivate() releasing slot {idx} (hash={})",
                self.instance_hash
            );
            store::release_slot(idx, self.instance_hash);
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let slot_idx = match self.slot_index {
            Some(idx) => idx,
            None => return ProcessStatus::Normal, // No slot, just pass through
        };

        let slot = store::slot(slot_idx);

        // Extract transport info
        let transport = context.transport();
        let is_playing = transport.playing;
        let bpm = transport.tempo.unwrap_or(120.0);
        let time_sig_num = transport.time_sig_numerator.unwrap_or(4) as u32;
        let time_sig_den = transport.time_sig_denominator.unwrap_or(4) as u32;
        let ppq = transport.pos_beats().unwrap_or(0.0);
        let bar_start_ppq = transport.bar_start_pos_beats().unwrap_or(0.0);
        let sample_pos = transport.pos_samples().unwrap_or(0);

        // Update playhead atomics
        slot.playhead.is_playing.store(is_playing, Ordering::Relaxed);
        slot.playhead.bpm.store(bpm.to_bits(), Ordering::Relaxed);
        slot.playhead
            .time_sig_num
            .store(time_sig_num, Ordering::Relaxed);
        slot.playhead
            .time_sig_den
            .store(time_sig_den, Ordering::Relaxed);
        slot.playhead
            .ppq_position
            .store(ppq.to_bits(), Ordering::Relaxed);
        slot.playhead
            .bar_start_ppq
            .store(bar_start_ppq.to_bits(), Ordering::Relaxed);

        // Get ring buffer write position (before pushing audio) for
        // DAW-transport-to-ring-buffer coordinate mapping in beat sync.
        // Use try_read() to avoid blocking the audio thread — fall back to 0
        // if the GUI holds the lock (slight time mapping staleness is acceptable).
        let ring_buf_pos = slot
            .buffers
            .try_read()
            .ok()
            .and_then(|g| g.as_ref().and_then(|bufs| bufs.first().map(|b| b.total_written())))
            .unwrap_or(0) as u64;

        // Update time mapping (before pushing audio)
        slot.time_mapping.update(
            ppq,
            sample_pos,
            ring_buf_pos,
            bpm,
            self.sample_rate as f64,
            buffer.samples(),
            is_playing,
        );

        // Update group from param
        slot.metadata
            .group
            .store(self.params.group.value() as u32, Ordering::Relaxed);

        // Push audio to ring buffers (try_lock to avoid blocking if GUI is reading)
        if let Ok(mut guard) = slot.buffers.try_write() {
            if let Some(bufs) = guard.as_mut() {
                let num_channels = buffer.channels();
                for (ch, channel_samples) in buffer.as_slice().iter().enumerate() {
                    if ch < bufs.len() && ch < num_channels {
                        bufs[ch].push(channel_samples);
                    }
                }
            }
        }

        // Update heartbeat (monotonic frame counter — GUI detects staleness
        // by checking whether the value changed between polls).
        slot.heartbeat.fetch_add(1, Ordering::Relaxed);

        // Audio is pass-through (input = output), nih-plug does this by default
        // since we read from buffer and don't modify it

        ProcessStatus::Normal
    }
}

impl ClapPlugin for PopeScope {
    const CLAP_ID: &'static str = "com.mpd.pope-scope";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("A multichannel real-time oscilloscope");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::Analyzer];
}

impl Vst3Plugin for PopeScope {
    const VST3_CLASS_ID: [u8; 16] = *b"PopeScopeMpdPlg\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Analyzer];
}

nih_export_clap!(PopeScope);
nih_export_vst3!(PopeScope);
