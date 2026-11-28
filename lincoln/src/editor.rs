use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use baseview::{
    Event, EventStatus, MouseButton, MouseEvent, WindowEvent, WindowOpenOptions, WindowScalePolicy,
};
use keyboard_types::{Key, KeyState, Modifiers};
use nih_plug::prelude::*;
use tiny_skia::{Color, Pixmap};
use tiny_skia_widgets as widgets;

use crate::crossover::{MIN_HZ, Splits};
use crate::{LincolnParams, Meters};

const WIDTH: u32 = 708;
const HEIGHT: u32 = 430;

const BACKGROUND: [u8; 3] = [23, 30, 39];
const PANEL: [u8; 3] = [34, 45, 58];
const TEXT: [u8; 3] = [235, 232, 218];
const MUTED: [u8; 3] = [158, 176, 190];
const BAND_COLORS: [[u8; 3]; 3] = [[232, 163, 124], [174, 201, 154], [133, 185, 224]];
const BAND_NAMES: [&str; 3] = ["Low", "Mid", "High"];

fn color([r, g, b]: [u8; 3]) -> Color {
    Color::from_rgba8(r, g, b, 255)
}

pub(crate) fn default_state() -> Arc<widgets::EditorState> {
    widgets::EditorState::from_size(WIDTH, HEIGHT)
}

#[derive(Clone, Copy, PartialEq)]
enum Dial {
    Depth,
    Time,
    Input,
    Output,
    Upward,
    Downward,
    SplitLow,
    SplitHigh,
    Threshold(usize),
    Gain(usize),
}

impl Dial {
    fn param(self, params: &LincolnParams) -> &FloatParam {
        match self {
            Self::Depth => &params.depth,
            Self::Time => &params.time,
            Self::Input => &params.input,
            Self::Output => &params.output,
            Self::Upward => &params.upward,
            Self::Downward => &params.downward,
            Self::SplitLow => &params.split_low,
            Self::SplitHigh => &params.split_high,
            Self::Threshold(i) => &params.bands[i].threshold,
            Self::Gain(i) => &params.bands[i].gain,
        }
    }

    fn displayed_normalized(self, params: &LincolnParams, splits: Splits) -> f32 {
        let param = self.param(params);
        match self {
            Self::SplitLow => param.preview_normalized(splits.lower),
            Self::SplitHigh => param.preview_normalized(splits.upper),
            _ => param.modulated_normalized_value(),
        }
    }

    fn base_normalized(self, params: &LincolnParams) -> f32 {
        self.param(params).unmodulated_normalized_value()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Dial(Dial),
    Upward(usize),
    Downward(usize),
    Bypass,
}

impl Action {
    fn toggle(self, params: &LincolnParams) -> Option<&BoolParam> {
        match self {
            Self::Dial(_) => None,
            Self::Upward(i) => Some(&params.bands[i].bypass_up),
            Self::Downward(i) => Some(&params.bands[i].bypass_down),
            Self::Bypass => Some(&params.bypass),
        }
    }
}

#[derive(Clone, Copy)]
struct Layout {
    x: f32,
    y: f32,
    scale: f32,
}

impl Layout {
    fn new(width: u32, height: u32, width_scale: f32) -> Self {
        let scale = width_scale
            .min(width as f32 / WIDTH as f32)
            .min(height as f32 / HEIGHT as f32);
        Self {
            x: (width as f32 - WIDTH as f32 * scale) * 0.5,
            y: (height as f32 - HEIGHT as f32 * scale) * 0.5,
            scale,
        }
    }

    fn rect(self, [x, y, w, h]: [f32; 4]) -> [f32; 4] {
        [
            self.x + x * self.scale,
            self.y + y * self.scale,
            w * self.scale,
            h * self.scale,
        ]
    }
}

#[derive(Default)]
struct BandMapActivity {
    observed_blocks: u64,
    last_progress: Option<Instant>,
}

impl BandMapActivity {
    fn splits(
        &mut self,
        meters: &Meters,
        targets: Splits,
        sample_rate: f32,
        now: Instant,
    ) -> Splits {
        let blocks = meters.processed_blocks.load(Ordering::Acquire);
        if blocks != self.observed_blocks {
            self.observed_blocks = blocks;
            self.last_progress = Some(now);
        }
        let block_size = meters.max_block_size.load(Ordering::Relaxed);
        let idle_after =
            Duration::from_secs_f64(2.0 * f64::from(block_size) / f64::from(sample_rate))
                .max(Duration::from_millis(150));
        if self
            .last_progress
            .is_some_and(|last| now.duration_since(last) < idle_after)
        {
            Splits::from_bits(meters.applied_splits.load(Ordering::Relaxed))
        } else {
            targets
        }
    }
}

struct Panel {
    text: widgets::TextRenderer,
    drag: widgets::DragState<Action>,
    edit: widgets::TextEditState<Dial>,
    display_gain: [f32; 3],
    band_map: BandMapActivity,
}

impl Panel {
    fn new() -> Self {
        Self {
            text: widgets::TextRenderer::new(widgets::DEFAULT_FONT),
            drag: widgets::DragState::new(),
            edit: widgets::TextEditState::new(),
            display_gain: [0.0; 3],
            band_map: BandMapActivity::default(),
        }
    }

    fn draw(
        &mut self,
        pixmap: &mut Pixmap,
        layout: Layout,
        params: &LincolnParams,
        meters: &Meters,
    ) {
        pixmap.fill(color(BACKGROUND));
        self.drag.clear_regions();

        let sample_rate = f32::from_bits(meters.sample_rate.load(Ordering::Relaxed));
        let splits = params.splits(sample_rate);
        let map_splits = self
            .band_map
            .splits(meters, splits, sample_rate, Instant::now());
        let mut frame = Frame {
            pixmap,
            layout,
            params,
            splits,
            panel: self,
        };
        frame.text([21.0, 36.0], "Lincoln", 25.0, color(TEXT));
        frame.button(
            [611.0, 17.0, 77.0, 25.0],
            "Bypass",
            Action::Bypass,
            params.bypass.value(),
            color(TEXT),
        );

        let global = [
            (Dial::Depth, "Depth"),
            (Dial::Time, "Time"),
            (Dial::Upward, "Upward"),
            (Dial::Downward, "Downward"),
            (Dial::Input, "In Gain"),
            (Dial::Output, "Out Gain"),
        ];
        for (i, (dial, label)) in global.into_iter().enumerate() {
            frame.dial([71.0 + i as f32 * 113.0, 99.0], 21.0, dial, label);
        }

        frame.rect([20.0, 145.0, 668.0, 103.0], color(PANEL));
        frame.dial([67.0, 198.0], 20.0, Dial::SplitLow, "Low / Mid");
        frame.dial([166.0, 198.0], 20.0, Dial::SplitHigh, "Mid / High");
        frame.draw_band_map(map_splits, Splits::maximum(sample_rate));

        for (i, name) in BAND_NAMES.into_iter().enumerate() {
            let db = f32::from_bits(meters.gain_db[i].load(Ordering::Relaxed));
            frame.panel.display_gain[i] += 0.20 * (db - frame.panel.display_gain[i]);
            frame.draw_band(i, name);
        }
    }
}

struct Frame<'a> {
    pixmap: &'a mut Pixmap,
    layout: Layout,
    params: &'a LincolnParams,
    splits: Splits,
    panel: &'a mut Panel,
}

impl Frame<'_> {
    fn rect(&mut self, rect: [f32; 4], ink: Color) {
        let [x, y, w, h] = self.layout.rect(rect);
        widgets::draw_rect(self.pixmap, x, y, w, h, ink);
    }

    fn text(&mut self, [x, y]: [f32; 2], label: &str, size: f32, ink: Color) {
        let [x, y, _, _] = self.layout.rect([x, y, 0.0, 0.0]);
        self.panel
            .text
            .draw_text(self.pixmap, x, y, label, size * self.layout.scale, ink);
    }

    fn dial(&mut self, [cx, cy]: [f32; 2], radius: f32, dial: Dial, label: &str) {
        let param = dial.param(self.params);
        let displayed = dial.displayed_normalized(self.params, self.splits);
        // The split dials show the applied (constrained) frequency, which the
        // unmodulated parameter value does not describe; they keep no arc.
        let (value, modulated) = match dial {
            Dial::SplitLow | Dial::SplitHigh => (displayed, None),
            _ => (dial.base_normalized(self.params), Some(displayed)),
        };
        let [x, y, r, _] = self.layout.rect([cx, cy, radius, 0.0]);
        widgets::draw_dial_scaled_ex(
            self.pixmap,
            &mut self.panel.text,
            x,
            y,
            r,
            label,
            &param.normalized_value_to_string(displayed, true),
            value,
            modulated,
            self.panel.edit.active_for(&dial),
            self.panel.edit.caret_visible(),
            color(TEXT),
            self.layout.scale,
        );
        self.hit(
            [
                cx - radius - 12.0,
                cy - radius - 19.0,
                radius * 2.0 + 24.0,
                radius * 2.0 + 35.0,
            ],
            Action::Dial(dial),
        );
    }

    fn button(&mut self, rect: [f32; 4], label: &str, action: Action, active: bool, ink: Color) {
        let [x, y, w, h] = self.layout.rect(rect);
        let (r, g, b, _) = widgets::color_to_rgba_u8(ink);
        widgets::draw_outline_button_scaled(
            self.pixmap,
            &mut self.panel.text,
            x,
            y,
            w,
            h,
            label,
            active,
            color([95, 112, 130]),
            color(MUTED),
            ink,
            Color::from_rgba8(r, g, b, 32),
            self.layout.scale,
        );
        self.hit(rect, action);
    }

    fn hit(&mut self, rect: [f32; 4], action: Action) {
        let [x, y, w, h] = self.layout.rect(rect);
        self.panel.drag.push_region(x, y, w, h, action);
    }

    fn draw_band_map(&mut self, splits: Splits, maximum: f32) {
        let x = |hz| 238.0 + widgets::freq_to_norm_x(hz, MIN_HZ, maximum) * 422.0;
        let boundaries = [238.0, x(splits.lower), x(splits.upper), 660.0];
        for (i, [r, g, b]) in BAND_COLORS.into_iter().enumerate() {
            let from = boundaries[i];
            let width = boundaries[i + 1] - from;
            self.rect([from, 173.0, width, 37.0], Color::from_rgba8(r, g, b, 35));
            self.rect([from, 173.0, width, 2.0], color([r, g, b]));
            if width >= 43.0 {
                self.text([from + 9.0, 196.0], BAND_NAMES[i], 12.0, color([r, g, b]));
            }
        }
        for (frequency, label) in [
            (20.0, "20 Hz"),
            (100.0, "100"),
            (1000.0, "1k"),
            (10_000.0, "10k"),
        ] {
            if frequency < maximum {
                self.rect([x(frequency), 210.0, 1.0, 5.0], color(MUTED));
                self.text([x(frequency), 229.0], label, 10.0, color(MUTED));
            }
        }
    }

    fn draw_band(&mut self, i: usize, name: &str) {
        let x = 20.0 + i as f32 * 226.0;
        let ink = color(BAND_COLORS[i]);
        let db = self.panel.display_gain[i];
        self.rect([x, 262.0, 216.0, 152.0], color(PANEL));
        self.rect([x, 262.0, 216.0, 2.0], ink);
        self.text([x + 12.0, 287.0], name, 15.0, color(TEXT));
        let label = format!("{db:+.1} dB");
        let width =
            self.panel.text.text_width(&label, 12.0 * self.layout.scale) / self.layout.scale;
        self.text([x + 204.0 - width, 287.0], &label, 12.0, color(MUTED));

        let zero = x + 12.0 + 192.0 * 0.4;
        let mark = zero + db.clamp(-32.0, 50.0) * 2.304;
        self.rect([x + 12.0, 297.0, 192.0, 7.0], color(BACKGROUND));
        self.rect([zero.min(mark), 297.0, (mark - zero).abs(), 7.0], ink);
        self.rect([zero, 296.0, 1.0, 9.0], color(TEXT));

        self.dial([x + 60.0, 350.0], 17.0, Dial::Threshold(i), "Threshold");
        self.dial([x + 156.0, 350.0], 17.0, Dial::Gain(i), "Gain");
        self.button(
            [x + 12.0, 383.0, 92.0, 22.0],
            "Upward",
            Action::Upward(i),
            !self.params.bands[i].bypass_up.value(),
            ink,
        );
        self.button(
            [x + 112.0, 383.0, 92.0, 22.0],
            "Downward",
            Action::Downward(i),
            !self.params.bands[i].bypass_down.value(),
            ink,
        );
    }
}

struct LincolnWindow {
    panel: Panel,
    layout: Layout,
    surface: widgets::SoftbufferSurface,
    params: Arc<LincolnParams>,
    meters: Arc<Meters>,
    context: Arc<dyn GuiContext>,
}

impl LincolnWindow {
    fn sample_rate(&self) -> f32 {
        f32::from_bits(self.meters.sample_rate.load(Ordering::Relaxed))
    }

    fn set_dial(&self, dial: Dial, normalized: f32) {
        let parameter = dial.param(&self.params);
        let value = parameter.preview_plain(normalized);
        let sample_rate = self.sample_rate();
        let splits = Splits::new(
            self.params.split_low.unmodulated_plain_value(),
            self.params.split_high.unmodulated_plain_value(),
            sample_rate,
        );
        let value = match dial {
            Dial::SplitLow => Splits::new(value, splits.upper, sample_rate).lower,
            Dial::SplitHigh => splits.limit_upper(value, sample_rate),
            _ => value,
        };
        ParamSetter::new(self.context.as_ref()).set_parameter(parameter, value);
    }

    fn edit_dial(&self, dial: Dial, normalized: f32) {
        let setter = ParamSetter::new(self.context.as_ref());
        let parameter = dial.param(&self.params);
        setter.begin_set_parameter(parameter);
        self.set_dial(dial, normalized);
        setter.end_set_parameter(parameter);
    }

    fn finish_drag(&mut self) {
        if let Some(Action::Dial(dial)) = self.panel.drag.end_drag() {
            ParamSetter::new(self.context.as_ref()).end_set_parameter(dial.param(&self.params));
        }
    }

    fn commit_text(&mut self) {
        if let Some((dial, text)) = self.panel.edit.commit()
            && let Some(value) = dial.param(&self.params).string_to_normalized_value(&text)
            && value.is_finite()
        {
            self.edit_dial(dial, value);
        }
    }

    fn begin_left(&mut self, modifiers: Modifiers) {
        self.finish_drag();
        self.commit_text();
        let action = self.panel.drag.hit_test().map(|region| region.action);
        let setter = ParamSetter::new(self.context.as_ref());
        match action {
            Some(action @ Action::Dial(dial)) => {
                let param = dial.param(&self.params);
                if self.panel.drag.check_double_click(&action) {
                    setter.begin_set_parameter(param);
                    setter.set_parameter(param, param.default_plain_value());
                    setter.end_set_parameter(param);
                } else {
                    let value = dial.base_normalized(&self.params);
                    self.panel
                        .drag
                        .begin_drag(action, value, modifiers.contains(Modifiers::SHIFT));
                    setter.begin_set_parameter(param);
                }
            }
            Some(action) => {
                self.panel.drag.clear_double_click();
                if let Some(param) = action.toggle(&self.params) {
                    setter.begin_set_parameter(param);
                    setter.set_parameter(param, !param.unmodulated_plain_value());
                    setter.end_set_parameter(param);
                }
            }
            None => self.panel.drag.clear_double_click(),
        }
    }

    fn begin_right(&mut self) {
        if self.panel.drag.active_action().is_some() {
            return;
        }
        self.panel.drag.clear_double_click();
        self.commit_text();
        if let Some(Action::Dial(dial)) = self.panel.drag.hit_test().map(|region| region.action) {
            let param = dial.param(&self.params);
            let norm = dial.base_normalized(&self.params);
            let initial = param.normalized_value_to_string(norm, false);
            self.panel.edit.begin(dial, &initial);
        }
    }
}

impl widgets::PhysicalEditor for LincolnWindow {
    fn prepare_resize(&mut self, _: &mut baseview::Window<'_>, backing_scale: f32) {
        self.finish_drag();
        self.commit_text();
        self.panel.drag.set_input_scale(backing_scale);
        self.panel.drag.clear_double_click();
    }
}

impl baseview::WindowHandler for LincolnWindow {
    fn on_frame(&mut self, _: &mut baseview::Window<'_>) {
        self.panel.draw(
            &mut self.surface.pixmap,
            self.layout,
            &self.params,
            &self.meters,
        );
        self.surface.present();
    }

    fn on_event(&mut self, _: &mut baseview::Window<'_>, event: Event) -> EventStatus {
        match event {
            Event::Window(WindowEvent::Resized(info)) => {
                let size = info.physical_size();
                self.layout =
                    Layout::new(size.width, size.height, widgets::editor_scale(&info, WIDTH));
                self.surface.resize(size.width, size.height);
            }
            Event::Window(WindowEvent::Unfocused | WindowEvent::WillClose) => {
                self.finish_drag();
                self.commit_text();
                self.panel.drag.clear_double_click();
            }
            Event::Mouse(MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                self.panel
                    .drag
                    .set_mouse(position.x as f32, position.y as f32);
                if let Some(Action::Dial(dial)) = self.panel.drag.active_action().copied() {
                    let current = dial.base_normalized(&self.params);
                    if let Some(target) = self.panel.drag.update_drag_with_sensitivity(
                        modifiers.contains(Modifiers::SHIFT),
                        current,
                        210.0 * self.layout.scale,
                    ) {
                        self.set_dial(dial, target);
                    }
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                modifiers,
            }) => self.begin_left(modifiers),
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Right,
                ..
            }) => self.begin_right(),
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            }) => self.finish_drag(),
            Event::Keyboard(event) if self.panel.edit.is_active() => {
                if event.state == KeyState::Down {
                    match event.key {
                        Key::Enter => self.commit_text(),
                        Key::Escape => self.panel.edit.cancel(),
                        Key::Backspace => self.panel.edit.backspace(),
                        Key::Character(s)
                            if s.eq_ignore_ascii_case("a")
                                && event
                                    .modifiers
                                    .intersects(Modifiers::CONTROL | Modifiers::META) =>
                        {
                            if let Some(dial) = self.panel.edit.active_for_any() {
                                self.panel.edit.begin(dial, "");
                            }
                        }
                        Key::Character(s) => {
                            s.chars().for_each(|c| self.panel.edit.insert_unit_char(c))
                        }
                        _ => {}
                    }
                }
            }
            _ => return EventStatus::Ignored,
        }
        EventStatus::Captured
    }
}

struct LincolnEditor {
    params: Arc<LincolnParams>,
    meters: Arc<Meters>,
    resize: Arc<AtomicU64>,
}

pub(crate) fn create(params: Arc<LincolnParams>, meters: Arc<Meters>) -> Box<dyn Editor> {
    Box::new(LincolnEditor {
        params,
        meters,
        resize: Arc::new(AtomicU64::new(0)),
    })
}

impl Editor for LincolnEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (width, height) = self.size();
        let params = self.params.clone();
        let meters = self.meters.clone();
        let resize = self.resize.clone();
        let options = WindowOpenOptions {
            title: "Lincoln".to_owned(),
            size: baseview::Size::new(f64::from(width), f64::from(height)),
            scale: WindowScalePolicy::ScaleFactor(1.0),
            gl_config: None,
        };
        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            options,
            move |window| {
                let state = params.editor_state.clone();
                let api = context.plugin_api();
                let inner = LincolnWindow {
                    panel: Panel::new(),
                    layout: Layout::new(width, height, width as f32 / WIDTH as f32),
                    surface: widgets::SoftbufferSurface::new(window, width, height),
                    params,
                    meters,
                    context,
                };
                widgets::EditorWindow::new(inner, state, resize, api)
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

    fn set_scale_factor(&self, _: f32) -> bool {
        false
    }

    fn set_size(&self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        self.resize.store(
            (u64::from(width) << 32) | u64::from(height),
            Ordering::Relaxed,
        );
        true
    }

    fn param_value_changed(&self, _: &str, _: f32) {}
    fn param_modulation_changed(&self, _: &str, _: f32) {}
    fn param_values_changed(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_keeps_the_band_map_on_the_last_applied_pair_until_idle() {
        let mut activity = BandMapActivity::default();
        let meters = Meters::default();
        let sample_rate = 48_000.0;
        let now = Instant::now();
        let applied = Splits::new(250.0, 4000.0, sample_rate);
        let target = Splits::new(300.0, 4200.0, sample_rate);
        assert_eq!(activity.splits(&meters, target, sample_rate, now), target);

        meters.publish_splits(applied);
        assert_eq!(activity.splits(&meters, applied, sample_rate, now), applied);
        assert_eq!(
            activity.splits(
                &meters,
                target,
                sample_rate,
                now + Duration::from_millis(30)
            ),
            applied
        );
        assert_eq!(
            activity.splits(
                &meters,
                target,
                sample_rate,
                now + Duration::from_millis(200)
            ),
            target
        );
    }

    #[test]
    fn compact_band_editor_keeps_text_entry_inside_the_dial_hit_region() {
        let params = LincolnParams::default();
        let meters = Meters::default();
        for (width, height) in [(354, 215), (266, 161), (1062, 645)] {
            let layout = Layout::new(width, height, width as f32 / WIDTH as f32);
            let mut panel = Panel::new();
            let mut normal = Pixmap::new(width, height).unwrap();
            let mut editing = Pixmap::new(width, height).unwrap();
            panel.draw(&mut normal, layout, &params, &meters);
            panel.edit.begin(Dial::Threshold(0), "100");
            panel.draw(&mut editing, layout, &params, &meters);

            let regions = panel.drag.regions();
            let dial = regions
                .iter()
                .find(|region| region.action == Action::Dial(Dial::Threshold(0)))
                .unwrap();
            let button = regions
                .iter()
                .find(|region| region.action == Action::Upward(0))
                .unwrap();
            let row_len = width as usize * 4;
            let mut changed = false;
            for (y, (before, after)) in normal
                .data()
                .chunks_exact(row_len)
                .zip(editing.data().chunks_exact(row_len))
                .enumerate()
            {
                if before != after {
                    changed = true;
                    assert!((y as f32) < (dial.y + dial.h).ceil());
                    assert!((y as f32) < button.y);
                }
            }
            assert!(changed);
        }
    }

    #[test]
    fn typed_gain_mute_round_trips_to_zero() {
        let params = LincolnParams::default();
        let gain = Dial::Gain(0).param(&params);
        let mut edit = widgets::TextEditState::new();
        edit.begin(Dial::Gain(0), "");
        for c in "-inf".chars() {
            edit.insert_unit_char(c);
        }
        let (_, text) = edit.commit().unwrap();
        assert_eq!(gain.string_to_normalized_value(&text), Some(0.0));
    }
}
