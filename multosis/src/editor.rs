//! Softbuffer + tiny-skia CPU editor for Multosis.
//!
//! Milestone 1b-ii-a: opens the window and renders the grid + live playhead.
//! Interaction (cell editing, loop-region drag, toolbar) is Milestone 1b-ii-b.

use baseview::{WindowOpenOptions, WindowScalePolicy};
use nih_plug::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};

use crate::MultosisParams;
use crate::editor::toolbar::{ToolbarControl, ToolbarOp};
use crate::editor::track_list::TrackDrag;
use crate::effects::EffectKind;
use crate::grid::LoopRegion;
use crate::handoff::GridHandoff;
use crate::playhead_display::PlayheadDisplay;
use crate::region::RegionSnapshot;
use crate::seq_status::SeqStatusDisplay;
use crate::undo::{ConfigSnapshot, UndoHistory};
use tiny_skia_widgets as widgets;
use tract_effects::editor::{ClickTracker, EditorContext, EditorGeometry, EffectEditor};
pub use tract_effects::editor::{mseg_color, mseg_node_tooltip_text};

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

/// Non-deterministic starting seed for the editor's `rng_seed` counter.
///
/// The counter advances by 1 per Randomize click, but if it always started
/// at the same value, every plugin re-add would replay the same sequence of
/// effect kinds picked by per-row Randomize on an empty grid. Seeding from
/// wall-clock nanoseconds gives a fresh starting point per editor open while
/// still keeping `randomize_track_effect` itself deterministic in its seed
/// argument (so tests keep working).
fn fresh_rng_seed() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1)
        .max(1)
}

fn clamp_track(row: usize) -> usize {
    row.min(crate::grid::ROWS - 1)
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
    /// Slot `row*4 + k`. The editor's playhead overlay reads the active one
    /// each frame.
    mseg_phases: Arc<[AtomicU32; 64]>,
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
    effect_editor: EffectEditor,
    /// The toolbar Speed selector's dropdown state. Visible in both views.
    speed_dropdown: widgets::dropdown::DropdownState<()>,
    /// Undo/redo history for the DAW-opaque config. Window-scoped — created
    /// fresh on window open, dropped on close.
    undo: UndoHistory<ConfigSnapshot>,
    /// Audio→GUI dirty flag: every edit (param dial, kind switch, …) sets it
    /// so `process()` re-bridges the persisted config into the engine.
    config_dirty: Arc<AtomicBool>,
    /// Editor → audio handoff posted on the release of a drag-and-drop track
    /// reorder. The audio thread drains it once per process block and swaps
    /// the engine's per-row DSP state to match the GUI's just-applied
    /// config swap. Encoded `((from + 1) << 8) | (to + 1)`; `0` = no swap.
    pending_track_swap: Arc<AtomicU32>,
    /// In-flight drag-and-drop reorder of the track list, if any. Populated
    /// on a track-row press and cleared on release (after applying the swap
    /// if the drop landed on a different track).
    track_drag: Option<TrackDrag>,
    /// Double-click detector for the toolbar Mix / Output sliders.
    toolbar_click: ClickTracker<ToolbarControl>,
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
        mseg_phases: Arc<[AtomicU32; 64]>,
        config_dirty: Arc<AtomicBool>,
        pending_track_swap: Arc<AtomicU32>,
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
            mouse_pos: (0.0, 0.0),
            text_renderer,
            gui_context,
            reset_request,
            toolbar_drag: widgets::DragState::new(),
            clipboard: None,
            rng_seed: fresh_rng_seed(),
            left_gesture: None,
            grid_cache: grid_view::GridCache::new(pw, ph),
            view: View::Grid,
            selected_track: 0,
            effect_editor: EffectEditor::default(),
            speed_dropdown: widgets::dropdown::DropdownState::new(),
            undo: UndoHistory::new(),
            config_dirty,
            pending_track_swap,
            track_drag: None,
        }
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface
            .resize_and_persist(pw, ph, &self.params.editor_state);
        self.grid_cache = grid_view::GridCache::new(pw, ph);
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
        match ctrl {
            ToolbarControl::Reset => {
                self.reset_request
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            // Speed opens a dropdown in on_event's ButtonPressed arm; slider
            // drags are begun there too. Neither lands here.
            ToolbarControl::Speed
            | ToolbarControl::Mix
            | ToolbarControl::Output
            | ToolbarControl::CompThreshold
            | ToolbarControl::CompRatio => {}
        }
    }

    /// Open the toolbar Speed dropdown anchored to its control rect.
    fn open_speed_dropdown(&mut self) {
        let rect = toolbar::control_rect(ToolbarControl::Speed, self.scale_factor);
        let items = toolbar::speed_items();
        let current = crate::clock::Speed::ALL
            .iter()
            .position(|&s| s == self.params.speed.value())
            .unwrap_or(0);
        let win = (self.physical_width as f32, self.physical_height as f32);
        self.speed_dropdown.open(
            (),
            rect,
            widgets::dropdown::DropdownList::flat(&items),
            current,
            false,
            win,
        );
    }

    /// Apply a Speed dropdown selection by writing `params.speed` via
    /// `ParamSetter`.
    fn apply_speed_selection(&mut self, idx: usize) {
        let all = crate::clock::Speed::ALL;
        let speed = all[idx.min(all.len() - 1)];
        let setter = ParamSetter::new(self.gui_context.as_ref());
        setter.begin_set_parameter(&self.params.speed);
        setter.set_parameter(&self.params.speed, speed);
        setter.end_set_parameter(&self.params.speed);
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
        match self.params.track_effects.lock() {
            Ok(cfg) => std::array::from_fn(|r| cfg[r].kind),
            _ => std::array::from_fn(|r| crate::effects::TrackEffect::default_for_row(r).kind),
        }
    }

    /// Per-track `(muted, soloed)` snapshot for the track-list draw. Same
    /// lock-failure fallback as `track_kinds`: all-false (no row muted, no
    /// row soloed) so a contended frame draws indistinguishable from the
    /// default state rather than a misleading mute/solo display.
    fn track_mute_solo(&self) -> ([bool; crate::grid::ROWS], [bool; crate::grid::ROWS]) {
        match self.params.track_effects.lock() {
            Ok(cfg) => (
                std::array::from_fn(|r| cfg[r].muted),
                std::array::from_fn(|r| cfg[r].soloed),
            ),
            _ => ([false; crate::grid::ROWS], [false; crate::grid::ROWS]),
        }
    }

    /// Toggle the M (mute) or S (solo) flag on row `row`, snapshot the
    /// config for undo, and mark the audio thread to re-bridge. Wrapped
    /// in `try_lock` so a brief lock-contended click is a no-op rather
    /// than a freeze.
    fn toggle_track_button(&mut self, row: usize, button: track_list::TrackButton) {
        if row >= crate::grid::ROWS {
            return;
        }
        let snap = self.snapshot();
        let opened = self.undo.begin_capture(snap);
        let changed = match self.params.track_effects.try_lock() {
            Ok(mut cfg) => {
                match button {
                    track_list::TrackButton::Mute => cfg[row].muted = !cfg[row].muted,
                    track_list::TrackButton::Solo => cfg[row].soloed = !cfg[row].soloed,
                }
                true
            }
            _ => false,
        };
        if changed {
            self.mark_config_dirty();
        }
        if opened {
            let after = self.snapshot();
            self.undo.commit_capture(&after);
        }
    }

    /// Mark the persisted effect/modulation config dirty so the audio thread
    /// re-bridges it on the next process block.
    fn mark_config_dirty(&self) {
        self.config_dirty.store(true, Ordering::Relaxed);
    }

    /// Swap tracks `from` and `to` end-to-end: grid cells, effect config, and
    /// modulation config flip across the editor-owned `params` mutexes; the
    /// audio engine receives a matching command via `pending_track_swap` so
    /// its DSP state (effect instances, MSEG phases, amplitudes) moves with
    /// the tracks. Captured as a single undo entry. No-op for `from == to`
    /// or any out-of-range index.
    fn swap_tracks(&mut self, from: usize, to: usize) {
        if from == to || from >= crate::grid::ROWS || to >= crate::grid::ROWS {
            return;
        }
        let snap = self.snapshot();
        let opened = self.undo.begin_capture(snap);
        if let (Ok(mut grid), Ok(mut effects), Ok(mut modu)) = (
            self.params.grid.lock(),
            self.params.track_effects.lock(),
            self.params.track_modulation.lock(),
        ) {
            track_list::swap_rows_pure(&mut grid, &mut effects, &mut modu, from, to);
            // Re-publish the freshly swapped grid through the GUI→audio
            // handoff so the audio thread sees the new cell layout before
            // the next process block (matches every other grid edit).
            self.grid_handoff.publish(*grid);
        }
        // Encode (from, to) for the audio thread's swap consumer.
        let encoded = (((from + 1) as u32) << 8) | ((to + 1) as u32);
        self.pending_track_swap.store(encoded, Ordering::Release);
        self.mark_config_dirty();
        if opened {
            let after = self.snapshot();
            self.undo.commit_capture(&after);
        }
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
            self.effect_editor.clear_selection();
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
            self.effect_editor.clear_selection();
            self.mark_config_dirty();
        }
    }

    /// The persisted `TrackEffect` for the currently selected track, or its
    /// row default if the mutex is contended.
    fn selected_track_effect(&self) -> crate::effects::TrackEffect {
        let row = self.selected_track;
        match self.params.track_effects.lock() {
            Ok(cfg) => cfg[row],
            _ => crate::effects::TrackEffect::default_for_row(row),
        }
    }

    /// The persisted `TrackModulation` for the currently selected track, or
    /// its row default if the mutex is contended.
    fn selected_track_modulation(&self) -> crate::modulation::TrackModulation {
        let row = self.selected_track;
        match self.params.track_modulation.lock() {
            Ok(cfg) => cfg[row].clone(),
            _ => crate::modulation::TrackModulation::default_for_row(row),
        }
    }

    fn effect_geometry(&self) -> EditorGeometry {
        EditorGeometry {
            layout: effect_editor::effect_layout(self.scale_factor),
            scale: self.scale_factor,
            window_size: (self.physical_width as f32, self.physical_height as f32),
        }
    }

    fn handle_effect_event(&mut self, event: &baseview::Event) -> bool {
        let geometry = self.effect_geometry();
        let response = {
            let (Ok(mut effects), Ok(mut modulation)) = (
                self.params.track_effects.lock(),
                self.params.track_modulation.lock(),
            ) else {
                return false;
            };
            self.effect_editor.handle_event(
                event,
                EditorContext {
                    effect: &mut effects[self.selected_track],
                    modulation: &mut modulation[self.selected_track],
                    text_renderer: &mut self.text_renderer,
                    geometry,
                    pointer_position: self.mouse_pos,
                },
            )
        };
        if response.changed {
            self.mark_config_dirty();
        }
        if response.randomize {
            self.randomize_selected_track_effect();
        }
        if response.back {
            self.view = View::Grid;
        }
        response.captured
    }

    fn draw_effect_view(&mut self) {
        let effect = self.selected_track_effect();
        let modulation = self.selected_track_modulation();
        let geometry = self.effect_geometry();
        let phases = std::array::from_fn(|k| {
            f32::from_bits(self.mseg_phases[self.selected_track * 4 + k].load(Ordering::Relaxed))
        });
        effect_editor::draw_heading(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            self.selected_track,
            self.scale_factor,
        );
        self.effect_editor.draw(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &effect,
            &modulation,
            phases,
            geometry,
        );
    }

    /// Handle a click on the Randomize button. The two spec'd branches:
    /// - If the selected track's effect is None, pick a random kind from
    ///   `EffectKind::ALL` that is NOT in use on any OTHER track; assign
    ///   it (resetting params + clamping modulation targets via
    ///   `switch_effect_kind`); then randomize all params.
    /// - Otherwise, keep the kind and randomize the params only.
    ///
    /// `mix` is preserved in both branches. `rng_seed` is consumed and
    /// advanced so each click produces a distinct re-roll.
    fn randomize_selected_track_effect(&mut self) {
        let row = self.selected_track;
        let seed = self.rng_seed;
        self.rng_seed = self.rng_seed.wrapping_add(1);
        // Gather kinds currently in use on OTHER tracks. Skip None
        // (it's not a "claim" on anything), and skip the current row
        // since we're about to overwrite it anyway.
        if let (Ok(mut eff), Ok(mut modu)) = (
            self.params.track_effects.lock(),
            self.params.track_modulation.lock(),
        ) {
            let in_use_elsewhere: Vec<EffectKind> = (0..eff.len())
                .filter(|&i| i != row)
                .map(|i| eff[i].kind)
                .filter(|k| *k != EffectKind::None)
                .collect();
            crate::randomize::randomize_track_effect(
                &mut eff[row],
                &mut modu[row],
                &in_use_elsewhere,
                seed,
            );
        }
        self.mark_config_dirty();
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
        let (mutes, solos) = self.track_mute_solo();
        let active = self.active_rows.load(Ordering::Relaxed);
        let selected = match self.view {
            View::Grid => None,
            View::Effect => Some(self.selected_track),
        };
        let (drag_source, drag_target) = match self.track_drag {
            Some(d) => (
                Some(d.from),
                track_list::track_at(self.mouse_pos.0, d.current_y, self.scale_factor),
            ),
            None => (None, None),
        };
        track_list::draw_track_list(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &kinds,
            &mutes,
            &solos,
            active,
            selected,
            drag_source,
            drag_target,
            self.scale_factor,
        );
        // Toolbar — both views.
        toolbar::draw_toolbar(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &self.params,
            &self.seq_status,
            self.scale_factor,
            self.speed_dropdown.is_open(),
        );
        // Drop popups draw last so they overlay every other control.
        if self.speed_dropdown.is_open() {
            let items = toolbar::speed_items();
            let win = (self.physical_width as f32, self.physical_height as f32);
            widgets::dropdown::draw_dropdown_popup(
                &mut self.surface.pixmap,
                &mut self.text_renderer,
                &self.speed_dropdown,
                &items,
                win,
            );
        }
        if self.view == View::Effect {
            let effect = self.selected_track_effect();
            let geometry = self.effect_geometry();
            self.effect_editor.draw_popup(
                &mut self.surface.pixmap,
                &mut self.text_renderer,
                &effect,
                geometry,
            );
        }
    }
}

impl baseview::WindowHandler for MultosisWindow {
    fn on_frame(&mut self, window: &mut baseview::Window) {
        widgets::consume_pending_resize(
            &self.pending_resize,
            (self.physical_width, self.physical_height),
            window,
        );
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
                // Track the cursor's y for an in-flight track drag so the
                // draw pass can highlight the drop target row.
                if let Some(d) = self.track_drag.as_mut() {
                    d.current_y = py;
                }
                self.toolbar_drag.set_mouse(px, py);
                if let Some(&ctrl) = self.toolbar_drag.active_action() {
                    let current = self.slider_normalized(ctrl);
                    if let Some(norm) = self.toolbar_drag.update_drag(false, current) {
                        self.set_slider(ctrl, norm);
                    }
                }
                let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                // Dropdown popup hover.
                if self.speed_dropdown.is_open() {
                    let items = toolbar::speed_items();
                    let win = (self.physical_width as f32, self.physical_height as f32);
                    self.speed_dropdown.on_mouse_move(px, py, &items, win);
                }

                if self.view == View::Effect {
                    self.handle_effect_event(&event);
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
                ..
            }) => {
                let snap = self.snapshot();
                self.undo.begin_capture(snap);
                let (px, py) = self.mouse_pos;
                if self.view == View::Effect
                    && !self.speed_dropdown.is_open()
                    && self.handle_effect_event(&event)
                {
                    return baseview::EventStatus::Captured;
                }
                // An open dropdown owns every click — selecting a row applies
                // it, clicking outside closes. Route this BEFORE checking any
                // other control so a click on the popup never hits the
                // control behind it.
                if self.speed_dropdown.is_open() {
                    let items = toolbar::speed_items();
                    let win = (self.physical_width as f32, self.physical_height as f32);
                    if let Some(widgets::dropdown::DropdownEvent::Selected((), idx)) =
                        self.speed_dropdown.on_mouse_down(px, py, &items, win)
                    {
                        self.apply_speed_selection(idx);
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
                    Some(ToolbarControl::Speed) => self.open_speed_dropdown(),
                    Some(ctrl) => self.handle_toolbar_button(ctrl),
                    None => match toolbar::op_hit(px, py, self.scale_factor) {
                        Some(op) => self.handle_toolbar_op(op),
                        None => {
                            // Track listing — both views. M/S button hit-test
                            // runs BEFORE track-select so a click on one of
                            // those small buttons just toggles its flag, with
                            // no track switch and no drag arming.
                            if let Some((row, button)) =
                                track_list::track_button_at(px, py, self.scale_factor)
                            {
                                self.toggle_track_button(row, button);
                                return baseview::EventStatus::Captured;
                            }
                            if let Some(row) = track_list::track_at(px, py, self.scale_factor) {
                                let new_track = clamp_track(row);
                                // Switching tracks changes which MSEG the shared
                                // editor operates on; clear the node selection so
                                // stale indices can't act on another track's MSEG.
                                if new_track != self.selected_track {
                                    self.effect_editor.clear_selection();
                                }
                                self.selected_track = new_track;
                                self.view = View::Effect;
                                // Arm a drag-and-drop reorder. If the release
                                // lands on a different row, the two tracks
                                // swap places; releasing on the same row is
                                // an ordinary track-select click.
                                self.track_drag = Some(TrackDrag {
                                    from: new_track,
                                    current_y: py,
                                });
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
                let status = if self.handle_effect_event(&event) {
                    baseview::EventStatus::Captured
                } else {
                    baseview::EventStatus::Ignored
                };
                if opened {
                    let after = self.snapshot();
                    self.undo.commit_capture(&after);
                }
                return status;
            }
            baseview::Event::Mouse(baseview::MouseEvent::WheelScrolled { delta, .. }) => {
                // Mousewheel scrolls the currently-open dropdown popup.
                // Lines deliver one click per detent; pixel-mode trackpads
                // deliver fine-grained pixel counts — scale the latter to
                // match (six-pack uses the same 0.05 factor).
                let dy = match delta {
                    baseview::ScrollDelta::Lines { y, .. } => *y,
                    baseview::ScrollDelta::Pixels { y, .. } => *y * 0.05,
                };
                let win = (self.physical_width as f32, self.physical_height as f32);
                if self.speed_dropdown.is_open() {
                    let items = toolbar::speed_items();
                    self.speed_dropdown.on_wheel(dy, &items, win);
                    return baseview::EventStatus::Captured;
                }
                if self.view == View::Effect && self.handle_effect_event(&event) {
                    return baseview::EventStatus::Captured;
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                if let Some(ctrl) = self.toolbar_drag.end_drag() {
                    self.end_slider(ctrl);
                }
                self.speed_dropdown.on_mouse_up();
                // Drop a drag-and-drop track reorder. If the cursor is over a
                // different track row, swap places (and update the selected
                // track so the dragged content stays in view at its new
                // position). Same-row or off-list releases just cancel.
                if let Some(drag) = self.track_drag.take() {
                    let (px, py) = self.mouse_pos;
                    if let Some(to) = track_list::track_at(px, py, self.scale_factor)
                        && to != drag.from
                    {
                        self.swap_tracks(drag.from, to);
                        self.selected_track = to;
                    }
                }
                if self.view == View::Effect {
                    self.handle_effect_event(&event);
                }
                if let Some(LeftGesture::GridPending { row, col }) = self.left_gesture {
                    self.commit_click(row, col);
                }
                self.left_gesture = None;
                let after = self.snapshot();
                self.undo.commit_capture(&after);
            }
            baseview::Event::Keyboard(_) if self.effect_editor.captures_keyboard() => {
                let snap = self.snapshot();
                let opened = self.undo.begin_capture(snap);
                let captured = self.handle_effect_event(&event);
                if opened {
                    let after = self.snapshot();
                    self.undo.commit_capture(&after);
                }
                return if captured {
                    baseview::EventStatus::Captured
                } else {
                    baseview::EventStatus::Ignored
                };
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
                    let snap = self.snapshot();
                    let opened = self.undo.begin_capture(snap);
                    let captured = self.handle_effect_event(&event);
                    if opened {
                        let after = self.snapshot();
                        self.undo.commit_capture(&after);
                    }
                    if captured {
                        return baseview::EventStatus::Captured;
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
    mseg_phases: Arc<[AtomicU32; 64]>,
    config_dirty: Arc<AtomicBool>,
    /// Editor → audio handoff for the GUI's drag-and-drop track-swap gesture.
    /// Encoded `((from + 1) << 8) | (to + 1)`; see `Multosis::pending_track_swap`.
    pending_track_swap: Arc<AtomicU32>,
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
    mseg_phases: Arc<[AtomicU32; 64]>,
    config_dirty: Arc<AtomicBool>,
    pending_track_swap: Arc<AtomicU32>,
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
        pending_track_swap,
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
        let pending_track_swap = Arc::clone(&self.pending_track_swap);

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
                    pending_track_swap,
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
