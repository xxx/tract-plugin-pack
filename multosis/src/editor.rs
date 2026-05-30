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
use crate::editor::track_list::TrackDrag;
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
/// The effect-kind, modulation-target, per-track trigger-source, and per-param
/// (Enum-format) dropdowns share a single `DropdownState<EffectAction>` — only
/// one is open at a time, and the payload distinguishes which trigger opened
/// it. `ParamDropdown(i)` carries the param's slot index (0..MAX_EFFECT_PARAMS)
/// so the selection routes back to `set_param(i, …)` on the right parameter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EffectAction {
    Kind,
    Target,
    Trigger,
    ParamDropdown(usize),
}

/// Clamp a candidate selected-track index into `0..ROWS`.
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

/// The four MSEG slots each get their own colour: Amp = sky blue (the
/// existing accent), MSEG 1 = amber, MSEG 2 = purple, MSEG 3 = mint.
/// Used as the value colour for `draw_mseg`/`draw_mseg_ghost`, the
/// active fill of the MSEG selector tab, and the modulation arc of
/// any param dial driven by that MSEG. Slot indices past 3 clamp to 3.
pub fn mseg_color(slot: usize) -> tiny_skia::Color {
    match slot.min(3) {
        0 => tiny_skia::Color::from_rgba8(0x4f, 0xc3, 0xf7, 0xff), // Amp — sky blue
        1 => tiny_skia::Color::from_rgba8(0xff, 0xc8, 0x58, 0xff), // MSEG 1 — amber
        2 => tiny_skia::Color::from_rgba8(0xc3, 0x78, 0xff, 0xff), // MSEG 2 — purple
        _ => tiny_skia::Color::from_rgba8(0x66, 0xd9, 0xa0, 0xff), // MSEG 3 — mint
    }
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
    /// Which MSEG slot the modulation section currently edits (0..3); 0 is the
    /// always-on amplitude MSEG, 1/2 are assignable. Read by `effect_hit`; the
    /// MODULATION-section UI is wired in Task 9.
    selected_mseg: usize,
    /// The effect editor's shared dropdown state — owns the open Kind / Target / Trigger popup.
    effect_dropdown: widgets::dropdown::DropdownState<EffectAction>,
    /// The toolbar Speed selector's dropdown state. Visible in both views.
    speed_dropdown: widgets::dropdown::DropdownState<()>,
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
            effect_click: ClickTracker::new(),
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
            // Default to the first assignable MSEG (slot 1, the leftmost
            // tab in visual order). Amp lives on the rightmost tab.
            selected_mseg: 1,
            effect_dropdown: widgets::dropdown::DropdownState::new(),
            speed_dropdown: widgets::dropdown::DropdownState::new(),
            effect_dial_drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
            mseg_edit: widgets::mseg::MsegEditState::new(),
            undo: UndoHistory::new(),
            mseg_last_click_time: std::time::Instant::now(),
            mseg_last_click_pos: (-999.0, -999.0),
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
        if let Ok(cfg) = self.params.track_effects.lock() {
            std::array::from_fn(|r| cfg[r].kind)
        } else {
            std::array::from_fn(|r| crate::effects::TrackEffect::default_for_row(r).kind)
        }
    }

    /// Per-track `(muted, soloed)` snapshot for the track-list draw. Same
    /// lock-failure fallback as `track_kinds`: all-false (no row muted, no
    /// row soloed) so a contended frame draws indistinguishable from the
    /// default state rather than a misleading mute/solo display.
    fn track_mute_solo(&self) -> ([bool; crate::grid::ROWS], [bool; crate::grid::ROWS]) {
        if let Ok(cfg) = self.params.track_effects.lock() {
            (
                std::array::from_fn(|r| cfg[r].muted),
                std::array::from_fn(|r| cfg[r].soloed),
            )
        } else {
            ([false; crate::grid::ROWS], [false; crate::grid::ROWS])
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
        let changed = if let Ok(mut cfg) = self.params.track_effects.try_lock() {
            match button {
                track_list::TrackButton::Mute => cfg[row].muted = !cfg[row].muted,
                track_list::TrackButton::Solo => cfg[row].soloed = !cfg[row].soloed,
            }
            true
        } else {
            false
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
        let hit = effect_editor::effect_hit(
            px,
            py,
            self.scale_factor,
            param_count,
            self.selected_mseg,
            trigger,
        );
        if let Some(EffectHit::Dial(i)) = hit {
            // Enum params are dropdowns, not editable text fields — let the
            // right-click fall through (left-click opens the dropdown).
            if self.param_enum_labels(i).is_some() {
                return baseview::EventStatus::Ignored;
            }
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
            let sel = self.selected_mseg.min(3);
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
        // Which Enum-format param has its dropdown open, if any.
        let open_param_dropdown = (0..crate::effects::MAX_EFFECT_PARAMS).find(|&i| {
            self.effect_dropdown
                .is_open_for(EffectAction::ParamDropdown(i))
        });
        let modulated_norms = self.compute_modulated_norms();
        effect_editor::draw_effect_section(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &track,
            self.selected_track,
            self.effect_dropdown.is_open_for(EffectAction::Kind),
            editing_dial,
            editing_mix,
            open_param_dropdown,
            &modulated_norms,
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
        let sel = self.selected_mseg.min(3);
        // Build the hover-node tooltip for the active MSEG. Amp (slot 0) shows
        // a dB readout; assignable MSEGs format through the target's ParamSpec
        // if one is set, or fall back to the raw 0..1 level.
        let tooltip: Option<(usize, String)> = self.mseg_edit.hovered_node().map(|idx| {
            let (spec, base, depth_polarity) = if sel == 0 {
                (None, 0.0, None)
            } else {
                let k = sel - 1;
                let target = modu.targets[k];
                let depth = modu.depths[k];
                let polarity = modu.msegs[sel].polarity;
                let spec = target.map(|t| {
                    use crate::effects::Effect as _;
                    let inst = crate::effects::EffectInstance::new(track.kind);
                    inst.parameters()[t]
                });
                let base = target.map(|t| track.params[t]).unwrap_or(0.0);
                (spec, base, Some((depth, polarity)))
            };
            let text =
                mseg_node_tooltip_text(sel, &modu.msegs[sel], idx, spec, base, depth_polarity);
            (idx, text)
        });
        let tooltip_ref = tooltip.as_ref().map(|(i, t)| (*i, t.as_str()));
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
            mseg_color(sel),
            tooltip_ref,
        );
        for m in 0..4 {
            if m != sel {
                let c = mseg_color(m);
                // Pack the slot colour with ghost alpha (~0x60).
                let r = (c.red() * 255.0).round() as u32;
                let g = (c.green() * 255.0).round() as u32;
                let b = (c.blue() * 255.0).round() as u32;
                let packed = (r << 24) | (g << 16) | (b << 8) | 0x60;
                widgets::mseg::draw_mseg_ghost(
                    &mut self.surface.pixmap,
                    lay.mseg_pane,
                    &modu.msegs[m],
                    &self.mseg_edit,
                    self.scale_factor,
                    packed,
                );
            }
        }
        // Playhead overlay: a thin vertical line at the active MSEG's current
        // phase, drawn over the active curve + ghosts.
        let phase =
            f32::from_bits(self.mseg_phases[self.selected_track * 4 + sel].load(Ordering::Relaxed));
        effect_editor::draw_mseg_playhead(
            &mut self.surface.pixmap,
            lay.mseg_pane,
            phase,
            self.scale_factor,
        );
        // Open MSEG dropdown (Grid / Style / right-click Transform menu) goes
        // last so it sits above every preceding layer — `draw_mseg` no longer
        // paints it inline because the ghost loop above would otherwise bury
        // it.
        widgets::mseg::draw_mseg_dropdown(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &self.mseg_edit,
            lay.mseg_pane,
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
        let Some(hit) = effect_editor::effect_hit(
            px,
            py,
            self.scale_factor,
            params,
            self.selected_mseg,
            trigger,
        ) else {
            return false;
        };
        let lay = effect_editor::effect_layout(self.scale_factor);
        match hit {
            EffectHit::Back => {
                self.view = View::Grid;
            }
            EffectHit::Kind => {
                let kind_items = effect_editor::kind_items();
                let current = EffectKind::ALL
                    .iter()
                    .position(|k| *k == self.selected_track_effect().kind)
                    .unwrap_or(0);
                let win = (self.physical_width as f32, self.physical_height as f32);
                self.effect_dropdown.open(
                    EffectAction::Kind,
                    lay.kind,
                    widgets::dropdown::DropdownList::sectioned(
                        &kind_items.items,
                        &kind_items.sections,
                    ),
                    current,
                    true,
                    win,
                );
            }
            EffectHit::Randomize => {
                self.randomize_selected_track_effect();
            }
            EffectHit::Dial(i) => {
                // Enum-format params open a dropdown over the dial slot instead
                // of being draggable. Double-click reset/text-edit/drag are
                // skipped for them — a discrete selector doesn't have a
                // continuous default to reset toward, and dragging a binary
                // value is meaningless.
                if let Some(labels) = self.param_enum_labels(i) {
                    let (dx, dy, dw, dh) = lay.dials[i];
                    let trigger_h = (dh * 0.32).max(22.0 * self.scale_factor);
                    // Match the renderer's per-label width so the popup
                    // anchors flush under the visible trigger rect.
                    let spec_format = self.selected_track_effect_param_format(i);
                    let trigger_w = effect_editor::enum_trigger_width_for(
                        &mut self.text_renderer,
                        spec_format,
                        trigger_h,
                        dw,
                        self.scale_factor,
                    );
                    let trigger_x = dx + (dw - trigger_w) * 0.5;
                    let trigger_y = dy + dh * 0.46;
                    let current = self.selected_track_effect().params[i].round() as usize;
                    let win = (self.physical_width as f32, self.physical_height as f32);
                    let items: Vec<&'static str> = labels.to_vec();
                    self.effect_dropdown.open(
                        EffectAction::ParamDropdown(i),
                        (trigger_x, trigger_y, trigger_w, trigger_h),
                        widgets::dropdown::DropdownList::flat(&items),
                        current.min(labels.len().saturating_sub(1)),
                        false,
                        win,
                    );
                } else if self.effect_click.check_and_update(EffectHit::Dial(i)) {
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
                self.selected_mseg = seg.min(3);
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
                    widgets::dropdown::DropdownList::flat(&items),
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
                let sel = self.selected_mseg.min(3);
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
                    widgets::dropdown::DropdownList::flat(&items),
                    current,
                    false,
                    win,
                );
            }
            EffectHit::TriggerRate => {
                let trigger = self.selected_track_modulation().trigger;
                let current_norm = match trigger {
                    TriggerSource::FreeHz { hz } => effects::value_to_norm(
                        hz,
                        effect_editor::TRIGGER_RATE_MIN_HZ,
                        effect_editor::TRIGGER_RATE_MAX_HZ,
                        effects::ParamScaling::Log,
                    ),
                    TriggerSource::Transient { threshold, .. } => {
                        // Sensitivity = 1 − normalised threshold.
                        1.0 - effects::value_to_norm(
                            threshold,
                            crate::modulation::TRANSIENT_THRESHOLD_MIN,
                            crate::modulation::TRANSIENT_THRESHOLD_MAX,
                            effects::ParamScaling::Log,
                        )
                    }
                    _ => return false,
                };
                if self.effect_click.check_and_update(EffectHit::TriggerRate) {
                    self.reset_trigger_rate_to_default();
                } else {
                    self.effect_dial_drag
                        .begin_drag(EffectHit::TriggerRate, current_norm, false);
                }
            }
            EffectHit::TriggerAux => {
                let trigger = self.selected_track_modulation().trigger;
                let TriggerSource::Transient { hold_ms, .. } = trigger else {
                    return false;
                };
                if self.effect_click.check_and_update(EffectHit::TriggerAux) {
                    self.reset_trigger_aux_to_default();
                } else {
                    let current_norm = effects::value_to_norm(
                        hold_ms,
                        crate::modulation::TRANSIENT_HOLD_MS_MIN,
                        crate::modulation::TRANSIENT_HOLD_MS_MAX,
                        effects::ParamScaling::Log,
                    );
                    self.effect_dial_drag
                        .begin_drag(EffectHit::TriggerAux, current_norm, false);
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
        let sel = self.selected_mseg.min(3);
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
        let k = self.selected_mseg.min(3);
        if let Ok(mut modu) = self.params.track_modulation.lock() {
            modu[row].msegs[k].sync_mode = new_mode;
        }
        self.mark_config_dirty();
    }

    /// Apply a length-slider drag value (norm) to the active MSEG. Writes
    /// `beats` or `time_seconds` depending on the active sync mode.
    fn apply_mseg_length_drag(&mut self, norm: f32) {
        let row = self.selected_track;
        let k = self.selected_mseg.min(3);
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
        let param_count = self.selected_track_param_count();
        let hit = effect_editor::effect_hit(
            px,
            py,
            self.scale_factor,
            param_count,
            self.selected_mseg,
            trigger,
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

    /// `Some(i)` when an Enum-format parameter's dropdown is currently open
    /// — `i` is the param slot index in `track.params`. `None` when no
    /// param dropdown is open (Kind / Target / Trigger dropdowns don't count).
    fn open_param_dropdown_index(&self) -> Option<usize> {
        (0..crate::effects::MAX_EFFECT_PARAMS).find(|&i| {
            self.effect_dropdown
                .is_open_for(EffectAction::ParamDropdown(i))
        })
    }

    /// The `&[&str]` labels list for the Enum-format param at slot `i` on the
    /// selected track, or `None` if that slot is out of range or not Enum-format.
    fn param_enum_labels(&self, i: usize) -> Option<&'static [&'static str]> {
        use crate::effects::Effect;
        let kind = self.selected_track_effect().kind;
        let specs = crate::effects::EffectInstance::new(kind).parameters();
        let spec = specs.get(i)?;
        match spec.format {
            crate::effects::ParamFormat::Enum { labels } => Some(labels),
            _ => None,
        }
    }

    /// Compute, for each parameter slot on the selected track, the
    /// current modulated value's normalised dial position — or `None`
    /// when that slot isn't being modulated by any assignable MSEG.
    /// Returned values are derived from the same `assignable_value`
    /// math the audio thread runs, fed with the live MSEG phase the
    /// audio thread publishes via `mseg_phases`. Driving the dial-
    /// modulation arc from this lets the editor mirror exactly what
    /// the engine is applying to each parameter in real time.
    ///
    /// Matches the engine's last-MSEG-wins ordering: if both
    /// `targets[0]` and `targets[1]` point at the same slot, the
    /// MSEG-2 (k=2) contribution is what shows on the dial.
    fn compute_modulated_norms(&self) -> [Option<(f32, u8)>; crate::effects::MAX_EFFECT_PARAMS] {
        use crate::effects::Effect;
        let mut result: [Option<(f32, u8)>; crate::effects::MAX_EFFECT_PARAMS] =
            [None; crate::effects::MAX_EFFECT_PARAMS];
        let track = self.selected_track_effect();
        let instance = crate::effects::EffectInstance::new(track.kind);
        let specs = instance.parameters();
        let Ok(modu) = self.params.track_modulation.lock() else {
            return result;
        };
        let row = self.selected_track;
        for k in 1..4 {
            // `targets` is indexed 0..2 (one per assignable MSEG —
            // msegs[1] and msegs[2]). `targets[k - 1]` is the slot
            // this MSEG modulates, or None when unassigned.
            let Some(target) = modu[row].targets[k - 1] else {
                continue;
            };
            let Some(&spec) = specs.get(target) else {
                continue;
            };
            let mseg = modu[row].msegs[k];
            let phase = f32::from_bits(self.mseg_phases[row * 4 + k].load(Ordering::Relaxed));
            let mseg_value = widgets::mseg::value_at_phase(&mseg, phase);
            let depth = modu[row].depths[k - 1];
            let modulated = crate::modulation::assignable_value(
                mseg_value,
                track.params[target],
                depth,
                spec,
                mseg.polarity,
            );
            let norm = crate::effects::value_to_norm(modulated, spec.min, spec.max, spec.scaling);
            result[target] = Some((norm, k as u8));
        }
        result
    }

    /// `ParamFormat` of the selected track's parameter `i` — used by the
    /// dropdown press handler to size the popup-anchor trigger rect the
    /// same way the renderer does.
    fn selected_track_effect_param_format(&self, i: usize) -> crate::effects::ParamFormat {
        use crate::effects::Effect;
        let kind = self.selected_track_effect().kind;
        let specs = crate::effects::EffectInstance::new(kind).parameters();
        specs
            .get(i)
            .map(|s| s.format)
            .unwrap_or(crate::effects::ParamFormat::Number {
                decimals: 0,
                unit: "",
            })
    }

    /// Apply a per-param Enum dropdown selection: write the label's index
    /// (as `f32`) into the track's `params[param_idx]` and mark dirty.
    fn apply_param_dropdown_selection(&mut self, param_idx: usize, item: usize) {
        if param_idx >= crate::effects::MAX_EFFECT_PARAMS {
            return;
        }
        if let Ok(mut eff) = self.params.track_effects.lock() {
            eff[self.selected_track].params[param_idx] = item as f32;
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

    /// Reset the primary trigger dial: `FreeHz` rate → 1.0 Hz; `Transient`
    /// threshold → its default. No-op for trigger sources without a primary
    /// dial. Marks dirty.
    fn reset_trigger_rate_to_default(&mut self) {
        if let Ok(mut cfg) = self.params.track_modulation.lock() {
            match &mut cfg[self.selected_track].trigger {
                TriggerSource::FreeHz { hz } => *hz = 1.0,
                TriggerSource::Transient { threshold, .. } => {
                    *threshold = crate::modulation::TRANSIENT_THRESHOLD_DEFAULT;
                }
                _ => return,
            }
            self.mark_config_dirty();
        }
    }

    /// Update the primary trigger dial from the rate-dial drag's normalised
    /// value: writes to `FreeHz.hz` (log-mapped through the Hz range) or
    /// `Transient.threshold` (log-mapped through the threshold range,
    /// inverted so dial-right = more sensitive). No-op otherwise.
    fn apply_trigger_rate_drag(&mut self, norm: f32) {
        if let Ok(mut cfg) = self.params.track_modulation.lock() {
            match &mut cfg[self.selected_track].trigger {
                TriggerSource::FreeHz { hz } => {
                    *hz = effects::norm_to_value(
                        norm,
                        effect_editor::TRIGGER_RATE_MIN_HZ,
                        effect_editor::TRIGGER_RATE_MAX_HZ,
                        effects::ParamScaling::Log,
                    );
                }
                TriggerSource::Transient { threshold, .. } => {
                    *threshold = effects::norm_to_value(
                        1.0 - norm,
                        crate::modulation::TRANSIENT_THRESHOLD_MIN,
                        crate::modulation::TRANSIENT_THRESHOLD_MAX,
                        effects::ParamScaling::Log,
                    );
                }
                _ => return,
            }
            self.mark_config_dirty();
        }
    }

    /// Reset the `Transient` Hold dial to its default. No-op when the active
    /// trigger source is not `Transient`. Marks dirty.
    fn reset_trigger_aux_to_default(&mut self) {
        if let Ok(mut cfg) = self.params.track_modulation.lock() {
            if let TriggerSource::Transient { hold_ms, .. } = &mut cfg[self.selected_track].trigger
            {
                *hold_ms = crate::modulation::TRANSIENT_HOLD_MS_DEFAULT;
                self.mark_config_dirty();
            }
        }
    }

    /// Update `Transient.hold_ms` from the aux-dial drag's normalised value.
    fn apply_trigger_aux_drag(&mut self, norm: f32) {
        let new_hold_ms = effects::norm_to_value(
            norm,
            crate::modulation::TRANSIENT_HOLD_MS_MIN,
            crate::modulation::TRANSIENT_HOLD_MS_MAX,
            effects::ParamScaling::Log,
        );
        if let Ok(mut cfg) = self.params.track_modulation.lock() {
            if let TriggerSource::Transient { hold_ms, .. } = &mut cfg[self.selected_track].trigger
            {
                *hold_ms = new_hold_ms;
                self.mark_config_dirty();
            }
        }
    }

    /// Apply a trigger-dropdown selection: convert the item index to a
    /// `TriggerSource` (carrying the current Hz / threshold / hold so a
    /// quick toggle through trigger sources keeps the dials' last position),
    /// write it, mark dirty.
    fn apply_trigger_selection(&mut self, idx: usize) {
        let (carried_hz, carried_threshold, carried_hold_ms) =
            match self.selected_track_modulation().trigger {
                TriggerSource::FreeHz { hz } => (
                    hz,
                    crate::modulation::TRANSIENT_THRESHOLD_DEFAULT,
                    crate::modulation::TRANSIENT_HOLD_MS_DEFAULT,
                ),
                TriggerSource::Transient { threshold, hold_ms } => (1.0, threshold, hold_ms),
                _ => (
                    1.0,
                    crate::modulation::TRANSIENT_THRESHOLD_DEFAULT,
                    crate::modulation::TRANSIENT_HOLD_MS_DEFAULT,
                ),
            };
        let new_trigger =
            effect_editor::trigger_from_item(idx, carried_hz, carried_threshold, carried_hold_ms);
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
        // One shared `effect_dropdown` state handles Kind, Target, and Trigger;
        // the items list depends on which one is open.
        if self.view == View::Effect && self.effect_dropdown.is_open() {
            let kind = self.selected_track_effect().kind;
            let items: Vec<&'static str> = if self.effect_dropdown.is_open_for(EffectAction::Target)
            {
                effect_editor::target_items(kind)
            } else if self.effect_dropdown.is_open_for(EffectAction::Trigger) {
                effect_editor::trigger_items().to_vec()
            } else if let Some(idx) = self.open_param_dropdown_index() {
                self.param_enum_labels(idx)
                    .map(|labs| labs.to_vec())
                    .unwrap_or_default()
            } else {
                effect_editor::kind_items().items
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
                        let current = match self.selected_track_modulation().trigger {
                            TriggerSource::FreeHz { hz } => effects::value_to_norm(
                                hz,
                                effect_editor::TRIGGER_RATE_MIN_HZ,
                                effect_editor::TRIGGER_RATE_MAX_HZ,
                                effects::ParamScaling::Log,
                            ),
                            TriggerSource::Transient { threshold, .. } => {
                                1.0 - effects::value_to_norm(
                                    threshold,
                                    crate::modulation::TRANSIENT_THRESHOLD_MIN,
                                    crate::modulation::TRANSIENT_THRESHOLD_MAX,
                                    effects::ParamScaling::Log,
                                )
                            }
                            _ => 0.5,
                        };
                        if let Some(norm) = self.effect_dial_drag.update_drag(shift, current) {
                            self.apply_trigger_rate_drag(norm);
                        }
                    }
                    Some(EffectHit::TriggerAux) => {
                        let current = match self.selected_track_modulation().trigger {
                            TriggerSource::Transient { hold_ms, .. } => effects::value_to_norm(
                                hold_ms,
                                crate::modulation::TRANSIENT_HOLD_MS_MIN,
                                crate::modulation::TRANSIENT_HOLD_MS_MAX,
                                effects::ParamScaling::Log,
                            ),
                            _ => 0.5,
                        };
                        if let Some(norm) = self.effect_dial_drag.update_drag(shift, current) {
                            self.apply_trigger_aux_drag(norm);
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
                // Dropdown popup hover.
                if self.speed_dropdown.is_open() {
                    let items = toolbar::speed_items();
                    let win = (self.physical_width as f32, self.physical_height as f32);
                    self.speed_dropdown.on_mouse_move(px, py, &items, win);
                }
                if self.effect_dropdown.is_open() {
                    let kind = self.selected_track_effect().kind;
                    let items: Vec<&'static str> =
                        if self.effect_dropdown.is_open_for(EffectAction::Target) {
                            effect_editor::target_items(kind)
                        } else if self.effect_dropdown.is_open_for(EffectAction::Trigger) {
                            effect_editor::trigger_items().to_vec()
                        } else if let Some(idx) = self.open_param_dropdown_index() {
                            self.param_enum_labels(idx)
                                .map(|labs| labs.to_vec())
                                .unwrap_or_default()
                        } else {
                            effect_editor::kind_items().items
                        };
                    let win = (self.physical_width as f32, self.physical_height as f32);
                    self.effect_dropdown.on_mouse_move(px, py, &items, win);
                }
                // MSEG node-drag follow: when the user is dragging a node, the
                // pointer can leave the pane rect and the drag should keep
                // tracking (matches miff's behaviour).
                if self.view == View::Effect {
                    let lay = effect_editor::effect_layout(self.scale_factor);
                    let sel = self.selected_mseg.min(3);
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
                if self.effect_dropdown.is_open() {
                    let kind = self.selected_track_effect().kind;
                    let items: Vec<&'static str> =
                        if self.effect_dropdown.is_open_for(EffectAction::Target) {
                            effect_editor::target_items(kind)
                        } else if self.effect_dropdown.is_open_for(EffectAction::Trigger) {
                            effect_editor::trigger_items().to_vec()
                        } else if let Some(idx) = self.open_param_dropdown_index() {
                            // Enum param's label list, for the popup items.
                            self.param_enum_labels(idx)
                                .map(|labs| labs.to_vec())
                                .unwrap_or_default()
                        } else {
                            effect_editor::kind_items().items
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
                            EffectAction::ParamDropdown(param_idx) => {
                                self.apply_param_dropdown_selection(param_idx, idx);
                            }
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
                    Some(ToolbarControl::Speed) => self.open_speed_dropdown(),
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
                                    self.mseg_edit.clear_selection();
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
                let status = self.on_right_press();
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
                if self.effect_dropdown.is_open() {
                    let kind = self.selected_track_effect().kind;
                    let items: Vec<&'static str> =
                        if self.effect_dropdown.is_open_for(EffectAction::Target) {
                            effect_editor::target_items(kind)
                        } else if self.effect_dropdown.is_open_for(EffectAction::Trigger) {
                            effect_editor::trigger_items().to_vec()
                        } else if let Some(idx) = self.open_param_dropdown_index() {
                            self.param_enum_labels(idx)
                                .map(|labs| labs.to_vec())
                                .unwrap_or_default()
                        } else {
                            effect_editor::kind_items().items
                        };
                    self.effect_dropdown.on_wheel(dy, &items, win);
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
                let _ = self.effect_dial_drag.end_drag();
                self.effect_dropdown.on_mouse_up();
                self.speed_dropdown.on_mouse_up();
                // Drop a drag-and-drop track reorder. If the cursor is over a
                // different track row, swap places (and update the selected
                // track so the dragged content stays in view at its new
                // position). Same-row or off-list releases just cancel.
                if let Some(drag) = self.track_drag.take() {
                    let (px, py) = self.mouse_pos;
                    if let Some(to) = track_list::track_at(px, py, self.scale_factor) {
                        if to != drag.from {
                            self.swap_tracks(drag.from, to);
                            self.selected_track = to;
                        }
                    }
                }
                // A release always terminates any in-flight MSEG node drag,
                // regardless of where the cursor is.
                if self.view == View::Effect {
                    let sel = self.selected_mseg.min(3);
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
            baseview::Event::Keyboard(ev) if self.effect_dropdown.is_open() => {
                // Swallow key-ups so the host DAW doesn't act on Enter/Esc
                // releases. The dropdown only cares about key-downs.
                if ev.state != keyboard_types::KeyState::Down {
                    return baseview::EventStatus::Captured;
                }
                let kind = self.selected_track_effect().kind;
                let items: Vec<&'static str> =
                    if self.effect_dropdown.is_open_for(EffectAction::Target) {
                        effect_editor::target_items(kind)
                    } else if self.effect_dropdown.is_open_for(EffectAction::Trigger) {
                        effect_editor::trigger_items().to_vec()
                    } else if let Some(idx) = self.open_param_dropdown_index() {
                        self.param_enum_labels(idx)
                            .map(|labs| labs.to_vec())
                            .unwrap_or_default()
                    } else {
                        effect_editor::kind_items().items
                    };
                let win = (self.physical_width as f32, self.physical_height as f32);
                let dd_key = match &ev.key {
                    keyboard_types::Key::ArrowUp => Some(widgets::dropdown::DropdownKey::Up),
                    keyboard_types::Key::ArrowDown => Some(widgets::dropdown::DropdownKey::Down),
                    keyboard_types::Key::Enter => Some(widgets::dropdown::DropdownKey::Enter),
                    keyboard_types::Key::Escape => Some(widgets::dropdown::DropdownKey::Esc),
                    keyboard_types::Key::Backspace => {
                        Some(widgets::dropdown::DropdownKey::Backspace)
                    }
                    keyboard_types::Key::PageUp => Some(widgets::dropdown::DropdownKey::PageUp),
                    keyboard_types::Key::PageDown => Some(widgets::dropdown::DropdownKey::PageDown),
                    keyboard_types::Key::Home => Some(widgets::dropdown::DropdownKey::Home),
                    keyboard_types::Key::End => Some(widgets::dropdown::DropdownKey::End),
                    _ => None,
                };
                let event = if let Some(k) = dd_key {
                    self.effect_dropdown.on_key(k, &items, win)
                } else if let keyboard_types::Key::Character(s) = &ev.key {
                    let mut last = None;
                    for c in s.chars() {
                        if let Some(e) = self.effect_dropdown.on_char(c, &items) {
                            last = Some(e);
                        }
                    }
                    last
                } else {
                    None
                };
                if let Some(widgets::dropdown::DropdownEvent::Selected(action, idx)) = event {
                    match action {
                        EffectAction::Kind => {
                            let kind = EffectKind::ALL[idx.min(EffectKind::ALL.len() - 1)];
                            self.apply_kind_switch(kind);
                        }
                        EffectAction::Target => self.apply_target_selection(idx),
                        EffectAction::Trigger => self.apply_trigger_selection(idx),
                        EffectAction::ParamDropdown(param_idx) => {
                            self.apply_param_dropdown_selection(param_idx, idx);
                        }
                    }
                }
                return baseview::EventStatus::Captured;
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
                            let sel = self.selected_mseg.min(3);
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

/// Format the tooltip text shown when a node is hovered on the MSEG editor.
///
/// The tooltip is rendered as two lines:
///
/// 1. **Position** — `X N%  Y …` where N is the node's time as a percentage
///    and the Y readout reflects the MSEG's polarity: unipolar prints the
///    raw `0..1` value, bipolar maps `0..1` to `-1..+1` (so the midline
///    `0.5` is `0.00`).
/// 2. **Dial value** — for slot `0` (Amp), the dB equivalent of the node's
///    linear-gain value; for slots `1`/`2` with a target set, the mapped
///    parameter value via `assignable_value` + `format_value`. Omitted
///    when the assignable MSEG has no target.
///
/// Returns an empty string if `node_idx` is out of range.
pub fn mseg_node_tooltip_text(
    slot: usize,
    data: &widgets::MsegData,
    node_idx: usize,
    spec: Option<crate::effects::ParamSpec>,
    base: f32,
    depth_polarity: Option<(f32, widgets::Polarity)>,
) -> String {
    if node_idx >= data.node_count {
        return String::new();
    }
    let value = data.nodes[node_idx].value;
    let time = data.nodes[node_idx].time;

    // Position line — Y display flips polarity-aware: bipolar shows
    // `(value - 0.5) * 2` so the midline reads `0.00` and the extremes
    // read `±1.00`; unipolar prints the raw `0..1` value.
    let y_str = match data.polarity {
        widgets::Polarity::Unipolar => format!("{value:.2}"),
        widgets::Polarity::Bipolar => format!("{:+.2}", (value - 0.5) * 2.0),
    };
    let pos_line = format!("X {:.0}%  Y {}", time * 100.0, y_str);

    // Dial-value line — slot-dependent.
    let dial_line: Option<String> = if slot == 0 {
        const FLOOR_DB: f32 = -80.0;
        if value <= 1e-4 {
            Some("-\u{221e} dB".to_string())
        } else {
            let db = (20.0 * value.log10()).max(FLOOR_DB);
            Some(format!("{db:.1} dB"))
        }
    } else {
        match (spec, depth_polarity) {
            (Some(spec), Some((depth, polarity))) => {
                let v = crate::modulation::assignable_value(value, base, depth, spec, polarity);
                Some(crate::effects::format_value(v, spec.format))
            }
            _ => None,
        }
    };

    match dial_line {
        Some(d) => format!("{pos_line}\n{d}"),
        None => pos_line,
    }
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

    #[test]
    fn mseg_color_returns_the_four_slot_hues_and_clamps_oob() {
        use tiny_skia::Color;
        let amp = mseg_color(0);
        let m1 = mseg_color(1);
        let m2 = mseg_color(2);
        let m3 = mseg_color(3);
        // Four distinct colours.
        let all = [amp, m1, m2, m3];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(rgb8(all[i]), rgb8(all[j]), "slots {i} and {j} share a hue");
            }
        }
        // Amp matches the existing accent (sky blue 0x4fc3f7).
        assert_eq!(rgb8(amp), (0x4f, 0xc3, 0xf7));
        // MSEG 1 amber.
        assert_eq!(rgb8(m1), (0xff, 0xc8, 0x58));
        // MSEG 2 purple.
        assert_eq!(rgb8(m2), (0xc3, 0x78, 0xff));
        // MSEG 3 mint.
        assert_eq!(rgb8(m3), (0x66, 0xd9, 0xa0));
        // OOB clamps to MSEG 3 (the highest valid slot).
        assert_eq!(rgb8(mseg_color(99)), rgb8(m3));

        fn rgb8(c: Color) -> (u8, u8, u8) {
            let r = (c.red() * 255.0).round() as u8;
            let g = (c.green() * 255.0).round() as u8;
            let b = (c.blue() * 255.0).round() as u8;
            (r, g, b)
        }
    }

    #[test]
    fn node_tooltip_text_for_amp_has_position_line_and_db_readout() {
        let mut data = widgets::MsegData::default();
        // node 1 sits at time = 1.0, value = 1.0 by default.
        data.nodes[1].value = 1.0;
        let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
        // Two lines: position then dial.
        assert_eq!(text, "X 100%  Y 1.00\n0.0 dB");

        data.nodes[1].value = 0.5;
        let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
        // Position: X = 100% (default last-node time); Y = 0.50 (unipolar).
        // Dial: 20·log10(0.5) ≈ -6.02 dB.
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines[0], "X 100%  Y 0.50");
        assert!(
            lines[1].starts_with("-6.0"),
            "expected ~-6.0 dB on the dial line, got {}",
            lines[1]
        );

        data.nodes[1].value = 0.00001;
        let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
        // The floor case keeps the position line and emits -∞ dB on the dial line.
        assert_eq!(text, "X 100%  Y 0.00\n-\u{221e} dB");
    }

    #[test]
    fn node_tooltip_text_bipolar_y_maps_value_to_minus_one_plus_one() {
        let mut data = widgets::MsegData::default();
        data.polarity = widgets::Polarity::Bipolar;
        data.nodes[1].value = 0.5; // midline → 0.00
        let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
        let pos_line = text.split('\n').next().unwrap();
        assert!(
            pos_line.contains("Y +0.00"),
            "midline should display as +0.00 in bipolar, got {pos_line}"
        );

        data.nodes[1].value = 0.0; // bottom → -1.00
        let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
        assert!(
            text.split('\n').next().unwrap().contains("Y -1.00"),
            "value 0 should display as -1.00 in bipolar, got {text}"
        );

        data.nodes[1].value = 1.0; // top → +1.00
        let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
        assert!(
            text.split('\n').next().unwrap().contains("Y +1.00"),
            "value 1 should display as +1.00 in bipolar, got {text}"
        );
    }

    #[test]
    fn node_tooltip_text_x_reflects_node_time() {
        let mut data = widgets::MsegData::default();
        // Insert a middle node at time 0.42; it lands at active-array index 1
        // and pushes the original endpoint to index 2.
        let i = data.insert_node(0.42, 0.5).unwrap();
        let text = mseg_node_tooltip_text(0, &data, i, None, 0.0, None);
        let pos_line = text.split('\n').next().unwrap();
        assert!(
            pos_line.starts_with("X 42%"),
            "X should reflect the node's time, got {pos_line}"
        );
    }

    #[test]
    fn node_tooltip_text_assignable_no_target_omits_the_dial_line() {
        let mut data = widgets::MsegData::default();
        data.nodes[1].value = 0.742;
        // Slot 1, no spec → position line only, no second line.
        let text = mseg_node_tooltip_text(1, &data, 1, None, 0.0, None);
        assert!(
            !text.contains('\n'),
            "no-target tooltip should be a single position line, got {text}"
        );
        assert_eq!(text, "X 100%  Y 0.74");
    }

    #[test]
    fn node_tooltip_text_out_of_range_node_returns_empty() {
        let data = widgets::MsegData::default();
        // Default MsegData has node_count == 2, so index 99 is OOB.
        let text = mseg_node_tooltip_text(0, &data, 99, None, 0.0, None);
        assert_eq!(text, "");
    }
}
