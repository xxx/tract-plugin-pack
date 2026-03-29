use nih_plug::prelude::*;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

mod editor;
pub mod groups;

// ── Background task for host-param sync ──────────────────────────────────────

/// Tasks dispatched from the audio thread to the main/GUI thread via
/// `ProcessContext::execute_gui()`.
#[derive(Clone, Copy)]
pub enum GainBrainTask {
    /// Sync the host-visible gain parameter to the given normalized value.
    /// Fired when group sync changes `effective_gain_db` without updating
    /// `params.gain`. The task executor calls `ParamSetter` on the main
    /// thread where it is safe for both CLAP and VST3.
    SyncGainParam { normalized: f32 },
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// Convert a dB value to millibels (1/100 dB).
fn db_to_millibels(db: f32) -> i32 {
    (db * 100.0).round() as i32
}

/// Convert millibels (1/100 dB) back to dB.
fn millibels_to_db(mb: i32) -> f32 {
    mb as f32 / 100.0
}

const GAIN_MIN_DB: f32 = -60.0;
const GAIN_MAX_DB: f32 = 60.0;

/// Clamp a dB value to the valid gain range.
fn clamp_db(db: f32) -> f32 {
    db.clamp(GAIN_MIN_DB, GAIN_MAX_DB)
}

// ── LinkMode ───────────────────────────────────────────────────────────────────

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum LinkMode {
    #[id = "absolute"]
    #[name = "Abs"]
    Absolute,
    #[id = "relative"]
    #[name = "Rel"]
    Relative,
}

/// Sentinel value for "no override pending" in group_gain_override.
const NO_OVERRIDE: i32 = i32::MIN;

// ── Plugin struct ──────────────────────────────────────────────────────────────

/// Global counter for assigning unique instance IDs (for debug logging).
static INSTANCE_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub struct GainBrain {
    /// Unique ID for this instance, used in debug log lines.
    instance_id: u32,
    params: Arc<GainBrainParams>,
    /// Last generation we observed from the group slot (to detect external writes).
    last_seen_generation: u32,
    /// Last baseline_generation we observed (to detect rebaseline events).
    last_baseline_generation: u32,
    /// Last gain value (millibels) we wrote/received to/from the group slot.
    last_sent_gain_millibels: i32,
    /// The param value (millibels) from the previous buffer. Used to detect
    /// user-initiated gain changes (as opposed to stale param values after
    /// a group override that can't update the param directly).
    last_param_value_mb: i32,
    /// The group param value from the previous buffer (to detect group changes).
    last_group: i32,
    /// The link_mode from the previous buffer (to detect mode changes).
    last_link_mode: LinkMode,
    /// The invert state from the previous buffer (to detect toggles).
    last_invert: bool,
    /// Sample rate, stored from initialize().
    sample_rate: f32,
    /// Sidecar for group-driven gain overrides. The process loop checks this
    /// each buffer. When a group update arrives we write the target gain in
    /// millibels here and retarget the smoother. NO_OVERRIDE means nothing pending.
    pub group_gain_override: Arc<AtomicI32>,
    /// Baseline gain (millibels) for relative-mode delta calculations.
    /// Set when joining a group in Relative mode.
    relative_baseline_mb: i32,
    /// The effective gain in dB that we are currently applying.
    effective_gain_db: f32,
    /// Shared with the editor so it can display the effective gain (which may
    /// differ from the parameter value when group sync overrides are active).
    pub display_gain_millibels: Arc<AtomicI32>,
    /// Counter for throttling per-buffer debug logs. Only logs the first
    /// N calls to sync_group after startup to avoid flooding.
    sync_call_count: u32,
    /// Shared holder for the GuiContext, populated when the editor is first
    /// spawned. Used by `task_executor` (on the main thread) to update the
    /// host-visible gain parameter when group sync changes the effective gain.
    /// Stays populated even after the editor window is closed since the
    /// GuiContext Arc remains valid for the plugin's lifetime.
    pub gui_context: Arc<std::sync::Mutex<Option<Arc<dyn GuiContext>>>>,
    /// Whether a `SyncGainParam` task is already in flight. Prevents flooding
    /// the task queue with duplicate sync requests every buffer.
    param_sync_pending: bool,
}

// ── Params ─────────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct GainBrainParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<editor::EditorState>,

    /// Persisted effective gain in millibels. Written by the audio thread
    /// whenever the effective gain changes (from group sync or user input).
    /// This ensures the host saves and restores the correct gain even when
    /// group sync has overridden the smoother without updating the param.
    #[persist = "effective-gain-mb"]
    pub effective_gain_mb: Arc<AtomicI32>,

    #[id = "gain"]
    pub gain: FloatParam,

    #[id = "group"]
    pub group: IntParam,

    #[id = "link_mode"]
    pub link_mode: EnumParam<LinkMode>,

    #[id = "invert"]
    pub invert: BoolParam,
}

impl Default for GainBrain {
    fn default() -> Self {
        let instance_id = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        nih_log!("gain-brain[{instance_id}]: Default::default()");
        Self {
            instance_id,
            params: Arc::new(GainBrainParams::new()),
            last_seen_generation: 0,
            last_baseline_generation: 0,
            last_sent_gain_millibels: 0,
            last_param_value_mb: 0,
            last_group: 0,
            last_link_mode: LinkMode::Relative,
            last_invert: false,
            sample_rate: 44100.0,
            group_gain_override: Arc::new(AtomicI32::new(NO_OVERRIDE)),
            relative_baseline_mb: 0,
            effective_gain_db: 0.0,
            display_gain_millibels: Arc::new(AtomicI32::new(0)),
            sync_call_count: 0,
            gui_context: Arc::new(std::sync::Mutex::new(None)),
            param_sync_pending: false,
        }
    }
}

impl GainBrainParams {
    fn new() -> Self {
        Self {
            editor_state: editor::default_editor_state(),
            effective_gain_mb: Arc::new(AtomicI32::new(0)),

            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(GAIN_MIN_DB),
                    max: util::db_to_gain(GAIN_MAX_DB),
                    factor: FloatRange::gain_skew_factor(GAIN_MIN_DB, GAIN_MAX_DB),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            group: IntParam::new("Group", 0, IntRange::Linear { min: 0, max: 16 }),

            link_mode: EnumParam::new("Link Mode", LinkMode::Relative),

            invert: BoolParam::new("Invert", false),
        }
    }
}

// ── Plugin impl ────────────────────────────────────────────────────────────────

impl Plugin for GainBrain {
    const NAME: &'static str = "Gain Brain";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;
    type SysExMessage = ();
    type BackgroundTask = GainBrainTask;

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn task_executor(&mut self) -> TaskExecutor<Self> {
        let gui_context = self.gui_context.clone();
        let params = self.params.clone();
        Box::new(move |task| match task {
            GainBrainTask::SyncGainParam { normalized } => {
                // This closure runs on the main/GUI thread (via execute_gui),
                // where ParamSetter operations are safe for both CLAP and VST3.
                if let Ok(guard) = gui_context.lock() {
                    if let Some(ctx) = guard.as_ref() {
                        let setter = ParamSetter::new(ctx.as_ref());
                        setter.begin_set_parameter(&params.gain);
                        setter.set_parameter_normalized(&params.gain, normalized);
                        setter.end_set_parameter(&params.gain);
                    }
                }
            }
        })
    }

    fn editor(&mut self, async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        // Capture the GuiContext immediately from AsyncExecutor so the
        // task executor can update host-visible params without the GUI
        // being open. This requires our nih-plug fork which exposes
        // gui_context on AsyncExecutor.
        if let Ok(mut guard) = self.gui_context.lock() {
            *guard = Some(async_executor.gui_context.clone());
        }
        editor::create(
            self.params.clone(),
            self.display_gain_millibels.clone(),
            self.group_gain_override.clone(),
            self.gui_context.clone(),
        )
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        let gain_db = util::gain_to_db(self.params.gain.value());
        let group = self.params.group.value();
        let link_mode = self.params.link_mode.value();
        let invert = self.params.invert.value();

        // Restore effective gain from the persisted field. On a fresh instance
        // this will be 0 (matching the default). On a restored session, this
        // is the last effective gain saved by process(), which may differ from
        // params.gain.value() if group sync had overridden the smoother.
        let persisted_mb = self.params.effective_gain_mb.load(Ordering::Relaxed);
        let persisted_db = millibels_to_db(persisted_mb);
        if persisted_mb != 0 || gain_db.abs() < 0.01 {
            // Only restore from persisted field if it's non-zero (real data)
            // or the param is also at zero (consistent).
            self.effective_gain_db = persisted_db;
        } else {
            // Persisted field is 0 but param is non-zero: this is likely a
            // legacy state without the persisted field. Use the param value.
            self.effective_gain_db = gain_db;
        }

        // Retarget the smoother to the restored effective gain so the first
        // process() buffer uses the correct gain.
        let target_gain = util::db_to_gain(self.effective_gain_db);
        self.params.gain.smoothed.reset(target_gain);

        // Sync tracking fields to the current param state so the write path
        // in sync_group() doesn't misfire on the first process() call.
        // Without this, last_param_value_mb=0 (from Default) would differ
        // from the host-restored param value, causing the write path to
        // overwrite the group slot with a stale value.
        self.last_param_value_mb = db_to_millibels(gain_db);
        self.last_group = group;
        self.last_link_mode = link_mode;
        self.last_invert = invert;
        self.param_sync_pending = false;

        // Sync display gain for the editor.
        self.display_gain_millibels.store(
            db_to_millibels(self.effective_gain_db),
            Ordering::Relaxed,
        );

        nih_log!(
            "gain-brain[{}]: initialize() gain={:.2}dB group={} mode={:?} invert={} \
             last_group={} last_param_mb={} last_sent_mb={} effective={:.2}dB persisted={}mb",
            self.instance_id, gain_db, group, link_mode, invert,
            self.last_group, self.last_param_value_mb,
            self.last_sent_gain_millibels, self.effective_gain_db, persisted_mb
        );
        true
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let num_channels = buffer.channels();
        if num_channels < 2 {
            return ProcessStatus::Normal;
        }

        // ── Group sync (once per buffer, before applying gain) ─────────
        self.sync_group();

        // ── Check for pending group gain override ──────────────────────
        let override_mb = self
            .group_gain_override
            .swap(NO_OVERRIDE, Ordering::Relaxed);
        if override_mb != NO_OVERRIDE {
            let target_db = clamp_db(millibels_to_db(override_mb));
            let target_gain = util::db_to_gain(target_db);
            nih_log!(
                "gain-brain[{}]: APPLY OVERRIDE {}mb -> {:.2}dB (param was {:.2}dB)",
                self.instance_id, override_mb, target_db,
                util::gain_to_db(self.params.gain.value())
            );
            self.params
                .gain
                .smoothed
                .set_target(self.sample_rate, target_gain);
            self.effective_gain_db = target_db;
        } else if !(1..=16).contains(&self.params.group.value()) {
            // No group active — effective gain simply tracks the parameter
            self.effective_gain_db = util::gain_to_db(self.params.gain.value());
        }
        // else: in a group with no new override — keep effective_gain_db as-is.
        // It was set by sync_group (from param change or prior override).

        // ── Apply gain ─────────────────────────────────────────────────
        let num_samples = buffer.samples();
        let channel_slices = buffer.as_slice();

        #[allow(clippy::needless_range_loop)]
        for i in 0..num_samples {
            let gain = self.params.gain.smoothed.next();
            channel_slices[0][i] *= gain;
            channel_slices[1][i] *= gain;
        }

        // Always write effective gain to display atomic for the editor
        let effective_mb = db_to_millibels(self.effective_gain_db);
        self.display_gain_millibels
            .store(effective_mb, Ordering::Relaxed);

        // Persist the effective gain so the host saves the correct value,
        // even when group sync has overridden the smoother without updating
        // the gain parameter's internal value.
        self.params.effective_gain_mb.store(effective_mb, Ordering::Relaxed);

        // ── Sync host-visible param via main-thread task ──────────────
        // When group sync changes effective_gain_db, the host's parameter
        // value stays stale (nih-plug has no audio-thread param setter).
        // We dispatch a task to the main thread which uses ParamSetter via
        // the stored GuiContext. This works regardless of whether the GUI
        // is open — the GuiContext remains valid once created.
        let param_db = util::gain_to_db(self.params.gain.value());
        if (self.effective_gain_db - param_db).abs() > 0.05 && !self.param_sync_pending {
            let target_linear = util::db_to_gain(self.effective_gain_db);
            let normalized = self.params.gain.preview_normalized(target_linear);
            context.execute_gui(GainBrainTask::SyncGainParam { normalized });
            self.param_sync_pending = true;
        } else if (self.effective_gain_db - param_db).abs() <= 0.05 {
            // The param has caught up (task was processed), clear the flag.
            self.param_sync_pending = false;
        }

        ProcessStatus::Normal
    }
}

// ── Group sync logic ───────────────────────────────────────────────────────────

/// State fields needed by transition/sync logic, borrowed separately to
/// satisfy the borrow checker.
struct SyncState<'a> {
    instance_id: u32,
    params: &'a GainBrainParams,
    last_seen_generation: &'a mut u32,
    last_baseline_generation: &'a mut u32,
    last_sent_gain_millibels: &'a mut i32,
    last_param_value_mb: &'a mut i32,
    last_group: &'a mut i32,
    last_link_mode: &'a mut LinkMode,
    last_invert: &'a mut bool,
    group_gain_override: &'a AtomicI32,
    relative_baseline_mb: &'a mut i32,
    effective_gain_db: &'a mut f32,
    sync_call_count: &'a mut u32,
}

impl GainBrain {
    /// Run once per buffer to synchronize gain with the shared group state.
    fn sync_group(&mut self) {
        let mut state = SyncState {
            instance_id: self.instance_id,
            params: &self.params,
            last_seen_generation: &mut self.last_seen_generation,
            last_baseline_generation: &mut self.last_baseline_generation,
            last_sent_gain_millibels: &mut self.last_sent_gain_millibels,
            last_param_value_mb: &mut self.last_param_value_mb,
            last_group: &mut self.last_group,
            last_link_mode: &mut self.last_link_mode,
            last_invert: &mut self.last_invert,
            group_gain_override: &self.group_gain_override,
            relative_baseline_mb: &mut self.relative_baseline_mb,
            effective_gain_db: &mut self.effective_gain_db,
            sync_call_count: &mut self.sync_call_count,
        };

        let group = state.params.group.value();
        let link_mode = state.params.link_mode.value();
        let call_num = *state.sync_call_count;
        *state.sync_call_count = call_num.saturating_add(1);
        // Only log the first 20 sync_group calls per instance to avoid flooding.
        if call_num < 20 {
            let param_gain_db = util::gain_to_db(state.params.gain.value());
            let param_mb = db_to_millibels(param_gain_db);
            nih_log!(
                "gain-brain[{}]: sync_group #{} param={:.2}dB({}mb) last_param_mb={} \
                 last_sent_mb={} effective={:.2}dB group={} last_group={} mode={:?}",
                state.instance_id, call_num, param_gain_db, param_mb,
                *state.last_param_value_mb, *state.last_sent_gain_millibels,
                *state.effective_gain_db, group, *state.last_group, link_mode
            );
        }

        // Detect parameter transitions.
        let group_changed = group != *state.last_group;
        let mode_changed = link_mode != *state.last_link_mode;

        if group_changed || mode_changed {
            nih_log!(
                "gain-brain[{}]: TRANSITION group {}→{} mode {:?}→{:?} effective={:.2}dB param={:.2}dB",
                state.instance_id,
                *state.last_group,
                group,
                *state.last_link_mode,
                link_mode,
                *state.effective_gain_db,
                util::gain_to_db(state.params.gain.value())
            );
            Self::handle_transition(&mut state, group, link_mode);
            nih_log!(
                "gain-brain[{}]: AFTER TRANSITION baseline={} last_sent={} effective={:.2}dB override={}",
                state.instance_id,
                *state.relative_baseline_mb,
                *state.last_sent_gain_millibels,
                *state.effective_gain_db,
                state.group_gain_override.load(Ordering::Relaxed)
            );
            *state.last_group = group;
            *state.last_link_mode = link_mode;
        }

        // Active sync only when group is 1-16.
        if !(1..=16).contains(&group) {
            return;
        }

        let invert = state.params.invert.value();

        // ── INVERT TOGGLE: re-write slot to prevent discontinuity ────
        if invert != *state.last_invert {
            let effective_mb = db_to_millibels(*state.effective_gain_db);
            let write_mb = if invert { -effective_mb } else { effective_mb };
            nih_log!(
                "gain-brain[{}]: INVERT TOGGLE {}→{} effective={}mb write={}mb (rebaseline)",
                state.instance_id,
                *state.last_invert,
                invert,
                effective_mb,
                write_mb
            );
            groups::write_slot_rebaseline(group as u8, write_mb);
            *state.last_sent_gain_millibels = write_mb;
            let updated_slot = groups::read_slot(group as u8);
            *state.last_seen_generation = updated_slot.generation;
            *state.last_baseline_generation = updated_slot.baseline_generation;
            *state.last_invert = invert;
        }

        let slot = groups::read_slot(group as u8);

        // ── READ PATH: check for external changes ──────────────────────
        // Track whether a read fired this buffer so the write path doesn't
        // overwrite effective_gain_db with the stale param value.
        let mut read_fired = false;

        // If baseline_generation changed, another instance toggled invert
        // or performed a rebaseline write. Re-baseline without applying a delta.
        if slot.baseline_generation != *state.last_baseline_generation {
            nih_log!(
                "gain-brain[{}]: READ REBASELINE baseline_gen {}→{} slot={}mb",
                state.instance_id,
                *state.last_baseline_generation,
                slot.baseline_generation,
                slot.gain_millibels
            );
            *state.relative_baseline_mb = slot.gain_millibels;
            *state.last_sent_gain_millibels = slot.gain_millibels;
            *state.last_seen_generation = slot.generation;
            *state.last_baseline_generation = slot.baseline_generation;
            if link_mode == LinkMode::Absolute {
                let applied_mb = if invert {
                    (-slot.gain_millibels).clamp(-6000, 6000)
                } else {
                    slot.gain_millibels
                };
                state
                    .group_gain_override
                    .store(applied_mb, Ordering::Relaxed);
                *state.effective_gain_db = millibels_to_db(applied_mb);
            }
            read_fired = true;
        } else if slot.generation != *state.last_seen_generation
            && slot.gain_millibels != *state.last_sent_gain_millibels
        {
            match link_mode {
                LinkMode::Absolute => {
                    let applied_mb = if invert {
                        (-slot.gain_millibels).clamp(-6000, 6000)
                    } else {
                        slot.gain_millibels
                    };
                    nih_log!(
                        "gain-brain[{}]: READ ABS slot={}mb gen={} last_sent={}mb invert={} -> override={}mb",
                        state.instance_id,
                        slot.gain_millibels,
                        slot.generation,
                        *state.last_sent_gain_millibels,
                        invert,
                        applied_mb
                    );
                    state
                        .group_gain_override
                        .store(applied_mb, Ordering::Relaxed);
                    *state.last_sent_gain_millibels = slot.gain_millibels;
                    *state.effective_gain_db = millibels_to_db(applied_mb);
                }
                LinkMode::Relative => {
                    let raw_delta = slot.gain_millibels - *state.relative_baseline_mb;
                    let delta_mb = if invert { -raw_delta } else { raw_delta };
                    let current_mb = db_to_millibels(*state.effective_gain_db);
                    let new_mb = current_mb + delta_mb;
                    let clamped_db = clamp_db(millibels_to_db(new_mb));
                    let clamped_mb = db_to_millibels(clamped_db);
                    nih_log!(
                        "gain-brain[{}]: READ REL slot={}mb baseline={}mb delta={}mb invert={} eff={:.2}->{:.2}dB",
                        state.instance_id, slot.gain_millibels, *state.relative_baseline_mb,
                        delta_mb, invert, millibels_to_db(current_mb), millibels_to_db(clamped_mb)
                    );
                    state
                        .group_gain_override
                        .store(clamped_mb, Ordering::Relaxed);
                    *state.relative_baseline_mb = slot.gain_millibels;
                    *state.last_sent_gain_millibels = clamped_mb;
                    *state.effective_gain_db = clamped_db;
                }
            }
            *state.last_seen_generation = slot.generation;
            read_fired = true;
        }

        // ── WRITE PATH: detect user-initiated gain changes ─────────────
        let current_gain_db = util::gain_to_db(state.params.gain.value());
        let current_mb = db_to_millibels(current_gain_db);

        if current_mb != *state.last_param_value_mb {
            let write_mb = if invert { -current_mb } else { current_mb };
            nih_log!(
                "gain-brain[{}]: WRITE param_mb {}→{} write={}mb invert={} effective_was={:.2}dB read_fired={}",
                state.instance_id,
                *state.last_param_value_mb,
                current_mb,
                write_mb,
                invert,
                *state.effective_gain_db,
                read_fired
            );
            // Only update effective_gain_db from the param if the read path
            // didn't fire this buffer — otherwise we'd overwrite the remote
            // delta with the stale param value.
            if !read_fired {
                *state.effective_gain_db = current_gain_db;
            }
            // All normal writes use write_slot (not rebaseline).
            // write_slot_rebaseline is reserved for the invert TOGGLE event
            // only — using it for every inverted write would cause relative
            // readers to silently drop all deltas from inverted writers.
            groups::write_slot(group as u8, write_mb);
            let updated_slot = groups::read_slot(group as u8);
            *state.last_seen_generation = updated_slot.generation;
            *state.last_sent_gain_millibels = write_mb;
            *state.relative_baseline_mb = write_mb;
        }
        *state.last_param_value_mb = current_mb;
    }

    /// Handle transitions when group or link_mode parameters change.
    fn handle_transition(
        state: &mut SyncState<'_>,
        new_group: i32,
        new_link_mode: LinkMode,
    ) {
        let old_group = *state.last_group;
        let old_link_mode = *state.last_link_mode;

        // Leaving a group (group -> 0): keep current gain, stop syncing.
        if new_group == 0 {
            // Nothing to do -- we just stop syncing.
            // Reset tracking state.
            *state.last_seen_generation = 0;
            *state.last_sent_gain_millibels = 0;
            *state.relative_baseline_mb = 0;
            return;
        }

        // Joining or changing to group 1-16 with link active.
        let slot = groups::read_slot(new_group as u8);
        // Use effective_gain_db (what the instance is actually outputting),
        // NOT params.gain.value() which may be stale after group overrides.
        let effective_mb = db_to_millibels(*state.effective_gain_db);

        let was_active = (1..=16).contains(&old_group);

        let invert = state.params.invert.value();

        nih_log!(
            "gain-brain[{}]: handle_transition was_active={} slot=[gain={}mb gen={} bgen={}] \
             effective={}mb param={:.2}dB invert={}",
            state.instance_id, was_active, slot.gain_millibels, slot.generation,
            slot.baseline_generation, effective_mb,
            util::gain_to_db(state.params.gain.value()), invert
        );

        if !was_active {
            // Newly joining a group (old group was 0).
            match new_link_mode {
                LinkMode::Absolute => {
                    if slot.generation == 0 {
                        // Empty slot (no other instances, or fresh restart).
                        // Write our effective gain to the slot so other
                        // instances joining later adopt our restored value
                        // instead of the slot's default 0.
                        let write_mb = if invert { -effective_mb } else { effective_mb };
                        nih_log!(
                            "gain-brain[{}]: JOIN ABS EMPTY SLOT -> write={}mb effective={}mb",
                            state.instance_id, write_mb, effective_mb
                        );
                        groups::write_slot(new_group as u8, write_mb);
                        *state.last_sent_gain_millibels = write_mb;
                    } else {
                        // Slot has data from other instances -- adopt their gain.
                        let applied_mb = if invert {
                            (-slot.gain_millibels).clamp(-6000, 6000)
                        } else {
                            slot.gain_millibels
                        };
                        nih_log!(
                            "gain-brain[{}]: JOIN ABS -> override={}mb (slot={}mb)",
                            state.instance_id, applied_mb, slot.gain_millibels
                        );
                        state
                            .group_gain_override
                            .store(applied_mb, Ordering::Relaxed);
                        *state.last_sent_gain_millibels = slot.gain_millibels;
                    }
                }
                LinkMode::Relative => {
                    // Initialize effective gain from the restored parameter value.
                    // On startup, Default::default() sets effective_gain_db=0.0,
                    // but the host may have already restored the param to a
                    // non-zero value. Use the param as the source of truth.
                    let param_db = util::gain_to_db(state.params.gain.value());
                    *state.effective_gain_db = clamp_db(param_db);
                    let restored_mb = db_to_millibels(*state.effective_gain_db);
                    // Keep current gain, baseline to group's value (tracks slot, not inverted).
                    nih_log!(
                        "gain-brain[{}]: JOIN REL -> baseline={}mb last_sent={}mb effective={:.2}dB (from param)",
                        state.instance_id, slot.gain_millibels, restored_mb,
                        *state.effective_gain_db
                    );
                    *state.relative_baseline_mb = slot.gain_millibels;
                    *state.last_sent_gain_millibels = restored_mb;
                }
            }
        } else {
            match (old_link_mode, new_link_mode) {
                // Absolute -> Absolute (group changed): adopt new group's gain.
                (LinkMode::Absolute, LinkMode::Absolute) => {
                    let applied_mb = if invert {
                        (-slot.gain_millibels).clamp(-6000, 6000)
                    } else {
                        slot.gain_millibels
                    };
                    state
                        .group_gain_override
                        .store(applied_mb, Ordering::Relaxed);
                    *state.last_sent_gain_millibels = slot.gain_millibels;
                }
                // Absolute -> Relative: keep current gain, baseline to it.
                (LinkMode::Absolute, LinkMode::Relative) => {
                    *state.relative_baseline_mb = effective_mb;
                    *state.last_sent_gain_millibels = effective_mb;
                }
                // Relative -> Absolute: snap to group's current gain.
                (LinkMode::Relative, LinkMode::Absolute) => {
                    let applied_mb = if invert {
                        (-slot.gain_millibels).clamp(-6000, 6000)
                    } else {
                        slot.gain_millibels
                    };
                    state
                        .group_gain_override
                        .store(applied_mb, Ordering::Relaxed);
                    *state.last_sent_gain_millibels = slot.gain_millibels;
                }
                // Relative -> Relative (group changed): keep gain, baseline to new group.
                (LinkMode::Relative, LinkMode::Relative) => {
                    *state.relative_baseline_mb = slot.gain_millibels;
                    *state.last_sent_gain_millibels = effective_mb;
                }
            }
        }

        // Sync last_param_value_mb to the current param so the write path
        // doesn't misfire on this buffer (the param may have changed while
        // ungrouped, but that's not a "new" change to propagate).
        // Exception: when freshly joining in Relative mode (!was_active),
        // leave last_param_value_mb at its previous value (0 from Default)
        // so the write path fires on this buffer and propagates the restored
        // param value to the slot.
        if was_active || new_link_mode == LinkMode::Absolute {
            let current_param_db = util::gain_to_db(state.params.gain.value());
            *state.last_param_value_mb = db_to_millibels(current_param_db);
        }

        *state.last_seen_generation = slot.generation;
        *state.last_baseline_generation = slot.baseline_generation;
    }
}

// ── CLAP / VST3 ────────────────────────────────────────────────────────────────

impl ClapPlugin for GainBrain {
    const CLAP_ID: &'static str = "com.mpd.gain-brain";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("A gain utility with cross-instance group linking");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Utility];
}

impl Vst3Plugin for GainBrain {
    const VST3_CLASS_ID: [u8; 16] = *b"GainBrainMpdPlg\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

nih_export_clap!(GainBrain);
nih_export_vst3!(GainBrain);

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_to_millibels_zero() {
        assert_eq!(db_to_millibels(0.0), 0);
    }

    #[test]
    fn test_db_to_millibels_positive() {
        assert_eq!(db_to_millibels(60.0), 6000);
    }

    #[test]
    fn test_db_to_millibels_negative() {
        assert_eq!(db_to_millibels(-60.0), -6000);
    }

    #[test]
    fn test_millibels_to_db_zero() {
        assert!((millibels_to_db(0) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_millibels_to_db_positive() {
        assert!((millibels_to_db(6000) - 60.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_millibels_to_db_negative() {
        assert!((millibels_to_db(-6000) - (-60.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn test_roundtrip_zero() {
        let db = 0.0_f32;
        let mb = db_to_millibels(db);
        let back = millibels_to_db(mb);
        assert!((db - back).abs() < f32::EPSILON);
    }

    #[test]
    fn test_roundtrip_positive() {
        let db = 12.34_f32;
        let mb = db_to_millibels(db);
        assert_eq!(mb, 1234);
        let back = millibels_to_db(mb);
        assert!((db - back).abs() < 0.01);
    }

    #[test]
    fn test_roundtrip_negative() {
        let db = -45.67_f32;
        let mb = db_to_millibels(db);
        assert_eq!(mb, -4567);
        let back = millibels_to_db(mb);
        assert!((db - back).abs() < 0.01);
    }

    #[test]
    fn test_roundtrip_fractional() {
        // 0.005 dB -> 0 or 1 millibels depending on rounding
        let db = 0.005_f32;
        let mb = db_to_millibels(db);
        assert_eq!(mb, 1); // 0.5 rounds to 1
        let back = millibels_to_db(mb);
        assert!((back - 0.01).abs() < f32::EPSILON);
    }

    #[test]
    fn test_clamp_db_within_range() {
        assert!((clamp_db(0.0) - 0.0).abs() < f32::EPSILON);
        assert!((clamp_db(30.0) - 30.0).abs() < f32::EPSILON);
        assert!((clamp_db(-30.0) - (-30.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn test_clamp_db_at_boundaries() {
        assert!((clamp_db(-60.0) - (-60.0)).abs() < f32::EPSILON);
        assert!((clamp_db(60.0) - 60.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_clamp_db_beyond_range() {
        assert!((clamp_db(-100.0) - (-60.0)).abs() < f32::EPSILON);
        assert!((clamp_db(100.0) - 60.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_no_override_sentinel() {
        // Verify the sentinel is a value that cannot represent a valid gain.
        // -60 dB = -6000 mb, +60 dB = 6000 mb, so i32::MIN is far outside range.
        assert!(NO_OVERRIDE < -6000);
    }

    #[test]
    fn test_link_mode_default() {
        let params = GainBrainParams::new();
        assert_eq!(params.link_mode.value(), LinkMode::Relative);
    }

    #[test]
    fn test_group_default() {
        let params = GainBrainParams::new();
        assert_eq!(params.group.value(), 0);
    }

    // ── Integration test helpers ──────────────────────────────────────────

    /// Build a `GainBrainParams` with specific initial values for gain, group,
    /// link_mode, and invert. This simulates what a host does after restoring
    /// plugin state: the params are constructed with the restored values so
    /// that `param.value()` returns the correct initial value.
    fn make_params(
        gain_db: f32,
        group: i32,
        link_mode: LinkMode,
        invert: bool,
    ) -> Arc<GainBrainParams> {
        Arc::new(GainBrainParams {
            editor_state: editor::default_editor_state(),
            effective_gain_mb: Arc::new(AtomicI32::new(db_to_millibels(gain_db))),

            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(gain_db),
                FloatRange::Skewed {
                    min: util::db_to_gain(GAIN_MIN_DB),
                    max: util::db_to_gain(GAIN_MAX_DB),
                    factor: FloatRange::gain_skew_factor(GAIN_MIN_DB, GAIN_MAX_DB),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            group: IntParam::new("Group", group, IntRange::Linear { min: 0, max: 16 }),
            link_mode: EnumParam::new("Link Mode", link_mode),
            invert: BoolParam::new("Invert", invert),
        })
    }

    /// Create a `GainBrain` instance using pre-built params.
    /// The instance starts with default internal state (last_group=0, etc.),
    /// simulating a freshly-created plugin whose params have been restored
    /// by the host before the first process() call.
    fn make_instance(params: Arc<GainBrainParams>) -> GainBrain {
        let instance_id = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        GainBrain {
            instance_id,
            params,
            last_seen_generation: 0,
            last_baseline_generation: 0,
            last_sent_gain_millibels: 0,
            last_param_value_mb: 0,
            last_group: 0,
            last_link_mode: LinkMode::Relative,
            last_invert: false,
            sample_rate: 48000.0,
            group_gain_override: Arc::new(AtomicI32::new(NO_OVERRIDE)),
            relative_baseline_mb: 0,
            effective_gain_db: 0.0,
            display_gain_millibels: Arc::new(AtomicI32::new(0)),
            sync_call_count: 0,
            gui_context: Arc::new(std::sync::Mutex::new(None)),
            param_sync_pending: false,
        }
    }

    /// Simulate initialize() for tests. Restores effective gain from the
    /// persisted field and resets the smoother. Must be called after
    /// make_instance() and before tick() to match the real plugin lifecycle.
    fn init(inst: &mut GainBrain) {
        let gain_db = util::gain_to_db(inst.params.gain.value());
        let persisted_mb = inst.params.effective_gain_mb.load(Ordering::Relaxed);
        let persisted_db = millibels_to_db(persisted_mb);
        if persisted_mb != 0 || gain_db.abs() < 0.01 {
            inst.effective_gain_db = persisted_db;
        } else {
            inst.effective_gain_db = gain_db;
        }
        let target_gain = util::db_to_gain(inst.effective_gain_db);
        inst.params.gain.smoothed.reset(target_gain);
        // Sync tracking fields to prevent write-path misfire (matches real
        // initialize() behavior).
        inst.last_param_value_mb = db_to_millibels(gain_db);
        inst.last_group = inst.params.group.value();
        inst.last_link_mode = inst.params.link_mode.value();
        inst.last_invert = inst.params.invert.value();
        inst.param_sync_pending = false;
        inst.display_gain_millibels.store(
            db_to_millibels(inst.effective_gain_db),
            Ordering::Relaxed,
        );
    }

    /// Run sync_group + drain any pending override (simulates one buffer of
    /// process() without needing a real audio Buffer).
    fn tick(inst: &mut GainBrain) {
        inst.sync_group();
        let override_mb = inst
            .group_gain_override
            .swap(NO_OVERRIDE, Ordering::Relaxed);
        if override_mb != NO_OVERRIDE {
            let target_db = clamp_db(millibels_to_db(override_mb));
            let target_gain = util::db_to_gain(target_db);
            inst.params
                .gain
                .smoothed
                .set_target(inst.sample_rate, target_gain);
            inst.effective_gain_db = target_db;
        } else if !(1..=16).contains(&inst.params.group.value()) {
            inst.effective_gain_db = util::gain_to_db(inst.params.gain.value());
        }
        // Mirror what process() does: persist effective gain for save/restore.
        inst.params
            .effective_gain_mb
            .store(db_to_millibels(inst.effective_gain_db), Ordering::Relaxed);
    }

    // ── Integration tests: Bitwig restart scenario ────────────────────────

    /// Simulates the Bitwig restart bug:
    /// 1. Two instances in group 1, relative mode
    /// 2. Instance 0: -7.40 dB, Instance 1: -1.00 dB (inverted)
    /// 3. Both sync and establish their gains
    /// 4. "Restart": drop both, reset group slots, recreate with restored params
    /// 5. Verify gains are restored correctly (not 0.0 dB)
    #[test]
    fn test_bitwig_restart_relative_mode() {
        // Use group 14 to avoid interference with other tests.
        let test_group: i32 = 14;
        groups::reset_slot(test_group as u8);

        // ── Phase 1: Create two instances and sync them ───────────────
        let params0 = make_params(-7.40, test_group, LinkMode::Relative, false);
        let mut inst0 = make_instance(params0.clone());

        let params1 = make_params(-1.00, test_group, LinkMode::Relative, true);
        let mut inst1 = make_instance(params1.clone());

        // Verify param values were set correctly.
        let inst0_gain_db = util::gain_to_db(inst0.params.gain.value());
        let inst1_gain_db = util::gain_to_db(inst1.params.gain.value());
        eprintln!(
            "Phase 1: inst0 param={:.2}dB, inst1 param={:.2}dB",
            inst0_gain_db, inst1_gain_db
        );
        assert!(
            (inst0_gain_db - (-7.40)).abs() < 0.1,
            "inst0 param should be ~-7.40 dB, got {:.2}",
            inst0_gain_db
        );
        assert!(
            (inst1_gain_db - (-1.00)).abs() < 0.1,
            "inst1 param should be ~-1.00 dB, got {:.2}",
            inst1_gain_db
        );

        // Run several sync cycles to let both instances stabilize.
        for _ in 0..5 {
            tick(&mut inst0);
            tick(&mut inst1);
        }

        // After sync, instance 0 should have effective gain ~-7.40 dB.
        eprintln!(
            "Phase 1 after sync: inst0 effective={:.2}dB, inst1 effective={:.2}dB",
            inst0.effective_gain_db, inst1.effective_gain_db
        );
        assert!(
            (inst0.effective_gain_db - (-7.40)).abs() < 0.2,
            "inst0 effective should be ~-7.40 dB, got {:.2}",
            inst0.effective_gain_db
        );
        // Instance 1 has its own gain of -1.00 dB; it should remain close.
        // (Relative mode: inst1 keeps its own gain, only moves by deltas.)
        assert!(
            (inst1.effective_gain_db - (-1.00)).abs() < 0.2,
            "inst1 effective should be ~-1.00 dB, got {:.2}",
            inst1.effective_gain_db
        );

        // Capture pre-restart values for comparison.
        let pre_restart_inst0_db = inst0.effective_gain_db;
        let pre_restart_inst1_db = inst1.effective_gain_db;

        // ── Phase 2: Simulate project close ───────────────────────────
        drop(inst0);
        drop(inst1);
        // In a real restart, the process dies and all static state is lost.
        groups::reset_slot(test_group as u8);

        // ── Phase 3: Recreate with restored params (simulating host restore) ──
        let restored_params0 = make_params(-7.40, test_group, LinkMode::Relative, false);
        let mut new_inst0 = make_instance(restored_params0.clone());
        init(&mut new_inst0);

        let restored_params1 = make_params(-1.00, test_group, LinkMode::Relative, true);
        let mut new_inst1 = make_instance(restored_params1.clone());
        init(&mut new_inst1);

        // Run several sync cycles (simulating process() buffers).
        for _ in 0..5 {
            tick(&mut new_inst0);
            tick(&mut new_inst1);
        }

        // ── Phase 4: Verify restored gains ────────────────────────────
        eprintln!(
            "Phase 3 after sync: inst0 effective={:.2}dB, inst1 effective={:.2}dB",
            new_inst0.effective_gain_db, new_inst1.effective_gain_db
        );

        // THE KEY ASSERTION: gains must match pre-restart values, NOT 0.0 dB.
        assert!(
            (new_inst0.effective_gain_db - pre_restart_inst0_db).abs() < 0.2,
            "RESTART BUG: inst0 effective={:.2}dB, expected ~{:.2}dB (pre-restart)",
            new_inst0.effective_gain_db,
            pre_restart_inst0_db
        );
        assert!(
            (new_inst1.effective_gain_db - pre_restart_inst1_db).abs() < 0.2,
            "RESTART BUG: inst1 effective={:.2}dB, expected ~{:.2}dB (pre-restart)",
            new_inst1.effective_gain_db,
            pre_restart_inst1_db
        );

        // Verify neither effective gain collapsed to 0.0 dB (the default).
        assert!(
            new_inst0.effective_gain_db.abs() > 1.0,
            "inst0 effective gain collapsed to ~0 dB: {:.2}",
            new_inst0.effective_gain_db
        );
        assert!(
            new_inst1.effective_gain_db.abs() > 0.5,
            "inst1 effective gain collapsed to ~0 dB: {:.2}",
            new_inst1.effective_gain_db
        );
    }

    /// Tests the absolute mode variant of the restart scenario.
    /// Two instances in absolute mode, same group. In absolute mode, when
    /// joining an empty slot (generation=0, gain=0), the instance adopts 0 dB.
    /// The real scenario: on restart, the first instance to join writes its
    /// restored param value to the slot, and the second instance adopts it.
    /// Both should end up at the saved gain, not 0 dB.
    #[test]
    fn test_bitwig_restart_absolute_mode() {
        let test_group: i32 = 15;
        groups::reset_slot(test_group as u8);

        // ── Phase 1: Establish pre-restart state ──────────────────────
        // In absolute mode, all instances converge on the same gain.
        // Pre-populate the slot as if the group was already at -4.50 dB,
        // then create instances that were saved at -4.50 dB.
        groups::write_slot(test_group as u8, -450);

        let params0 = make_params(-4.50, test_group, LinkMode::Absolute, false);
        let mut inst0 = make_instance(params0);

        let params1 = make_params(-4.50, test_group, LinkMode::Absolute, false);
        let mut inst1 = make_instance(params1);

        for _ in 0..5 {
            tick(&mut inst0);
            tick(&mut inst1);
        }

        let pre_restart_db = inst0.effective_gain_db;
        eprintln!(
            "Absolute pre-restart: inst0={:.2}dB, inst1={:.2}dB",
            inst0.effective_gain_db, inst1.effective_gain_db
        );

        // Both should be at -4.50 dB.
        assert!(
            (inst0.effective_gain_db - (-4.50)).abs() < 0.2,
            "pre-restart: inst0 should be ~-4.50 dB, got {:.2}",
            inst0.effective_gain_db
        );
        assert!(
            (inst1.effective_gain_db - (-4.50)).abs() < 0.2,
            "pre-restart: inst1 should be ~-4.50 dB, got {:.2}",
            inst1.effective_gain_db
        );

        // ── Phase 2: Restart ──────────────────────────────────────────
        drop(inst0);
        drop(inst1);
        groups::reset_slot(test_group as u8);

        // After restart, slots are zeroed. The first instance to join
        // should write its restored value; the second should adopt it.
        let restored_params0 = make_params(-4.50, test_group, LinkMode::Absolute, false);
        let mut new_inst0 = make_instance(restored_params0);
        init(&mut new_inst0);

        let restored_params1 = make_params(-4.50, test_group, LinkMode::Absolute, false);
        let mut new_inst1 = make_instance(restored_params1);
        init(&mut new_inst1);

        for _ in 0..5 {
            tick(&mut new_inst0);
            tick(&mut new_inst1);
        }

        // ── Phase 3: Verify ───────────────────────────────────────────
        eprintln!(
            "Absolute post-restart: inst0={:.2}dB, inst1={:.2}dB",
            new_inst0.effective_gain_db, new_inst1.effective_gain_db
        );

        // When joining an empty slot (generation=0), the instance writes its
        // effective gain to the slot rather than adopting 0. This means on
        // restart, the first instance populates the slot with its restored
        // value, and subsequent instances adopt it. Both should end up at
        // the pre-restart gain.
        assert!(
            (new_inst0.effective_gain_db - pre_restart_db).abs() < 0.2,
            "RESTART (abs): inst0 effective={:.2}dB, expected ~{:.2}dB",
            new_inst0.effective_gain_db,
            pre_restart_db
        );
        assert!(
            (new_inst1.effective_gain_db - pre_restart_db).abs() < 0.2,
            "RESTART (abs): inst1 effective={:.2}dB, expected ~{:.2}dB",
            new_inst1.effective_gain_db,
            pre_restart_db
        );
    }

    /// Tests that group_gain_override is set when a remote instance changes
    /// gain, which is the mechanism the host would use to observe param updates.
    /// Bug: "they also don't seem to report their values to the daw correctly.
    /// bitwig's device wrapper in the device chain does not see updates when
    /// another group member changes the value."
    ///
    /// Scenario: inst0 is already established in a group at -3.00 dB.
    /// inst1 joins the same group in Absolute mode. inst1 should receive
    /// an override so the host sees the param update.
    #[test]
    fn test_group_override_reports_to_host() {
        let test_group: i32 = 13;
        groups::reset_slot(test_group as u8);

        // Pre-populate the slot as if inst0 had already written -3.00 dB.
        groups::write_slot(test_group as u8, -300);

        // Instance 1 joins the group in Absolute mode, starting at 0.0 dB.
        // It should see the slot's -300mb and set a group_gain_override.
        let params1 = make_params(0.0, test_group, LinkMode::Absolute, false);
        let mut inst1 = make_instance(params1);

        // Call sync_group directly to check the override BEFORE tick() consumes it.
        inst1.sync_group();

        let override_val = inst1.group_gain_override.load(Ordering::Relaxed);
        let slot = groups::read_slot(test_group as u8);
        eprintln!(
            "Override check: inst1 group_gain_override={}mb (NO_OVERRIDE={})",
            override_val, NO_OVERRIDE
        );
        eprintln!(
            "Slot state: gain={}mb, gen={}, bgen={}",
            slot.gain_millibels, slot.generation, slot.baseline_generation
        );

        // After handle_transition fires for inst1 (JOIN ABS), an override
        // should be pending. The override is the mechanism by which the
        // process() loop retargets the smoother. For the host to see the
        // update, the override must propagate (currently it only changes the
        // smoother, not the param's stored value — this is the bug).
        assert_ne!(
            override_val, NO_OVERRIDE,
            "group_gain_override was not set — host would not see the param update"
        );

        // The override should reflect the slot's -3.00 dB.
        assert!(
            (millibels_to_db(override_val) - (-3.00)).abs() < 0.2,
            "override should be ~-3.00 dB, got {:.2} dB",
            millibels_to_db(override_val)
        );

        // Now verify the second part of the bug: after a group member changes
        // the slot value, the receiving instance gets an override for each
        // change. Simulate inst0 changing gain to -6.00 dB.
        groups::write_slot(test_group as u8, -600);

        // Consume the pending override first (as process() would).
        tick(&mut inst1);

        // Now another remote write arrives.
        groups::write_slot(test_group as u8, -900);
        inst1.sync_group();

        let override_val2 = inst1.group_gain_override.load(Ordering::Relaxed);
        eprintln!(
            "Second override: inst1 group_gain_override={}mb",
            override_val2
        );
        assert_ne!(
            override_val2, NO_OVERRIDE,
            "second override was not set — host misses subsequent group changes"
        );
        assert!(
            (millibels_to_db(override_val2) - (-9.00)).abs() < 0.2,
            "second override should be ~-9.00 dB, got {:.2} dB",
            millibels_to_db(override_val2)
        );
    }

    /// Tests that after restart, the display_gain_millibels atomic reflects
    /// the correct effective gain (not 0), which is what the editor reads.
    #[test]
    fn test_restart_display_gain_updates() {
        let test_group: i32 = 12;
        groups::reset_slot(test_group as u8);

        let params0 = make_params(-5.00, test_group, LinkMode::Relative, false);
        let mut inst0 = make_instance(params0);
        init(&mut inst0);

        // Run several ticks.
        for _ in 0..5 {
            tick(&mut inst0);
            // Simulate what process() does after the override is consumed:
            inst0.display_gain_millibels.store(
                db_to_millibels(inst0.effective_gain_db),
                Ordering::Relaxed,
            );
        }

        let display_mb = inst0.display_gain_millibels.load(Ordering::Relaxed);
        let display_db = millibels_to_db(display_mb);
        eprintln!(
            "Display gain after restart: {}mb ({:.2}dB), effective={:.2}dB",
            display_mb, display_db, inst0.effective_gain_db
        );

        assert!(
            (display_db - (-5.00)).abs() < 0.2,
            "display gain should be ~-5.00 dB, got {:.2} dB",
            display_db
        );
    }

    /// Verifies that effective_gain_mb is persisted and correctly restored.
    ///
    /// Scenario: Instance in a group receives a remote gain override that
    /// changes the effective gain to -8.00 dB while the param stays at 0 dB.
    /// After "restart" (simulated by recreating the instance with the
    /// persisted effective_gain_mb), the effective gain should restore to
    /// -8.00 dB, not fall back to the param's 0 dB.
    #[test]
    fn test_persist_effective_gain_saves_and_restores() {
        let test_group: i32 = 11;
        groups::reset_slot(test_group as u8);

        // ── Phase 1: Instance receives a remote override ─────────────
        // Pre-populate the slot with -8.00 dB so when inst0 joins in
        // absolute mode, it adopts -8.00 dB from the slot.
        groups::write_slot(test_group as u8, -800);

        let params0 = make_params(0.0, test_group, LinkMode::Absolute, false);
        let mut inst0 = make_instance(params0.clone());

        // Run ticks to let the override fire.
        for _ in 0..5 {
            tick(&mut inst0);
        }

        // The effective gain should be -8.00 dB (from the slot).
        assert!(
            (inst0.effective_gain_db - (-8.00)).abs() < 0.2,
            "effective gain should be ~-8.00 dB, got {:.2}",
            inst0.effective_gain_db
        );

        // The param is still 0 dB (the bug: host sees this stale value).
        let param_db = util::gain_to_db(inst0.params.gain.value());
        eprintln!(
            "Phase 1: param={:.2}dB effective={:.2}dB persist={}mb",
            param_db,
            inst0.effective_gain_db,
            inst0.params.effective_gain_mb.load(Ordering::Relaxed)
        );

        // But the persisted field has the correct value.
        let persisted_mb = inst0.params.effective_gain_mb.load(Ordering::Relaxed);
        assert!(
            (millibels_to_db(persisted_mb) - (-8.00)).abs() < 0.2,
            "effective_gain_mb should persist ~-8.00 dB, got {:.2} dB",
            millibels_to_db(persisted_mb)
        );

        // ── Phase 2: Simulate restart ────────────────────────────────
        let saved_mb = inst0.params.effective_gain_mb.load(Ordering::Relaxed);
        drop(inst0);
        groups::reset_slot(test_group as u8);

        // Recreate with restored params. The param stays at 0 dB (what
        // the host saved from the stale param value), but the persisted
        // effective_gain_mb has the correct value.
        let restored_params = make_params(0.0, test_group, LinkMode::Absolute, false);
        // Simulate host restoring the persist field (overwrite the default).
        restored_params
            .effective_gain_mb
            .store(saved_mb, Ordering::Relaxed);

        let mut new_inst = make_instance(restored_params.clone());

        // Simulate initialize(): restore effective gain from persist field.
        new_inst.sample_rate = 48000.0;
        let persisted_mb = new_inst.params.effective_gain_mb.load(Ordering::Relaxed);
        let persisted_db = millibels_to_db(persisted_mb);
        new_inst.effective_gain_db = persisted_db;
        new_inst
            .params
            .gain
            .smoothed
            .reset(util::db_to_gain(persisted_db));

        // Run ticks.
        for _ in 0..5 {
            tick(&mut new_inst);
        }

        // ── Phase 3: Verify restored gain ────────────────────────────
        eprintln!(
            "Phase 2 after restore: effective={:.2}dB persist={}mb",
            new_inst.effective_gain_db,
            new_inst.params.effective_gain_mb.load(Ordering::Relaxed)
        );

        assert!(
            (new_inst.effective_gain_db - (-8.00)).abs() < 0.2,
            "PERSIST BUG: effective={:.2}dB, expected ~-8.00dB",
            new_inst.effective_gain_db
        );
    }

    /// Verifies that effective_gain_mb is updated on every tick/process
    /// cycle, so the host always has the latest value to persist.
    #[test]
    fn test_effective_gain_mb_tracks_group_overrides() {
        let test_group: i32 = 10;
        groups::reset_slot(test_group as u8);

        groups::write_slot(test_group as u8, -300);

        let params0 = make_params(0.0, test_group, LinkMode::Absolute, false);
        let mut inst0 = make_instance(params0.clone());

        tick(&mut inst0);

        // After tick, the override should have fired and effective_gain_mb
        // should match the slot's -3.00 dB.
        let mb = inst0.params.effective_gain_mb.load(Ordering::Relaxed);
        assert!(
            (millibels_to_db(mb) - (-3.00)).abs() < 0.2,
            "effective_gain_mb should track -3.00 dB, got {:.2} dB",
            millibels_to_db(mb)
        );

        // Now a remote write changes to -12.00 dB.
        groups::write_slot(test_group as u8, -1200);
        tick(&mut inst0);

        let mb2 = inst0.params.effective_gain_mb.load(Ordering::Relaxed);
        assert!(
            (millibels_to_db(mb2) - (-12.00)).abs() < 0.2,
            "effective_gain_mb should track -12.00 dB, got {:.2} dB",
            millibels_to_db(mb2)
        );
    }
}
