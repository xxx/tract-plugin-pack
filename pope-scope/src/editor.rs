//! Softbuffer-based editor for pope-scope. CPU rendering via tiny-skia.

use baseview::{WindowOpenOptions, WindowScalePolicy};
use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::renderer;
use crate::snapshot::{self, WaveSnapshot};
use crate::theme;
use crate::PopeScopeParams;
use tiny_skia_widgets as widgets;

const WINDOW_WIDTH: u32 = 800;
const WINDOW_HEIGHT: u32 = 500;
const CONTROL_BAR_HEIGHT: f32 = 80.0;
const TITLE_BAR_HEIGHT: f32 = 28.0;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

// ── Hit testing ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct HitRegion {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    action: HitAction,
}

#[derive(Clone, Copy, PartialEq)]
enum HitAction {
    Dial(ParamId),
    Button(ButtonAction),
}

#[derive(Clone, Copy, PartialEq)]
enum ParamId {
    Timebase,
    MinDb,
    MaxDb,
}

#[derive(Clone, Copy, PartialEq)]
enum ButtonAction {
    ScaleDown,
    ScaleUp,
    ToggleFreeze,
    ToggleMono,
    CycleDisplayMode,
    CycleDrawStyle,
    CycleSyncMode,
    CycleSyncUnit,
}

// ── Peak hold state ─────────────────────────────────────────────────────

/// Per-track peak hold: holds peak level for 2 seconds, then decays at 20 dB/s.
struct PeakHoldEntry {
    peak_db: f32,
    hold_time_remaining: f32, // seconds
}

impl PeakHoldEntry {
    fn new() -> Self {
        Self {
            peak_db: -96.0,
            hold_time_remaining: 0.0,
        }
    }

    fn update(&mut self, new_peak_db: f32, dt: f32) {
        if new_peak_db > self.peak_db {
            self.peak_db = new_peak_db;
            self.hold_time_remaining = 2.0;
        } else if self.hold_time_remaining > 0.0 {
            self.hold_time_remaining -= dt;
        } else {
            self.peak_db -= 20.0 * dt; // decay at 20 dB/s
            if self.peak_db < -96.0 {
                self.peak_db = -96.0;
            }
        }
    }
}

// ── Window handler ──────────────────────────────────────────────────────

struct PopeScopeWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    shared_scale: Arc<AtomicCell<f32>>,

    params: Arc<PopeScopeParams>,
    sample_rate: f32,
    text_renderer: widgets::TextRenderer,

    /// Cached snapshots for freeze mode.
    cached_snapshots: Vec<WaveSnapshot>,
    /// Per-slot peak hold state.
    peak_holds: Vec<PeakHoldEntry>,
    /// Last frame timestamp for dt computation.
    last_frame_time: std::time::Instant,
    hit_regions: Vec<HitRegion>,
    drag_active: Option<HitAction>,
    drag_start_y: f32,
    drag_start_value: f32,
    last_shift_state: bool,
    granular_drag_start_y: f32,
    granular_drag_start_value: f32,
    mouse_x: f32,
    mouse_y: f32,
    last_click_time: std::time::Instant,
    last_click_action: Option<HitAction>,
}

impl PopeScopeWindow {
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<PopeScopeParams>,
        sample_rate: f32,
        shared_scale: Arc<AtomicCell<f32>>,
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
            params,
            sample_rate,
            text_renderer,
            cached_snapshots: Vec::new(),
            peak_holds: Vec::new(),
            last_frame_time: std::time::Instant::now(),
            hit_regions: Vec::new(),
            drag_active: None,
            drag_start_y: 0.0,
            drag_start_value: 0.0,
            last_shift_state: false,
            granular_drag_start_y: 0.0,
            granular_drag_start_value: 0.0,
            mouse_x: 0.0,
            mouse_y: 0.0,
            last_click_time: std::time::Instant::now(),
            last_click_action: None,
        }
    }

    // ── Param access helpers ────────────────────────────────────────────

    fn float_param(&self, id: ParamId) -> &FloatParam {
        match id {
            ParamId::Timebase => &self.params.timebase,
            ParamId::MinDb => &self.params.min_db,
            ParamId::MaxDb => &self.params.max_db,
        }
    }

    fn begin_set_param(&self, setter: &ParamSetter, id: ParamId) {
        setter.begin_set_parameter(self.float_param(id));
    }

    fn set_param_normalized(&self, setter: &ParamSetter, id: ParamId, normalized: f32) {
        setter.set_parameter_normalized(self.float_param(id), normalized);
    }

    fn end_set_param(&self, setter: &ParamSetter, id: ParamId) {
        setter.end_set_parameter(self.float_param(id));
    }

    fn reset_param_to_default(&self, setter: &ParamSetter, id: ParamId) {
        use nih_plug::prelude::Param;
        let p = self.float_param(id);
        setter.begin_set_parameter(p);
        setter.set_parameter_normalized(p, p.default_normalized_value());
        setter.end_set_parameter(p);
    }

    // ── Layout helpers ──────────────────────────────────────────────────

    /// Returns (x, y, w, h) for the waveform display area.
    pub fn waveform_rect(&self) -> (f32, f32, f32, f32) {
        let s = self.scale_factor;
        let title_h = TITLE_BAR_HEIGHT * s;
        let control_h = CONTROL_BAR_HEIGHT * s;
        let w = self.physical_width as f32;
        let h = self.physical_height as f32;
        (0.0, title_h, w, (h - title_h - control_h).max(1.0))
    }

    // ── Drawing ─────────────────────────────────────────────────────────

    fn draw(&mut self) {
        let s = self.scale_factor;
        let pw = self.physical_width as f32;
        let ph = self.physical_height as f32;

        self.hit_regions.clear();

        // Pre-compute waveform rect before borrowing text_renderer
        let (wx, wy, ww, wh) = self.waveform_rect();

        // Fill background
        self.surface.pixmap.fill(theme::to_color(theme::BG));

        let tr = &mut self.text_renderer;

        // ── Title bar ───────────────────────────────────────────────────
        let title_h = TITLE_BAR_HEIGHT * s;
        let pad = 8.0 * s;
        let title_size = 14.0 * s;
        let small_font = 11.0 * s;

        tr.draw_text(
            &mut self.surface.pixmap,
            pad,
            pad + title_size,
            "Pope Scope",
            title_size,
            theme::to_color(theme::FG),
        );

        // Scale buttons (top right)
        let btn_size = 20.0 * s;
        let scale_label_w = 40.0 * s;

        let plus_x = pw - pad - btn_size;
        let plus_y = pad;
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            plus_x,
            plus_y,
            btn_size,
            btn_size,
            "+",
            false,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: plus_x,
            y: plus_y,
            w: btn_size,
            h: btn_size,
            action: HitAction::Button(ButtonAction::ScaleUp),
        });

        let pct_text = format!("{}%", (self.scale_factor * 100.0).round() as u32);
        let pct_x = plus_x - scale_label_w;
        let pct_text_w = tr.text_width(&pct_text, small_font);
        tr.draw_text(
            &mut self.surface.pixmap,
            pct_x + (scale_label_w - pct_text_w) / 2.0,
            plus_y + small_font + 3.0 * s,
            &pct_text,
            small_font,
            theme::to_color(theme::PRIMARY_DIM),
        );

        let minus_x = pct_x - btn_size;
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            minus_x,
            plus_y,
            btn_size,
            btn_size,
            "-",
            false,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: minus_x,
            y: plus_y,
            w: btn_size,
            h: btn_size,
            action: HitAction::Button(ButtonAction::ScaleDown),
        });

        // Title bar bottom border
        widgets::draw_rect(
            &mut self.surface.pixmap,
            0.0,
            title_h - 1.0,
            pw,
            1.0,
            theme::to_color(theme::BORDER),
        );

        // ── Waveform area ────────────────────────────────────────────────
        // Draw border below waveform area
        widgets::draw_rect(
            &mut self.surface.pixmap,
            wx,
            wy + wh,
            ww,
            1.0,
            theme::to_color(theme::BORDER),
        );

        // Build snapshots (or reuse cached if frozen)
        let freeze = self.params.freeze.value();
        let sync_mode_val = self.params.sync_mode.value();
        let group = self.params.group.value() as u32;
        let decimation = self.params.decimation.value() as usize;
        let mix_to_mono = self.params.mix_to_mono.value();
        let display_mode_val = self.params.display_mode.value();
        let draw_style_val = self.params.draw_style.value();
        let min_db_val = self.params.min_db.value();
        let max_db_val = self.params.max_db.value();

        if !freeze {
            self.cached_snapshots = match sync_mode_val {
                crate::SyncMode::Free => {
                    let timebase = self.params.timebase.value();
                    snapshot::build_snapshots_free(
                        group,
                        timebase,
                        self.sample_rate,
                        decimation,
                        mix_to_mono,
                    )
                }
                crate::SyncMode::BeatSync => {
                    let sync_bars = self.params.sync_unit.value().to_bars();
                    snapshot::build_snapshots_beat_sync(
                        group,
                        sync_bars,
                        self.sample_rate,
                        decimation,
                        mix_to_mono,
                    )
                }
            };
        }

        // Apply solo/mute filtering
        let snapshots = &self.cached_snapshots;
        let any_solo = snapshots.iter().any(|s| s.solo);
        let visible: Vec<&WaveSnapshot> = snapshots
            .iter()
            .filter(|s| {
                if s.mute {
                    return false;
                }
                if any_solo && !s.solo {
                    return false;
                }
                true
            })
            .collect();

        // Update peak holds
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.last_frame_time).as_secs_f32();
        self.last_frame_time = now;

        // Ensure peak_holds has enough entries
        while self.peak_holds.len() < 16 {
            self.peak_holds.push(PeakHoldEntry::new());
        }
        for snap in &visible {
            if snap.slot_index < self.peak_holds.len() {
                self.peak_holds[snap.slot_index].update(snap.peak_db, dt);
            }
        }

        // Render waveforms based on display mode
        if visible.is_empty() {
            let placeholder = "Waiting for audio...";
            let placeholder_size = 16.0 * s;
            let placeholder_w = tr.text_width(placeholder, placeholder_size);
            tr.draw_text(
                &mut self.surface.pixmap,
                wx + (ww - placeholder_w) / 2.0,
                wy + wh / 2.0,
                placeholder,
                placeholder_size,
                theme::to_color(theme::PRIMARY_DIM),
            );
        } else {
            match display_mode_val {
                crate::DisplayMode::Vertical => {
                    // Each track gets a vertical strip
                    let num_tracks = visible.len();
                    let track_h = wh / num_tracks as f32;
                    for (i, snap) in visible.iter().enumerate() {
                        let ty = wy + i as f32 * track_h;
                        // Draw amplitude grid
                        renderer::draw_amplitude_grid(
                            &mut self.surface.pixmap,
                            wx,
                            ty,
                            ww,
                            track_h,
                            min_db_val,
                            max_db_val,
                            tr,
                            s,
                        );
                        // Draw time/beat grid
                        match sync_mode_val {
                            crate::SyncMode::Free => {
                                renderer::draw_time_grid(
                                    &mut self.surface.pixmap,
                                    wx,
                                    ty,
                                    ww,
                                    track_h,
                                    self.params.timebase.value(),
                                    tr,
                                    s,
                                    i == num_tracks - 1, // labels on bottom track only
                                );
                            }
                            crate::SyncMode::BeatSync => {
                                let total_beats = self.params.sync_unit.value().to_bars()
                                    * snap.beats_per_bar as f64;
                                renderer::draw_beat_grid(
                                    &mut self.surface.pixmap,
                                    wx,
                                    ty,
                                    ww,
                                    track_h,
                                    snap.beats_per_bar,
                                    total_beats,
                                    tr,
                                    s,
                                    i == num_tracks - 1,
                                );
                            }
                        }
                        // Draw waveform
                        let samples = if mix_to_mono && !snap.mono_mix.is_empty() {
                            &snap.mono_mix
                        } else if !snap.audio_data.is_empty() {
                            &snap.audio_data[0]
                        } else {
                            continue;
                        };
                        renderer::draw_waveform(
                            &mut self.surface.pixmap,
                            samples,
                            wx,
                            ty,
                            ww,
                            track_h,
                            min_db_val,
                            max_db_val,
                            snap.display_color,
                            draw_style_val,
                        );
                        // Draw peak hold line
                        if snap.slot_index < self.peak_holds.len() {
                            let peak_db = self.peak_holds[snap.slot_index].peak_db;
                            if peak_db > min_db_val {
                                let centre_y = ty + track_h / 2.0;
                                let half_h = track_h / 2.0;
                                let y_pos =
                                    renderer::sample_to_y(
                                        10.0f32.powf(peak_db / 20.0),
                                        min_db_val,
                                        max_db_val,
                                        centre_y,
                                        half_h,
                                    );
                                // Dashed line
                                let dash_len = 4.0 * s;
                                let mut dx = wx;
                                while dx < wx + ww {
                                    let seg = (dx + dash_len).min(wx + ww) - dx;
                                    widgets::draw_rect(
                                        &mut self.surface.pixmap,
                                        dx,
                                        y_pos,
                                        seg,
                                        1.0,
                                        theme::to_color(snap.display_color),
                                    );
                                    dx += dash_len * 2.0;
                                }
                            }
                        }
                        // Track separator
                        if i < num_tracks - 1 {
                            widgets::draw_rect(
                                &mut self.surface.pixmap,
                                wx,
                                ty + track_h - 1.0,
                                ww,
                                1.0,
                                theme::to_color(theme::BORDER),
                            );
                        }
                    }
                }
                crate::DisplayMode::Overlay => {
                    // All tracks overlaid in the same area
                    renderer::draw_amplitude_grid(
                        &mut self.surface.pixmap,
                        wx,
                        wy,
                        ww,
                        wh,
                        min_db_val,
                        max_db_val,
                        tr,
                        s,
                    );
                    match sync_mode_val {
                        crate::SyncMode::Free => {
                            renderer::draw_time_grid(
                                &mut self.surface.pixmap,
                                wx,
                                wy,
                                ww,
                                wh,
                                self.params.timebase.value(),
                                tr,
                                s,
                                true,
                            );
                        }
                        crate::SyncMode::BeatSync => {
                            if let Some(first) = visible.first() {
                                let total_beats = self.params.sync_unit.value().to_bars()
                                    * first.beats_per_bar as f64;
                                renderer::draw_beat_grid(
                                    &mut self.surface.pixmap,
                                    wx,
                                    wy,
                                    ww,
                                    wh,
                                    first.beats_per_bar,
                                    total_beats,
                                    tr,
                                    s,
                                    true,
                                );
                            }
                        }
                    }
                    for snap in &visible {
                        let samples = if mix_to_mono && !snap.mono_mix.is_empty() {
                            &snap.mono_mix
                        } else if !snap.audio_data.is_empty() {
                            &snap.audio_data[0]
                        } else {
                            continue;
                        };
                        renderer::draw_waveform(
                            &mut self.surface.pixmap,
                            samples,
                            wx,
                            wy,
                            ww,
                            wh,
                            min_db_val,
                            max_db_val,
                            snap.display_color,
                            draw_style_val,
                        );
                    }
                }
                crate::DisplayMode::Sum => {
                    // Sum all visible tracks into one mono signal
                    renderer::draw_amplitude_grid(
                        &mut self.surface.pixmap,
                        wx,
                        wy,
                        ww,
                        wh,
                        min_db_val,
                        max_db_val,
                        tr,
                        s,
                    );
                    match sync_mode_val {
                        crate::SyncMode::Free => {
                            renderer::draw_time_grid(
                                &mut self.surface.pixmap,
                                wx,
                                wy,
                                ww,
                                wh,
                                self.params.timebase.value(),
                                tr,
                                s,
                                true,
                            );
                        }
                        crate::SyncMode::BeatSync => {
                            if let Some(first) = visible.first() {
                                let total_beats = self.params.sync_unit.value().to_bars()
                                    * first.beats_per_bar as f64;
                                renderer::draw_beat_grid(
                                    &mut self.surface.pixmap,
                                    wx,
                                    wy,
                                    ww,
                                    wh,
                                    first.beats_per_bar,
                                    total_beats,
                                    tr,
                                    s,
                                    true,
                                );
                            }
                        }
                    }
                    // Compute summed signal
                    let max_len = visible
                        .iter()
                        .filter_map(|s| {
                            if mix_to_mono && !s.mono_mix.is_empty() {
                                Some(s.mono_mix.len())
                            } else if !s.audio_data.is_empty() {
                                Some(s.audio_data[0].len())
                            } else {
                                None
                            }
                        })
                        .max()
                        .unwrap_or(0);
                    if max_len > 0 {
                        let mut sum = vec![0.0f32; max_len];
                        for snap in &visible {
                            let samples = if mix_to_mono && !snap.mono_mix.is_empty() {
                                &snap.mono_mix
                            } else if !snap.audio_data.is_empty() {
                                &snap.audio_data[0]
                            } else {
                                continue;
                            };
                            for (i, &s_val) in samples.iter().enumerate() {
                                if i < max_len {
                                    sum[i] += s_val;
                                }
                            }
                        }
                        renderer::draw_waveform(
                            &mut self.surface.pixmap,
                            &sum,
                            wx,
                            wy,
                            ww,
                            wh,
                            min_db_val,
                            max_db_val,
                            theme::FG,
                            draw_style_val,
                        );
                    }
                }
            }
        }

        // Draw cursor if mouse is in waveform area
        if self.mouse_x >= wx
            && self.mouse_x < wx + ww
            && self.mouse_y >= wy
            && self.mouse_y < wy + wh
        {
            renderer::draw_cursor(&mut self.surface.pixmap, self.mouse_x, wy, wh);
        }

        // ── Control bar ─────────────────────────────────────────────────
        let control_h = CONTROL_BAR_HEIGHT * s;
        let control_y = ph - control_h;
        let row_h = 26.0 * s;
        let label_font = 10.0 * s;
        let gap = 6.0 * s;

        // Two rows of controls
        let row1_y = control_y + gap;
        let row2_y = row1_y + row_h + gap;

        let mut cx = pad;

        // ── Row 1: Display | Style | Sync | Unit | Freeze | Mono ────────

        // Display mode
        let display_mode = self.params.display_mode.value();
        let display_idx = match display_mode {
            crate::DisplayMode::Vertical => 0,
            crate::DisplayMode::Overlay => 1,
            crate::DisplayMode::Sum => 2,
        };
        let display_label = "Display";
        let display_w = 120.0 * s;
        tr.draw_text(
            &mut self.surface.pixmap,
            cx,
            row1_y - 1.0,
            display_label,
            label_font,
            theme::to_color(theme::PRIMARY_DIM),
        );
        widgets::draw_stepped_selector(
            &mut self.surface.pixmap,
            tr,
            cx,
            row1_y + label_font,
            display_w,
            row_h,
            &["Vert", "Over", "Sum"],
            display_idx,
        );
        self.hit_regions.push(HitRegion {
            x: cx,
            y: row1_y + label_font,
            w: display_w,
            h: row_h,
            action: HitAction::Button(ButtonAction::CycleDisplayMode),
        });

        cx += display_w + gap;

        // Draw style
        let draw_style = self.params.draw_style.value();
        let style_idx = match draw_style {
            crate::DrawStyle::Line => 0,
            crate::DrawStyle::Filled => 1,
            crate::DrawStyle::Both => 2,
        };
        let style_w = 120.0 * s;
        tr.draw_text(
            &mut self.surface.pixmap,
            cx,
            row1_y - 1.0,
            "Style",
            label_font,
            theme::to_color(theme::PRIMARY_DIM),
        );
        widgets::draw_stepped_selector(
            &mut self.surface.pixmap,
            tr,
            cx,
            row1_y + label_font,
            style_w,
            row_h,
            &["Line", "Fill", "Both"],
            style_idx,
        );
        self.hit_regions.push(HitRegion {
            x: cx,
            y: row1_y + label_font,
            w: style_w,
            h: row_h,
            action: HitAction::Button(ButtonAction::CycleDrawStyle),
        });

        cx += style_w + gap;

        // Sync mode
        let sync_mode = self.params.sync_mode.value();
        let sync_idx = match sync_mode {
            crate::SyncMode::Free => 0,
            crate::SyncMode::BeatSync => 1,
        };
        let sync_w = 90.0 * s;
        tr.draw_text(
            &mut self.surface.pixmap,
            cx,
            row1_y - 1.0,
            "Sync",
            label_font,
            theme::to_color(theme::PRIMARY_DIM),
        );
        widgets::draw_stepped_selector(
            &mut self.surface.pixmap,
            tr,
            cx,
            row1_y + label_font,
            sync_w,
            row_h,
            &["Free", "Beat"],
            sync_idx,
        );
        self.hit_regions.push(HitRegion {
            x: cx,
            y: row1_y + label_font,
            w: sync_w,
            h: row_h,
            action: HitAction::Button(ButtonAction::CycleSyncMode),
        });

        cx += sync_w + gap;

        // Sync unit (only visible in beat sync mode)
        if sync_mode == crate::SyncMode::BeatSync {
            let sync_unit = self.params.sync_unit.value();
            let unit_idx = match sync_unit {
                crate::SyncUnit::Quarter => 0,
                crate::SyncUnit::Half => 1,
                crate::SyncUnit::One => 2,
                crate::SyncUnit::Two => 3,
                crate::SyncUnit::Four => 4,
            };
            let unit_w = 150.0 * s;
            tr.draw_text(
                &mut self.surface.pixmap,
                cx,
                row1_y - 1.0,
                "Unit",
                label_font,
                theme::to_color(theme::PRIMARY_DIM),
            );
            widgets::draw_stepped_selector(
                &mut self.surface.pixmap,
                tr,
                cx,
                row1_y + label_font,
                unit_w,
                row_h,
                &["1/4", "1/2", "1", "2", "4"],
                unit_idx,
            );
            self.hit_regions.push(HitRegion {
                x: cx,
                y: row1_y + label_font,
                w: unit_w,
                h: row_h,
                action: HitAction::Button(ButtonAction::CycleSyncUnit),
            });

            cx += unit_w + gap;
        }

        // Freeze toggle
        let freeze = self.params.freeze.value();
        let freeze_w = 50.0 * s;
        tr.draw_text(
            &mut self.surface.pixmap,
            cx,
            row1_y - 1.0,
            "Freeze",
            label_font,
            theme::to_color(theme::PRIMARY_DIM),
        );
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            cx,
            row1_y + label_font,
            freeze_w,
            row_h,
            if freeze { "ON" } else { "OFF" },
            freeze,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: cx,
            y: row1_y + label_font,
            w: freeze_w,
            h: row_h,
            action: HitAction::Button(ButtonAction::ToggleFreeze),
        });

        cx += freeze_w + gap;

        // Mono toggle
        let mono = self.params.mix_to_mono.value();
        let mono_w = 50.0 * s;
        tr.draw_text(
            &mut self.surface.pixmap,
            cx,
            row1_y - 1.0,
            "Mono",
            label_font,
            theme::to_color(theme::PRIMARY_DIM),
        );
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            cx,
            row1_y + label_font,
            mono_w,
            row_h,
            if mono { "ON" } else { "OFF" },
            mono,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: cx,
            y: row1_y + label_font,
            w: mono_w,
            h: row_h,
            action: HitAction::Button(ButtonAction::ToggleMono),
        });

        // ── Row 2: Timebase slider | Min dB slider | Max dB slider ──────

        let mut cx2 = pad;
        let slider_w = 200.0 * s;

        // Timebase (only visible in Free mode)
        if sync_mode == crate::SyncMode::Free {
            let timebase = self.params.timebase.value();
            let timebase_text = if timebase >= 1000.0 {
                format!("{:.1}s", timebase / 1000.0)
            } else {
                format!("{:.0}ms", timebase)
            };
            tr.draw_text(
                &mut self.surface.pixmap,
                cx2,
                row2_y - 1.0,
                "Timebase",
                label_font,
                theme::to_color(theme::PRIMARY_DIM),
            );
            let tb_normalized = self.params.timebase.modulated_normalized_value();
            widgets::draw_slider(
                &mut self.surface.pixmap,
                tr,
                cx2,
                row2_y + label_font,
                slider_w,
                row_h,
                "",
                &timebase_text,
                tb_normalized,
            );
            self.hit_regions.push(HitRegion {
                x: cx2,
                y: row2_y + label_font,
                w: slider_w,
                h: row_h,
                action: HitAction::Dial(ParamId::Timebase),
            });

            cx2 += slider_w + gap;
        }

        // Min dB slider
        let min_db = self.params.min_db.value();
        let min_db_text = format!("{:.0} dB", min_db);
        tr.draw_text(
            &mut self.surface.pixmap,
            cx2,
            row2_y - 1.0,
            "Min dB",
            label_font,
            theme::to_color(theme::PRIMARY_DIM),
        );
        let min_normalized = self.params.min_db.modulated_normalized_value();
        widgets::draw_slider(
            &mut self.surface.pixmap,
            tr,
            cx2,
            row2_y + label_font,
            slider_w,
            row_h,
            "",
            &min_db_text,
            min_normalized,
        );
        self.hit_regions.push(HitRegion {
            x: cx2,
            y: row2_y + label_font,
            w: slider_w,
            h: row_h,
            action: HitAction::Dial(ParamId::MinDb),
        });

        cx2 += slider_w + gap;

        // Max dB slider
        let max_db = self.params.max_db.value();
        let max_db_text = format!("{:.0} dB", max_db);
        tr.draw_text(
            &mut self.surface.pixmap,
            cx2,
            row2_y - 1.0,
            "Max dB",
            label_font,
            theme::to_color(theme::PRIMARY_DIM),
        );
        let max_normalized = self.params.max_db.modulated_normalized_value();
        widgets::draw_slider(
            &mut self.surface.pixmap,
            tr,
            cx2,
            row2_y + label_font,
            slider_w,
            row_h,
            "",
            &max_db_text,
            max_normalized,
        );
        self.hit_regions.push(HitRegion {
            x: cx2,
            y: row2_y + label_font,
            w: slider_w,
            h: row_h,
            action: HitAction::Dial(ParamId::MaxDb),
        });

    }

    fn apply_scale_change(&mut self, delta: f32, window: &mut baseview::Window) {
        let old = self.scale_factor;
        self.scale_factor = (self.scale_factor + delta).clamp(0.75, 3.0);
        if (self.scale_factor - old).abs() > 0.01 {
            self.shared_scale.store(self.scale_factor);
            let new_w = (WINDOW_WIDTH as f32 * self.scale_factor).round() as u32;
            let new_h = (WINDOW_HEIGHT as f32 * self.scale_factor).round() as u32;
            self.params.editor_state.store_size(new_w, new_h);
            window.resize(baseview::Size::new(new_w as f64, new_h as f64));
            self.gui_context.request_resize();
        }
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }
}

impl baseview::WindowHandler for PopeScopeWindow {
    fn on_frame(&mut self, _window: &mut baseview::Window) {
        self.draw();
        self.surface.present();
    }

    fn on_event(
        &mut self,
        window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        match &event {
            baseview::Event::Window(baseview::WindowEvent::Resized(info)) => {
                self.physical_width = info.physical_size().width;
                self.physical_height = info.physical_size().height;
                self.resize_buffers();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                self.mouse_x = position.x as f32;
                self.mouse_y = position.y as f32;

                if let Some(HitAction::Dial(param_id)) = self.drag_active {
                    let shift_now = modifiers.contains(keyboard_types::Modifiers::SHIFT);

                    // Detect shift transitions for fine control
                    let current_normalized = self.float_param(param_id).modulated_normalized_value();
                    if shift_now && !self.last_shift_state {
                        self.granular_drag_start_y = self.mouse_y;
                        self.granular_drag_start_value = current_normalized;
                    } else if !shift_now && self.last_shift_state {
                        self.drag_start_y = self.mouse_y;
                        self.drag_start_value = current_normalized;
                    }

                    // Drag: 600px = full range, up = increase
                    let normalized_per_pixel = 1.0 / 600.0;
                    let target = if shift_now {
                        let delta_y = self.granular_drag_start_y - self.mouse_y;
                        (self.granular_drag_start_value + delta_y * normalized_per_pixel * 0.1)
                            .clamp(0.0, 1.0)
                    } else {
                        let delta_y = self.drag_start_y - self.mouse_y;
                        (self.drag_start_value + delta_y * normalized_per_pixel).clamp(0.0, 1.0)
                    };

                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.set_param_normalized(&setter, param_id, target);

                    self.last_shift_state = shift_now;
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers,
            }) => {
                let mx = self.mouse_x;
                let my = self.mouse_y;

                let hit = self
                    .hit_regions
                    .iter()
                    .find(|r| mx >= r.x && mx < r.x + r.w && my >= r.y && my < r.y + r.h)
                    .cloned();

                if let Some(region) = hit {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    let now = std::time::Instant::now();
                    let is_double_click = now.duration_since(self.last_click_time).as_millis()
                        < 400
                        && self.last_click_action.as_ref() == Some(&region.action);
                    self.last_click_time = now;
                    self.last_click_action = Some(region.action);

                    // End any pending drag
                    if let Some(HitAction::Dial(id)) = self.drag_active.take() {
                        self.end_set_param(&setter, id);
                    }

                    match region.action {
                        HitAction::Dial(param_id) => {
                            if is_double_click {
                                self.reset_param_to_default(&setter, param_id);
                            } else {
                                let normalized =
                                    self.float_param(param_id).modulated_normalized_value();
                                self.drag_start_y = my;
                                self.drag_start_value = normalized;
                                self.granular_drag_start_y = my;
                                self.granular_drag_start_value = normalized;
                                self.last_shift_state =
                                    modifiers.contains(keyboard_types::Modifiers::SHIFT);
                                self.drag_active = Some(HitAction::Dial(param_id));
                                self.begin_set_param(&setter, param_id);
                            }
                        }
                        HitAction::Button(btn) => match btn {
                            ButtonAction::ScaleDown => {
                                self.apply_scale_change(-0.25, window);
                            }
                            ButtonAction::ScaleUp => {
                                self.apply_scale_change(0.25, window);
                            }
                            ButtonAction::ToggleFreeze => {
                                let current = self.params.freeze.value();
                                setter.begin_set_parameter(&self.params.freeze);
                                setter.set_parameter(&self.params.freeze, !current);
                                setter.end_set_parameter(&self.params.freeze);
                            }
                            ButtonAction::ToggleMono => {
                                let current = self.params.mix_to_mono.value();
                                setter.begin_set_parameter(&self.params.mix_to_mono);
                                setter.set_parameter(&self.params.mix_to_mono, !current);
                                setter.end_set_parameter(&self.params.mix_to_mono);
                            }
                            ButtonAction::CycleDisplayMode => {
                                // Determine which segment was clicked
                                let rel_x = mx - region.x;
                                let seg_w = region.w / 3.0;
                                let clicked_idx = (rel_x / seg_w) as usize;
                                let mode = match clicked_idx {
                                    0 => crate::DisplayMode::Vertical,
                                    1 => crate::DisplayMode::Overlay,
                                    _ => crate::DisplayMode::Sum,
                                };
                                setter.begin_set_parameter(&self.params.display_mode);
                                setter.set_parameter(&self.params.display_mode, mode);
                                setter.end_set_parameter(&self.params.display_mode);
                            }
                            ButtonAction::CycleDrawStyle => {
                                let rel_x = mx - region.x;
                                let seg_w = region.w / 3.0;
                                let clicked_idx = (rel_x / seg_w) as usize;
                                let style = match clicked_idx {
                                    0 => crate::DrawStyle::Line,
                                    1 => crate::DrawStyle::Filled,
                                    _ => crate::DrawStyle::Both,
                                };
                                setter.begin_set_parameter(&self.params.draw_style);
                                setter.set_parameter(&self.params.draw_style, style);
                                setter.end_set_parameter(&self.params.draw_style);
                            }
                            ButtonAction::CycleSyncMode => {
                                let rel_x = mx - region.x;
                                let seg_w = region.w / 2.0;
                                let clicked_idx = (rel_x / seg_w) as usize;
                                let mode = match clicked_idx {
                                    0 => crate::SyncMode::Free,
                                    _ => crate::SyncMode::BeatSync,
                                };
                                setter.begin_set_parameter(&self.params.sync_mode);
                                setter.set_parameter(&self.params.sync_mode, mode);
                                setter.end_set_parameter(&self.params.sync_mode);
                            }
                            ButtonAction::CycleSyncUnit => {
                                let rel_x = mx - region.x;
                                let seg_w = region.w / 5.0;
                                let clicked_idx = (rel_x / seg_w) as usize;
                                let unit = match clicked_idx {
                                    0 => crate::SyncUnit::Quarter,
                                    1 => crate::SyncUnit::Half,
                                    2 => crate::SyncUnit::One,
                                    3 => crate::SyncUnit::Two,
                                    _ => crate::SyncUnit::Four,
                                };
                                setter.begin_set_parameter(&self.params.sync_unit);
                                setter.set_parameter(&self.params.sync_unit, unit);
                                setter.end_set_parameter(&self.params.sync_unit);
                            }
                        },
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                if let Some(HitAction::Dial(id)) = self.drag_active.take() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.end_set_param(&setter, id);
                }
            }
            baseview::Event::Keyboard(kb_event) => {
                use keyboard_types::{Key, KeyState, Modifiers};
                if kb_event.state == KeyState::Down
                    && kb_event.modifiers.contains(Modifiers::CONTROL)
                {
                    match &kb_event.key {
                        Key::Character(c) if c == "=" || c == "+" => {
                            self.apply_scale_change(0.25, window);
                        }
                        Key::Character(c) if c == "-" => {
                            self.apply_scale_change(-0.25, window);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        baseview::EventStatus::Captured
    }
}

// ── Editor trait implementation ──────────────────────────────────────────

pub(crate) struct PopeScopeEditor {
    params: Arc<PopeScopeParams>,
    shared_sample_rate: Arc<AtomicU32>,
    scaling_factor: Arc<AtomicCell<f32>>,
}

pub(crate) fn create(
    params: Arc<PopeScopeParams>,
    shared_sample_rate: Arc<AtomicU32>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(PopeScopeEditor {
        params,
        shared_sample_rate,
        scaling_factor: Arc::new(AtomicCell::new(1.0)),
    }))
}

impl Editor for PopeScopeEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.75, 3.0);
        self.scaling_factor.store(sf);

        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let shared_scale = Arc::clone(&self.scaling_factor);
        let sample_rate = self.shared_sample_rate.load(Ordering::Relaxed) as f32;

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Pope Scope"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                PopeScopeWindow::new(window, gui_context, params, sample_rate, shared_scale, sf)
            },
        );

        self.params.editor_state.set_open(true);
        Box::new(widgets::EditorHandle::new(
            self.params.editor_state.clone(),
            window,
        ))
    }

    fn size(&self) -> (u32, u32) {
        let sf = self.scaling_factor.load();
        let w = (WINDOW_WIDTH as f32 * sf).round() as u32;
        let h = (WINDOW_HEIGHT as f32 * sf).round() as u32;
        (w, h)
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        if self.params.editor_state.is_open() {
            return false;
        }
        self.scaling_factor.store(factor);
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
    fn test_peak_hold_new_peak() {
        let mut ph = PeakHoldEntry::new();
        assert_eq!(ph.peak_db, -96.0);

        ph.update(-12.0, 0.016);
        assert_eq!(ph.peak_db, -12.0);
        assert!((ph.hold_time_remaining - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_peak_hold_holds_for_2_seconds() {
        let mut ph = PeakHoldEntry::new();
        ph.update(-6.0, 0.016);

        // Advance 1 second with lower peaks — should still hold
        for _ in 0..60 {
            ph.update(-20.0, 1.0 / 60.0);
        }
        assert_eq!(ph.peak_db, -6.0);
        assert!(ph.hold_time_remaining > 0.0);
    }

    #[test]
    fn test_peak_hold_decays_after_hold() {
        let mut ph = PeakHoldEntry::new();
        ph.update(-6.0, 0.016);

        // Exhaust hold time
        ph.update(-96.0, 2.1);

        // Now it should decay
        let before = ph.peak_db;
        ph.update(-96.0, 0.5);
        assert!(ph.peak_db < before);
        // 20 dB/s * 0.5s = 10 dB decay
        assert!((ph.peak_db - (before - 10.0)).abs() < 0.01);
    }

    #[test]
    fn test_peak_hold_floors_at_minus_96() {
        let mut ph = PeakHoldEntry::new();
        ph.update(-90.0, 0.016);

        // Exhaust hold and decay well past -96
        ph.update(-96.0, 2.1);
        ph.update(-96.0, 10.0);
        assert_eq!(ph.peak_db, -96.0);
    }

    #[test]
    fn test_peak_hold_new_higher_peak_resets() {
        let mut ph = PeakHoldEntry::new();
        ph.update(-12.0, 0.016);

        // Advance past hold
        ph.update(-96.0, 2.1);
        ph.update(-96.0, 0.5);
        let decayed = ph.peak_db;
        assert!(decayed < -12.0);

        // New higher peak resets
        ph.update(-3.0, 0.016);
        assert_eq!(ph.peak_db, -3.0);
        assert!((ph.hold_time_remaining - 2.0).abs() < 0.001);
    }
}
