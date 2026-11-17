use super::*;
use crate::modulation::{
    DetectorInput, FOLLOWER_ATTACK_MAX_MS, FOLLOWER_GAIN_DB, FOLLOWER_HOLD_MAX_MS,
    FOLLOWER_RELEASE_MAX_MS, FollowerConfig, FollowerMode, FollowerReading,
};
use widgets::dropdown::DropdownState;
mod events;
mod view;

type Rect = (f32, f32, f32, f32);
const ROWS: usize = 11;
const MODES: [&str; 3] = ["Follow", "Add", "Multiply"];
const INPUTS: [&str; 2] = ["Main", "Sidechain"];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Amount,
    Attack,
    Hold,
    Release,
    Threshold,
    Gain,
    Highpass,
    Lowpass,
}
impl Field {
    const ALL: [Self; 8] = [
        Self::Amount,
        Self::Attack,
        Self::Hold,
        Self::Release,
        Self::Threshold,
        Self::Gain,
        Self::Highpass,
        Self::Lowpass,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Amount => "Amount",
            Self::Attack => "Attack",
            Self::Hold => "Hold",
            Self::Release => "Release",
            Self::Threshold => "Threshold",
            Self::Gain => "Gain",
            Self::Highpass => "High-pass",
            Self::Lowpass => "Low-pass",
        }
    }
    fn detector(self) -> Option<DetectorField> {
        match self {
            Self::Threshold => Some(DetectorField::Level),
            Self::Highpass => Some(DetectorField::Highpass),
            Self::Lowpass => Some(DetectorField::Lowpass),
            _ => None,
        }
    }
    fn bounds(self, cfg: FollowerConfig, slot: usize) -> (f32, f32) {
        match self {
            Self::Amount => (
                if cfg.routes[slot].mode == FollowerMode::Add {
                    -200.0
                } else {
                    0.0
                },
                200.0,
            ),
            Self::Attack => (0.0, FOLLOWER_ATTACK_MAX_MS),
            Self::Hold => (0.0, FOLLOWER_HOLD_MAX_MS),
            Self::Release => (0.0, FOLLOWER_RELEASE_MAX_MS),
            Self::Gain => (-FOLLOWER_GAIN_DB, FOLLOWER_GAIN_DB),
            _ => (0.0, 1.0),
        }
    }
    fn value(self, cfg: FollowerConfig, slot: usize) -> Option<f32> {
        match self {
            Self::Amount => Some(cfg.routes[slot].amount * 100.0),
            Self::Attack => Some(cfg.attack_ms),
            Self::Hold => Some(cfg.hold_ms),
            Self::Release => Some(cfg.release_ms),
            Self::Gain => Some(cfg.gain_db),
            _ => self.detector().and_then(|field| field.value(cfg.detector)),
        }
    }
    fn set(self, cfg: &mut FollowerConfig, slot: usize, value: Option<f32>) {
        if let Some(field) = self.detector() {
            field.set(&mut cfg.detector, value);
            return;
        }
        let Some(value) = value else {
            return;
        };
        match self {
            Self::Amount => cfg.routes[slot].amount = value * 0.01,
            Self::Attack => cfg.attack_ms = value,
            Self::Hold => cfg.hold_ms = value,
            Self::Release => cfg.release_ms = value,
            Self::Gain => cfg.gain_db = value,
            _ => {}
        }
    }
    fn norm(self, cfg: FollowerConfig, slot: usize) -> f32 {
        if let Some(field) = self.detector() {
            return field.norm(field.value(cfg.detector));
        }
        let (lo, hi) = self.bounds(cfg, slot);
        let value = self.value(cfg, slot).unwrap_or(0.0);
        if matches!(self, Self::Attack | Self::Hold | Self::Release) {
            value.ln_1p() / hi.ln_1p()
        } else {
            (value - lo) / (hi - lo)
        }
    }
    fn at_norm(self, cfg: FollowerConfig, slot: usize, norm: f32) -> Option<f32> {
        if let Some(field) = self.detector() {
            return field.value_at_norm(norm);
        }
        let (lo, hi) = self.bounds(cfg, slot);
        let n = norm.clamp(0.0, 1.0);
        Some(
            if matches!(self, Self::Attack | Self::Hold | Self::Release) {
                (n * hi.ln_1p()).exp_m1()
            } else {
                lo + n * (hi - lo)
            },
        )
    }
    fn parse(self, cfg: FollowerConfig, slot: usize, text: &str) -> Option<Option<f32>> {
        if let Some(field) = self.detector() {
            return field.parse(text);
        }
        let format = effects::ParamFormat::Number {
            decimals: 1,
            unit: match self {
                Self::Amount => "%",
                Self::Gain => "dB",
                _ => "ms",
            },
        };
        let value = effects::parse_value(text, format).filter(|value| value.is_finite())?;
        let (lo, hi) = self.bounds(cfg, slot);
        (lo..=hi).contains(&value).then_some(Some(value))
    }
    fn display(self, cfg: FollowerConfig, slot: usize) -> String {
        if let Some(field) = self.detector() {
            return field.display(field.value(cfg.detector));
        }
        let value = self.value(cfg, slot).unwrap_or(0.0);
        match self {
            Self::Amount => format!("{value:.0}%"),
            Self::Gain => format!("{value:.1} dB"),
            _ => format!("{value:.1} ms"),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Mode,
    Input,
}
impl Choice {
    fn items(self) -> &'static [&'static str] {
        match self {
            Self::Mode => &MODES,
            Self::Input => &INPUTS,
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
struct Binding {
    kind: EffectKind,
    config: FollowerConfig,
    slot: usize,
    live: Option<(usize, usize, u64)>,
}
pub(super) struct Panel {
    open: bool,
    owner: Option<Binding>,
    pub(super) focus: usize,
    pub(super) scroll: usize,
    dropdown: DropdownState<Choice>,
    drag: widgets::DragState<Field>,
    text: widgets::TextEditState<Field>,
    click: ClickTracker<Field>,
    drag_start: Option<FollowerConfig>,
}
impl Default for Panel {
    fn default() -> Self {
        Self {
            open: false,
            owner: None,
            focus: 0,
            scroll: 0,
            dropdown: DropdownState::new(),
            drag: widgets::DragState::new(),
            text: widgets::TextEditState::new(),
            click: ClickTracker::new(),
            drag_start: None,
        }
    }
}
impl Panel {
    pub(in crate::editor) fn close(&mut self) {
        self.open = false;
        self.owner = None;
        self.dropdown.close();
        self.drag.end_drag();
        self.text.cancel();
        self.drag_start = None;
    }
    pub(in crate::editor) fn open(&self) -> bool {
        self.open
    }
    pub(in crate::editor) fn dragging(&self) -> bool {
        self.drag.active_action().is_some()
    }
    fn choice(&self) -> Option<Choice> {
        [Choice::Mode, Choice::Input]
            .into_iter()
            .find(|choice| self.dropdown.is_open_for(*choice))
    }
    fn reveal(&mut self, visible: usize) {
        if self.focus < self.scroll {
            self.scroll = self.focus;
        } else if self.focus >= self.scroll + visible {
            self.scroll = self.focus + 1 - visible;
        }
        self.scroll = self.scroll.min(ROWS.saturating_sub(visible));
    }
}
pub(super) struct Layout {
    pub panel: Rect,
    pub close: Rect,
    pub body: Rect,
    pub rows: [Option<Rect>; ROWS],
    pub visible: usize,
    pub footer: Rect,
    pub reset: Rect,
    pub listen: Rect,
    pub scale: f32,
}
pub(super) fn layout(geometry: EditorGeometry, scroll: usize) -> Option<Layout> {
    let (ww, wh) = geometry.window_size;
    let s = geometry.scale.min(((ww - 8.0) / 480.0).max(0.5));
    let pad = (8.0 * s).max(4.0);
    let w = (480.0 * s).max(280.0).min(ww - 2.0 * pad);
    let h = (560.0 * s).max(320.0).min(wh - 2.0 * pad);
    let header = (50.0 * s).max(40.0);
    let footer = (90.0 * s).max(88.0);
    let row = (36.0 * s).max(28.0);
    if w < 216.0 || h < header + footer + row {
        return None;
    }
    let pane = geometry.layout.mseg_pane;
    let x = (pane.0 + (pane.2 - w) * 0.5).clamp(pad, (ww - w - pad).max(pad));
    let y = (pane.1 + (pane.3 - h) * 0.5).clamp(pad, (wh - h - pad).max(pad));
    let visible = (((h - header - footer) / row).floor() as usize).clamp(1, ROWS);
    let scroll = scroll.min(ROWS - visible);
    let body = (x + pad, y + header, w - 2.0 * pad, visible as f32 * row);
    Some(Layout {
        panel: (x, y, w, h),
        close: (
            x + w - pad - (22.0 * s).max(18.0),
            y + pad,
            (22.0 * s).max(18.0),
            (22.0 * s).max(18.0),
        ),
        body,
        rows: std::array::from_fn(|index| {
            (index >= scroll && index < scroll + visible).then(|| {
                (
                    body.0,
                    body.1 + (index - scroll) as f32 * row,
                    body.2 - 8.0,
                    (row - 4.0 * s).min((32.0 * s).max(24.0)),
                )
            })
        }),
        visible,
        footer: (x + pad, y + h - footer, w - 2.0 * pad, footer),
        reset: (
            x + pad,
            y + h - pad - (26.0 * s).max(24.0),
            (130.0 * s).max(110.0).min((w - 2.0 * pad - 6.0 * s) * 0.5),
            (26.0 * s).max(24.0),
        ),
        listen: (
            x + w - pad - (130.0 * s).max(110.0).min((w - 2.0 * pad - 6.0 * s) * 0.5),
            y + h - pad - (26.0 * s).max(24.0),
            (130.0 * s).max(110.0).min((w - 2.0 * pad - 6.0 * s) * 0.5),
            (26.0 * s).max(24.0),
        ),
        scale: s,
    })
}
fn bounded_text(
    tr: &mut widgets::TextRenderer,
    pm: &mut tiny_skia::Pixmap,
    at: (f32, f32),
    text: &str,
    size: f32,
    width: f32,
    color: tiny_skia::Color,
) {
    let mut end = text.len();
    while end > 0 && tr.text_width(&text[..end], size) > width {
        end = text[..end].char_indices().next_back().map_or(0, |(i, _)| i);
    }
    tr.draw_text(pm, at.0, at.1, &text[..end], size, color);
}
impl EffectEditor {
    fn follower_binding(&self, effect: &TrackEffect, cfg: &TrackModulation) -> Binding {
        Binding {
            kind: effect.kind,
            config: cfg.follower.normalized(),
            slot: self.selected_mseg.min(3),
            live: self.live.as_ref().map(|source| source.binding_key()),
        }
    }
    pub fn follower_panel_open(&self) -> bool {
        self.follower.open
    }
    pub(in crate::editor) fn check_follower_binding(
        &mut self,
        effect: &TrackEffect,
        cfg: &TrackModulation,
    ) -> bool {
        if self.follower.open && self.follower.owner != Some(self.follower_binding(effect, cfg)) {
            self.stop_listening_source(crate::modulation::AuditionSource::Follower);
            self.follower.close();
            true
        } else {
            false
        }
    }
    pub(in crate::editor) fn follower_reading(&self) -> Option<FollowerReading> {
        let owner = self.follower.owner?;
        self.live
            .as_ref()
            .and_then(|source| source.frame)
            .filter(|frame| frame.kind == owner.kind)
            .and_then(|frame| frame.follower)
            .filter(|reading| {
                reading.config == owner.config
                    && reading.level.is_finite()
                    && reading.envelope.is_finite()
            })
    }
}
