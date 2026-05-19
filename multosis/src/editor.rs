//! Softbuffer + tiny-skia CPU editor for Multosis.
//!
//! Milestone 1b-ii-a: opens the window and renders the grid + live wavefront.
//! Interaction (cell editing, loop-region drag, toolbar) is Milestone 1b-ii-b.

use baseview::{WindowOpenOptions, WindowScalePolicy};
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::editor::toolbar::{ToolbarControl, ToolbarOp};
use crate::grid::LoopRegion;
use crate::handoff::GridHandoff;
use crate::region::RegionSnapshot;
use crate::seq_status::SeqStatusDisplay;
use crate::wavefront_display::WavefrontDisplay;
use crate::MultosisParams;
use tiny_skia_widgets as widgets;

pub mod grid_view;
pub mod toolbar;

/// Editor window size. Derived from the grid layout in `grid_view`:
/// width  = 2*MARGIN + COLS*CELL + 3*GROUP_GAP = 16 + 1280 + 24 + 16
/// height = STATUS_H + GUTTER + ROWS*CELL + MARGIN = 88 + 14 + 640 + 16
/// (kept in sync by the `window_size_matches_the_grid` test).
pub const WINDOW_WIDTH: u32 = 1336;
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
            mouse_pos: (0.0, 0.0),
            text_renderer,
            gui_context,
            reset_request,
            toolbar_drag: widgets::DragState::new(),
            clipboard: None,
            rng_seed: 1,
            left_gesture: None,
            grid_cache: grid_view::GridCache::new(pw, ph),
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
            ToolbarControl::Bank => {
                use crate::effects::EffectBank;
                let next = match self.params.effect_bank.value() {
                    EffectBank::Lowpass => EffectBank::Bitcrush,
                    EffectBank::Bitcrush => EffectBank::Lowpass,
                };
                setter.begin_set_parameter(&self.params.effect_bank);
                setter.set_parameter(&self.params.effect_bank, next);
                setter.end_set_parameter(&self.params.effect_bank);
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

    fn draw(&mut self) {
        let grid = self.params.grid.lock().map(|g| *g).unwrap_or_default();
        // Update the cache (full rebuild on scale change; otherwise only
        // re-renders cells that changed).
        self.grid_cache.update(&grid, self.scale_factor);
        // Blit the cached cells into the surface — replaces both the old
        // fill_pixmap_opaque clear and draw_grid_cells.
        self.surface
            .pixmap
            .data_mut()
            .copy_from_slice(self.grid_cache.pixmap().data());
        // Cursor-dependent overlay (loop-region outline, nubs, move grip).
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
        toolbar::draw_toolbar(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &self.params,
            &self.seq_status,
            self.scale_factor,
        );
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
                if let Some(&ctrl) = self.toolbar_drag.active_action() {
                    let current = self.slider_normalized(ctrl);
                    if let Some(norm) = self.toolbar_drag.update_drag(false, current) {
                        self.set_slider(ctrl, norm);
                    }
                }
                let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
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
                            self.left_gesture = Some(LeftGesture::GridPaint { value, last: cur });
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
                            self.left_gesture = Some(LeftGesture::GridPaint { value, last: cur });
                        }
                    }
                    None => {}
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                let (px, py) = self.mouse_pos;
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
                    },
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Right,
                ..
            }) => {
                self.handle_grid_click(true);
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                if let Some(ctrl) = self.toolbar_drag.end_drag() {
                    self.end_slider(ctrl);
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
    pending_resize: Arc<AtomicU64>,
}

/// Build the editor.
pub fn create(
    params: Arc<MultosisParams>,
    wavefront_display: Arc<WavefrontDisplay>,
    seq_status: Arc<SeqStatusDisplay>,
    grid_handoff: Arc<GridHandoff>,
    reset_request: Arc<AtomicBool>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(MultosisEditor {
        params,
        wavefront_display,
        seq_status,
        grid_handoff,
        reset_request,
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
