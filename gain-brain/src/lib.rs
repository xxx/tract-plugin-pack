use nih_plug::prelude::*;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

mod editor;
pub mod groups;

/// Debug-only logging. Compiles to nothing in release builds, avoiding
/// format!() heap allocations and stderr writes on the audio thread.
macro_rules! debug_log {
    ($($arg:tt)*) => {
        #[cfg(debug_assertions)]
        nih_log!($($arg)*);
    };
}

// ── Background task for host-param sync ──────────────────────────────────────

/// Tasks dispatched from the audio thread to the main/GUI thread via
/// `ProcessContext::execute_gui()`.
#[derive(Clone, Copy)]
pub enum GainBrainTask {
    /// Sync the host-visible gain parameter to the given normalized value.
    /// Fired when group sync changes `effective_gain_db` without updating
    /// `params.gain`. The task executor calls `ParamSetter` on the main
    /// thread where it is safe for both CLAP and VST3.
    SyncGainParam {
        normalized: f32,
        /// The param's normalized value at the time this task was dispatched.
        /// If the param has changed from this value by the time the task
        /// executes, the user has interacted and we should skip the update.
        stale_normalized: f32,
    },
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
#[cfg(debug_assertions)]
static INSTANCE_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub struct GainBrain {
    /// Unique ID for this instance, used in debug log lines.
    #[cfg(debug_assertions)]
    instance_id: u32,
    params: Arc<GainBrainParams>,
    /// Last cumulative_delta value we observed. Used for self-echo suppression
    /// and relative delta computation.
    last_seen_cumulative: i32,
    /// Last epoch we observed. Used for rebaseline detection.
    last_seen_epoch: u32,
    /// Last generation we observed (for absolute mode change detection).
    last_seen_generation: u32,
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
    /// The effective gain in dB that we are currently applying.
    effective_gain_db: f32,
    /// Shared with the editor so it can display the effective gain (which may
    /// differ from the parameter value when group sync overrides are active).
    pub display_gain_millibels: Arc<AtomicI32>,
    /// Shared holder for the GuiContext, populated when the editor is first
    /// spawned. Used by `task_executor` (on the main thread) to update the
    /// host-visible gain parameter when group sync changes the effective gain.
    /// Stays populated even after the editor window is closed since the
    /// GuiContext Arc remains valid for the plugin's lifetime.
    pub gui_context: Arc<std::sync::Mutex<Option<Arc<dyn GuiContext>>>>,
    /// When a SyncGainParam task is in flight, stores the target millibel
    /// value that the task will set the param to. `None` = no pending sync.
    /// Used to distinguish "SyncGainParam arrived" (param moves to this
    /// target) from "user changed the knob" (param moves elsewhere).
    param_sync_target_mb: Option<i32>,
    /// The param's millibel value at the time param_sync_target_mb was set.
    /// While sync is pending, if the param still equals this value, the user
    /// hasn't touched it → block writes. If the param changes away from this
    /// value (and isn't the sync target), the user moved the knob → fire write.
    stale_param_mb: i32,
    /// User gain override from the GUI. Written by the editor when the user
    /// drags or double-clicks the gain knob while a group is active. The
    /// audio thread reads and clears this each buffer. This provides a
    /// reliable user-intent signal that survives the SyncGainParam race:
    /// even if SyncGainParam overwrites the param, this override ensures
    /// the user's intended value is applied.
    pub user_gain_override: Arc<AtomicI32>,
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
        #[cfg(debug_assertions)]
        let instance_id = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        debug_log!("gain-brain[{instance_id}]: Default::default()");
        Self {
            #[cfg(debug_assertions)]
            instance_id,
            params: Arc::new(GainBrainParams::new()),
            last_seen_cumulative: 0,
            last_seen_epoch: 0,
            last_seen_generation: 0,
            last_param_value_mb: 0,
            last_group: 0,
            last_link_mode: LinkMode::Relative,
            last_invert: false,
            sample_rate: 44100.0,
            group_gain_override: Arc::new(AtomicI32::new(NO_OVERRIDE)),
            effective_gain_db: 0.0,
            display_gain_millibels: Arc::new(AtomicI32::new(0)),
            gui_context: Arc::new(std::sync::Mutex::new(None)),
            param_sync_target_mb: None,
            stale_param_mb: 0,
            user_gain_override: Arc::new(AtomicI32::new(NO_OVERRIDE)),
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
            GainBrainTask::SyncGainParam {
                normalized,
                stale_normalized,
            } => {
                // This closure runs on the main/GUI thread (via execute_gui),
                // where ParamSetter operations are safe for both CLAP and VST3.
                //
                // Only apply if the param's current value matches the stale
                // value we recorded when dispatching. If the user has
                // interacted (double-click, drag), the param will have
                // changed and we skip the update to avoid overwriting
                // the user's input.
                let current = params.gain.preview_normalized(params.gain.value());
                if (current - stale_normalized).abs() > 0.001 {
                    // Param has been modified since dispatch — skip.
                    return;
                }
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
            self.gui_context.clone(),
            self.user_gain_override.clone(),
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
            self.effective_gain_db = persisted_db;
        } else {
            self.effective_gain_db = gain_db;
        }

        // Retarget the smoother to the restored effective gain so the first
        // process() buffer uses the correct gain.
        let target_gain = util::db_to_gain(self.effective_gain_db);
        self.params.gain.smoothed.reset(target_gain);

        // Sync tracking fields to the current param state so the write path
        // in sync_group() doesn't misfire on the first process() call.
        self.last_param_value_mb = db_to_millibels(gain_db);

        // Update active counts for the group refcount.
        if (1..=16).contains(&self.last_group) {
            groups::decrement_active(self.last_group as u8);
        }

        self.last_link_mode = link_mode;
        self.last_invert = invert;
        self.param_sync_target_mb = None;

        // Join the group via handle_transition so the slot gets properly
        // baselined (stale reset or live baseline). Set last_group to 0
        // first so the transition logic treats this as a fresh join.
        #[cfg(debug_assertions)]
        let prev_last_group = self.last_group;
        self.last_group = 0;
        if (1..=16).contains(&group) {
            self.handle_transition(group, link_mode);
        }
        self.last_group = group;

        // Sync display gain for the editor.
        self.display_gain_millibels.store(
            db_to_millibels(self.effective_gain_db),
            Ordering::Relaxed,
        );

        debug_log!(
            "gain-brain[{}]: initialize() gain={:.2}dB group={} mode={:?} invert={} \
             effective={:.2}dB persisted={}mb prev_last_group={}",
            self.instance_id, gain_db, group, link_mode, invert,
            self.effective_gain_db, persisted_mb, prev_last_group
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
            debug_log!(
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

        // Persist the effective gain so the host saves the correct value.
        self.params.effective_gain_mb.store(effective_mb, Ordering::Relaxed);

        // ── Sync host-visible param via main-thread task ──────────────
        let param_db = util::gain_to_db(self.params.gain.value());
        if (self.effective_gain_db - param_db).abs() > 0.05 && self.param_sync_target_mb.is_none() {
            let target_mb = effective_mb;
            let target_linear = util::db_to_gain(self.effective_gain_db);
            let normalized = self.params.gain.preview_normalized(target_linear);
            let stale_normalized = self.params.gain.preview_normalized(self.params.gain.value());
            context.execute_gui(GainBrainTask::SyncGainParam {
                normalized,
                stale_normalized,
            });
            self.param_sync_target_mb = Some(target_mb);
            self.stale_param_mb = db_to_millibels(param_db);
        }
        // Don't clear param_sync_target_mb here — it gets cleared by
        // the sync_blocked check inside sync_group() when the SyncGainParam
        // actually arrives (param matches target).

        // When in a group, request the host to keep calling process() even
        // when the track is silent. This is critical: sync_group() runs inside
        // process(), so without it, user knob changes are never written to
        // the shared slot, and reads from other instances are never applied.
        if (1..=16).contains(&self.params.group.value()) {
            ProcessStatus::KeepAlive
        } else {
            ProcessStatus::Normal
        }
    }

    fn deactivate(&mut self) {
        if (1..=16).contains(&self.last_group) {
            groups::decrement_active(self.last_group as u8);
        }
        debug_log!(
            "gain-brain[{}]: deactivate() last_group={}",
            self.instance_id, self.last_group
        );
        self.last_group = 0;
    }
}

// ── Group sync logic ───────────────────────────────────────────────────────────

impl GainBrain {
    /// Run once per buffer to synchronize gain with the shared group state.
    fn sync_group(&mut self) {
        let group = self.params.group.value();
        let link_mode = self.params.link_mode.value();
        let invert = self.params.invert.value();

        // ── Transitions (group/mode changes) ──
        let group_changed = group != self.last_group;
        let mode_changed = link_mode != self.last_link_mode;
        if group_changed || mode_changed {
            debug_log!(
                "gain-brain[{}]: TRANSITION group {}→{} mode {:?}→{:?} effective={:.2}dB",
                self.instance_id, self.last_group, group,
                self.last_link_mode, link_mode, self.effective_gain_db
            );
            self.handle_transition(group, link_mode);
            self.last_group = group;
            self.last_link_mode = link_mode;
        }

        if !(1..=16).contains(&group) {
            return;
        }

        // ── Invert toggle → local rebaseline (no epoch bump) ──
        if invert != self.last_invert {
            debug_log!(
                "gain-brain[{}]: INVERT TOGGLE {}→{} effective={:.2}dB",
                self.instance_id, self.last_invert, invert, self.effective_gain_db
            );
            let snap = groups::read_slot(group as u8);
            self.last_seen_cumulative = snap.cumulative_delta;
            self.last_seen_generation = snap.generation;
            self.last_invert = invert;
        }

        let snap = groups::read_slot(group as u8);

        // ── READ PATH ──
        let mut read_fired = false;

        // Epoch change → rebaseline (don't apply delta)
        if snap.epoch != self.last_seen_epoch {
            debug_log!(
                "gain-brain[{}]: READ REBASELINE epoch {}→{} cum={}",
                self.instance_id, self.last_seen_epoch, snap.epoch, snap.cumulative_delta
            );
            self.last_seen_cumulative = snap.cumulative_delta;
            self.last_seen_epoch = snap.epoch;
            self.last_seen_generation = snap.generation;
            // For absolute mode, also adopt the absolute gain
            if link_mode == LinkMode::Absolute {
                let canonical = snap.absolute_gain;
                let local = if invert { -canonical } else { canonical };
                let local = local.clamp(-6000, 6000);
                self.group_gain_override.store(local, Ordering::Relaxed);
                self.effective_gain_db = millibels_to_db(local);
                self.last_param_value_mb = local;
            }
            read_fired = true;
        } else {
            match link_mode {
                LinkMode::Absolute => {
                    if snap.generation != self.last_seen_generation {
                        let canonical = snap.absolute_gain;
                        let local = if invert { -canonical } else { canonical };
                        let local = local.clamp(-6000, 6000);
                        debug_log!(
                            "gain-brain[{}]: READ ABS canonical={}mb local={}mb gen={}",
                            self.instance_id, canonical, local, snap.generation
                        );
                        self.group_gain_override.store(local, Ordering::Relaxed);
                        self.effective_gain_db = millibels_to_db(local);
                        self.last_seen_generation = snap.generation;
                        self.last_seen_cumulative = snap.cumulative_delta;
                        self.last_param_value_mb = local;
                        read_fired = true;
                    }
                }
                LinkMode::Relative => {
                    if snap.cumulative_delta != self.last_seen_cumulative {
                        let canonical_delta = snap.cumulative_delta - self.last_seen_cumulative;
                        let local_delta = if invert { -canonical_delta } else { canonical_delta };
                        let current_mb = db_to_millibels(self.effective_gain_db);
                        let new_mb = current_mb + local_delta;
                        let clamped_db = clamp_db(millibels_to_db(new_mb));
                        let clamped_mb = db_to_millibels(clamped_db);
                        debug_log!(
                            "gain-brain[{}]: READ REL canonical_delta={}mb local_delta={}mb eff={:.2}->{:.2}dB",
                            self.instance_id, canonical_delta, local_delta,
                            self.effective_gain_db, clamped_db
                        );
                        self.group_gain_override.store(clamped_mb, Ordering::Relaxed);
                        self.effective_gain_db = clamped_db;
                        self.last_seen_cumulative = snap.cumulative_delta;
                        self.last_seen_generation = snap.generation;
                        self.last_param_value_mb = clamped_mb;
                        read_fired = true;
                    }
                }
            }
        }

        // ── WRITE PATH ──
        //
        // Check for explicit user gain override first. This is written by
        // the GUI when the user drags or double-clicks the gain knob. It
        // provides a reliable user-intent signal that survives the
        // SyncGainParam race condition.
        let user_override_mb = self
            .user_gain_override
            .swap(NO_OVERRIDE, Ordering::Relaxed);
        if user_override_mb != NO_OVERRIDE && !read_fired {
            let effective_mb = db_to_millibels(self.effective_gain_db);
            let local_delta = user_override_mb - effective_mb;
            if local_delta.abs() > 1 {
                let canonical_delta = if invert { -local_delta } else { local_delta };
                let canonical_absolute = if invert {
                    -user_override_mb
                } else {
                    user_override_mb
                };

                debug_log!(
                    "gain-brain[{}]: USER WRITE local_delta={}mb canonical_delta={}mb canonical_abs={}mb",
                    self.instance_id, local_delta, canonical_delta, canonical_absolute
                );

                let (old_cumulative, new_gen) = groups::add_delta(group as u8, canonical_delta);
                groups::set_absolute(group as u8, canonical_absolute);

                self.last_seen_cumulative = old_cumulative + canonical_delta;
                self.last_seen_generation = new_gen;

                self.effective_gain_db = clamp_db(millibels_to_db(user_override_mb));
            }
            // Update param tracking to the user's value. Keep
            // param_sync_target_mb intact — the stale SyncGainParam is
            // still in flight and we need the sync_blocked check to
            // absorb it when it arrives (case b: param changes to target).
            // Update stale_param_mb so the "unchanged stale" check (case a)
            // doesn't misidentify the user's new param value as stale.
            self.last_param_value_mb = user_override_mb;
            let current_gain_db = util::gain_to_db(self.params.gain.value());
            self.stale_param_mb = db_to_millibels(current_gain_db);
        } else {
            // Fall back to param-based change detection.
            let current_gain_db = util::gain_to_db(self.params.gain.value());
            let current_mb = db_to_millibels(current_gain_db);

            // When sync is pending, the param may be:
            //   (a) still at the stale value → not user input, block write
            //   (b) at the sync target → SyncGainParam arrived, update tracking
            //   (c) at some other value → user changed the knob, fire write
            let sync_blocked = if let Some(target) = self.param_sync_target_mb {
                if (current_mb - self.stale_param_mb).abs() <= 1 {
                    true
                } else if (current_mb - target).abs() <= 1 {
                    self.last_param_value_mb = current_mb;
                    self.param_sync_target_mb = None;
                    true
                } else {
                    debug_log!(
                        "gain-brain[{}]: USER OVERRIDE via param: param={}mb stale={}mb target={}mb",
                        self.instance_id, current_mb, self.stale_param_mb, target
                    );
                    self.param_sync_target_mb = None;
                    false
                }
            } else {
                false
            };

            if (current_mb - self.last_param_value_mb).abs() > 1
                && !read_fired
                && !sync_blocked
            {
                let local_delta = current_mb - self.last_param_value_mb;
                let canonical_delta = if invert { -local_delta } else { local_delta };
                let canonical_absolute = if invert { -current_mb } else { current_mb };

                debug_log!(
                    "gain-brain[{}]: WRITE local_delta={}mb canonical_delta={}mb canonical_abs={}mb",
                    self.instance_id, local_delta, canonical_delta, canonical_absolute
                );

                let (old_cumulative, new_gen) = groups::add_delta(group as u8, canonical_delta);
                groups::set_absolute(group as u8, canonical_absolute);

                self.last_seen_cumulative = old_cumulative + canonical_delta;
                self.last_seen_generation = new_gen;

                self.effective_gain_db = millibels_to_db(current_mb);
            }

            if !read_fired {
                self.last_param_value_mb = current_mb;
            }
        }
    }

    /// Handle transitions when group or link_mode parameters change.
    fn handle_transition(&mut self, new_group: i32, new_link_mode: LinkMode) {
        let old_group = self.last_group;

        // Leaving a group
        if (1..=16).contains(&old_group) {
            groups::decrement_active(old_group as u8);
        }

        // Not joining any group
        if !(1..=16).contains(&new_group) {
            self.last_seen_cumulative = 0;
            self.last_seen_epoch = 0;
            self.last_seen_generation = 0;
            return;
        }

        // Joining a group
        groups::increment_active(new_group as u8);
        let count = groups::active_count(new_group as u8);

        if count <= 1 {
            // First instance (stale slot) — reset
            debug_log!(
                "gain-brain[{}]: TRANSITION STALE RESET group={} effective={:.2}dB",
                self.instance_id, new_group, self.effective_gain_db
            );
            groups::reset_cumulative(new_group as u8);
            self.last_seen_cumulative = 0;
            self.last_seen_epoch = groups::read_slot(new_group as u8).epoch;
            self.last_seen_generation = 0;

            // For absolute mode, seed the slot with our effective gain so
            // the next instance to join can adopt it.
            if new_link_mode == LinkMode::Absolute {
                let effective_mb = db_to_millibels(self.effective_gain_db);
                let invert = self.params.invert.value();
                let canonical = if invert { -effective_mb } else { effective_mb };
                let (old_cum, new_gen) = groups::add_delta(new_group as u8, canonical);
                groups::set_absolute(new_group as u8, canonical);
                self.last_seen_cumulative = old_cum + canonical;
                self.last_seen_generation = new_gen;
            }
        } else {
            // Joining a live group — baseline to current state
            let snap = groups::read_slot(new_group as u8);
            debug_log!(
                "gain-brain[{}]: TRANSITION JOIN LIVE group={} cum={} abs={} epoch={} gen={} effective={:.2}dB",
                self.instance_id, new_group, snap.cumulative_delta, snap.absolute_gain,
                snap.epoch, snap.generation, self.effective_gain_db
            );
            self.last_seen_cumulative = snap.cumulative_delta;
            self.last_seen_epoch = snap.epoch;
            self.last_seen_generation = snap.generation;

            if new_link_mode == LinkMode::Absolute {
                let canonical = snap.absolute_gain;
                let invert = self.params.invert.value();
                let local = if invert { -canonical } else { canonical };
                let local = local.clamp(-6000, 6000);
                self.group_gain_override.store(local, Ordering::Relaxed);
                self.effective_gain_db = millibels_to_db(local);
            }
            // Relative: keep own effective gain, just baseline cumulative
        }

        // Sync param tracking
        let param_db = util::gain_to_db(self.params.gain.value());
        self.last_param_value_mb = db_to_millibels(param_db);
        self.last_invert = self.params.invert.value();
    }
}

// ── CLAP / VST3 ────────────────────────────────────────────────────────────────

impl ClapPlugin for GainBrain {
    const CLAP_ID: &'static str = "com.mpd.gain-brain";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("A gain utility with cross-instance group linking");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::Utility, ClapFeature::Stereo];
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
        let db = 0.005_f32;
        let mb = db_to_millibels(db);
        assert_eq!(mb, 1);
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
        const { assert!(NO_OVERRIDE < -6000) };
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

    fn make_instance(params: Arc<GainBrainParams>) -> GainBrain {
        let instance_id = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        GainBrain {
            instance_id,
            params,
            last_seen_cumulative: 0,
            last_seen_epoch: 0,
            last_seen_generation: 0,
            last_param_value_mb: 0,
            last_group: 0,
            last_link_mode: LinkMode::Relative,
            last_invert: false,
            sample_rate: 48000.0,
            group_gain_override: Arc::new(AtomicI32::new(NO_OVERRIDE)),
            effective_gain_db: 0.0,
            display_gain_millibels: Arc::new(AtomicI32::new(0)),
            gui_context: Arc::new(std::sync::Mutex::new(None)),
            param_sync_target_mb: None,
            stale_param_mb: 0,
            user_gain_override: Arc::new(AtomicI32::new(NO_OVERRIDE)),
        }
    }

    fn init(inst: &mut GainBrain) {
        let gain_db = util::gain_to_db(inst.params.gain.value());
        let group = inst.params.group.value();
        let link_mode = inst.params.link_mode.value();
        let persisted_mb = inst.params.effective_gain_mb.load(Ordering::Relaxed);
        let persisted_db = millibels_to_db(persisted_mb);
        if persisted_mb != 0 || gain_db.abs() < 0.01 {
            inst.effective_gain_db = persisted_db;
        } else {
            inst.effective_gain_db = gain_db;
        }
        let target_gain = util::db_to_gain(inst.effective_gain_db);
        inst.params.gain.smoothed.reset(target_gain);
        inst.last_param_value_mb = db_to_millibels(gain_db);
        // Update active counts (mirrors real initialize()).
        if (1..=16).contains(&inst.last_group) {
            groups::decrement_active(inst.last_group as u8);
        }
        inst.last_link_mode = link_mode;
        inst.last_invert = inst.params.invert.value();
        inst.param_sync_target_mb = None;
        // Join the group via handle_transition (mirrors real initialize()).
        inst.last_group = 0;
        if (1..=16).contains(&group) {
            inst.handle_transition(group, link_mode);
        }
        inst.last_group = group;
        inst.display_gain_millibels.store(
            db_to_millibels(inst.effective_gain_db),
            Ordering::Relaxed,
        );
    }

    #[allow(dead_code)]
    fn deinit(inst: &mut GainBrain) {
        if (1..=16).contains(&inst.last_group) {
            groups::decrement_active(inst.last_group as u8);
        }
        inst.last_group = 0;
    }

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
        // Mirror process()'s param_sync_target_mb logic so the write path
        // doesn't misinterpret stale param values as user knob changes.
        // Only SET a new sync target when none exists. Don't clear it here
        // — it gets cleared by the sync_blocked check inside sync_group()
        // when the SyncGainParam actually arrives (param matches target).
        let param_db = util::gain_to_db(inst.params.gain.value());
        if (inst.effective_gain_db - param_db).abs() > 0.05 && inst.param_sync_target_mb.is_none() {
            let target_mb = db_to_millibels(inst.effective_gain_db);
            inst.param_sync_target_mb = Some(target_mb);
            inst.stale_param_mb = db_to_millibels(param_db);
        }
        inst.params
            .effective_gain_mb
            .store(db_to_millibels(inst.effective_gain_db), Ordering::Relaxed);
    }

    // ── Integration tests ────────────────────────────────────────────────

    #[test]
    fn test_relative_no_invert() {
        groups::reset_slot(1);
        let mut a = make_instance(make_params(0.0, 1, LinkMode::Relative, false));
        let mut b = make_instance(make_params(0.0, 1, LinkMode::Relative, false));
        init(&mut a); init(&mut b);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // A drags to +3dB
        a.params = make_params(3.0, 1, LinkMode::Relative, false);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        assert!((b.effective_gain_db - 3.0).abs() < 0.5, "B should follow A: got {}", b.effective_gain_db);
    }

    #[test]
    fn test_relative_b_inverted() {
        groups::reset_slot(2);
        let mut a = make_instance(make_params(0.0, 2, LinkMode::Relative, false));
        let mut b = make_instance(make_params(0.0, 2, LinkMode::Relative, true));
        init(&mut a); init(&mut b);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // A drags to +3dB
        a.params = make_params(3.0, 2, LinkMode::Relative, false);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        assert!((b.effective_gain_db - (-3.0)).abs() < 0.5, "B inverted should go -3dB: got {}", b.effective_gain_db);
    }

    #[test]
    fn test_invert_toggle_no_jump() {
        groups::reset_slot(3);
        let mut a = make_instance(make_params(5.0, 3, LinkMode::Relative, false));
        let mut b = make_instance(make_params(5.0, 3, LinkMode::Relative, false));
        init(&mut a); init(&mut b);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        let b_before = b.effective_gain_db;
        // B toggles invert
        b.params = make_params(5.0, 3, LinkMode::Relative, true);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // B's effective should not jump
        assert!((b.effective_gain_db - b_before).abs() < 0.5, "B should not jump on invert toggle: got {} (was {})", b.effective_gain_db, b_before);
        // A's effective should not jump
        assert!((a.effective_gain_db - 5.0).abs() < 0.5, "A should not jump on B's invert toggle: got {}", a.effective_gain_db);
    }

    #[test]
    fn test_rapid_writes_delayed_read() {
        groups::reset_slot(4);
        let mut a = make_instance(make_params(0.0, 4, LinkMode::Relative, false));
        let mut b = make_instance(make_params(0.0, 4, LinkMode::Relative, false));
        init(&mut a); init(&mut b);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // A writes 5 times without B reading
        for gain in [1.0, 2.0, 3.0, 4.0, 5.0] {
            a.params = make_params(gain, 4, LinkMode::Relative, false);
            tick(&mut a);
        }

        // B reads once — should get the full +5dB delta
        for _ in 0..3 { tick(&mut b); }
        assert!((b.effective_gain_db - 5.0).abs() < 0.5, "B should get cumulative delta: got {}", b.effective_gain_db);
    }

    #[test]
    fn test_self_echo_suppression() {
        groups::reset_slot(5);
        let mut a = make_instance(make_params(0.0, 5, LinkMode::Relative, false));
        init(&mut a);
        for _ in 0..3 { tick(&mut a); }

        a.params = make_params(3.0, 5, LinkMode::Relative, false);
        tick(&mut a);

        // A should be at 3dB (from its own write), not 6dB (from reading its own delta)
        assert!((a.effective_gain_db - 3.0).abs() < 0.5, "A should not read its own delta: got {}", a.effective_gain_db);
    }

    #[test]
    fn test_stale_slot_cleared_on_join() {
        groups::reset_slot(6);
        // Simulate stale data
        groups::add_delta(6, 1000);
        // No active instances

        let mut a = make_instance(make_params(0.0, 6, LinkMode::Relative, false));
        init(&mut a);
        for _ in 0..3 { tick(&mut a); }

        // A should be at 0dB, not affected by stale 1000mb
        assert!(a.effective_gain_db.abs() < 0.5, "A should not be affected by stale data: got {}", a.effective_gain_db);
    }

    #[test]
    fn test_late_joiner_doesnt_clobber() {
        groups::reset_slot(7);
        let mut a = make_instance(make_params(6.0, 7, LinkMode::Relative, false));
        init(&mut a);
        for _ in 0..3 { tick(&mut a); }

        let mut b = make_instance(make_params(0.0, 7, LinkMode::Relative, false));
        init(&mut b);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // A should still be at ~6dB
        assert!((a.effective_gain_db - 6.0).abs() < 0.5, "A should not be clobbered: got {}", a.effective_gain_db);
    }

    #[test]
    fn test_absolute_mode() {
        groups::reset_slot(8);
        let mut a = make_instance(make_params(5.0, 8, LinkMode::Absolute, false));
        let mut b = make_instance(make_params(0.0, 8, LinkMode::Absolute, false));
        init(&mut a); init(&mut b);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // B should adopt A's value
        assert!((b.effective_gain_db - 5.0).abs() < 0.5, "B should adopt A's gain: got {}", b.effective_gain_db);
    }

    #[test]
    fn test_absolute_mode_inverted() {
        groups::reset_slot(9);
        let mut a = make_instance(make_params(5.0, 9, LinkMode::Absolute, false));
        let mut b = make_instance(make_params(0.0, 9, LinkMode::Absolute, true));
        init(&mut a); init(&mut b);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // B inverted should adopt -5dB
        assert!((b.effective_gain_db - (-5.0)).abs() < 0.5, "B inverted should adopt -5dB: got {}", b.effective_gain_db);
    }

    /// Regression: toggling invert on/off without moving gain must not break
    /// group sync. Previously, each invert toggle bumped the shared epoch,
    /// causing other instances to rebaseline and lose their cumulative delta
    /// tracking. After the toggle cycle, subsequent writes from A would not
    /// propagate to B because B's rebaselined cumulative matched the slot.
    #[test]
    fn test_invert_toggle_cycle_does_not_break_sync() {
        groups::reset_slot(10);
        let mut a = make_instance(make_params(0.0, 10, LinkMode::Relative, false));
        let mut b = make_instance(make_params(0.0, 10, LinkMode::Relative, false));
        init(&mut a); init(&mut b);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // B toggles invert ON then OFF without moving gain
        b.params = make_params(0.0, 10, LinkMode::Relative, true);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }
        b.params = make_params(0.0, 10, LinkMode::Relative, false);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // Now A moves to +4dB — B must follow
        a.params = make_params(4.0, 10, LinkMode::Relative, false);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        assert!(
            (b.effective_gain_db - 4.0).abs() < 0.5,
            "B must follow A after invert toggle cycle: got {}",
            b.effective_gain_db
        );
    }

    /// Regression: exact bug-report scenario. A toggles invert on/off while B
    /// is not ticking (host not calling process). Then A moves gain. When B
    /// finally wakes up it must see the full delta — not swallow it due to a
    /// stale epoch-driven rebaseline.
    #[test]
    fn test_invert_toggle_cycle_delayed_detection() {
        groups::reset_slot(11);
        let mut a = make_instance(make_params(5.0, 11, LinkMode::Relative, false));
        let mut b = make_instance(make_params(5.0, 11, LinkMode::Relative, false));
        init(&mut a); init(&mut b);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // A toggles invert on then off — ONLY A ticks, B sleeps
        a.params = make_params(5.0, 11, LinkMode::Relative, true);
        tick(&mut a);  // A detects toggle
        a.params = make_params(5.0, 11, LinkMode::Relative, false);
        tick(&mut a);  // A detects toggle back

        // A moves to 8dB — ONLY A ticks, B still sleeping
        a.params = make_params(8.0, 11, LinkMode::Relative, false);
        for _ in 0..3 { tick(&mut a); }

        // B finally wakes up — must see the +3dB delta
        for _ in 0..3 { tick(&mut b); }
        assert!((b.effective_gain_db - 8.0).abs() < 0.5,
            "B must follow A after delayed toggle detection: got {}", b.effective_gain_db);
    }

    // ── Regression tests: SyncGainParam feedback ────────────────────────
    //
    // These tests simulate the real-world race condition where the async
    // SyncGainParam task (dispatched via execute_gui, runs on the GUI
    // thread) arrives AFTER the user has already made another param
    // change, overwriting that change with a stale value.
    //
    // The fix: the GUI writes to `user_gain_override` whenever the user
    // interacts with the gain knob. The audio thread reads this override
    // and applies it with priority over the (possibly stale) param value.
    // This provides a reliable user-intent signal that survives the
    // SyncGainParam race.
    //
    // In these tests, writing to `user_gain_override` simulates the
    // GUI's user-intent signal. The param (`make_params(...)`) may be
    // overwritten by a stale SyncGainParam, but the override ensures
    // the user's intended value is applied.

    /// Regression 1: Double-click reset eaten by stale SyncGainParam.
    ///
    /// The user double-clicks B to reset to 0dB. The GUI writes 0mb to
    /// user_gain_override. Even if SyncGainParam overwrites the param
    /// back to -5dB, the override ensures the reset is applied.
    #[test]
    fn test_regression_double_click_reset_propagates() {
        groups::reset_slot(12);
        let mut a = make_instance(make_params(0.0, 12, LinkMode::Relative, false));
        let mut b = make_instance(make_params(0.0, 12, LinkMode::Relative, false));
        init(&mut a); init(&mut b);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // A moves to -5dB
        a.params = make_params(-5.0, 12, LinkMode::Relative, false);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // B should have followed A to -5dB
        assert!((b.effective_gain_db - (-5.0)).abs() < 0.5,
            "B should follow A to -5dB, got {:.2}", b.effective_gain_db);

        // User double-clicks B to reset to 0dB.
        // The GUI writes user_gain_override AND sets the param.
        b.user_gain_override.store(0, Ordering::Relaxed);
        b.params = make_params(0.0, 12, LinkMode::Relative, false);
        for _ in 0..2 { tick(&mut b); }

        // Stale SyncGainParam arrives — overwrites B's param to -5dB.
        // But user_gain_override already fired on the previous tick.
        b.params = make_params(-5.0, 12, LinkMode::Relative, false);
        tick(&mut b);

        // Let everything settle.
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        assert!((b.effective_gain_db).abs() < 0.5,
            "B should be at 0dB after double-click reset, got {:.2}",
            b.effective_gain_db);
        assert!((a.effective_gain_db).abs() < 0.5,
            "A should follow B to 0dB, got {:.2}", a.effective_gain_db);
    }

    /// Regression 2: Same race with inverted B. The reset to 0dB
    /// must propagate correctly through inversion.
    #[test]
    fn test_regression_post_reset_sync_works() {
        groups::reset_slot(13);
        let mut a = make_instance(make_params(0.0, 13, LinkMode::Relative, false));
        let mut b = make_instance(make_params(0.0, 13, LinkMode::Relative, true));
        init(&mut a); init(&mut b);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // A moves to -5dB (B inverted follows to +5dB)
        a.params = make_params(-5.0, 13, LinkMode::Relative, false);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        assert!((b.effective_gain_db - 5.0).abs() < 0.5,
            "B inverted should follow to +5dB, got {:.2}", b.effective_gain_db);

        // User double-clicks B to 0dB.
        b.user_gain_override.store(0, Ordering::Relaxed);
        b.params = make_params(0.0, 13, LinkMode::Relative, true);
        for _ in 0..2 { tick(&mut b); }

        // Stale SyncGainParam arrives — overwrites B's param to +5dB.
        b.params = make_params(5.0, 13, LinkMode::Relative, true);
        tick(&mut b);

        // Let settle.
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // SyncGainParam on A (param catches up)
        a.params = make_params(a.effective_gain_db, 13, LinkMode::Relative, false);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        assert!((b.effective_gain_db).abs() < 0.5,
            "B should be at 0dB after double-click reset, got {:.2}",
            b.effective_gain_db);
        assert!((a.effective_gain_db).abs() < 0.5,
            "A should follow B's reset to 0dB, got {:.2}",
            a.effective_gain_db);
    }

    /// Regression 3: User move eaten by stale SyncGainParam.
    /// The GUI writes user_gain_override when the user drags,
    /// ensuring the move is applied even if SyncGainParam overwrites.
    #[test]
    fn test_regression_value_does_not_revert_after_sync() {
        groups::reset_slot(14);
        let mut a = make_instance(make_params(0.0, 14, LinkMode::Relative, false));
        let mut b = make_instance(make_params(0.0, 14, LinkMode::Relative, false));
        init(&mut a); init(&mut b);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        // A moves to -5dB
        a.params = make_params(-5.0, 14, LinkMode::Relative, false);
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        assert!((b.effective_gain_db - (-5.0)).abs() < 0.5,
            "B should follow A to -5dB, got {:.2}", b.effective_gain_db);

        // User moves B to -2dB before SyncGainParam arrives.
        b.user_gain_override
            .store(db_to_millibels(-2.0), Ordering::Relaxed);
        b.params = make_params(-2.0, 14, LinkMode::Relative, false);
        for _ in 0..2 { tick(&mut b); }

        // Stale SyncGainParam arrives — overwrites B's param to -5dB.
        b.params = make_params(-5.0, 14, LinkMode::Relative, false);
        tick(&mut b);

        // Let settle.
        for _ in 0..3 { tick(&mut a); tick(&mut b); }

        assert!((b.effective_gain_db - (-2.0)).abs() < 0.5,
            "B should be at -2dB (user's move), not reverted to -5dB, got {:.2}",
            b.effective_gain_db);
        assert!((a.effective_gain_db - (-2.0)).abs() < 0.5,
            "A should follow B to -2dB, got {:.2}", a.effective_gain_db);
    }
}
