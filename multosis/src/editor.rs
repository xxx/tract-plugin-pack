//! Softbuffer + tiny-skia CPU editor for Multosis.
//!
//! Milestone 1b-ii-a: opens the window and renders the grid + live playhead.
//! Interaction (cell editing, loop-region drag, toolbar) is Milestone 1b-ii-b.

use baseview::{WindowOpenOptions, WindowScalePolicy};
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::editor::effect_editor::EffectHit;
use crate::editor::toolbar::{ToolbarControl, ToolbarOp};
use crate::effects::{self, EffectKind, ParamSpec};
use crate::grid::LoopRegion;
use crate::handoff::GridHandoff;
use crate::modulation::TriggerSource;
use crate::playhead_display::PlayheadDisplay;
use crate::region::RegionSnapshot;
use crate::seq_status::SeqStatusDisplay;
use crate::undo::{ConfigSnapshot, UndoHistory};
use crate::MultosisParams;
use tiny_skia_widgets as widgets;

pub mod effect_editor;
pub mod grid_view;
pub mod toolbar;
pub mod track_list;

/// Editor window size. Derived from the grid layout in `grid_view`:
/// width  = 2*MARGIN + TRACK_PANEL_W + COLS*CELL + 3*GROUP_GAP
///        = 16 + 120 + 1280 + 24 + 16 = 1456
/// height = STATUS_H + GUTTER + ROWS*CELL + MARGIN = 88 + 14 + 640 + 16 = 758
/// (kept in sync by the `window_size_matches_the_grid` test).
pub const WINDOW_WIDTH: u32 = 1456;
pub const WINDOW_HEIGHT: u32 = 758;

pub use widgets::EditorState;

/// The in-progress left-button gesture on the grid or loop region. The press
/// dispatch in `on_event` selects exactly one; `None` means no left drag.
#[derive(Clone, Copy, Debug)]
enum LeftGesture {
    /// Dragging a loop-region edge or corner to resize it.
    ResizeRegion(grid_view::RegionHandle),
    /// A left press on a grid cell that has not moved yet — a click in
    /// waiting. Becomes a click on release, or a paint drag if the cursor
    /// leaves the cell (later task).
    GridPending { row: usize, col: usize },
    /// An active paint drag — `value` is the `enabled` state being painted
    /// across the stroke, `last` is the most recently painted cell.
    GridPaint { value: bool, last: (usize, usize) },
    /// An active loop-region move — `press` is the cursor position when the
    /// grip was grabbed, `region_at_press` is the region geometry then.
    MoveRegion {
        press: (f32, f32),
        region_at_press: LoopRegion,
    },
}

/// Which screen the window's main area shows. The toolbar and track listing
/// are drawn in both; only the main area to the right of the panel swaps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Grid,
    Effect,
}

/// Which dropdown an `EffectAction`-tagged `DropdownState` event refers to.
/// The effect-kind, modulation-target, and per-track trigger-source dropdowns
/// share a single `DropdownState<EffectAction>` — only one is open at a time,
/// and the payload distinguishes which trigger opened it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EffectAction {
    Kind,
    Target,
    Trigger,
}

/// Clamp a candidate selected-track index into `0..ROWS`.
fn clamp_track(row: usize) -> usize {
    row.min(crate::grid::ROWS - 1)
}

/// Time window for double-click-to-reset on sliders/dials.
const DOUBLE_CLICK_MS: u128 = 400;

/// Two consecutive presses on the same control within `DOUBLE_CLICK_MS`
/// register as a double-click. The control's identity is carried in `A`.
struct ClickTracker<A: Copy + PartialEq> {
    last_time: std::time::Instant,
    last_action: Option<A>,
}

impl<A: Copy + PartialEq> ClickTracker<A> {
    fn new() -> Self {
        Self {
            last_time: std::time::Instant::now(),
            last_action: None,
        }
    }

    /// Record this press; return `true` if it is a double-click on the same
    /// control as the previous press within the time window.
    fn check_and_update(&mut self, action: A) -> bool {
        let now = std::time::Instant::now();
        let is_double = self.last_action == Some(action)
            && now.duration_since(self.last_time).as_millis() < DOUBLE_CLICK_MS;
        self.last_time = now;
        self.last_action = Some(action);
        is_double
    }
}

/// The baseview window handler — owns the surface and draws each frame.
struct MultosisWindow {
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    /// Packed `(w << 32) | h` pending host-initiated resize, read next frame.
    pending_resize: Arc<AtomicU64>,
    params: Arc<MultosisParams>,
    playhead_display: Arc<PlayheadDisplay>,
    seq_status: Arc<SeqStatusDisplay>,
    grid_handoff: Arc<GridHandoff>,
    /// Audio→GUI mirror of the engine's active-row mask. Task 3 draws with it.
    active_rows: Arc<AtomicU16>,
    /// Audio→GUI mirror of every MSEG's free-running phase, as f32 bit-patterns.
    /// Slot `row*3 + k`. The editor's playhead overlay reads the active one
    /// each frame.
    mseg_phases: Arc<[AtomicU32; 48]>,
    /// Latest cursor position in physical pixels, updated on CursorMoved.
    mouse_pos: (f32, f32),
    text_renderer: widgets::TextRenderer,
    gui_context: Arc<dyn GuiContext>,
    reset_request: Arc<AtomicBool>,
    toolbar_drag: widgets::DragState<ToolbarControl>,
    /// The loop-region clipboard for Copy/Paste.
    clipboard: Option<RegionSnapshot>,
    /// Seed advanced on each randomize op so successive clicks differ.
    rng_seed: u32,
    /// The active left-button gesture, if any.
    left_gesture: Option<LeftGesture>,
    /// Cached render of the grid cells — re-renders only changed cells each frame.
    grid_cache: grid_view::GridCache,
    /// Which screen is showing.
    view: View,
    /// The track the Effect view edits (`0..ROWS`).
    selected_track: usize,
    /// Which MSEG slot the modulation section currently edits (0..3); 0 is the
    /// always-on amplitude MSEG, 1/2 are assignable. Read by `effect_hit`; the
    /// MODULATION-section UI is wired in Task 9.
    selected_mseg: usize,
    /// The effect editor's shared dropdown state — owns the open Kind / Target / Trigger popup.
    effect_dropdown: widgets::dropdown::DropdownState<EffectAction>,
    /// The effect editor's parameter-dial drag state — one in-flight dial drag
    /// at a time, tagged with the slot index via `EffectHit::Dial(i)` or by
    /// `EffectHit::Depth` for the modulation depth dial.
    effect_dial_drag: widgets::DragState<EffectHit>,
    /// Right-click text-entry state for the effect-param dials. Only
    /// `EffectHit::Dial(i)` is ever begun on this; the depth and trigger-rate
    /// dials stay drag-only.
    text_edit: widgets::TextEditState<EffectHit>,
    /// MSEG editor state — owns hover/drag/last-node info for the active MSEG.
    /// Full-editor mode: the strip (sync/length/play-mode/randomize/style) is
    /// drawn inside the MSEG pane and routed through the widget's own handlers.
    mseg_edit: widgets::mseg::MsegEditState,
    /// Undo/redo history for the DAW-opaque config. Window-scoped — created
    /// fresh on window open, dropped on close.
    undo: UndoHistory<ConfigSnapshot>,
    /// Timestamp of the last left press on the MSEG pane, for double-click
    /// detection (~400 ms / ~8 px).
    mseg_last_click_time: std::time::Instant,
    /// Position of the last left press on the MSEG pane, for double-click
    /// detection.
    mseg_last_click_pos: (f32, f32),
    /// Audio→GUI dirty flag: every edit (param dial, kind switch, …) sets it
    /// so `process()` re-bridges the persisted config into the engine.
    config_dirty: Arc<AtomicBool>,
    /// Double-click detector for the toolbar Mix / Output sliders.
    toolbar_click: ClickTracker<ToolbarControl>,
    /// Double-click detector for the effect-editor parameter / depth /
    /// trigger-rate dials.
    effect_click: ClickTracker<EffectHit>,
}

impl MultosisWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut baseview::Window<'_>,
        params: Arc<MultosisParams>,
        playhead_display: Arc<PlayheadDisplay>,
        seq_status: Arc<SeqStatusDisplay>,
        grid_handoff: Arc<GridHandoff>,
        pending_resize: Arc<AtomicU64>,
        gui_context: Arc<dyn GuiContext>,
        reset_request: Arc<AtomicBool>,
        active_rows: Arc<AtomicU16>,
        mseg_phases: Arc<[AtomicU32; 48]>,
        config_dirty: Arc<AtomicBool>,
        scale_factor: f32,
    ) -> Self {
        let pw = (WINDOW_WIDTH as f32 * scale_factor).round() as u32;
        let ph = (WINDOW_HEIGHT as f32 * scale_factor).round() as u32;
        let surface = widgets::SoftbufferSurface::new(window, pw, ph);
        let text_renderer = widgets::TextRenderer::new(include_bytes!("fonts/DejaVuSans.ttf"));
        Self {
            surface,
            physical_width: pw,
            physical_height: ph,
            scale_factor,
            pending_resize,
            params,
            playhead_display,
            seq_status,
            grid_handoff,
            active_rows,
            mseg_phases,
            toolbar_click: ClickTracker::new(),
            effect_click: ClickTracker::new(),
            mouse_pos: (0.0, 0.0),
            text_renderer,
            gui_context,
            reset_request,
            toolbar_drag: widgets::DragState::new(),
            clipboard: None,
            rng_seed: 1,
            left_gesture: None,
            grid_cache: grid_view::GridCache::new(pw, ph),
            view: View::Grid,
            selected_track: 0,
            selected_mseg: 0,
            effect_dropdown: widgets::dropdown::DropdownState::new(),
            effect_dial_drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
            mseg_edit: widgets::mseg::MsegEditState::new(),
            undo: UndoHistory::new(),
            mseg_last_click_time: std::time::Instant::now(),
            mseg_last_click_pos: (-999.0, -999.0),
            config_dirty,
        }
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.grid_cache = grid_view::GridCache::new(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }

    /// Toggle the `enabled` flag of cell `(row, col)` and republish the grid.
    fn commit_click(&mut self, row: usize, col: usize) {
        if let Ok(mut grid) = self.params.grid.lock() {
            grid_view::apply_grid_click(&mut grid, row, col);
            self.grid_handoff.publish(*grid);
        }
    }

    /// Set `enabled = value` on every given cell and republish the grid.
    fn paint_cells(&mut self, value: bool, cells: &[(usize, usize)]) {
        if cells.is_empty() {
            return;
        }
        if let Ok(mut grid) = self.params.grid.lock() {
            for &(row, col) in cells {
                grid.cell_mut(row, col).enabled = value;
            }
            self.grid_handoff.publish(*grid);
        }
    }

    /// The loop-region resize handle under the cursor, if the cursor is over
    /// one.
    fn region_handle_under_cursor(&self) -> Option<grid_view::RegionHandle> {
        let (px, py) = self.mouse_pos;
        let region = self.params.grid.lock().ok()?.loop_region;
        grid_view::region_handle_hit(px, py, region, self.scale_factor)
    }

    /// If the cursor is over the loop region's move grip, begin a move
    /// gesture and return `true`.
    fn try_begin_region_move(&mut self) -> bool {
        let region = match self.params.grid.lock() {
            Ok(grid) => grid.loop_region,
            Err(_) => return false,
        };
        let (px, py) = self.mouse_pos;
        if grid_view::region_grip_hit(px, py, region, self.scale_factor) {
            self.left_gesture = Some(LeftGesture::MoveRegion {
                press: self.mouse_pos,
                region_at_press: region,
            });
            true
        } else {
            false
        }
    }

    /// Translate the loop region for the in-progress move and republish.
    fn update_region_move(&mut self, press: (f32, f32), region_at_press: LoopRegion) {
        let scale = self.scale_factor;
        let (px, py) = self.mouse_pos;
        let drow = grid_view::row_at(py, scale) as i32 - grid_view::row_at(press.1, scale) as i32;
        let dcol =
            grid_view::column_at(px, scale) as i32 - grid_view::column_at(press.0, scale) as i32;
        if let Ok(mut grid) = self.params.grid.lock() {
            grid.loop_region = grid_view::apply_region_move(region_at_press, drow, dcol);
            self.grid_handoff.publish(*grid);
        }
    }

    /// Resize the loop region for the in-progress drag of `handle`, then
    /// republish the grid.
    fn update_region_drag(&mut self, handle: grid_view::RegionHandle) {
        let (px, py) = self.mouse_pos;
        let scale = self.scale_factor;
        if let Ok(mut grid) = self.params.grid.lock() {
            grid.loop_region = match handle {
                grid_view::RegionHandle::Edge(edge) => {
                    let index = match edge {
                        grid_view::RegionEdge::Left | grid_view::RegionEdge::Right => {
                            grid_view::column_at(px, scale)
                        }
                        grid_view::RegionEdge::Top | grid_view::RegionEdge::Bottom => {
                            grid_view::row_at(py, scale)
                        }
                    };
                    grid_view::apply_region_drag(grid.loop_region, edge, index)
                }
                grid_view::RegionHandle::Corner(corner) => grid_view::apply_region_corner_drag(
                    grid.loop_region,
                    corner,
                    grid_view::row_at(py, scale),
                    grid_view::column_at(px, scale),
                ),
            };
            self.grid_handoff.publish(*grid);
        }
    }

    /// Handle a left click on a non-slider toolbar control.
    fn handle_toolbar_button(&mut self, ctrl: ToolbarControl) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        match ctrl {
            ToolbarControl::Speed => {
                // Cycle to the next speed division.
                let all = crate::clock::Speed::ALL;
                let cur = self.params.speed.value();
                let idx = all.iter().position(|&s| s == cur).unwrap_or(0);
                let next = all[(idx + 1) % all.len()];
                setter.begin_set_parameter(&self.params.speed);
                setter.set_parameter(&self.params.speed, next);
                setter.end_set_parameter(&self.params.speed);
            }
            ToolbarControl::Reset => {
                self.reset_request
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            // Slider drags are begun in on_event's ButtonPressed arm.
            ToolbarControl::Mix
            | ToolbarControl::Output
            | ToolbarControl::CompThreshold
            | ToolbarControl::CompRatio => {}
        }
    }

    /// Handle a left click on a lower-row operation button.
    fn handle_toolbar_op(&mut self, op: ToolbarOp) {
        let Ok(mut grid) = self.params.grid.lock() else {
            return;
        };
        match op {
            ToolbarOp::Copy => {
                // Copy snapshots the loop region; it does not change the grid.
                self.clipboard = Some(grid.copy_region());
                return;
            }
            ToolbarOp::Paste => {
                if let Some(snap) = &self.clipboard {
                    grid.paste_region(snap);
                }
            }
            other => {
                toolbar::apply_grid_op(&mut grid, other, self.rng_seed);
                self.rng_seed = self.rng_seed.wrapping_add(1);
            }
        }
        // Paste / Reinit / Randomize all changed the grid — republish.
        self.grid_handoff.publish(*grid);
    }

    /// The current normalized value of a slider control.
    fn slider_normalized(&self, ctrl: ToolbarControl) -> f32 {
        match ctrl {
            ToolbarControl::Mix => self.params.mix.unmodulated_normalized_value(),
            ToolbarControl::Output => self.params.output_gain.unmodulated_normalized_value(),
            ToolbarControl::CompThreshold => {
                self.params.comp_threshold.unmodulated_normalized_value()
            }
            ToolbarControl::CompRatio => self.params.comp_ratio.unmodulated_normalized_value(),
            _ => 0.0,
        }
    }

    /// Begin a host parameter gesture for a slider control.
    fn begin_slider(&self, ctrl: ToolbarControl) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        match ctrl {
            ToolbarControl::Mix => setter.begin_set_parameter(&self.params.mix),
            ToolbarControl::Output => setter.begin_set_parameter(&self.params.output_gain),
            ToolbarControl::CompThreshold => {
                setter.begin_set_parameter(&self.params.comp_threshold)
            }
            ToolbarControl::CompRatio => setter.begin_set_parameter(&self.params.comp_ratio),
            _ => {}
        }
    }

    /// End a host parameter gesture for a slider control.
    fn end_slider(&self, ctrl: ToolbarControl) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        match ctrl {
            ToolbarControl::Mix => setter.end_set_parameter(&self.params.mix),
            ToolbarControl::Output => setter.end_set_parameter(&self.params.output_gain),
            ToolbarControl::CompThreshold => setter.end_set_parameter(&self.params.comp_threshold),
            ToolbarControl::CompRatio => setter.end_set_parameter(&self.params.comp_ratio),
            _ => {}
        }
    }

    /// Reset a toolbar slider to its default value (double-click handler).
    /// Mix → 100%; Output → 0 dB; Comp Threshold → −6 dB; Comp Ratio → 4:1.
    fn reset_toolbar_slider(&self, ctrl: ToolbarControl) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        match ctrl {
            ToolbarControl::Mix => {
                setter.begin_set_parameter(&self.params.mix);
                setter.set_parameter(&self.params.mix, 1.0);
                setter.end_set_parameter(&self.params.mix);
            }
            ToolbarControl::Output => {
                setter.begin_set_parameter(&self.params.output_gain);
                setter.set_parameter(&self.params.output_gain, nih_plug::util::db_to_gain(0.0));
                setter.end_set_parameter(&self.params.output_gain);
            }
            ToolbarControl::CompThreshold => {
                setter.begin_set_parameter(&self.params.comp_threshold);
                setter.set_parameter(&self.params.comp_threshold, -6.0);
                setter.end_set_parameter(&self.params.comp_threshold);
            }
            ToolbarControl::CompRatio => {
                setter.begin_set_parameter(&self.params.comp_ratio);
                setter.set_parameter(&self.params.comp_ratio, 4.0);
                setter.end_set_parameter(&self.params.comp_ratio);
            }
            _ => {}
        }
    }

    /// Set a slider control to a normalized value mid-gesture. `begin_slider`
    /// and `end_slider` bracket the whole drag.
    fn set_slider(&self, ctrl: ToolbarControl, norm: f32) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        match ctrl {
            ToolbarControl::Mix => setter.set_parameter_normalized(&self.params.mix, norm),
            ToolbarControl::Output => {
                setter.set_parameter_normalized(&self.params.output_gain, norm)
            }
            ToolbarControl::CompThreshold => {
                setter.set_parameter_normalized(&self.params.comp_threshold, norm)
            }
            ToolbarControl::CompRatio => {
                setter.set_parameter_normalized(&self.params.comp_ratio, norm)
            }
            _ => {}
        }
    }

    /// The track-effect kinds, for the track listing. Reads the persisted
    /// config; falls back to row defaults on lock contention.
    fn track_kinds(&self) -> [crate::effects::EffectKind; crate::grid::ROWS] {
        if let Ok(cfg) = self.params.track_effects.lock() {
            std::array::from_fn(|r| cfg[r].kind)
        } else {
            std::array::from_fn(|r| crate::effects::TrackEffect::default_for_row(r).kind)
        }
    }

    /// Mark the persisted effect/modulation config dirty so the audio thread
    /// re-bridges it on the next process block.
    fn mark_config_dirty(&self) {
        self.config_dirty.store(true, Ordering::Relaxed);
    }

    /// Snapshot the current DAW-opaque config (grid, effects, modulation).
    fn snapshot(&self) -> ConfigSnapshot {
        ConfigSnapshot::capture(&self.params)
    }

    /// Undo the last captured edit, if any: restore the config, drop the now
    /// stale MSEG node selection, and mark the config dirty for the audio
    /// thread to re-bridge.
    fn do_undo(&mut self) {
        // A mouse gesture is mid-flight — its capture window is open. Leave
        // the in-progress edit atomic; ignore the undo until the gesture ends.
        if self.undo.is_capturing() {
            return;
        }
        let current = self.snapshot();
        if let Some(snap) = self.undo.undo(current) {
            snap.restore(&self.params);
            self.mseg_edit.clear_selection();
            self.mark_config_dirty();
        }
    }

    /// Redo the last undone edit, if any.
    fn do_redo(&mut self) {
        // A mouse gesture is mid-flight — its capture window is open. Leave
        // the in-progress edit atomic; ignore the undo until the gesture ends.
        if self.undo.is_capturing() {
            return;
        }
        let current = self.snapshot();
        if let Some(snap) = self.undo.redo(current) {
            snap.restore(&self.params);
            self.mseg_edit.clear_selection();
            self.mark_config_dirty();
        }
    }

    /// Handle a right-button press in the effect view. Returns the event
    /// status. Extracted from `on_event` so the undo-capture bracket has a
    /// single exit point.
    fn on_right_press(&mut self) -> baseview::EventStatus {
        let (px, py) = self.mouse_pos;
        if self.effect_dial_drag.active_action().is_some() {
            return baseview::EventStatus::Captured;
        }
        // First, hit-test for a param dial — only `EffectHit::Dial(i)`
        // gets text entry. Depth, trigger, and trigger-rate stay
        // drag-only.
        let param_count = self.selected_track_param_count();
        let trigger = self.selected_track_modulation().trigger;
        let is_free_hz = matches!(trigger, TriggerSource::FreeHz { .. });
        let hit = effect_editor::effect_hit(
            px,
            py,
            self.scale_factor,
            param_count,
            self.selected_mseg,
            is_free_hz,
        );
        if let Some(EffectHit::Dial(i)) = hit {
            // Right-clicking a *different* dial while one is already
            // being edited would silently discard the prior edit if we
            // jumped straight to `begin` (which clears the buffer).
            // Commit-and-apply the prior edit first.
            match self.text_edit.commit() {
                Some((EffectHit::Dial(prev), text)) => self.commit_dial_text_edit(prev, &text),
                Some((EffectHit::Mix, text)) => self.commit_mix_text_edit(&text),
                _ => {}
            }
            if let Some(spec) = self.param_spec(i) {
                let value = self.selected_track_effect().params[i];
                self.text_edit.begin(
                    EffectHit::Dial(i),
                    &effects::format_value_bare(value, spec.format),
                );
            }
            return baseview::EventStatus::Captured;
        }
        if let Some(EffectHit::Mix) = hit {
            // Commit any prior edit before seeding a new one.
            match self.text_edit.commit() {
                Some((EffectHit::Dial(prev), text)) => self.commit_dial_text_edit(prev, &text),
                Some((EffectHit::Mix, text)) => self.commit_mix_text_edit(&text),
                _ => {}
            }
            let pct = (self.selected_track_effect().mix * 100.0).round() as i32;
            self.text_edit.begin(EffectHit::Mix, &format!("{pct}"));
            return baseview::EventStatus::Captured;
        }
        let lay = effect_editor::effect_layout(self.scale_factor);
        if effect_editor::in_rect(lay.mseg_pane, px, py) {
            let sel = self.selected_mseg.min(2);
            let changed = if let Ok(mut modu) = self.params.track_modulation.lock() {
                let row = self.selected_track;
                self.mseg_edit.on_right_click(
                    px,
                    py,
                    &mut modu[row].msegs[sel],
                    lay.mseg_pane,
                    self.scale_factor,
                )
            } else {
                None
            };
            if changed == Some(widgets::mseg::MsegEdit::Changed) {
                self.mark_config_dirty();
            }
        }
        baseview::EventStatus::Captured
    }

    /// The persisted `TrackEffect` for the currently selected track, or its
    /// row default if the mutex is contended.
    fn selected_track_effect(&self) -> crate::effects::TrackEffect {
        let row = self.selected_track;
        if let Ok(cfg) = self.params.track_effects.lock() {
            cfg[row]
        } else {
            crate::effects::TrackEffect::default_for_row(row)
        }
    }

    /// The persisted `TrackModulation` for the currently selected track, or
    /// its row default if the mutex is contended.
    fn selected_track_modulation(&self) -> crate::modulation::TrackModulation {
        let row = self.selected_track;
        if let Ok(cfg) = self.params.track_modulation.lock() {
            cfg[row].clone()
        } else {
            crate::modulation::TrackModulation::default_for_row(row)
        }
    }

    /// Check if `(x, y)` is a double-click on the MSEG pane. Records the click
    /// position/time and returns `true` if the previous click was within
    /// 400 ms and within 8 pixels.
    fn mseg_double_click_check(&mut self, x: f32, y: f32) -> bool {
        let now = std::time::Instant::now();
        let elapsed_ms = now.duration_since(self.mseg_last_click_time).as_millis();
        let (px, py) = self.mseg_last_click_pos;
        let dist_sq = (x - px) * (x - px) + (y - py) * (y - py);
        let is_double = elapsed_ms < 400 && dist_sq < 64.0;
        self.mseg_last_click_time = now;
        self.mseg_last_click_pos = (x, y);
        is_double
    }

    /// Draw the effect editor (right of the track panel). The toolbar and
    /// track listing are drawn separately in `draw`.
    fn draw_effect_view(&mut self) {
        let track = self.selected_track_effect();
        let lay = effect_editor::effect_layout(self.scale_factor);
        // If a dial-text edit is active, hand the buffer + caret state to the
        // section drawer so it can render in place of the formatted value.
        let editing_dial: Option<(usize, &str, bool)> = match self.text_edit.active_for_any() {
            Some(EffectHit::Dial(i)) => {
                let caret_on = self.text_edit.caret_visible();
                self.text_edit
                    .active_for(&EffectHit::Dial(i))
                    .map(|buf| (i, buf, caret_on))
            }
            _ => None,
        };
        let editing_mix: Option<(&str, bool)> = match self.text_edit.active_for_any() {
            Some(EffectHit::Mix) => {
                let caret_on = self.text_edit.caret_visible();
                self.text_edit
                    .active_for(&EffectHit::Mix)
                    .map(|buf| (buf, caret_on))
            }
            _ => None,
        };
        effect_editor::draw_effect_section(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &track,
            self.selected_track,
            self.effect_dropdown.is_open_for(EffectAction::Kind),
            editing_dial,
            editing_mix,
            self.scale_factor,
        );
        effect_editor::draw_section_header(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            lay.effect_header,
            "EFFECT",
            self.scale_factor,
        );
        effect_editor::draw_section_header(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            lay.modulation_header,
            "MODULATION",
            self.scale_factor,
        );
        // MODULATION section.
        let modu = self.selected_track_modulation();
        let sel = self.selected_mseg.min(2);
        // Active MSEG first — `draw_mseg` opens with an opaque canvas fill
        // that would otherwise wipe the ghosts. Ghosts are then drawn on top
        // at low alpha (≈ 38%) so they read as faint context, not foreground.
        widgets::mseg::draw_mseg(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            lay.mseg_pane,
            &modu.msegs[sel],
            &self.mseg_edit,
            self.scale_factor,
        );
        for m in 0..3 {
            if m != sel {
                widgets::mseg::draw_mseg_ghost(
                    &mut self.surface.pixmap,
                    lay.mseg_pane,
                    &modu.msegs[m],
                    &self.mseg_edit,
                    self.scale_factor,
                    0x5A504060,
                );
            }
        }
        // Playhead overlay: a thin vertical line at the active MSEG's current
        // phase, drawn last so it sits over the curve.
        let phase =
            f32::from_bits(self.mseg_phases[self.selected_track * 3 + sel].load(Ordering::Relaxed));
        effect_editor::draw_mseg_playhead(
            &mut self.surface.pixmap,
            lay.mseg_pane,
            phase,
            self.scale_factor,
        );
        // Selector + target + depth (when on an assignable MSEG).
        let (target, depth) = if sel == 0 {
            (None, 0.0)
        } else {
            let k = sel - 1;
            (modu.targets[k], modu.depths[k])
        };
        effect_editor::draw_modulation_controls(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            sel,
            track.kind,
            target,
            depth,
            self.effect_dropdown.is_open_for(EffectAction::Target),
            self.scale_factor,
        );
        // Per-track trigger control + (conditionally) rate dial.
        let trigger = self.selected_track_modulation().trigger;
        effect_editor::draw_trigger_controls(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            trigger,
            self.effect_dropdown.is_open_for(EffectAction::Trigger),
            self.scale_factor,
        );
        // Active MSEG's sync/length controls (sit on the modulation row to
        // the right of the depth dial).
        effect_editor::draw_mseg_controls(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &modu.msegs[sel],
            self.scale_factor,
        );
    }

    /// Handle a left press while in `View::Effect`. Returns `true` if the
    /// press hit a control owned by the effect editor (so the caller can stop
    /// further routing). `ctrl` toggles MSEG node selection.
    fn on_effect_press(&mut self, px: f32, py: f32, ctrl: bool) -> bool {
        let params = self.selected_track_param_count();
        let trigger = self.selected_track_modulation().trigger;
        let is_free_hz = matches!(trigger, TriggerSource::FreeHz { .. });
        let Some(hit) = effect_editor::effect_hit(
            px,
            py,
            self.scale_factor,
            params,
            self.selected_mseg,
            is_free_hz,
        ) else {
            return false;
        };
        let lay = effect_editor::effect_layout(self.scale_factor);
        match hit {
            EffectHit::Back => {
                self.view = View::Grid;
            }
            EffectHit::Kind => {
                let items: Vec<&'static str> = effect_editor::kind_items();
                let current = EffectKind::ALL
                    .iter()
                    .position(|k| *k == self.selected_track_effect().kind)
                    .unwrap_or(0);
                let win = (self.physical_width as f32, self.physical_height as f32);
                self.effect_dropdown.open(
                    EffectAction::Kind,
                    lay.kind,
                    &items,
                    current,
                    false,
                    win,
                );
            }
            EffectHit::Dial(i) => {
                if self.effect_click.check_and_update(EffectHit::Dial(i)) {
                    self.reset_effect_dial_to_default(i);
                } else if let Some(spec) = self.param_spec(i) {
                    let value = self.selected_track_effect().params[i];
                    let norm = effects::value_to_norm(value, spec.min, spec.max, spec.scaling);
                    self.effect_dial_drag
                        .begin_drag(EffectHit::Dial(i), norm, false);
                }
            }
            EffectHit::MsegSelector(seg) => {
                let old = self.selected_mseg;
                self.selected_mseg = seg.min(2);
                // Only clear when the selection actually changed; re-clicking
                // the active MSEG must not discard the node selection.
                if self.selected_mseg != old {
                    self.mseg_edit.clear_selection();
                }
            }
            EffectHit::Target => {
                let kind = self.selected_track_effect().kind;
                let items: Vec<&'static str> = effect_editor::target_items(kind);
                let current = if self.selected_mseg == 0 {
                    0
                } else {
                    let modu = self.selected_track_modulation();
                    effect_editor::target_to_item(modu.targets[self.selected_mseg - 1])
                };
                let win = (self.physical_width as f32, self.physical_height as f32);
                self.effect_dropdown.open(
                    EffectAction::Target,
                    lay.target,
                    &items,
                    current,
                    false,
                    win,
                );
            }
            EffectHit::Depth => {
                if self.selected_mseg != 0 {
                    if self.effect_click.check_and_update(EffectHit::Depth) {
                        self.reset_depth_to_default();
                    } else {
                        let modu = self.selected_track_modulation();
                        let depth = modu.depths[self.selected_mseg - 1];
                        let norm = ((depth + 1.0) / 2.0).clamp(0.0, 1.0);
                        self.effect_dial_drag
                            .begin_drag(EffectHit::Depth, norm, false);
                    }
                }
            }
            EffectHit::MsegPane => {
                let sel = self.selected_mseg.min(2);
                let is_double = self.mseg_double_click_check(px, py);
                let changed = if let Ok(mut modu) = self.params.track_modulation.lock() {
                    let row = self.selected_track;
                    if is_double {
                        self.mseg_edit.on_double_click(
                            px,
                            py,
                            &mut modu[row].msegs[sel],
                            lay.mseg_pane,
                            self.scale_factor,
                        )
                    } else {
                        self.mseg_edit.on_mouse_down(
                            px,
                            py,
                            &mut modu[row].msegs[sel],
                            lay.mseg_pane,
                            self.scale_factor,
                            ctrl,
                        )
                    }
                } else {
                    None
                };
                if changed == Some(widgets::mseg::MsegEdit::Changed) {
                    self.mark_config_dirty();
                }
            }
            EffectHit::Trigger => {
                let trigger = self.selected_track_modulation().trigger;
                let current = effect_editor::trigger_to_item(trigger);
                let items = effect_editor::trigger_items();
                let win = (self.physical_width as f32, self.physical_height as f32);
                self.effect_dropdown.open(
                    EffectAction::Trigger,
                    lay.trigger,
                    &items,
                    current,
                    false,
                    win,
                );
            }
            EffectHit::TriggerRate => {
                let trigger = self.selected_track_modulation().trigger;
                if let TriggerSource::FreeHz { hz } = trigger {
                    if self.effect_click.check_and_update(EffectHit::TriggerRate) {
                        self.reset_trigger_rate_to_default();
                    } else {
                        let current_norm = effects::value_to_norm(
                            hz,
                            effect_editor::TRIGGER_RATE_MIN_HZ,
                            effect_editor::TRIGGER_RATE_MAX_HZ,
                            effects::ParamScaling::Log,
                        );
                        self.effect_dial_drag.begin_drag(
                            EffectHit::TriggerRate,
                            current_norm,
                            false,
                        );
                    }
                } else {
                    return false;
                }
            }
            EffectHit::MsegSync(seg) => {
                self.apply_mseg_sync(seg);
            }
            EffectHit::MsegLength => {
                let norm = self.active_mseg_length_norm();
                self.effect_dial_drag
                    .begin_drag(EffectHit::MsegLength, norm, false);
            }
            EffectHit::Mix => {
                if self.effect_click.check_and_update(EffectHit::Mix) {
                    self.reset_mix_to_default();
                } else {
                    let norm = self.selected_track_effect().mix;
                    self.effect_dial_drag
                        .begin_drag(EffectHit::Mix, norm, false);
                }
            }
        }
        true
    }

    /// The normalized 0..1 position of the active MSEG's length slider, given
    /// its current sync mode (the slider range and scaling depend on it).
    fn active_mseg_length_norm(&self) -> f32 {
        let modu = self.selected_track_modulation();
        let sel = self.selected_mseg.min(2);
        let mseg = modu.msegs[sel];
        match mseg.sync_mode {
            tiny_skia_widgets::SyncMode::Time => effects::value_to_norm(
                mseg.time_seconds,
                effect_editor::MSEG_LENGTH_TIME_MIN,
                effect_editor::MSEG_LENGTH_TIME_MAX,
                effects::ParamScaling::Log,
            ),
            tiny_skia_widgets::SyncMode::Beat => effect_editor::beats_value_to_norm(mseg.beats),
        }
    }

    /// Switch the active MSEG's sync mode to `seg` (0 = Time, 1 = Beat).
    /// Both `time_seconds` and `beats` persist independently on the MsegData;
    /// switching just changes which field the engine consults.
    fn apply_mseg_sync(&mut self, seg: usize) {
        let new_mode = if seg == 0 {
            tiny_skia_widgets::SyncMode::Time
        } else {
            tiny_skia_widgets::SyncMode::Beat
        };
        let row = self.selected_track;
        let k = self.selected_mseg.min(2);
        if let Ok(mut modu) = self.params.track_modulation.lock() {
            modu[row].msegs[k].sync_mode = new_mode;
        }
        self.mark_config_dirty();
    }

    /// Apply a length-slider drag value (norm) to the active MSEG. Writes
    /// `beats` or `time_seconds` depending on the active sync mode.
    fn apply_mseg_length_drag(&mut self, norm: f32) {
        let row = self.selected_track;
        let k = self.selected_mseg.min(2);
        if let Ok(mut modu) = self.params.track_modulation.lock() {
            let mseg = &mut modu[row].msegs[k];
            match mseg.sync_mode {
                tiny_skia_widgets::SyncMode::Time => {
                    mseg.time_seconds = effects::norm_to_value(
                        norm,
                        effect_editor::MSEG_LENGTH_TIME_MIN,
                        effect_editor::MSEG_LENGTH_TIME_MAX,
                        effects::ParamScaling::Log,
                    );
                }
                tiny_skia_widgets::SyncMode::Beat => {
                    mseg.beats = effect_editor::beats_norm_to_value(norm);
                }
            }
        }
        self.mark_config_dirty();
    }

    /// The parameter count of the currently selected track's effect kind.
    fn selected_track_param_count(&self) -> usize {
        crate::effects::param_count(self.selected_track_effect().kind)
    }

    /// The `ParamSpec` for slot `i` of the currently selected track's effect
    /// kind, or `None` if the slot is out of range.
    fn param_spec(&self, i: usize) -> Option<ParamSpec> {
        use crate::effects::Effect;
        let kind = self.selected_track_effect().kind;
        let instance = crate::effects::EffectInstance::new(kind);
        instance.parameters().get(i).copied()
    }

    /// Parse the typed text for dial `i`, clamp to the param's range, write
    /// to the persisted config, and mark dirty. Parse failure is a silent
    /// no-op — the dial keeps its previous value.
    fn commit_dial_text_edit(&mut self, i: usize, text: &str) {
        let Some(spec) = self.param_spec(i) else {
            return;
        };
        let Some(v) = effects::parse_value(text, spec.format) else {
            return;
        };
        let clamped = v.clamp(spec.min, spec.max);
        if let Ok(mut cfg) = self.params.track_effects.lock() {
            cfg[self.selected_track].params[i] = clamped;
        }
        self.mark_config_dirty();
    }

    /// Parse a Mix-dial text entry (a percentage number, e.g. `50`), clamp
    /// to 0..100, and write it as a 0..1 mix. A parse failure is a silent
    /// no-op — the dial keeps its previous value.
    fn commit_mix_text_edit(&mut self, text: &str) {
        let cleaned = text.trim();
        if let Ok(pct) = cleaned.parse::<f32>() {
            let value = (pct / 100.0).clamp(0.0, 1.0);
            if let Ok(mut cfg) = self.params.track_effects.lock() {
                cfg[self.selected_track].mix = value;
            }
            self.mark_config_dirty();
        }
    }

    /// If a dial-text edit is active, decide whether to commit or cancel it
    /// based on the incoming press location. A press on the active dial
    /// cancels (the user wants to drag); a press anywhere else commits.
    fn finalize_dial_edit_for_press(&mut self, px: f32, py: f32) {
        let Some(active) = self.text_edit.active_for_any() else {
            return;
        };
        // Resolve the press to an effect-editor hit, if any.
        let trigger = self.selected_track_modulation().trigger;
        let is_free_hz = matches!(trigger, TriggerSource::FreeHz { .. });
        let param_count = self.selected_track_param_count();
        let hit = effect_editor::effect_hit(
            px,
            py,
            self.scale_factor,
            param_count,
            self.selected_mseg,
            is_free_hz,
        );
        if hit == Some(active) {
            // Click on the active dial → cancel; the normal effect-press path
            // then starts a drag.
            self.text_edit.cancel();
        } else {
            match self.text_edit.commit() {
                Some((EffectHit::Dial(i), text)) => self.commit_dial_text_edit(i, &text),
                Some((EffectHit::Mix, text)) => self.commit_mix_text_edit(&text),
                // None: no edit was active — cancel() is a no-op. A new text-editable
                // EffectHit variant needs its own Some(...) arm above, not this.
                _ => self.text_edit.cancel(),
            }
        }
    }

    /// Apply a dial drag's new normalized value to slot `i` of the currently
    /// selected track's effect, marking config dirty.
    fn apply_effect_dial(&mut self, i: usize, norm: f32) {
        let Some(spec) = self.param_spec(i) else {
            return;
        };
        let value = effects::norm_to_value(norm, spec.min, spec.max, spec.scaling);
        if let Ok(mut cfg) = self.params.track_effects.lock() {
            cfg[self.selected_track].params[i] = value;
        }
        self.mark_config_dirty();
    }

    /// Apply a kind switch for the currently selected track: replace the kind,
    /// reset the params to its defaults, and clamp the track's modulation
    /// targets to the new arity. Marks config dirty.
    fn apply_kind_switch(&mut self, kind: EffectKind) {
        let row = self.selected_track;
        if let (Ok(mut eff), Ok(mut modu)) = (
            self.params.track_effects.lock(),
            self.params.track_modulation.lock(),
        ) {
            crate::modulation::switch_effect_kind(&mut eff[row], &mut modu[row], kind);
        }
        self.mark_config_dirty();
    }

    /// Apply a depth-dial drag value (normalized 0..1) to the selected
    /// assignable MSEG's depth (mapped to bipolar -1..1). Marks config dirty.
    fn apply_depth_drag(&mut self, norm: f32) {
        if self.selected_mseg == 0 {
            return;
        }
        let depth = (norm.clamp(0.0, 1.0) * 2.0 - 1.0).clamp(-1.0, 1.0);
        let row = self.selected_track;
        let k = self.selected_mseg - 1;
        if let Ok(mut modu) = self.params.track_modulation.lock() {
            modu[row].depths[k] = depth;
        }
        self.mark_config_dirty();
    }

    /// Apply a target-dropdown selection (item index) to the selected
    /// assignable MSEG. Marks config dirty.
    fn apply_target_selection(&mut self, item: usize) {
        if self.selected_mseg == 0 {
            return;
        }
        let target = effect_editor::target_from_item(item);
        let row = self.selected_track;
        let k = self.selected_mseg - 1;
        if let Ok(mut modu) = self.params.track_modulation.lock() {
            modu[row].targets[k] = target;
        }
        self.mark_config_dirty();
    }

    /// Reset effect-param dial slot `i` to the ParamSpec default. Marks dirty.
    fn reset_effect_dial_to_default(&mut self, i: usize) {
        let Some(spec) = self.param_spec(i) else {
            return;
        };
        if let Ok(mut cfg) = self.params.track_effects.lock() {
            cfg[self.selected_track].params[i] = spec.default;
        }
        self.mark_config_dirty();
    }

    /// Reset the active assignable MSEG's modulation depth to 0 (no
    /// modulation). Marks dirty. No-op when the amplitude MSEG is selected.
    fn reset_depth_to_default(&mut self) {
        if self.selected_mseg == 0 {
            return;
        }
        let row = self.selected_track;
        let k = self.selected_mseg - 1;
        if let Ok(mut modu) = self.params.track_modulation.lock() {
            modu[row].depths[k] = 0.0;
        }
        self.mark_config_dirty();
    }

    /// Apply a Mix-dial drag's new normalized value (0..1) to the selected
    /// track's per-track mix, marking config dirty.
    fn apply_mix(&mut self, norm: f32) {
        let value = norm.clamp(0.0, 1.0);
        if let Ok(mut cfg) = self.params.track_effects.lock() {
            cfg[self.selected_track].mix = value;
        }
        self.mark_config_dirty();
    }

    /// Reset the selected track's per-track mix to fully wet (1.0). Marks
    /// dirty. Backs double-clicking the Mix dial.
    fn reset_mix_to_default(&mut self) {
        if let Ok(mut cfg) = self.params.track_effects.lock() {
            cfg[self.selected_track].mix = 1.0;
        }
        self.mark_config_dirty();
    }

    /// Reset the per-track FreeHz trigger rate to 1.0 Hz. Marks dirty.
    /// No-op when the active trigger source is not `FreeHz`.
    fn reset_trigger_rate_to_default(&mut self) {
        if let Ok(mut cfg) = self.params.track_modulation.lock() {
            if let TriggerSource::FreeHz { hz } = &mut cfg[self.selected_track].trigger {
                *hz = 1.0;
                self.mark_config_dirty();
            }
        }
    }

    /// Update the trigger rate from the rate-dial drag's normalised value.
    fn apply_trigger_rate_drag(&mut self, norm: f32) {
        let new_hz = effects::norm_to_value(
            norm,
            effect_editor::TRIGGER_RATE_MIN_HZ,
            effect_editor::TRIGGER_RATE_MAX_HZ,
            effects::ParamScaling::Log,
        );
        if let Ok(mut cfg) = self.params.track_modulation.lock() {
            if let TriggerSource::FreeHz { hz } = &mut cfg[self.selected_track].trigger {
                *hz = new_hz;
                self.mark_config_dirty();
            }
        }
    }

    /// Apply a trigger-dropdown selection: convert the item index to a
    /// `TriggerSource` (carrying the current Hz if any), write it, mark dirty.
    fn apply_trigger_selection(&mut self, idx: usize) {
        let carried_hz = match self.selected_track_modulation().trigger {
            TriggerSource::FreeHz { hz } => hz,
            _ => 1.0,
        };
        let new_trigger = effect_editor::trigger_from_item(idx, carried_hz);
        if let Ok(mut cfg) = self.params.track_modulation.lock() {
            cfg[self.selected_track].trigger = new_trigger;
            self.mark_config_dirty();
        }
    }

    fn draw(&mut self) {
        match self.view {
            View::Grid => {
                let grid = self.params.grid.lock().map(|g| *g).unwrap_or_default();
                self.grid_cache.update(&grid, self.scale_factor);
                self.surface
                    .pixmap
                    .data_mut()
                    .copy_from_slice(self.grid_cache.pixmap().data());
                grid_view::draw_region_overlay(
                    &mut self.surface.pixmap,
                    &grid,
                    self.scale_factor,
                    Some(self.mouse_pos),
                );
                grid_view::draw_playhead(
                    &mut self.surface.pixmap,
                    self.playhead_display.column(),
                    grid.loop_region,
                    self.scale_factor,
                );
            }
            View::Effect => {
                // Paint the window background, then cover the main area
                // (right of the track panel, below the toolbar) with the
                // editor backdrop so no grid cells bleed through between the
                // effect-editor controls. The toolbar and track listing are
                // drawn fresh below.
                widgets::fill_pixmap_opaque(&mut self.surface.pixmap, widgets::color_bg());
                let ox = (grid_view::MARGIN + grid_view::TRACK_PANEL_W) * self.scale_factor;
                let oy = (grid_view::STATUS_H + grid_view::GUTTER) * self.scale_factor;
                let mw = self.physical_width as f32 - ox - grid_view::MARGIN * self.scale_factor;
                let mh = self.physical_height as f32 - oy - grid_view::MARGIN * self.scale_factor;
                widgets::draw_rect(
                    &mut self.surface.pixmap,
                    ox,
                    oy,
                    mw,
                    mh,
                    widgets::color_control_bg(),
                );
                self.draw_effect_view();
            }
        }
        // Track listing — both views.
        let kinds = self.track_kinds();
        let active = self.active_rows.load(Ordering::Relaxed);
        let selected = match self.view {
            View::Grid => None,
            View::Effect => Some(self.selected_track),
        };
        track_list::draw_track_list(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &kinds,
            active,
            selected,
            self.scale_factor,
        );
        // Toolbar — both views.
        toolbar::draw_toolbar(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &self.params,
            &self.seq_status,
            self.scale_factor,
        );
        // Drop popups draw last so they overlay every other control. One
        // shared `effect_dropdown` state handles Kind, Target, and Trigger;
        // the items list depends on which one is open.
        if self.view == View::Effect && self.effect_dropdown.is_open() {
            let kind = self.selected_track_effect().kind;
            let items: Vec<&'static str> = if self.effect_dropdown.is_open_for(EffectAction::Target)
            {
                effect_editor::target_items(kind)
            } else if self.effect_dropdown.is_open_for(EffectAction::Trigger) {
                effect_editor::trigger_items().to_vec()
            } else {
                effect_editor::kind_items()
            };
            let win = (self.physical_width as f32, self.physical_height as f32);
            widgets::dropdown::draw_dropdown_popup(
                &mut self.surface.pixmap,
                &mut self.text_renderer,
                &self.effect_dropdown,
                &items,
                win,
            );
        }
    }
}

impl baseview::WindowHandler for MultosisWindow {
    fn on_frame(&mut self, window: &mut baseview::Window) {
        let packed = self.pending_resize.swap(0, Ordering::Relaxed);
        if packed != 0 {
            let new_w = (packed >> 32) as u32;
            let new_h = (packed & 0xFFFF_FFFF) as u32;
            if new_w > 0
                && new_h > 0
                && (new_w != self.physical_width || new_h != self.physical_height)
            {
                window.resize(baseview::Size::new(new_w as f64, new_h as f64));
            }
        }
        self.draw();
        self.surface.present();
    }

    fn on_event(
        &mut self,
        _window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        match &event {
            baseview::Event::Window(baseview::WindowEvent::Resized(info)) => {
                self.physical_width = info.physical_size().width;
                self.physical_height = info.physical_size().height;
                self.scale_factor =
                    (self.physical_width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
                self.resize_buffers();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                let (px, py) = (position.x as f32, position.y as f32);
                self.mouse_pos = (px, py);
                self.toolbar_drag.set_mouse(px, py);
                self.effect_dial_drag.set_mouse(px, py);
                if let Some(&ctrl) = self.toolbar_drag.active_action() {
                    let current = self.slider_normalized(ctrl);
                    if let Some(norm) = self.toolbar_drag.update_drag(false, current) {
                        self.set_slider(ctrl, norm);
                    }
                }
                // Effect dial drag: vertical drag → normalized → ParamSpec or
                // depth dial (bipolar).
                let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                match self.effect_dial_drag.active_action().copied() {
                    Some(EffectHit::Dial(i)) => {
                        let current = if let Some(spec) = self.param_spec(i) {
                            let value = self.selected_track_effect().params[i];
                            effects::value_to_norm(value, spec.min, spec.max, spec.scaling)
                        } else {
                            0.0
                        };
                        if let Some(norm) = self.effect_dial_drag.update_drag(shift, current) {
                            self.apply_effect_dial(i, norm);
                        }
                    }
                    Some(EffectHit::Depth) => {
                        let current = if self.selected_mseg == 0 {
                            0.5
                        } else {
                            let modu = self.selected_track_modulation();
                            ((modu.depths[self.selected_mseg - 1] + 1.0) / 2.0).clamp(0.0, 1.0)
                        };
                        if let Some(norm) = self.effect_dial_drag.update_drag(shift, current) {
                            self.apply_depth_drag(norm);
                        }
                    }
                    Some(EffectHit::TriggerRate) => {
                        let current = effects::value_to_norm(
                            match self.selected_track_modulation().trigger {
                                TriggerSource::FreeHz { hz } => hz,
                                _ => 1.0,
                            },
                            effect_editor::TRIGGER_RATE_MIN_HZ,
                            effect_editor::TRIGGER_RATE_MAX_HZ,
                            effects::ParamScaling::Log,
                        );
                        if let Some(norm) = self.effect_dial_drag.update_drag(shift, current) {
                            self.apply_trigger_rate_drag(norm);
                        }
                    }
                    Some(EffectHit::MsegLength) => {
                        let current = self.active_mseg_length_norm();
                        if let Some(norm) = self.effect_dial_drag.update_drag(shift, current) {
                            self.apply_mseg_length_drag(norm);
                        }
                    }
                    Some(EffectHit::Mix) => {
                        let current = self.selected_track_effect().mix;
                        if let Some(norm) = self.effect_dial_drag.update_drag(shift, current) {
                            self.apply_mix(norm);
                        }
                    }
                    _ => {}
                }
                // Dropdown popup hover — pick the items list matching the open
                // dropdown so highlight indices map to the right labels.
                if self.effect_dropdown.is_open() {
                    let kind = self.selected_track_effect().kind;
                    let items: Vec<&'static str> =
                        if self.effect_dropdown.is_open_for(EffectAction::Target) {
                            effect_editor::target_items(kind)
                        } else if self.effect_dropdown.is_open_for(EffectAction::Trigger) {
                            effect_editor::trigger_items().to_vec()
                        } else {
                            effect_editor::kind_items()
                        };
                    let win = (self.physical_width as f32, self.physical_height as f32);
                    self.effect_dropdown.on_mouse_move(px, py, &items, win);
                }
                // MSEG node-drag follow: when the user is dragging a node, the
                // pointer can leave the pane rect and the drag should keep
                // tracking (matches miff's behaviour).
                if self.view == View::Effect {
                    let lay = effect_editor::effect_layout(self.scale_factor);
                    let sel = self.selected_mseg.min(2);
                    let changed = if let Ok(mut modu) = self.params.track_modulation.lock() {
                        let row = self.selected_track;
                        self.mseg_edit.on_mouse_move(
                            px,
                            py,
                            &mut modu[row].msegs[sel],
                            lay.mseg_pane,
                            self.scale_factor,
                            shift,
                        )
                    } else {
                        None
                    };
                    if changed == Some(widgets::mseg::MsegEdit::Changed) {
                        self.mark_config_dirty();
                    }
                }
                // Grid/region/paint gestures only apply in the grid view.
                if self.view == View::Grid {
                    match self.left_gesture {
                        Some(LeftGesture::ResizeRegion(handle)) => self.update_region_drag(handle),
                        Some(LeftGesture::MoveRegion {
                            press,
                            region_at_press,
                        }) => self.update_region_move(press, region_at_press),
                        Some(LeftGesture::GridPending { row, col }) => {
                            let cur = (
                                grid_view::row_at(py, self.scale_factor),
                                grid_view::column_at(px, self.scale_factor),
                            );
                            if cur != (row, col) {
                                // The press has become a paint drag.
                                let value = !shift;
                                let cells = grid_view::cells_between((row, col), cur);
                                self.paint_cells(value, &cells);
                                self.left_gesture =
                                    Some(LeftGesture::GridPaint { value, last: cur });
                            }
                        }
                        Some(LeftGesture::GridPaint { value, last }) => {
                            let cur = (
                                grid_view::row_at(py, self.scale_factor),
                                grid_view::column_at(px, self.scale_factor),
                            );
                            if cur != last {
                                let cells = grid_view::cells_between(last, cur);
                                self.paint_cells(value, &cells);
                                self.left_gesture =
                                    Some(LeftGesture::GridPaint { value, last: cur });
                            }
                        }
                        None => {}
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers,
            }) => {
                let snap = self.snapshot();
                self.undo.begin_capture(snap);
                let (px, py) = self.mouse_pos;
                // Auto-commit/cancel any in-flight dial text edit BEFORE any
                // other press routing. A press on the editing dial cancels (so
                // the normal effect-press path can begin a drag); a press
                // anywhere else commits the typed value.
                self.finalize_dial_edit_for_press(px, py);
                // An open dropdown owns every click — selecting a row applies
                // it, clicking outside closes. Route this BEFORE checking any
                // other control so a click on the popup never hits the
                // control behind it.
                if self.effect_dropdown.is_open() {
                    let kind = self.selected_track_effect().kind;
                    let items: Vec<&'static str> =
                        if self.effect_dropdown.is_open_for(EffectAction::Target) {
                            effect_editor::target_items(kind)
                        } else if self.effect_dropdown.is_open_for(EffectAction::Trigger) {
                            effect_editor::trigger_items().to_vec()
                        } else {
                            effect_editor::kind_items()
                        };
                    let win = (self.physical_width as f32, self.physical_height as f32);
                    if let Some(widgets::dropdown::DropdownEvent::Selected(action, idx)) =
                        self.effect_dropdown.on_mouse_down(px, py, &items, win)
                    {
                        match action {
                            EffectAction::Kind => {
                                let kind = EffectKind::ALL[idx.min(EffectKind::ALL.len() - 1)];
                                self.apply_kind_switch(kind);
                            }
                            EffectAction::Target => {
                                self.apply_target_selection(idx);
                            }
                            EffectAction::Trigger => self.apply_trigger_selection(idx),
                        }
                    }
                    return baseview::EventStatus::Captured;
                }
                match toolbar::toolbar_hit(px, py, self.scale_factor) {
                    Some(
                        ctrl @ (ToolbarControl::Mix
                        | ToolbarControl::Output
                        | ToolbarControl::CompThreshold
                        | ToolbarControl::CompRatio),
                    ) => {
                        if self.toolbar_click.check_and_update(ctrl) {
                            self.reset_toolbar_slider(ctrl);
                        } else {
                            let current = self.slider_normalized(ctrl);
                            self.toolbar_drag.begin_drag(ctrl, current, false);
                            self.begin_slider(ctrl);
                        }
                    }
                    Some(ctrl) => self.handle_toolbar_button(ctrl),
                    None => match toolbar::op_hit(px, py, self.scale_factor) {
                        Some(op) => self.handle_toolbar_op(op),
                        None => {
                            // The effect editor owns its own main-area hits;
                            // they take priority over re-selecting a track.
                            if self.view == View::Effect
                                && self.on_effect_press(
                                    px,
                                    py,
                                    modifiers.contains(keyboard_types::Modifiers::CONTROL),
                                )
                            {
                                return baseview::EventStatus::Captured;
                            }
                            // Track listing — both views.
                            if let Some(row) = track_list::track_at(px, py, self.scale_factor) {
                                let new_track = clamp_track(row);
                                // Switching tracks changes which MSEG the shared
                                // editor operates on; clear the node selection so
                                // stale indices can't act on another track's MSEG.
                                if new_track != self.selected_track {
                                    self.mseg_edit.clear_selection();
                                }
                                self.selected_track = new_track;
                                self.view = View::Effect;
                            } else if self.view == View::Grid {
                                // Grid: region handle / region move / cell pending.
                                if let Some(handle) = self.region_handle_under_cursor() {
                                    self.left_gesture = Some(LeftGesture::ResizeRegion(handle));
                                } else if self.try_begin_region_move() {
                                    // left_gesture set inside try_begin_region_move
                                } else if let Some((row, col)) =
                                    grid_view::cell_at(px, py, self.scale_factor)
                                {
                                    self.left_gesture = Some(LeftGesture::GridPending { row, col });
                                }
                            }
                        }
                    },
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Right,
                ..
            }) if self.view == View::Effect => {
                let snap = self.snapshot();
                let opened = self.undo.begin_capture(snap);
                let status = self.on_right_press();
                if opened {
                    let after = self.snapshot();
                    self.undo.commit_capture(&after);
                }
                return status;
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                if let Some(ctrl) = self.toolbar_drag.end_drag() {
                    self.end_slider(ctrl);
                }
                let _ = self.effect_dial_drag.end_drag();
                self.effect_dropdown.on_mouse_up();
                // A release always terminates any in-flight MSEG node drag,
                // regardless of where the cursor is.
                if self.view == View::Effect {
                    let sel = self.selected_mseg.min(2);
                    let lay = effect_editor::effect_layout(self.scale_factor);
                    let changed = if let Ok(mut modu) = self.params.track_modulation.lock() {
                        let row = self.selected_track;
                        self.mseg_edit.on_mouse_up(
                            &mut modu[row].msegs[sel],
                            lay.mseg_pane,
                            self.scale_factor,
                        )
                    } else {
                        None
                    };
                    if changed == Some(widgets::mseg::MsegEdit::Changed) {
                        self.mark_config_dirty();
                    }
                }
                if let Some(LeftGesture::GridPending { row, col }) = self.left_gesture {
                    self.commit_click(row, col);
                }
                self.left_gesture = None;
                let after = self.snapshot();
                self.undo.commit_capture(&after);
            }
            baseview::Event::Keyboard(ev) if self.text_edit.is_active() => {
                // Swallow key-ups while editing so the host DAW doesn't
                // process Enter/Escape releases as its own shortcuts.
                if ev.state != keyboard_types::KeyState::Down {
                    return baseview::EventStatus::Captured;
                }
                match &ev.key {
                    keyboard_types::Key::Character(s) => {
                        for c in s.chars() {
                            self.text_edit.insert_char(c);
                        }
                    }
                    keyboard_types::Key::Backspace => self.text_edit.backspace(),
                    keyboard_types::Key::Escape => self.text_edit.cancel(),
                    keyboard_types::Key::Enter => {
                        let snap = self.snapshot();
                        let opened = self.undo.begin_capture(snap);
                        match self.text_edit.commit() {
                            Some((EffectHit::Dial(i), text)) => {
                                self.commit_dial_text_edit(i, &text)
                            }
                            Some((EffectHit::Mix, text)) => self.commit_mix_text_edit(&text),
                            _ => {}
                        }
                        if opened {
                            let after = self.snapshot();
                            self.undo.commit_capture(&after);
                        }
                    }
                    _ => return baseview::EventStatus::Ignored,
                }
                return baseview::EventStatus::Captured;
            }
            baseview::Event::Keyboard(ev) => {
                if ev.state != keyboard_types::KeyState::Down {
                    return baseview::EventStatus::Ignored;
                }
                // Undo / redo — handled before any capture so the keystroke
                // itself is never recorded as an editing gesture. Active in
                // both views.
                if ev.modifiers.contains(keyboard_types::Modifiers::CONTROL) {
                    let is_z = matches!(
                        &ev.key,
                        keyboard_types::Key::Character(s) if s.eq_ignore_ascii_case("z")
                    );
                    let is_y = matches!(
                        &ev.key,
                        keyboard_types::Key::Character(s) if s.eq_ignore_ascii_case("y")
                    );
                    let shift = ev.modifiers.contains(keyboard_types::Modifiers::SHIFT);
                    if (is_z && shift) || is_y {
                        self.do_redo();
                        return baseview::EventStatus::Captured;
                    }
                    if is_z {
                        self.do_undo();
                        return baseview::EventStatus::Captured;
                    }
                }
                if self.view == View::Effect {
                    match &ev.key {
                        keyboard_types::Key::Delete | keyboard_types::Key::Backspace => {
                            let snap = self.snapshot();
                            let opened = self.undo.begin_capture(snap);
                            let sel = self.selected_mseg.min(2);
                            let changed = if let Ok(mut modu) = self.params.track_modulation.lock()
                            {
                                let row = self.selected_track;
                                self.mseg_edit.delete_selection(&mut modu[row].msegs[sel])
                            } else {
                                None
                            };
                            if changed == Some(widgets::mseg::MsegEdit::Changed) {
                                self.mark_config_dirty();
                            }
                            if opened {
                                let after = self.snapshot();
                                self.undo.commit_capture(&after);
                            }
                            if changed == Some(widgets::mseg::MsegEdit::Changed) {
                                return baseview::EventStatus::Captured;
                            }
                        }
                        _ => {}
                    }
                }
                return baseview::EventStatus::Ignored;
            }
            _ => {}
        }
        baseview::EventStatus::Captured
    }
}

/// The nih-plug `Editor` — spawns the window.
struct MultosisEditor {
    params: Arc<MultosisParams>,
    playhead_display: Arc<PlayheadDisplay>,
    seq_status: Arc<SeqStatusDisplay>,
    grid_handoff: Arc<GridHandoff>,
    reset_request: Arc<AtomicBool>,
    active_rows: Arc<AtomicU16>,
    mseg_phases: Arc<[AtomicU32; 48]>,
    config_dirty: Arc<AtomicBool>,
    pending_resize: Arc<AtomicU64>,
}

/// Build the editor.
#[allow(clippy::too_many_arguments)]
pub fn create(
    params: Arc<MultosisParams>,
    playhead_display: Arc<PlayheadDisplay>,
    seq_status: Arc<SeqStatusDisplay>,
    grid_handoff: Arc<GridHandoff>,
    reset_request: Arc<AtomicBool>,
    active_rows: Arc<AtomicU16>,
    mseg_phases: Arc<[AtomicU32; 48]>,
    config_dirty: Arc<AtomicBool>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(MultosisEditor {
        params,
        playhead_display,
        seq_status,
        grid_handoff,
        reset_request,
        active_rows,
        mseg_phases,
        config_dirty,
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}

impl Editor for MultosisEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);

        let params = Arc::clone(&self.params);
        let playhead_display = Arc::clone(&self.playhead_display);
        let seq_status = Arc::clone(&self.seq_status);
        let grid_handoff = Arc::clone(&self.grid_handoff);
        let pending_resize = Arc::clone(&self.pending_resize);
        let gui_context = Arc::clone(&context);
        let reset_request = Arc::clone(&self.reset_request);
        let active_rows = Arc::clone(&self.active_rows);
        let mseg_phases = Arc::clone(&self.mseg_phases);
        let config_dirty = Arc::clone(&self.config_dirty);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Multosis"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                MultosisWindow::new(
                    window,
                    params,
                    playhead_display,
                    seq_status,
                    grid_handoff,
                    pending_resize,
                    gui_context,
                    reset_request,
                    active_rows,
                    mseg_phases,
                    config_dirty,
                    sf,
                )
            },
        );

        self.params.editor_state.set_open(true);
        Box::new(widgets::EditorHandle::new(
            self.params.editor_state.clone(),
            window,
        ))
    }

    fn size(&self) -> (u32, u32) {
        self.params.editor_state.size()
    }

    fn set_scale_factor(&self, _factor: f32) -> bool {
        false
    }

    fn set_size(&self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        self.pending_resize
            .store(((width as u64) << 32) | (height as u64), Ordering::Relaxed);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_track_keeps_indices_in_range() {
        assert_eq!(clamp_track(0), 0);
        assert_eq!(clamp_track(15), 15);
        assert_eq!(clamp_track(99), 15);
    }
}
