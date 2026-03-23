use nih_plug::prelude::*;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

mod editor;
pub mod groups;
pub mod widgets;

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
    group_file: Option<groups::GroupFile>,
    /// Last generation we observed from the group slot (to detect external writes).
    last_seen_generation: u32,
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
}

// ── Params ─────────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct GainBrainParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<editor::GainBrainEditorState>,

    #[id = "gain"]
    pub gain: FloatParam,

    #[id = "group"]
    pub group: IntParam,

    #[id = "link_mode"]
    pub link_mode: EnumParam<LinkMode>,
}

impl Default for GainBrain {
    fn default() -> Self {
        let group_file = match groups::GroupFile::open(&groups::GroupFile::default_path()) {
            Ok(gf) => Some(gf),
            Err(e) => {
                nih_log!("gain-brain: failed to open group file: {e}");
                None
            }
        };
        let instance_id = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        nih_log!("gain-brain[{instance_id}]: initialized");
        Self {
            instance_id,
            params: Arc::new(GainBrainParams::new()),
            group_file,
            last_seen_generation: 0,
            last_sent_gain_millibels: 0,
            last_param_value_mb: 0,
            last_group: 0,
            last_link_mode: LinkMode::Absolute,
            sample_rate: 44100.0,
            group_gain_override: Arc::new(AtomicI32::new(NO_OVERRIDE)),
            relative_baseline_mb: 0,
            effective_gain_db: 0.0,
            display_gain_millibels: Arc::new(AtomicI32::new(0)),
        }
    }
}

impl GainBrainParams {
    fn new() -> Self {
        Self {
            editor_state: editor::GainBrainEditorState::default_state(),

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

            link_mode: EnumParam::new("Link Mode", LinkMode::Absolute),
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
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.display_gain_millibels.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        true
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let num_channels = buffer.channels();
        if num_channels < 2 {
            return ProcessStatus::Normal;
        }

        // ── Group sync (once per buffer, before applying gain) ─────────
        self.sync_group();

        // ── Check for pending group gain override ──────────────────────
        let override_mb = self.group_gain_override.swap(NO_OVERRIDE, Ordering::Relaxed);
        if override_mb != NO_OVERRIDE {
            let target_db = clamp_db(millibels_to_db(override_mb));
            let target_gain = util::db_to_gain(target_db);
            self.params.gain.smoothed.set_target(self.sample_rate, target_gain);
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
        self.display_gain_millibels
            .store(db_to_millibels(self.effective_gain_db), Ordering::Relaxed);

        ProcessStatus::Normal
    }
}

// ── Group sync logic ───────────────────────────────────────────────────────────

/// State fields needed by transition/sync logic, borrowed separately from group_file
/// to satisfy the borrow checker.
struct SyncState<'a> {
    instance_id: u32,
    params: &'a GainBrainParams,
    last_seen_generation: &'a mut u32,
    last_sent_gain_millibels: &'a mut i32,
    last_param_value_mb: &'a mut i32,
    last_group: &'a mut i32,
    last_link_mode: &'a mut LinkMode,
    group_gain_override: &'a AtomicI32,
    relative_baseline_mb: &'a mut i32,
    effective_gain_db: &'a mut f32,
}

impl GainBrain {
    /// Run once per buffer to synchronize gain with the shared group file.
    fn sync_group(&mut self) {
        let group_file = match self.group_file.as_mut() {
            Some(gf) => gf,
            None => return,
        };

        let mut state = SyncState {
            instance_id: self.instance_id,
            params: &self.params,
            last_seen_generation: &mut self.last_seen_generation,
            last_sent_gain_millibels: &mut self.last_sent_gain_millibels,
            last_param_value_mb: &mut self.last_param_value_mb,
            last_group: &mut self.last_group,
            last_link_mode: &mut self.last_link_mode,
            group_gain_override: &self.group_gain_override,
            relative_baseline_mb: &mut self.relative_baseline_mb,
            effective_gain_db: &mut self.effective_gain_db,
        };

        let group = state.params.group.value();
        let link_mode = state.params.link_mode.value();

        // Detect parameter transitions.
        let group_changed = group != *state.last_group;
        let mode_changed = link_mode != *state.last_link_mode;

        if group_changed || mode_changed {
            nih_log!("[{}] TRANSITION: group {}→{} mode {:?}→{:?} effective={:.1}dB param={:.1}dB",
                state.instance_id, *state.last_group, group, *state.last_link_mode, link_mode,
                *state.effective_gain_db, util::gain_to_db(state.params.gain.value()));
            Self::handle_transition(&mut state, group_file, group, link_mode);
            nih_log!("[{}] AFTER TRANSITION: baseline={} last_sent={} effective={:.1}dB",
                state.instance_id, *state.relative_baseline_mb,
                *state.last_sent_gain_millibels, *state.effective_gain_db);
            *state.last_group = group;
            *state.last_link_mode = link_mode;
        }

        // Active sync only when group is 1-16.
        if !(1..=16).contains(&group) {
            return;
        }

        let slot = group_file.read_slot(group as u8);

        // ── READ PATH: check for external changes ──────────────────────
        let id = state.instance_id;
        if slot.generation != *state.last_seen_generation
            && slot.gain_millibels != *state.last_sent_gain_millibels
        {
            match link_mode {
                LinkMode::Absolute => {
                    nih_log!("[{id}] READ abs: slot={} gen={} last_sent={} → override",
                        slot.gain_millibels, slot.generation, *state.last_sent_gain_millibels);
                    state
                        .group_gain_override
                        .store(slot.gain_millibels, Ordering::Relaxed);
                    *state.last_sent_gain_millibels = slot.gain_millibels;
                    *state.effective_gain_db = millibels_to_db(slot.gain_millibels);
                }
                LinkMode::Relative => {
                    let delta_mb = slot.gain_millibels - *state.relative_baseline_mb;
                    let current_mb = db_to_millibels(*state.effective_gain_db);
                    let new_mb = current_mb + delta_mb;
                    let clamped_db = clamp_db(millibels_to_db(new_mb));
                    let clamped_mb = db_to_millibels(clamped_db);
                    let slot_db = millibels_to_db(slot.gain_millibels);
                    let new_db = millibels_to_db(clamped_mb);
                    let offset_db = slot_db - new_db;
                    nih_log!("[{id}] READ rel: slot={:.1}dB baseline={} delta={} eff={:.1}→{:.1}dB offset={:.1}dB",
                        slot_db, *state.relative_baseline_mb, delta_mb,
                        millibels_to_db(current_mb), new_db, offset_db);
                    state
                        .group_gain_override
                        .store(clamped_mb, Ordering::Relaxed);
                    *state.relative_baseline_mb = slot.gain_millibels;
                    *state.last_sent_gain_millibels = clamped_mb;
                    *state.effective_gain_db = clamped_db;
                }
            }
            *state.last_seen_generation = slot.generation;
        }

        // ── WRITE PATH: detect user-initiated gain changes ─────────────
        // Compare param value against what it was LAST buffer, not against
        // last_sent. After a group override, the param still holds the old
        // value (we can't set it from the audio thread), so comparing
        // against last_sent would cause us to write the stale param value
        // back to the slot, overwriting the override.
        let current_gain_db = util::gain_to_db(state.params.gain.value());
        let current_mb = db_to_millibels(current_gain_db);

        if current_mb != *state.last_param_value_mb {
            nih_log!("[{id}] WRITE: param changed {}→{} (effective was {:.1})",
                *state.last_param_value_mb, current_mb, *state.effective_gain_db);
            *state.effective_gain_db = current_gain_db;
            group_file.write_slot(group as u8, current_mb);
            *state.last_sent_gain_millibels = current_mb;
            let updated_slot = group_file.read_slot(group as u8);
            *state.last_seen_generation = updated_slot.generation;
        }
        *state.last_param_value_mb = current_mb;
    }

    /// Handle transitions when group or link_mode parameters change.
    fn handle_transition(
        state: &mut SyncState<'_>,
        group_file: &mut groups::GroupFile,
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
        let slot = group_file.read_slot(new_group as u8);
        // Use effective_gain_db (what the instance is actually outputting),
        // NOT params.gain.value() which may be stale after group overrides.
        let effective_mb = db_to_millibels(*state.effective_gain_db);

        let was_active = (1..=16).contains(&old_group);

        if !was_active {
            // Newly joining a group (old group was 0).
            match new_link_mode {
                LinkMode::Absolute => {
                    // Adopt the group's current gain.
                    state
                        .group_gain_override
                        .store(slot.gain_millibels, Ordering::Relaxed);
                    *state.last_sent_gain_millibels = slot.gain_millibels;
                }
                LinkMode::Relative => {
                    // Keep current gain, baseline to group's value.
                    *state.relative_baseline_mb = slot.gain_millibels;
                    *state.last_sent_gain_millibels = effective_mb;
                }
            }
        } else {
            match (old_link_mode, new_link_mode) {
                // Absolute -> Absolute (group changed): adopt new group's gain.
                (LinkMode::Absolute, LinkMode::Absolute) => {
                    state
                        .group_gain_override
                        .store(slot.gain_millibels, Ordering::Relaxed);
                    *state.last_sent_gain_millibels = slot.gain_millibels;
                }
                // Absolute -> Relative: keep current gain, baseline to it.
                (LinkMode::Absolute, LinkMode::Relative) => {
                    *state.relative_baseline_mb = effective_mb;
                    *state.last_sent_gain_millibels = effective_mb;
                }
                // Relative -> Absolute: snap to group's current gain.
                (LinkMode::Relative, LinkMode::Absolute) => {
                    state
                        .group_gain_override
                        .store(slot.gain_millibels, Ordering::Relaxed);
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
        let current_param_db = util::gain_to_db(state.params.gain.value());
        *state.last_param_value_mb = db_to_millibels(current_param_db);

        *state.last_seen_generation = slot.generation;
    }
}

// ── CLAP / VST3 ────────────────────────────────────────────────────────────────

impl ClapPlugin for GainBrain {
    const CLAP_ID: &'static str = "com.mpd.gain-brain";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("A gain utility with cross-instance group linking");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for GainBrain {
    const VST3_CLASS_ID: [u8; 16] = *b"GainBrainMpdPlg\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Fx,
        Vst3SubCategory::Tools,
    ];
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
        assert_eq!(params.link_mode.value(), LinkMode::Absolute);
    }

    #[test]
    fn test_group_default() {
        let params = GainBrainParams::new();
        assert_eq!(params.group.value(), 0);
    }
}
