//! Softbuffer-based editor for miff. CPU rendering via tiny-skia.
//!
//! Layout (880×620, freely resizable):
//! - Top region (~55%): MSEG editor (curve-only mode)
//! - Middle region (~29%): Filter response view placeholder (Task 12)
//! - Bottom strip (~16%): Controls placeholder (Task 10)

use baseview::{WindowOpenOptions, WindowScalePolicy};
use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tiny_skia_widgets as widgets;
use tiny_skia_widgets::mseg::MsegEditState;

use crate::MiffParams;

pub const WINDOW_WIDTH: u32 = 880;
pub const WINDOW_HEIGHT: u32 = 620;
// Used in the Resized event handler; wired into host-resize in Task 13.
#[allow(dead_code)]
const MIN_WIDTH: u32 = 680;
#[allow(dead_code)]
const MIN_HEIGHT: u32 = 480;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

// ── Hit actions ─────────────────────────────────────────────────────────

/// Hit actions for miff's editor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HitAction {
    ModeSelector,
    MixDial,
    GainDial,
    LengthDial,
}

// ── Layout ───────────────────────────────────────────────────────────────

/// Compute the three editor regions from physical pixel dimensions.
/// Returns `(mseg_rect, response_rect, strip_rect)` each as `(x, y, w, h)`.
#[allow(clippy::type_complexity)]
pub(crate) fn layout(w: f32, h: f32) -> (
    (f32, f32, f32, f32),
    (f32, f32, f32, f32),
    (f32, f32, f32, f32),
) {
    // full-width regions; sub-widgets have their own internal padding
    let mseg_h = h * 0.55;
    let response_h = h * 0.29;
    let strip_h = h - mseg_h - response_h;

    let mseg_rect = (0.0, 0.0, w, mseg_h);
    let response_rect = (0.0, mseg_h, w, response_h);
    let strip_rect = (0.0, mseg_h + response_h, w, strip_h);

    (mseg_rect, response_rect, strip_rect)
}

// ── Strip layout helper ──────────────────────────────────────────────────

/// Sizing constants for the bottom control strip.
const STRIP_PAD: f32 = 8.0;
const MODE_W: f32 = 160.0;
const MODE_H: f32 = 28.0;
const DIAL_RADIUS: f32 = 26.0;

/// Compute the four control rects within `strip_rect` at `scale`.
///
/// Returns `Vec<((x, y, w, h), HitAction)>` for each control, left→right:
/// Mode selector, Mix dial, Gain dial, Length dial.
/// This is a pure function and is unit-testable without a window.
pub(crate) fn strip_regions(
    strip_rect: (f32, f32, f32, f32),
    scale: f32,
) -> Vec<((f32, f32, f32, f32), HitAction)> {
    let (sx, sy, sw, sh) = strip_rect;
    let pad = STRIP_PAD * scale;
    let mode_w = MODE_W * scale;
    let mode_h = MODE_H * scale;

    // Mode selector: left-aligned, vertically centred in the strip.
    let mode_x = sx + pad;
    let mode_y = sy + (sh - mode_h) * 0.5;

    // Three dials share the remaining horizontal space to the right.
    let dials_x_start = mode_x + mode_w + pad;
    let dials_region_w = (sx + sw) - dials_x_start - pad;
    let dial_hit = DIAL_RADIUS * 3.2 * scale;
    let dial_cy = sy + sh * 0.5;

    // Place three dials evenly across dials_region_w.
    let dial_labels = [
        (HitAction::MixDial, 0usize),
        (HitAction::GainDial, 1),
        (HitAction::LengthDial, 2),
    ];
    let n_dials = 3_f32;
    let slot_w = dials_region_w / n_dials;

    let mut regions = Vec::with_capacity(4);

    regions.push(((mode_x, mode_y, mode_w, mode_h), HitAction::ModeSelector));

    for (action, idx) in dial_labels {
        let cx = dials_x_start + slot_w * (idx as f32 + 0.5);
        let rx = cx - dial_hit * 0.5;
        let ry = dial_cy - dial_hit * 0.5;
        regions.push(((rx, ry, dial_hit, dial_hit), action));
    }

    regions
}

// ── Window handler ──────────────────────────────────────────────────────

// Fields wired up in Tasks 10-12
#[allow(dead_code)]
struct MiffWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    shared_scale: Arc<AtomicCell<f32>>,
    pending_resize: Arc<AtomicU64>,

    params: Arc<MiffParams>,
    text_renderer: widgets::TextRenderer,
    drag: widgets::DragState<HitAction>,
    text_edit: widgets::TextEditState<HitAction>,

    /// GUI → audio kernel handoff (used when re-baking in Task 11).
    kernel_handoff: Arc<crate::kernel::KernelHandoff>,
    /// Curve-only MSEG editor state.
    mseg_state: MsegEditState,
    /// Input spectrum shadow for the response view (Task 12).
    input_spectrum: Arc<Mutex<Vec<f32>>>,
}

// Methods wired up in Tasks 10-12 (draw path) and Task 13 (spawn).
#[allow(dead_code)]
impl MiffWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<MiffParams>,
        kernel_handoff: Arc<crate::kernel::KernelHandoff>,
        input_spectrum: Arc<Mutex<Vec<f32>>>,
        shared_scale: Arc<AtomicCell<f32>>,
        pending_resize: Arc<AtomicU64>,
        scale_factor: f32,
    ) -> Self {
        let pw = (WINDOW_WIDTH as f32 * scale_factor).round() as u32;
        let ph = (WINDOW_HEIGHT as f32 * scale_factor).round() as u32;

        let surface = widgets::SoftbufferSurface::new(window, pw, ph);

        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let text_renderer = widgets::TextRenderer::new(font_data);

        Self {
            gui_context,
            surface,
            physical_width: pw,
            physical_height: ph,
            scale_factor,
            shared_scale,
            pending_resize,
            params,
            text_renderer,
            drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
            kernel_handoff,
            mseg_state: MsegEditState::new_curve_only(),
            input_spectrum,
        }
    }

    fn draw(&mut self) {
        let s = self.scale_factor;
        let w = self.physical_width as f32;
        let h = self.physical_height as f32;

        self.drag.clear_regions();
        self.surface.pixmap.fill(widgets::color_bg());

        let (mseg_rect, response_rect, strip_rect) = layout(w, h);

        // ── MSEG region (top ~55%) ──────────────────────────────────────────
        // Lock the curve, copy the `Copy` value out, then release before drawing.
        let curve = self
            .params
            .curve
            .lock()
            .map(|c| *c)
            .unwrap_or_default();

        widgets::mseg::draw_mseg(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            mseg_rect,
            &curve,
            &self.mseg_state,
            s,
        );

        // ── Response region (middle ~29%) — placeholder until Task 12 ──────
        {
            let (rx, ry, rw, rh) = response_rect;
            widgets::draw_rect(
                &mut self.surface.pixmap,
                rx,
                ry,
                rw,
                rh,
                widgets::color_control_bg(),
            );
            widgets::draw_rect_outline(
                &mut self.surface.pixmap,
                rx,
                ry,
                rw,
                rh,
                widgets::color_border(),
                1.0,
            );
        }

        // ── Bottom strip (~16%) ────────────────────────────────────────────
        self.draw_strip(strip_rect, s);
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }

    /// Draw the bottom control strip: Mode stepped selector + Mix/Gain/Length dials.
    /// Also registers hit regions into `self.drag` for all four controls.
    fn draw_strip(&mut self, strip_rect: (f32, f32, f32, f32), scale: f32) {
        use nih_plug::prelude::Param;

        let regions = strip_regions(strip_rect, scale);

        // Mode selector active index: Raw = 0, Phaseless = 1.
        let mode_idx = if self.params.mode.value() == crate::MiffMode::Raw {
            0usize
        } else {
            1
        };

        for ((rx, ry, rw, rh), action) in regions {
            match action {
                HitAction::ModeSelector => {
                    widgets::draw_stepped_selector(
                        &mut self.surface.pixmap,
                        &mut self.text_renderer,
                        rx,
                        ry,
                        rw,
                        rh,
                        &["Raw", "Phaseless"],
                        mode_idx,
                    );
                }
                HitAction::MixDial => {
                    let p = &self.params.mix;
                    let unmod = p.unmodulated_normalized_value();
                    let modulated = p.modulated_normalized_value();
                    let value_text = p.normalized_value_to_string(modulated, true);
                    let editing_buf = self
                        .text_edit
                        .active_for(&HitAction::MixDial)
                        .map(str::to_owned);
                    let caret = self.text_edit.caret_visible();
                    let cx = rx + rw * 0.5;
                    let cy = ry + rh * 0.5;
                    let radius = DIAL_RADIUS * scale;
                    widgets::draw_dial_ex(
                        &mut self.surface.pixmap,
                        &mut self.text_renderer,
                        cx,
                        cy,
                        radius,
                        "Mix",
                        &value_text,
                        unmod,
                        Some(modulated),
                        editing_buf.as_deref(),
                        caret,
                    );
                }
                HitAction::GainDial => {
                    let p = &self.params.gain;
                    let unmod = p.unmodulated_normalized_value();
                    let modulated = p.modulated_normalized_value();
                    let value_text = p.normalized_value_to_string(modulated, true);
                    let editing_buf = self
                        .text_edit
                        .active_for(&HitAction::GainDial)
                        .map(str::to_owned);
                    let caret = self.text_edit.caret_visible();
                    let cx = rx + rw * 0.5;
                    let cy = ry + rh * 0.5;
                    let radius = DIAL_RADIUS * scale;
                    widgets::draw_dial_ex(
                        &mut self.surface.pixmap,
                        &mut self.text_renderer,
                        cx,
                        cy,
                        radius,
                        "Gain",
                        &value_text,
                        unmod,
                        Some(modulated),
                        editing_buf.as_deref(),
                        caret,
                    );
                }
                HitAction::LengthDial => {
                    let p = &self.params.length;
                    let unmod = p.unmodulated_normalized_value();
                    let modulated = p.modulated_normalized_value();
                    let value_text = p.normalized_value_to_string(modulated, true);
                    let editing_buf = self
                        .text_edit
                        .active_for(&HitAction::LengthDial)
                        .map(str::to_owned);
                    let caret = self.text_edit.caret_visible();
                    let cx = rx + rw * 0.5;
                    let cy = ry + rh * 0.5;
                    let radius = DIAL_RADIUS * scale;
                    widgets::draw_dial_ex(
                        &mut self.surface.pixmap,
                        &mut self.text_renderer,
                        cx,
                        cy,
                        radius,
                        "Length",
                        &value_text,
                        unmod,
                        Some(modulated),
                        editing_buf.as_deref(),
                        caret,
                    );
                }
            }
            // Register hit region for every control.
            self.drag.push_region(rx, ry, rw, rh, action);
        }
    }
}

impl baseview::WindowHandler for MiffWindow {
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
                self.physical_width = info.physical_size().width.max(MIN_WIDTH);
                self.physical_height = info.physical_size().height.max(MIN_HEIGHT);
                let sf = (self.physical_width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
                self.scale_factor = sf;
                self.shared_scale.store(sf);
                self.resize_buffers();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorEntered) => {
                self.drag.on_cursor_entered();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorLeft) => {
                self.drag.on_cursor_left();
            }
            baseview::Event::Keyboard(_) => {
                // Full keyboard routing (TextEditState) is wired in Task 11;
                // until then, don't swallow the host's keyboard shortcuts.
                return baseview::EventStatus::Ignored;
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved { .. }) => {
                // Full mouse routing wired in Task 11.
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed { .. }) => {
                // Full mouse routing wired in Task 11.
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased { .. }) => {
                // Full mouse routing wired in Task 11.
            }
            _ => {}
        }

        baseview::EventStatus::Captured
    }
}

// ── Editor trait implementation ─────────────────────────────────────────

// Wired into Plugin::editor() in Task 13.
#[allow(dead_code)]
pub(crate) struct MiffEditor {
    params: Arc<MiffParams>,
    kernel_handoff: Arc<crate::kernel::KernelHandoff>,
    input_spectrum: Arc<Mutex<Vec<f32>>>,
    scaling_factor: Arc<AtomicCell<f32>>,
    pending_resize: Arc<AtomicU64>,
}

// Called from Plugin::editor() in Task 13.
#[allow(dead_code)]
pub(crate) fn create(
    params: Arc<MiffParams>,
    kernel_handoff: Arc<crate::kernel::KernelHandoff>,
    input_spectrum: Arc<Mutex<Vec<f32>>>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(MiffEditor {
        params,
        kernel_handoff,
        input_spectrum,
        scaling_factor: Arc::new(AtomicCell::new(1.0)),
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}

impl Editor for MiffEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
        self.scaling_factor.store(sf);

        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let kernel_handoff = Arc::clone(&self.kernel_handoff);
        let input_spectrum = Arc::clone(&self.input_spectrum);
        let shared_scale = Arc::clone(&self.scaling_factor);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("miff"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                MiffWindow::new(
                    window,
                    gui_context,
                    params,
                    kernel_handoff,
                    input_spectrum,
                    shared_scale,
                    pending_resize,
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

    fn set_scale_factor(&self, factor: f32) -> bool {
        if self.params.editor_state.is_open() {
            return false;
        }
        self.scaling_factor.store(factor);
        true
    }

    fn set_size(&self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        let packed = ((width as u64) << 32) | (height as u64);
        self.pending_resize.store(packed, Ordering::Relaxed);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_regions_stack_without_overlap() {
        let (mseg, response, strip) = layout(880.0, 620.0);
        // mseg above response above strip, no overlap.
        // mseg bottom <= response top (within float tolerance)
        assert!(
            mseg.1 + mseg.3 <= response.1 + 0.01,
            "mseg bottom ({}) > response top ({})",
            mseg.1 + mseg.3,
            response.1
        );
        // response bottom <= strip top
        assert!(
            response.1 + response.3 <= strip.1 + 0.01,
            "response bottom ({}) > strip top ({})",
            response.1 + response.3,
            strip.1
        );
    }

    #[test]
    fn layout_regions_cover_full_height() {
        let h = 620.0_f32;
        let (mseg, response, strip) = layout(880.0, h);
        let total = mseg.3 + response.3 + strip.3;
        assert!(
            (total - h).abs() < 0.1,
            "regions don't sum to window height: {total} vs {h}"
        );
    }

    #[test]
    fn layout_regions_share_full_width() {
        let w = 880.0_f32;
        let (mseg, response, strip) = layout(w, 620.0);
        assert!((mseg.2 - w).abs() < 0.01);
        assert!((response.2 - w).abs() < 0.01);
        assert!((strip.2 - w).abs() < 0.01);
    }

    #[test]
    fn strip_regions_are_within_strip_and_disjoint() {
        let w = 880.0_f32;
        let h = 620.0_f32;
        let (_, _, strip_rect) = layout(w, h);
        let (sx, sy, sw, sh) = strip_rect;

        let regions = strip_regions(strip_rect, 1.0);
        assert_eq!(regions.len(), 4, "expected exactly 4 control regions");

        // All rects must lie inside the strip.
        for ((rx, ry, rw, rh), action) in &regions {
            assert!(
                rx >= &sx && ry >= &sy && rx + rw <= sx + sw + 0.5 && ry + rh <= sy + sh + 0.5,
                "{action:?} rect ({rx},{ry},{rw},{rh}) falls outside strip ({sx},{sy},{sw},{sh})"
            );
        }

        // All rects must be pairwise non-overlapping.
        for i in 0..regions.len() {
            for j in (i + 1)..regions.len() {
                let ((ax, ay, aw, ah), a_action) = regions[i];
                let ((bx, by, bw, bh), b_action) = regions[j];
                let overlap = ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by;
                assert!(
                    !overlap,
                    "{a_action:?} and {b_action:?} rects overlap"
                );
            }
        }
    }

    #[test]
    fn draw_smoke_headless() {
        // Draw into a Pixmap directly to verify the draw path doesn't panic and
        // leaves a non-zero-alpha pixel inside the MSEG region.
        use tiny_skia_widgets::mseg::{draw_mseg, MsegEditState};
        use tiny_skia_widgets::{TextRenderer, color_bg};
        use tiny_skia::Pixmap;

        // Use the embedded font (same as MiffWindow::new).
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let mut text_renderer = TextRenderer::new(font_data);

        let w = 880_u32;
        let h = 620_u32;
        let mut pm = Pixmap::new(w, h).unwrap();
        pm.fill(color_bg());

        let (mseg_rect, _response_rect, _strip_rect) = layout(w as f32, h as f32);

        let curve = crate::kernel::default_flat_curve();
        let state = MsegEditState::new_curve_only();

        draw_mseg(&mut pm, &mut text_renderer, mseg_rect, &curve, &state, 1.0);

        // Sample a pixel well inside the MSEG canvas (quarter-width, half-height).
        let sample_x = (mseg_rect.0 + mseg_rect.2 * 0.25) as u32;
        let sample_y = (mseg_rect.1 + mseg_rect.3 * 0.5) as u32;
        let alpha = pm.pixels()[(sample_y * w + sample_x) as usize].alpha();
        assert!(alpha > 0, "MSEG region not painted at ({sample_x}, {sample_y})");
    }
}
