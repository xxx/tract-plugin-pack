//! Softbuffer + tiny-skia CPU editor for Multosis.
//!
//! Milestone 1b-ii-a: opens the window and renders the grid + live wavefront.
//! Interaction (cell editing, loop-region drag, toolbar) is Milestone 1b-ii-b.

use baseview::{WindowOpenOptions, WindowScalePolicy};
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::Arc;

use crate::editor::effect_editor::EffectHit;
use crate::editor::toolbar::{ToolbarControl, ToolbarOp};
use crate::effects::{EffectKind, ParamSpec};
use crate::grid::LoopRegion;
use crate::handoff::GridHandoff;
use crate::region::RegionSnapshot;
use crate::seq_status::SeqStatusDisplay;
use crate::wavefront_display::WavefrontDisplay;
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
    GridPending {
        row: usize,
        col: usize,
        zone: grid_view::CellZone,
    },
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
/// The effect-kind dropdown (`Kind`) and the modulation target dropdown
/// (`Target`) share a single `DropdownState<EffectAction>` — only one is open
/// at a time, and the payload distinguishes which trigger opened it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EffectAction {
    Kind,
    Target,
}

/// Clamp a candidate selected-track index into `0..ROWS`.
fn clamp_track(row: usize) -> usize {
    row.min(crate::grid::ROWS - 1)
}

/// Map a parameter `value` to `[0, 1]` against its spec range. Degenerate
/// (max <= min) specs map to 0.
fn normalize_param(value: f32, spec: ParamSpec) -> f32 {
    if spec.max > spec.min {
        ((value - spec.min) / (spec.max - spec.min)).clamp(0.0, 1.0)
    } else {
        0.0
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
    wavefront_display: Arc<WavefrontDisplay>,
    seq_status: Arc<SeqStatusDisplay>,
    grid_handoff: Arc<GridHandoff>,
    /// Audio→GUI mirror of the engine's active-row mask. Task 3 draws with it.
    active_rows: Arc<AtomicU16>,
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
    /// The effect editor's dropdown state — owns the open kind/target popup.
    kind_dropdown: widgets::dropdown::DropdownState<EffectAction>,
    /// The effect editor's parameter-dial drag state — one in-flight dial drag
    /// at a time, tagged with the slot index via `EffectHit::Dial(i)` or by
    /// `EffectHit::Depth` for the modulation depth dial.
    effect_dial_drag: widgets::DragState<EffectHit>,
    /// MSEG editor state — owns hover/drag/last-node info for the active MSEG.
    /// Curve-only mode: the strip controls are external (per-MSEG selectors).
    mseg_edit: widgets::mseg::MsegEditState,
    /// Timestamp of the last left press on the MSEG pane, for double-click
    /// detection (~400 ms / ~8 px).
    mseg_last_click_time: std::time::Instant,
    /// Position of the last left press on the MSEG pane, for double-click
    /// detection.
    mseg_last_click_pos: (f32, f32),
    /// Audio→GUI dirty flag: every edit (param dial, kind switch, …) sets it
    /// so `process()` re-bridges the persisted config into the engine.
    config_dirty: Arc<AtomicBool>,
}

impl MultosisWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut baseview::Window<'_>,
        params: Arc<MultosisParams>,
        wavefront_display: Arc<WavefrontDisplay>,
        seq_status: Arc<SeqStatusDisplay>,
        grid_handoff: Arc<GridHandoff>,
        pending_resize: Arc<AtomicU64>,
        gui_context: Arc<dyn GuiContext>,
        reset_request: Arc<AtomicBool>,
        active_rows: Arc<AtomicU16>,
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
            wavefront_display,
            seq_status,
            grid_handoff,
            active_rows,
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
            kind_dropdown: widgets::dropdown::DropdownState::new(),
            effect_dial_drag: widgets::DragState::new(),
            mseg_edit: widgets::mseg::MsegEditState::new_curve_only(),
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

    /// Apply a resolved cell edit and republish the grid.
    fn commit_click(&mut self, row: usize, col: usize, zone: grid_view::CellZone, right: bool) {
        if let Ok(mut grid) = self.params.grid.lock() {
            grid_view::apply_grid_click(&mut grid, row, col, zone, right);
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

    /// Apply a click at the current cursor position (used for right-click,
    /// which still edits on press).
    fn handle_grid_click(&mut self, right: bool) {
        let (px, py) = self.mouse_pos;
        if let Some((row, col, zone)) = grid_view::cell_zone(px, py, self.scale_factor) {
            self.commit_click(row, col, zone, right);
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
            ToolbarControl::AutoRestart => {
                let next = !self.params.auto_restart.value();
                setter.begin_set_parameter(&self.params.auto_restart);
                setter.set_parameter(&self.params.auto_restart, next);
                setter.end_set_parameter(&self.params.auto_restart);
            }
            ToolbarControl::Reset => {
                self.reset_request
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            // Mix/Output drags are begun in on_event's ButtonPressed arm.
            ToolbarControl::Mix | ToolbarControl::Output => {}
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
        // Paste / Reset / Reinit / Randomize all changed the grid — republish.
        self.grid_handoff.publish(*grid);
    }

    /// The current normalized value of a slider control.
    fn slider_normalized(&self, ctrl: ToolbarControl) -> f32 {
        match ctrl {
            ToolbarControl::Mix => self.params.mix.unmodulated_normalized_value(),
            ToolbarControl::Output => self.params.output_gain.unmodulated_normalized_value(),
            _ => 0.0,
        }
    }

    /// Begin a host parameter gesture for a slider control.
    fn begin_slider(&self, ctrl: ToolbarControl) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        match ctrl {
            ToolbarControl::Mix => setter.begin_set_parameter(&self.params.mix),
            ToolbarControl::Output => setter.begin_set_parameter(&self.params.output_gain),
            _ => {}
        }
    }

    /// End a host parameter gesture for a slider control.
    fn end_slider(&self, ctrl: ToolbarControl) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        match ctrl {
            ToolbarControl::Mix => setter.end_set_parameter(&self.params.mix),
            ToolbarControl::Output => setter.end_set_parameter(&self.params.output_gain),
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
        effect_editor::draw_effect_section(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &track,
            self.selected_track,
            self.kind_dropdown.is_open_for(EffectAction::Kind),
            self.scale_factor,
        );
        // MODULATION section.
        let modu = self.selected_track_modulation();
        let sel = self.selected_mseg.min(2);
        // Ghosts (inactive MSEGs) first, behind the active curve.
        for m in 0..3 {
            if m != sel {
                widgets::mseg::draw_mseg_ghost(
                    &mut self.surface.pixmap,
                    lay.mseg_pane,
                    &modu.msegs[m],
                    &self.mseg_edit,
                    self.scale_factor,
                    0x5A5040FF,
                );
            }
        }
        // Active MSEG.
        widgets::mseg::draw_mseg(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            lay.mseg_pane,
            &modu.msegs[sel],
            &self.mseg_edit,
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
            self.kind_dropdown.is_open_for(EffectAction::Target),
            self.scale_factor,
        );
    }

    /// Handle a left press while in `View::Effect`. Returns `true` if the
    /// press hit a control owned by the effect editor (so the caller can stop
    /// further routing). `shift` is read from the cursor-modifier set for
    /// fine-grained MSEG node placement.
    fn on_effect_press(&mut self, px: f32, py: f32, shift: bool) -> bool {
        let params = self.selected_track_param_count();
        let Some(hit) =
            effect_editor::effect_hit(px, py, self.scale_factor, params, self.selected_mseg)
        else {
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
                self.kind_dropdown
                    .open(EffectAction::Kind, lay.kind, &items, current, false, win);
            }
            EffectHit::Dial(i) => {
                if let Some(spec) = self.param_spec(i) {
                    let value = self.selected_track_effect().params[i];
                    let norm = normalize_param(value, spec);
                    self.effect_dial_drag
                        .begin_drag(EffectHit::Dial(i), norm, false);
                }
            }
            EffectHit::MsegSelector(seg) => {
                self.selected_mseg = seg.min(2);
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
                self.kind_dropdown.open(
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
                    let modu = self.selected_track_modulation();
                    let depth = modu.depths[self.selected_mseg - 1];
                    let norm = ((depth + 1.0) / 2.0).clamp(0.0, 1.0);
                    self.effect_dial_drag
                        .begin_drag(EffectHit::Depth, norm, false);
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
                            shift,
                        )
                    }
                } else {
                    None
                };
                if changed == Some(widgets::mseg::MsegEdit::Changed) {
                    self.mark_config_dirty();
                }
            }
        }
        true
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

    /// Apply a dial drag's new normalized value to slot `i` of the currently
    /// selected track's effect, marking config dirty.
    fn apply_effect_dial(&mut self, i: usize, norm: f32) {
        let Some(spec) = self.param_spec(i) else {
            return;
        };
        let value = spec.min + norm.clamp(0.0, 1.0) * (spec.max - spec.min);
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
        if let Ok(mut cfg) = self.params.track_effects.lock() {
            cfg[row].kind = kind;
            cfg[row].params = crate::effects::default_params_for_kind(kind);
        }
        if let Ok(mut modu) = self.params.track_modulation.lock() {
            modu[row].clamp_targets(crate::effects::param_count(kind));
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
                grid_view::draw_wavefront(
                    &mut self.surface.pixmap,
                    &self.wavefront_display,
                    self.scale_factor,
                );
            }
            View::Effect => {
                // The grid cache already paints the full window background;
                // reuse it as the backdrop, then draw the effect editor over
                // the main area.
                let grid = self.params.grid.lock().map(|g| *g).unwrap_or_default();
                self.grid_cache.update(&grid, self.scale_factor);
                self.surface
                    .pixmap
                    .data_mut()
                    .copy_from_slice(self.grid_cache.pixmap().data());
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
        // shared `kind_dropdown` state handles both Kind and Target; the
        // items list depends on which one is open.
        if self.view == View::Effect && self.kind_dropdown.is_open() {
            let kind = self.selected_track_effect().kind;
            let items: Vec<&'static str> = if self.kind_dropdown.is_open_for(EffectAction::Target) {
                effect_editor::target_items(kind)
            } else {
                effect_editor::kind_items()
            };
            let win = (self.physical_width as f32, self.physical_height as f32);
            widgets::dropdown::draw_dropdown_popup(
                &mut self.surface.pixmap,
                &mut self.text_renderer,
                &self.kind_dropdown,
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
                            normalize_param(value, spec)
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
                    _ => {}
                }
                // Dropdown popup hover — pick the items list matching the open
                // dropdown so highlight indices map to the right labels.
                if self.kind_dropdown.is_open() {
                    let kind = self.selected_track_effect().kind;
                    let items: Vec<&'static str> =
                        if self.kind_dropdown.is_open_for(EffectAction::Target) {
                            effect_editor::target_items(kind)
                        } else {
                            effect_editor::kind_items()
                        };
                    let win = (self.physical_width as f32, self.physical_height as f32);
                    self.kind_dropdown.on_mouse_move(px, py, &items, win);
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
                        Some(LeftGesture::GridPending { row, col, zone: _ }) => {
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
                let (px, py) = self.mouse_pos;
                let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                // An open dropdown owns every click — selecting a row applies
                // it, clicking outside closes. Route this BEFORE checking any
                // other control so a click on the popup never hits the
                // control behind it.
                if self.kind_dropdown.is_open() {
                    let kind = self.selected_track_effect().kind;
                    let items: Vec<&'static str> =
                        if self.kind_dropdown.is_open_for(EffectAction::Target) {
                            effect_editor::target_items(kind)
                        } else {
                            effect_editor::kind_items()
                        };
                    let win = (self.physical_width as f32, self.physical_height as f32);
                    if let Some(widgets::dropdown::DropdownEvent::Selected(action, idx)) =
                        self.kind_dropdown.on_mouse_down(px, py, &items, win)
                    {
                        match action {
                            EffectAction::Kind => {
                                let kind = EffectKind::ALL[idx.min(EffectKind::ALL.len() - 1)];
                                self.apply_kind_switch(kind);
                            }
                            EffectAction::Target => {
                                self.apply_target_selection(idx);
                            }
                        }
                    }
                    return baseview::EventStatus::Captured;
                }
                match toolbar::toolbar_hit(px, py, self.scale_factor) {
                    Some(ctrl @ (ToolbarControl::Mix | ToolbarControl::Output)) => {
                        let current = self.slider_normalized(ctrl);
                        self.toolbar_drag.begin_drag(ctrl, current, false);
                        self.begin_slider(ctrl);
                    }
                    Some(ctrl) => self.handle_toolbar_button(ctrl),
                    None => match toolbar::op_hit(px, py, self.scale_factor) {
                        Some(op) => self.handle_toolbar_op(op),
                        None => {
                            // The effect editor owns its own main-area hits;
                            // they take priority over re-selecting a track.
                            if self.view == View::Effect && self.on_effect_press(px, py, shift) {
                                return baseview::EventStatus::Captured;
                            }
                            // Track listing — both views.
                            if let Some(row) = track_list::track_at(px, py, self.scale_factor) {
                                self.selected_track = clamp_track(row);
                                self.view = View::Effect;
                            } else if self.view == View::Grid {
                                // Grid: region handle / region move / cell pending.
                                if let Some(handle) = self.region_handle_under_cursor() {
                                    self.left_gesture = Some(LeftGesture::ResizeRegion(handle));
                                } else if self.try_begin_region_move() {
                                    // left_gesture set inside try_begin_region_move
                                } else if let Some((row, col, zone)) =
                                    grid_view::cell_zone(px, py, self.scale_factor)
                                {
                                    self.left_gesture =
                                        Some(LeftGesture::GridPending { row, col, zone });
                                }
                            }
                        }
                    },
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Right,
                ..
            }) if self.view == View::Grid => {
                // Right-click cell editing applies only in the grid view.
                self.handle_grid_click(true);
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Right,
                ..
            }) if self.view == View::Effect => {
                // Right-click on the MSEG pane toggles segment-stepped flag
                // (see `MsegEditState::on_right_click`). Ignored elsewhere.
                let (px, py) = self.mouse_pos;
                if self.effect_dial_drag.active_action().is_some() {
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
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                if let Some(ctrl) = self.toolbar_drag.end_drag() {
                    self.end_slider(ctrl);
                }
                let _ = self.effect_dial_drag.end_drag();
                self.kind_dropdown.on_mouse_up();
                // A release always terminates any in-flight MSEG node drag,
                // regardless of where the cursor is.
                if self.view == View::Effect {
                    let sel = self.selected_mseg.min(2);
                    let changed = if let Ok(mut modu) = self.params.track_modulation.lock() {
                        let row = self.selected_track;
                        self.mseg_edit.on_mouse_up(&mut modu[row].msegs[sel])
                    } else {
                        None
                    };
                    if changed == Some(widgets::mseg::MsegEdit::Changed) {
                        self.mark_config_dirty();
                    }
                }
                if let Some(LeftGesture::GridPending { row, col, zone }) = self.left_gesture {
                    self.commit_click(row, col, zone, false);
                }
                self.left_gesture = None;
            }
            _ => {}
        }
        baseview::EventStatus::Captured
    }
}

/// The nih-plug `Editor` — spawns the window.
struct MultosisEditor {
    params: Arc<MultosisParams>,
    wavefront_display: Arc<WavefrontDisplay>,
    seq_status: Arc<SeqStatusDisplay>,
    grid_handoff: Arc<GridHandoff>,
    reset_request: Arc<AtomicBool>,
    active_rows: Arc<AtomicU16>,
    config_dirty: Arc<AtomicBool>,
    pending_resize: Arc<AtomicU64>,
}

/// Build the editor.
pub fn create(
    params: Arc<MultosisParams>,
    wavefront_display: Arc<WavefrontDisplay>,
    seq_status: Arc<SeqStatusDisplay>,
    grid_handoff: Arc<GridHandoff>,
    reset_request: Arc<AtomicBool>,
    active_rows: Arc<AtomicU16>,
    config_dirty: Arc<AtomicBool>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(MultosisEditor {
        params,
        wavefront_display,
        seq_status,
        grid_handoff,
        reset_request,
        active_rows,
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
        let wavefront_display = Arc::clone(&self.wavefront_display);
        let seq_status = Arc::clone(&self.seq_status);
        let grid_handoff = Arc::clone(&self.grid_handoff);
        let pending_resize = Arc::clone(&self.pending_resize);
        let gui_context = Arc::clone(&context);
        let reset_request = Arc::clone(&self.reset_request);
        let active_rows = Arc::clone(&self.active_rows);
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
                    wavefront_display,
                    seq_status,
                    grid_handoff,
                    pending_resize,
                    gui_context,
                    reset_request,
                    active_rows,
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
