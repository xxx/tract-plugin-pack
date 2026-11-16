use super::help::{draw_fitted, wrap};
use super::{MsegEdit, MsegEditState};
use crate::mseg::library::FAVORITE_COUNT;
use crate::mseg::render::in_rect;
use crate::mseg::{CurveShape, FactoryShape, MsegData, WavePalette, factory_shape, segment_value};
use crate::{
    TextRenderer, color_accent, color_bg, color_border, color_control_bg, color_muted, color_text,
    draw_rect, draw_rect_outline,
};
use keyboard_types::{Key, KeyState, KeyboardEvent, Modifiers};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tiny_skia::{Paint, PathBuilder, Pixmap, Stroke, Transform};

type Rect = (f32, f32, f32, f32);

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Page {
    #[default]
    Factory,
    Favorites,
    Project,
}

impl Page {
    fn label(self) -> &'static str {
        match self {
            Self::Factory => "Factory",
            Self::Favorites => "Favorites",
            Self::Project => "Project",
        }
    }

    fn short_label(self) -> &'static str {
        match self {
            Self::Factory => "Fct",
            Self::Favorites => "Fav",
            Self::Project => "Prj",
        }
    }
}

#[derive(Default)]
enum Favorite {
    #[default]
    Empty,
    Shape(CurveShape),
    Unavailable(String),
}

#[derive(Default)]
pub(super) struct BrowserState {
    open: bool,
    page: Page,
    selected: usize,
    factory: Vec<CurveShape>,
    favorites: [Favorite; FAVORITE_COUNT],
    project_binding: Option<Arc<Mutex<WavePalette>>>,
    project: WavePalette,
    project_error: Option<&'static str>,
    message: String,
    last_click: Option<(Page, usize, Instant)>,
}

#[derive(Clone)]
pub(crate) struct BrowserLayout {
    pub panel: Rect,
    pub close: Rect,
    pub tabs: Vec<Rect>,
    pub cards: [Rect; FAVORITE_COUNT],
    pub buttons: [Rect; 5],
    detail: Rect,
    hint: Rect,
}

impl MsegEditState {
    /// The GUI/preset bank supplied by the embedding host.
    pub fn wave_palette(&self) -> Option<&Arc<Mutex<WavePalette>>> {
        self.browser.project_binding.as_ref()
    }

    /// Bind a GUI/preset bank shared by the curves of one plugin instance.
    pub fn bind_wave_palette(&mut self, palette: Arc<Mutex<WavePalette>>) {
        if self
            .browser
            .project_binding
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &palette))
        {
            return;
        }
        self.close_curve_browser();
        self.browser.project_binding = Some(palette);
        self.browser.project = WavePalette::default();
        self.browser.project_error = None;
    }

    fn browser_pages(&self) -> &'static [Page] {
        if self.browser.project_binding.is_some() {
            &[Page::Factory, Page::Favorites, Page::Project]
        } else {
            &[Page::Factory, Page::Favorites]
        }
    }

    fn select_browser_page(&mut self, page: Page) {
        self.browser.page = page;
        self.browser.selected = 0;
        self.browser.last_click = None;
        self.browser.message.clear();
        if page == Page::Project {
            self.refresh_browser_project();
        }
    }

    fn refresh_browser_project(&mut self) {
        let snapshot = self
            .browser
            .project_binding
            .as_ref()
            .ok_or("Project waves unavailable")
            .and_then(|binding| {
                binding
                    .lock()
                    .map(|palette| palette.clone())
                    .map_err(|_| "Project waves unavailable")
            });
        match snapshot {
            Ok(palette) => {
                self.browser.project = palette;
                self.browser.project_error = None;
            }
            Err(error) => self.browser.project_error = Some(error),
        }
        self.browser.last_click = None;
        self.browser.message.clear();
    }

    pub fn curve_browser_open(&self) -> bool {
        self.browser.open
    }

    /// Open a snapshot of factory curves and shared favorites without changing the curve.
    pub fn open_curve_browser(&mut self, rect: Rect, scale: f32) -> bool {
        if !scale.is_finite()
            || scale <= 0.0
            || self.is_dragging()
            || self.has_open_menu()
            || self.geometry_read_only()
        {
            return false;
        }
        if self.browser_layout(rect, scale).is_none() {
            self.set_status_message("Enlarge the window to browse curves");
            return false;
        }
        self.set_overlay_scale(scale);
        self.clear_hover_hint();
        self.browser.open = true;
        self.browser.page = Page::Factory;
        self.browser.selected = 0;
        self.browser.last_click = None;
        self.browser.factory = FactoryShape::ALL.into_iter().map(factory_shape).collect();
        self.refresh_browser_favorites();
        true
    }

    pub(super) fn close_curve_browser(&mut self) {
        self.browser.open = false;
        self.browser.last_click = None;
    }

    fn refresh_browser_favorites(&mut self) {
        self.browser.last_click = None;
        let library = self.curve_library();
        self.browser.favorites = std::array::from_fn(|slot| match &library {
            Ok(library) => match library.load_favorite(slot) {
                Ok(shape) => Favorite::Shape(shape),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Favorite::Empty,
                Err(err) => Favorite::Unavailable(err.to_string()),
            },
            Err(err) => Favorite::Unavailable(err.to_string()),
        });
        self.browser.message.clear();
    }

    fn browser_shape(&self) -> Option<&CurveShape> {
        match self.browser.page {
            Page::Factory => self.browser.factory.get(self.browser.selected),
            Page::Favorites => match &self.browser.favorites[self.browser.selected] {
                Favorite::Shape(shape) => Some(shape),
                _ => None,
            },
            Page::Project => self
                .browser
                .project
                .get(self.browser.selected)
                .filter(|_| self.browser.project_error.is_none()),
        }
    }

    fn browser_actions(&self) -> [bool; 5] {
        let load = self.browser_shape().is_some();
        let (store, clear) = match self.browser.page {
            Page::Factory => (false, false),
            Page::Favorites => (
                true,
                !matches!(
                    self.browser.favorites[self.browser.selected],
                    Favorite::Empty
                ),
            ),
            Page::Project => (self.browser.project_error.is_none(), load),
        };
        [load, load, store, clear, true]
    }

    fn browser_action(&mut self, action: usize, data: &mut MsegData) -> Option<MsegEdit> {
        if self.browser.page == Page::Project {
            return self.project_browser_action(action, data);
        }
        if !self.browser_actions().get(action).copied().unwrap_or(false) {
            return None;
        }
        if action < 2 {
            let shape = self.browser_shape()?.clone();
            return self.apply_shape(data, shape, action == 1);
        }
        let slot = self.browser.selected;
        match action {
            2 => {
                match self
                    .curve_library()
                    .and_then(|library| library.store_favorite(slot, &CurveShape::capture(data)))
                {
                    Ok(()) => {
                        self.refresh_browser_favorites();
                        self.browser.message = format!("Favorite {} stored", slot + 1);
                    }
                    Err(err) => self.browser.message = format!("Store failed: {err}"),
                }
            }
            3 => {
                match self
                    .curve_library()
                    .and_then(|library| library.remove_favorite(slot))
                {
                    Ok(()) => {
                        self.refresh_browser_favorites();
                        self.browser.message = format!("Favorite {} cleared", slot + 1);
                    }
                    Err(err) => self.browser.message = format!("Clear failed: {err}"),
                }
            }
            4 => self.refresh_browser_favorites(),
            _ => {}
        }
        None
    }

    fn project_browser_action(&mut self, action: usize, data: &mut MsegData) -> Option<MsegEdit> {
        let binding = self.browser.project_binding.as_ref()?.clone();
        let Ok(mut live) = binding.lock() else {
            self.browser.project_error = Some("Project waves unavailable");
            self.browser.message = "Project waves unavailable".to_owned();
            return None;
        };
        self.browser.last_click = None;
        if action == 4 {
            self.browser.project = live.clone();
            self.browser.project_error = None;
            self.browser.message = "Project waves refreshed".to_owned();
            return None;
        }
        if self.browser.project != *live || self.browser.project_error.is_some() {
            self.browser.project = live.clone();
            self.browser.project_error = None;
            self.browser.message = "Project waves changed; choose a slot again".to_owned();
            return None;
        }
        let slot = self.browser.selected;
        if action < 2 {
            let shape = live.get(slot)?.clone();
            drop(live);
            return self.apply_shape(data, shape, action == 1);
        }
        let result = match action {
            2 => live.store(slot, &CurveShape::capture(data)),
            3 => live.clear(slot),
            _ => return None,
        };
        match result {
            Ok(_) => {
                self.browser.project = live.clone();
                self.browser.message = format!(
                    "Project {} {}",
                    slot + 1,
                    if action == 2 { "stored" } else { "cleared" }
                );
            }
            Err(error) => self.browser.message = error.to_string(),
        }
        None
    }

    pub(crate) fn browser_layout(&self, rect: Rect, scale: f32) -> Option<BrowserLayout> {
        let base = self.help_layout(rect, scale)?;
        let (x, y, w, h) = base.panel;
        let s = scale.max(0.01);
        let pad = (12.0 * s).max(6.0);
        let gap = (6.0 * s).max(4.0);
        let header = (34.0 * s).max(28.0);
        let tab_h = (26.0 * s).max(20.0);
        let button_h = (28.0 * s).max(20.0);
        let line = ((12.0 * s).max(10.0) * 1.4).ceil();
        let footer_h = 3.0 * line + button_h + 3.0 * gap;
        let body_y = y + header + tab_h + gap;
        let footer_y = y + h - footer_h;
        let inner_w = w - 2.0 * pad;
        let card_w = (inner_w - 3.0 * gap) / 4.0;
        let card_h = (footer_y - gap - body_y - gap) / 2.0;
        let preview = thumbnail_rect((0.0, 0.0, card_w, card_h), s);
        let min_preview_scale = s.max(1.0);
        if preview.2 < 16.0 * min_preview_scale || preview.3 < 8.0 * min_preview_scale {
            return None;
        }
        let tab_count = self.browser_pages().len();
        let tab_w = (inner_w - gap * (tab_count - 1) as f32) / tab_count as f32;
        let button_w = (inner_w - 4.0 * gap) / 5.0;
        Some(BrowserLayout {
            panel: base.panel,
            close: base.close,
            tabs: (0..tab_count)
                .map(|i| (x + pad + i as f32 * (tab_w + gap), y + header, tab_w, tab_h))
                .collect(),
            cards: std::array::from_fn(|i| {
                (
                    x + pad + (i % 4) as f32 * (card_w + gap),
                    body_y + (i / 4) as f32 * (card_h + gap),
                    card_w,
                    card_h,
                )
            }),
            buttons: std::array::from_fn(|i| {
                (
                    x + pad + i as f32 * (button_w + gap),
                    footer_y + line + gap,
                    button_w,
                    button_h,
                )
            }),
            detail: (x + pad, footer_y, inner_w, line),
            hint: (
                x + pad,
                footer_y + line + 2.0 * gap + button_h,
                inner_w,
                line * 2.0,
            ),
        })
    }

    pub(super) fn browser_mouse_down(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        rect: Rect,
        scale: f32,
        double: bool,
    ) -> Option<MsegEdit> {
        let Some(layout) = self.browser_layout(rect, scale) else {
            self.close_curve_browser();
            return None;
        };
        if in_rect(layout.close, x, y) || !in_rect(layout.panel, x, y) {
            self.close_curve_browser();
        } else if let Some(tab) = layout.tabs.iter().position(|r| in_rect(*r, x, y)) {
            self.select_browser_page(self.browser_pages()[tab]);
        } else if let Some(card) = layout.cards.iter().position(|r| in_rect(*r, x, y)) {
            self.browser.selected = card;
            self.browser.message.clear();
            if double || self.browser_double_click(card, Instant::now()) {
                return self.browser_action(0, data);
            }
        } else if let Some(action) = layout.buttons.iter().position(|r| in_rect(*r, x, y)) {
            self.browser.last_click = None;
            return self.browser_action(action, data);
        } else {
            self.browser.last_click = None;
        }
        None
    }

    pub(super) fn browser_key(
        &mut self,
        event: &KeyboardEvent,
        data: &mut MsegData,
    ) -> Option<MsegEdit> {
        if event.state != KeyState::Down {
            return None;
        }
        self.browser.last_click = None;
        if event.key == Key::Escape {
            self.close_curve_browser();
            return None;
        }
        if event
            .modifiers
            .intersects(Modifiers::CONTROL | Modifiers::META | Modifiers::ALT)
        {
            return None;
        }
        let selected = self.browser.selected;
        match &event.key {
            Key::Tab => {
                let pages = self.browser_pages();
                let index = pages
                    .iter()
                    .position(|page| *page == self.browser.page)
                    .unwrap_or(0);
                let delta = if event.modifiers.contains(Modifiers::SHIFT) {
                    pages.len() - 1
                } else {
                    1
                };
                self.select_browser_page(pages[(index + delta) % pages.len()]);
            }
            Key::ArrowLeft => self.browser.selected = (selected + 7) % FAVORITE_COUNT,
            Key::ArrowRight => self.browser.selected = (selected + 1) % FAVORITE_COUNT,
            Key::ArrowUp | Key::ArrowDown => {
                self.browser.selected = (selected + 4) % FAVORITE_COUNT
            }
            Key::Home => self.browser.selected = 0,
            Key::End => self.browser.selected = FAVORITE_COUNT - 1,
            Key::Enter => {
                return self.browser_action(
                    usize::from(event.modifiers.contains(Modifiers::SHIFT)),
                    data,
                );
            }
            Key::Delete | Key::Backspace => return self.browser_action(3, data),
            Key::Character(text) if text.eq_ignore_ascii_case("s") => {
                return self.browser_action(2, data);
            }
            Key::Character(text) if text.eq_ignore_ascii_case("r") => {
                return self.browser_action(4, data);
            }
            _ => return None,
        }
        self.browser.message.clear();
        None
    }

    fn browser_double_click(&mut self, card: usize, now: Instant) -> bool {
        if self
            .browser
            .last_click
            .take()
            .is_some_and(|(page, previous, time)| {
                page == self.browser.page
                    && previous == card
                    && now
                        .checked_duration_since(time)
                        .is_some_and(|elapsed| elapsed < Duration::from_millis(400))
            })
        {
            true
        } else {
            self.browser.last_click = Some((self.browser.page, card, now));
            false
        }
    }

    pub(crate) fn draw_curve_browser(
        &self,
        pm: &mut Pixmap,
        tr: &mut TextRenderer,
        rect: Rect,
        scale: f32,
    ) {
        if !self.curve_browser_open() {
            return;
        }
        let Some(layout) = self.browser_layout(rect, scale) else {
            return;
        };
        let s = scale.max(0.01);
        let (x, y, w, h) = layout.panel;
        draw_rect(pm, x, y, w, h, color_bg());
        draw_rect_outline(pm, x, y, w, h, color_accent(), 1.0);
        draw_fitted(
            tr,
            pm,
            (x + 8.0 * s, y + 22.0 * s.max(0.7)),
            "Curve browser",
            (16.0 * s).max(11.0),
            layout.close.0 - x - 14.0 * s,
            color_text(),
        );
        let r = layout.close;
        crate::controls::draw_button(pm, tr, r.0, r.1, r.2, r.3, "X", false, false);
        for (page, r) in self.browser_pages().iter().zip(layout.tabs) {
            let active = self.browser.page == *page;
            crate::controls::draw_button(pm, tr, r.0, r.1, r.2, r.3, "", active, false);
            let size = (r.3 * 0.5).max(10.0);
            let available = (r.2 - 8.0).max(0.0);
            let label = if tr.text_width(page.label(), size) <= available {
                page.label()
            } else {
                page.short_label()
            };
            let width = tr.text_width(label, size).min(available);
            draw_fitted(
                tr,
                pm,
                (r.0 + (r.2 - width) * 0.5, r.1 + (r.3 + size) * 0.5 - 2.0),
                label,
                size,
                available,
                if active { color_bg() } else { color_text() },
            );
        }
        let font = (12.0 * s).max(10.0);
        for (i, card) in layout.cards.into_iter().enumerate() {
            let (shape, label) = match self.browser.page {
                Page::Factory => (
                    self.browser.factory.get(i),
                    FactoryShape::LABELS[i].to_owned(),
                ),
                Page::Favorites => match &self.browser.favorites[i] {
                    Favorite::Shape(shape) => (Some(shape), format!("Favorite {}", i + 1)),
                    Favorite::Empty => (None, format!("{}: Empty", i + 1)),
                    Favorite::Unavailable(_) => (None, format!("{}: Unavailable", i + 1)),
                },
                Page::Project => {
                    let shape = self
                        .browser
                        .project
                        .get(i)
                        .filter(|_| self.browser.project_error.is_none());
                    (
                        shape,
                        if shape.is_some() {
                            format!("Project {}", i + 1)
                        } else {
                            format!(
                                "{}: {}",
                                i + 1,
                                if self.browser.project_error.is_some() {
                                    "Unavailable"
                                } else {
                                    "Empty"
                                }
                            )
                        },
                    )
                }
            };
            draw_rect(pm, card.0, card.1, card.2, card.3, color_control_bg());
            draw_rect_outline(
                pm,
                card.0,
                card.1,
                card.2,
                card.3,
                if self.browser.selected == i {
                    color_accent()
                } else {
                    color_border()
                },
                s.max(1.0),
            );
            if let Some(shape) = shape {
                draw_thumbnail(pm, shape, thumbnail_rect(card, s), s);
            }
            draw_fitted(
                tr,
                pm,
                (card.0 + 4.0, card.1 + card.3 - 5.0),
                &label,
                font,
                card.2 - 8.0,
                if shape.is_some() {
                    color_text()
                } else {
                    color_muted()
                },
            );
        }
        let detail = if !self.browser.message.is_empty() {
            self.browser.message.clone()
        } else {
            let slot = self.browser.selected;
            match self.browser.page {
                Page::Factory => FactoryShape::LABELS[slot].to_owned(),
                Page::Favorites => match &self.browser.favorites[slot] {
                    Favorite::Shape(shape) => {
                        format!("Favorite {} - {} points", slot + 1, shape.nodes().len())
                    }
                    Favorite::Empty => format!(
                        "Favorite {} is empty; Store saves the current curve",
                        slot + 1
                    ),
                    Favorite::Unavailable(err) => {
                        format!("Favorite {} unavailable: {err}", slot + 1)
                    }
                },
                Page::Project => {
                    if let Some(error) = self.browser.project_error {
                        error.to_owned()
                    } else if let Some(shape) = self.browser.project.get(slot) {
                        format!("Project {} - {} points", slot + 1, shape.nodes().len())
                    } else {
                        format!(
                            "Project {} is empty; Store saves the current curve",
                            slot + 1
                        )
                    }
                }
            }
        };
        draw_fitted(
            tr,
            pm,
            (layout.detail.0, layout.detail.1 + font),
            &detail,
            font,
            layout.detail.2,
            color_text(),
        );
        for ((r, label), enabled) in layout
            .buttons
            .into_iter()
            .zip(["Load", "Fit", "Store", "Clear", "Refresh"])
            .zip(self.browser_actions())
        {
            crate::controls::draw_button(pm, tr, r.0, r.1, r.2, r.3, "", false, false);
            let size = (r.3 * 0.5).max(10.0);
            let size = size * ((r.2 - 8.0) / tr.text_width(label, size)).min(1.0);
            let width = tr.text_width(label, size);
            tr.draw_text(
                pm,
                r.0 + (r.2 - width) * 0.5,
                r.1 + (r.3 + size) * 0.5 - 2.0,
                label,
                size,
                if enabled { color_text() } else { color_muted() },
            );
        }
        let hint = if self.browser.page != Page::Factory {
            "Enter: load; Shift+Enter: fit; S: store; Del: clear; R: refresh"
        } else {
            "Arrows: select; Tab: favorites; Enter: load; Shift+Enter: fit; Esc: close"
        };
        let lines = wrap(tr, hint, font, layout.hint.2);
        for (i, text) in lines.iter().take(2).enumerate() {
            tr.draw_text(
                pm,
                layout.hint.0,
                layout.hint.1 + font + i as f32 * layout.hint.3 * 0.5,
                text,
                font,
                color_muted(),
            );
        }
    }
}

fn thumbnail_rect(card: Rect, scale: f32) -> Rect {
    let pad = (6.0 * scale).max(4.0);
    let font = (12.0 * scale).max(10.0);
    (
        card.0 + pad,
        card.1 + pad,
        (card.2 - 2.0 * pad).max(0.0),
        (card.3 - 2.0 * pad - font * 1.5).max(0.0),
    )
}

#[cfg(test)]
mod project_tests;

fn draw_thumbnail(pm: &mut Pixmap, shape: &CurveShape, rect: Rect, scale: f32) {
    if rect.2 <= 0.0 || rect.3 <= 0.0 {
        return;
    }
    let at = |time: f32, value: f32| (rect.0 + time * rect.2, rect.1 + (1.0 - value) * rect.3);
    let mut path = PathBuilder::new();
    let nodes = shape.nodes();
    let Some(first) = nodes.first() else {
        return;
    };
    let start = at(first.time, first.value);
    path.move_to(start.0, start.1);
    for pair in nodes.windows(2) {
        if pair[0].stepped {
            let end = at(pair[1].time, pair[0].value);
            path.line_to(end.0, end.1);
        } else {
            let samples = ((pair[1].time - pair[0].time) * rect.2)
                .ceil()
                .clamp(2.0, 1024.0) as usize;
            for sample in 1..samples {
                let phase =
                    pair[0].time + (pair[1].time - pair[0].time) * sample as f32 / samples as f32;
                let point = at(phase, segment_value(&pair[0], &pair[1], phase));
                path.line_to(point.0, point.1);
            }
        }
        let end = at(pair[1].time, pair[1].value);
        path.line_to(end.0, end.1);
    }
    if let Some(path) = path.finish() {
        let mut paint = Paint::default();
        paint.set_color(color_accent());
        pm.stroke_path(
            &path,
            &paint,
            &Stroke {
                width: scale.max(0.8),
                ..Stroke::default()
            },
            Transform::identity(),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::library::CurveLibrary;
    use crate::mseg::render::{phase_to_x, power_buttons, value_to_y};
    use crate::mseg::{
        HoldMode, MsegTool, PlayMode, PointWeight, Polarity, SegmentShape, StripId, SyncMode,
        ValueGrid,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    const RECT: Rect = (20.0, 70.0, 1000.0, 420.0);
    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn isolated(curve_only: bool) -> (MsegEditState, CurveLibrary, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "tract-browser-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut state = if curve_only {
            MsegEditState::new_curve_only()
        } else {
            MsegEditState::new()
        };
        state.set_curve_library_directory(root.clone());
        state.set_overlay_bounds((1040.0, 720.0));
        (state, CurveLibrary::at(root.clone()), root)
    }

    fn key(key: Key) -> KeyboardEvent {
        KeyboardEvent {
            key,
            state: KeyState::Down,
            ..KeyboardEvent::default()
        }
    }

    fn press(state: &mut MsegEditState, data: &mut MsegData, r: Rect) -> Option<MsegEdit> {
        state.on_mouse_down(r.0 + r.2 * 0.5, r.1 + r.3 * 0.5, data, RECT, 1.0, false)
    }

    #[test]
    fn shapes_entry_and_all_factory_loads_preserve_destination_settings() {
        for curve_only in [false, true] {
            let (mut state, _, root) = isolated(curve_only);
            for (index, kind) in FactoryShape::ALL.into_iter().enumerate() {
                let mut data = MsegData {
                    play_mode: PlayMode::Cyclic,
                    hold: HoldMode::Sustain(0),
                    sync_mode: SyncMode::Beat,
                    time_seconds: 2.5,
                    beats: 7.0,
                    time_divisions: 12,
                    value_steps: 5,
                    value_grid: ValueGrid::Semitones,
                    snap: false,
                    polarity: Polarity::Bipolar,
                    ..MsegData::default()
                };
                let before = data;
                state.selection = 3;
                let shape_button = power_buttons(state.layout(RECT, 1.0).strip, 1.0).shapes;
                assert_eq!(press(&mut state, &mut data, shape_button), None);
                assert!(state.dropdown_is_open_for(StripId::Shapes));
                state.on_key(&key(Key::Home), &mut data, RECT, 1.0);
                assert_eq!(
                    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
                    (true, None)
                );
                assert!(state.curve_browser_open() && state.has_open_menu());
                let card = state.browser_layout(RECT, 1.0).unwrap().cards[index];
                assert_eq!(press(&mut state, &mut data, card), None);
                assert_eq!(data, before);
                let mut expected = before;
                factory_shape(kind).apply(&mut expected, false).unwrap();
                assert_eq!(
                    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
                    (true, Some(MsegEdit::Changed))
                );
                assert_eq!(data, expected);
                assert!(!state.curve_browser_open() && !state.has_open_menu());
                assert_eq!(state.selection_count(), 0);
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn fitted_keyboard_and_double_click_loads_use_existing_geometry_and_noop_rules() {
        let (mut state, _, root) = isolated(false);
        let mut data = MsegData::default();
        data.nodes[0].value = 0.3;
        data.nodes[1].value = 0.7;
        let before = data;
        assert!(state.open_curve_browser(RECT, 1.0));
        state.on_key(&key(Key::End), &mut data, RECT, 1.0);
        let mut expected = before;
        factory_shape(FactoryShape::Duck)
            .apply(&mut expected, true)
            .unwrap();
        let fit = KeyboardEvent {
            modifiers: Modifiers::SHIFT,
            ..key(Key::Enter)
        };
        assert_eq!(
            state.on_key(&fit, &mut data, RECT, 1.0),
            (true, Some(MsegEdit::Changed))
        );
        assert_eq!(data, expected);
        assert!(state.open_curve_browser(RECT, 1.0));
        let card = state.browser_layout(RECT, 1.0).unwrap().cards[7];
        assert_eq!(
            state.on_double_click(card.0 + 2.0, card.1 + 2.0, &mut data, RECT, 1.0),
            Some(MsegEdit::Changed)
        );
        assert_eq!(data.active(), factory_shape(FactoryShape::Duck).nodes());
        assert!(state.open_curve_browser(RECT, 1.0));
        state.on_key(&key(Key::End), &mut data, RECT, 1.0);
        assert_eq!(
            state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
            (true, None)
        );
        assert!(!state.curve_browser_open());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn browsing_dismissal_and_modal_input_preserve_pen_repetition_and_selection() {
        let (mut state, _, root) = isolated(false);
        let mut data = MsegData {
            snap: false,
            ..MsegData::default()
        };
        let layout = state.layout(RECT, 1.0);
        state.set_tool(MsegTool::Line);
        let point = (phase_to_x(&layout, 0.1), value_to_y(&layout, 0.2));
        state.on_mouse_move(point.0, point.1, &mut data, RECT, 1.0, false);
        assert_eq!(
            state.on_mouse_down(point.0, point.1, &mut data, RECT, 1.0, false),
            Some(MsegEdit::Changed)
        );
        state.on_mouse_up(&mut data, RECT, 1.0);
        state.selection = 3;
        let before = data;
        let preview = state.pen_preview().to_vec();
        assert!(!preview.is_empty());
        let viewport = state.viewport();
        assert!(state.open_curve_browser(RECT, 1.0));
        for key_value in [
            Key::ArrowRight,
            Key::ArrowDown,
            Key::Home,
            Key::Tab,
            Key::End,
            Key::Tab,
            Key::F1,
            Key::Character("?".into()),
            Key::Delete,
        ] {
            assert_eq!(
                state.on_key(&key(key_value), &mut data, RECT, 1.0),
                (true, None)
            );
        }
        for key_value in ["a", "c", "v", "z"] {
            let event = KeyboardEvent {
                modifiers: Modifiers::CONTROL,
                ..key(Key::Character(key_value.into()))
            };
            assert_eq!(state.on_key(&event, &mut data, RECT, 1.0), (true, None));
        }
        assert!(state.on_pan_start(point.0, point.1, RECT, 1.0));
        assert!(!state.is_panning());
        assert!(state.on_scroll(point.0, point.1, -3.0, true, &data, RECT, 1.0));
        assert_eq!(
            state.on_mouse_move(0.0, 0.0, &mut data, RECT, 1.0, false),
            None
        );
        assert_eq!(state.on_mouse_up(&mut data, RECT, 1.0), None);
        state.on_key(
            &KeyboardEvent {
                state: KeyState::Up,
                ..key(Key::Enter)
            },
            &mut data,
            RECT,
            1.0,
        );
        assert!(state.curve_browser_open() && !state.help_open());
        state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
        assert_eq!(data, before);
        assert_eq!(state.selection_count(), 2);
        assert_eq!(state.tool(), MsegTool::Line);
        assert_eq!(state.pen_preview(), preview);
        assert_eq!(state.viewport(), viewport);
        assert_eq!(
            state.on_mouse_down(point.0, point.1, &mut data, RECT, 1.0, false),
            Some(MsegEdit::Changed)
        );
        let last = preview.last().unwrap();
        assert!(
            data.active()
                .iter()
                .any(|node| (node.time - last.time).abs() < 1e-5
                    && (node.value - last.value).abs() < 1e-5)
        );
        state.on_mouse_up(&mut data, RECT, 1.0);
        for right in [false, true] {
            let before = data;
            assert!(state.open_curve_browser(RECT, 1.0));
            if right {
                state.on_right_click(point.0, point.1, &mut data, RECT, 1.0);
            } else {
                state.on_mouse_down(-1.0, -1.0, &mut data, RECT, 1.0, false);
            }
            assert!(!state.curve_browser_open());
            assert_eq!(data, before);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shared_favorites_refresh_store_clear_and_corrupt_entries_are_transactional() {
        let (mut state, library, root) = isolated(false);
        std::fs::write(root.join("favorite-2.json"), "broken curve").unwrap();
        std::fs::create_dir(root.join("favorite-3.json")).unwrap();
        let mut data = MsegData::default();
        data.nodes[0].shape = SegmentShape::SCurve;
        data.nodes[1].weight = PointWeight::Soft;
        let before = data;
        assert!(state.open_curve_browser(RECT, 1.0));
        state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
        assert!(state.browser_shape().is_none());
        assert_eq!(
            state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
            (true, None)
        );
        let other = CurveLibrary::at(root.clone());
        other
            .store_favorite(0, &factory_shape(FactoryShape::Triangle))
            .unwrap();
        assert!(state.browser_shape().is_none());
        state.on_key(&key(Key::Character("r".into())), &mut data, RECT, 1.0);
        assert_eq!(
            state.browser_shape(),
            Some(&factory_shape(FactoryShape::Triangle))
        );
        state.on_key(&key(Key::ArrowRight), &mut data, RECT, 1.0);
        assert!(matches!(
            state.browser.favorites[1],
            Favorite::Unavailable(_)
        ));
        assert_eq!(
            state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
            (true, None)
        );
        state.on_key(&key(Key::Character("s".into())), &mut data, RECT, 1.0);
        assert_eq!(
            other.load_favorite(1).unwrap(),
            CurveShape::capture(&before)
        );
        state.on_key(&key(Key::Delete), &mut data, RECT, 1.0);
        assert!(matches!(state.browser.favorites[1], Favorite::Empty));
        assert!(other.load_favorite(1).is_err());
        state.on_key(&key(Key::ArrowRight), &mut data, RECT, 1.0);
        state.on_key(&key(Key::Delete), &mut data, RECT, 1.0);
        assert!(state.browser.message.starts_with("Clear failed:"));
        assert!(root.join("favorite-3.json").is_dir());
        assert_eq!(data, before);
        state.on_key(&key(Key::Home), &mut data, RECT, 1.0);
        std::fs::write(
            root.join("favorite-1.json"),
            "changed while browser is open",
        )
        .unwrap();
        let mut pm = Pixmap::new(1040, 720).unwrap();
        state.draw_curve_browser(
            &mut pm,
            &mut TextRenderer::new(crate::DEFAULT_FONT),
            RECT,
            1.0,
        );
        assert_eq!(
            state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
            (true, Some(MsegEdit::Changed))
        );
        assert_eq!(data.active(), factory_shape(FactoryShape::Triangle).nodes());
        assert!(library.load_favorite(0).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unavailable_storage_keeps_factory_usable_and_never_edits_the_destination() {
        let (mut state, _, root) = isolated(false);
        let blocked = root.join("blocked");
        std::fs::write(&blocked, "not a directory").unwrap();
        state.set_curve_library_directory(blocked.clone());
        let mut data = MsegData::default();
        let before = data;
        assert!(state.open_curve_browser(RECT, 1.0));
        for event in [Key::Character("s".into()), Key::Delete] {
            state.on_key(&key(event), &mut data, RECT, 1.0);
        }
        assert_eq!(
            std::fs::read_to_string(&blocked).unwrap(),
            "not a directory"
        );
        state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
        assert!(
            state
                .browser
                .favorites
                .iter()
                .all(|slot| matches!(slot, Favorite::Unavailable(_)))
        );
        for (button, message) in [(2, "Store failed:"), (3, "Clear failed:")] {
            let r = state.browser_layout(RECT, 1.0).unwrap().buttons[button];
            assert_eq!(press(&mut state, &mut data, r), None);
            assert!(state.browser.message.starts_with(message));
            assert_eq!(data, before);
        }
        state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
        state.on_key(&key(Key::End), &mut data, RECT, 1.0);
        assert_eq!(
            state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
            (true, Some(MsegEdit::Changed))
        );
        assert_eq!(data.active(), factory_shape(FactoryShape::Duck).nodes());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn browser_preserves_overlay_and_gesture_priority_and_cancellation_closes_it() {
        let (mut state, _, root) = isolated(false);
        let mut data = MsegData::default();
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(!state.open_curve_browser(RECT, scale));
        }
        state.open_help(1.0);
        assert!(!state.open_curve_browser(RECT, 1.0));
        state.cancel_interaction();
        state.selection = 1;
        assert!(state.open_point_inspector(&data));
        assert!(!state.open_curve_browser(RECT, 1.0));
        state.cancel_interaction();
        let layout = state.layout(RECT, 1.0);
        state.on_mouse_down(
            phase_to_x(&layout, 0.5),
            value_to_y(&layout, 0.3),
            &mut data,
            RECT,
            1.0,
            false,
        );
        assert!(state.is_dragging());
        assert!(!state.open_curve_browser(RECT, 1.0));
        state.on_mouse_up(&mut data, RECT, 1.0);
        assert!(state.open_curve_browser(RECT, 1.0));
        state.cancel_interaction();
        assert!(!state.curve_browser_open() && !state.has_open_menu());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn browser_renders_all_cards_inside_native_bounds_at_supported_scales() {
        for curve_only in [false, true] {
            for (scale, bounds) in [
                (0.5, (570.0, 352.0)),
                (1.0, (1140.0, 704.0)),
                (2.0, (2280.0, 1408.0)),
                (1.0, (300.0, 260.0)),
            ] {
                let (mut state, _, root) = isolated(curve_only);
                state.set_overlay_bounds(bounds);
                let rect = (
                    10.0 * scale,
                    bounds.1 * 0.65,
                    bounds.0 - 20.0 * scale,
                    bounds.1 * 0.3,
                );
                let mut data = MsegData::default();
                assert!(state.open_curve_browser(rect, scale));
                let layout = state.browser_layout(rect, scale).unwrap();
                for r in [layout.panel, layout.close, layout.detail, layout.hint]
                    .into_iter()
                    .chain(layout.cards)
                    .chain(layout.tabs)
                    .chain(layout.buttons)
                {
                    assert!(
                        r.0 >= 0.0 && r.1 >= 0.0 && r.0 + r.2 <= bounds.0 && r.1 + r.3 <= bounds.1,
                        "{scale}x {r:?} in {bounds:?}"
                    );
                }
                for page in 0..2 {
                    let mut pm = Pixmap::new(bounds.0 as u32, bounds.1 as u32).unwrap();
                    let mut tr = TextRenderer::new(crate::DEFAULT_FONT);
                    state.draw_curve_browser(&mut pm, &mut tr, rect, scale);
                    for card in layout.cards {
                        let ink = (card.1.ceil() as u32 + 2..(card.1 + card.3).floor() as u32 - 2)
                            .flat_map(|y| {
                                (card.0.ceil() as u32 + 2..(card.0 + card.2).floor() as u32 - 2)
                                    .map(move |x| (x, y))
                            })
                            .filter(|&(x, y)| {
                                let pixel = pm.pixel(x, y).unwrap();
                                let bg = color_control_bg().to_color_u8();
                                pixel.red() != bg.red()
                                    || pixel.green() != bg.green()
                                    || pixel.blue() != bg.blue()
                            })
                            .count();
                        assert!(ink > 10, "No thumbnail/label at {scale}x page {page}");
                    }
                    let hint = if page == 0 {
                        "Arrows: select; Tab: favorites; Enter: load; Shift+Enter: fit; Esc: close"
                    } else {
                        "Enter: load; Shift+Enter: fit; S: store; Del: clear; R: refresh"
                    };
                    assert!(
                        wrap(&mut tr, hint, (12.0 * scale).max(10.0), layout.hint.2).len() <= 2
                    );
                    assert_eq!(data, MsegData::default());
                    state.on_key(&key(Key::Tab), &mut data, rect, scale);
                }
                let close = layout.close;
                state.on_mouse_down(
                    close.0 + close.2 * 0.5,
                    close.1 + close.3 * 0.5,
                    &mut data,
                    rect,
                    scale,
                    false,
                );
                assert!(!state.curve_browser_open());
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }

    #[test]
    fn thumbnails_follow_native_easing_and_vertical_step_edges() {
        for weight_only in [false, true] {
            let mut data = MsegData::default();
            if weight_only {
                data.nodes[0].weight = PointWeight::Soft;
                data.nodes[1].weight = PointWeight::Soft;
            } else {
                data.nodes[0].shape = SegmentShape::SCurve;
            }
            let mut pm = Pixmap::new(120, 120).unwrap();
            draw_thumbnail(
                &mut pm,
                &CurveShape::capture(&data),
                (10.0, 10.0, 100.0, 100.0),
                1.0,
            );
            assert!((93..=96).any(|y| pm.pixel(35, y).unwrap().alpha() > 0));
            assert_eq!(pm.pixel(35, 85).unwrap().alpha(), 0);
        }
        let mut pm = Pixmap::new(120, 120).unwrap();
        draw_thumbnail(
            &mut pm,
            &factory_shape(FactoryShape::Square),
            (10.0, 10.0, 100.0, 100.0),
            1.0,
        );
        assert!(pm.pixel(35, 10).unwrap().alpha() > 0);
        assert!(pm.pixel(60, 60).unwrap().alpha() > 0);
        assert_eq!(pm.pixel(35, 60).unwrap().alpha(), 0);
    }

    #[test]
    fn accepted_layouts_render_curve_pixels_above_every_card_label() {
        for (scale, width, heights) in [
            (0.5, 440.0, [180.0, 200.0, 220.0]),
            (640.0 / 880.0, 640.0, [185.0, 210.0, 230.0]),
            (1.0, 880.0, [240.0, 280.0, 320.0]),
            (2.0, 1760.0, [480.0, 560.0, 640.0]),
        ] {
            let (mut state, _, root) = isolated(true);
            let mut data = MsegData::default();
            let mut accepted = 0;
            let mut tr = TextRenderer::new(crate::DEFAULT_FONT);
            for height in heights {
                state.set_overlay_bounds((width, height));
                let rect = (0.0, 0.0, width, height * 0.53);
                if !state.open_curve_browser(rect, scale) {
                    assert_eq!(
                        state.status_message(),
                        "Enlarge the window to browse curves"
                    );
                    continue;
                }
                accepted += 1;
                let layout = state.browser_layout(rect, scale).unwrap();
                let mut pm = Pixmap::new(width as u32, height as u32).unwrap();
                state.draw_curve_browser(&mut pm, &mut tr, rect, scale);
                for (index, card) in layout.cards.into_iter().enumerate() {
                    let mut ink = 0;
                    let mut top = u32::MAX;
                    let mut bottom = 0;
                    let label_space = (12.0 * scale).max(10.0) + 6.0;
                    for y in
                        card.1.ceil() as u32 + 3..(card.1 + card.3 - label_space).floor() as u32
                    {
                        for x in card.0.ceil() as u32 + 3..(card.0 + card.2).floor() as u32 - 3 {
                            let pixel = pm.pixel(x, y).unwrap();
                            if pixel.blue().saturating_sub(pixel.red()) > 50 {
                                ink += 1;
                                top = top.min(y);
                                bottom = bottom.max(y);
                            }
                        }
                    }
                    assert!(
                        ink > 20,
                        "{width}x{height} at {scale}x: card {index} has no curve preview"
                    );
                    if index == 2 {
                        assert!(
                            bottom - top >= 5,
                            "{width}x{height} at {scale}x: ramp preview is too short"
                        );
                    }
                }
                state.on_key(&key(Key::Escape), &mut data, rect, scale);
            }
            assert!(accepted > 0 && accepted < heights.len());
            assert_eq!(data, MsegData::default());
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn double_click_pairs_require_the_same_page_card_and_uninterrupted_time_window() {
        let mut state = MsegEditState::new();
        let now = Instant::now();
        assert!(!state.browser_double_click(0, now));
        assert!(state.browser_double_click(0, now + Duration::from_millis(399)));
        assert!(!state.browser_double_click(0, now + Duration::from_millis(399)));
        assert!(!state.browser_double_click(0, now + Duration::from_millis(799)));
        assert!(!state.browser_double_click(1, now + Duration::from_millis(800)));
        state.browser.page = Page::Favorites;
        assert!(!state.browser_double_click(1, now + Duration::from_millis(801)));
        let mut data = MsegData::default();
        state.browser_key(&key(Key::Home), &mut data);
        assert!(!state.browser_double_click(1, now + Duration::from_millis(802)));
        assert!(!state.browser_double_click(1, now));
        state.close_curve_browser();
        assert!(!state.browser_double_click(1, now + Duration::from_millis(1)));
    }

    #[test]
    fn undersized_shapes_entry_reports_a_visible_error_without_opening_or_loading() {
        for (bounds, rect, scale) in [
            ((640.0, 180.0), (0.0, 0.0, 640.0, 95.4), 640.0 / 880.0),
            ((640.0, 185.0), (0.0, 0.0, 640.0, 98.05), 640.0 / 880.0),
            ((0.0, 0.0), (0.0, 0.0, 200.0, 120.0), 1.0),
        ] {
            let (mut state, _, root) = isolated(true);
            state.set_overlay_bounds(bounds);
            let mut data = MsegData::default();
            let before = data;
            let layout = state.layout(rect, scale);
            let trigger = power_buttons(layout.strip, scale).shapes;
            assert_eq!(
                state.on_mouse_down(
                    trigger.0 + 2.0,
                    trigger.1 + 2.0,
                    &mut data,
                    rect,
                    scale,
                    false
                ),
                None
            );
            assert!(state.dropdown_is_open_for(StripId::Shapes));
            state.on_key(&key(Key::Home), &mut data, rect, scale);
            assert_eq!(
                state.on_key(&key(Key::Enter), &mut data, rect, scale),
                (true, None)
            );
            assert!(!state.curve_browser_open() && !state.has_open_menu());
            assert_eq!(
                state.status_message(),
                "Enlarge the window to browse curves"
            );
            assert_eq!(
                state.on_key(&key(Key::Enter), &mut data, rect, scale).1,
                None
            );
            assert_eq!(data, before);
            let size = if bounds.0 > 0.0 {
                bounds
            } else {
                (rect.2, rect.3)
            };
            let mut pm = Pixmap::new(size.0 as u32, size.1 as u32).unwrap();
            let mut tr = TextRenderer::new(crate::DEFAULT_FONT);
            crate::mseg::draw_mseg(
                &mut pm,
                &mut tr,
                rect,
                &data,
                &state,
                scale,
                color_accent(),
                None,
            );
            let actual = pm.clone();
            state.set_status_message(" ");
            crate::mseg::draw_mseg(
                &mut pm,
                &mut tr,
                rect,
                &data,
                &state,
                scale,
                color_accent(),
                None,
            );
            assert!(
                actual
                    .pixels()
                    .iter()
                    .zip(pm.pixels())
                    .filter(|(a, b)| a != b)
                    .count()
                    > 60
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
