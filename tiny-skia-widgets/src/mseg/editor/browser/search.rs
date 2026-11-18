use super::user::{NameField, draw_name_field};
use super::*;

const QUERY_LIMIT: usize = 256;

pub(super) struct BrowserSearch {
    pub visible: bool,
    pub focused: bool,
    field: NameField,
    terms: Vec<String>,
    document: Option<Box<MsegData>>,
}

impl Default for BrowserSearch {
    fn default() -> Self {
        Self {
            visible: false,
            focused: false,
            field: NameField::with_limit(String::new(), QUERY_LIMIT),
            terms: Vec::new(),
            document: None,
        }
    }
}

impl BrowserSearch {
    pub(super) fn text(&self) -> &str {
        &self.field.text
    }
    pub(super) fn has_query(&self) -> bool {
        !self.terms.is_empty()
    }
    pub(super) fn document_matches(&self, data: &MsegData) -> bool {
        self.document
            .as_deref()
            .is_none_or(|document| *document == *data)
    }
    fn update_terms(&mut self) {
        self.terms = self
            .field
            .text
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
    }
    pub(super) fn set_text(&mut self, text: &str) {
        self.field = NameField::with_limit(text.to_owned(), QUERY_LIMIT);
        self.update_terms();
    }
    pub(super) fn matches(&self, name: &str, category: &str) -> bool {
        if self.terms.is_empty() {
            return true;
        }
        let name = name.to_lowercase();
        let category = if category.is_empty() {
            "Uncategorized"
        } else {
            category
        }
        .to_lowercase();
        self.terms
            .iter()
            .all(|term| name.contains(term) || category.contains(term))
    }
}

impl MsegEditState {
    pub(super) fn search_supported(&self) -> bool {
        matches!(self.browser.page, Page::Factory | Page::User)
    }

    pub(super) fn open_browser_search(&mut self, data: &MsegData, rect: Rect, scale: f32) {
        if !self.search_supported() {
            return;
        }
        if !self.browser.search.visible {
            self.browser.search.document = Some(Box::new(*data));
        }
        self.browser.search.visible = true;
        self.browser.search_close_click = None;
        self.browser.search.focused = true;
        self.browser.search.field.select_all();
        self.browser.last_click = None;
        self.browser.message.clear();
        self.validate_browser_layout(rect, scale);
    }

    fn update_search_view(&mut self) {
        let selected = self
            .browser_visible_indices()
            .contains(&self.browser.selected)
            .then_some(self.browser.selected);
        self.browser.search.update_terms();
        self.rebuild_browser_view(selected, 0);
        self.browser.message.clear();
    }

    pub(super) fn clear_browser_search(&mut self, close: bool) {
        self.browser.search_close_click = None;
        let selected = self
            .browser_visible_indices()
            .contains(&self.browser.selected)
            .then_some(self.browser.selected);
        if close {
            self.browser.search = BrowserSearch::default();
        } else {
            self.browser.search.set_text("");
            self.browser.search.focused = true;
        }
        self.rebuild_browser_view(selected, 0);
        self.browser.message.clear();
    }

    pub(super) fn reveal_browser_search(&mut self, name: &str, category: &str) {
        if !self.browser.search.matches(name, category) {
            self.browser.search.set_text("");
            self.browser.search.focused = false;
        }
    }

    pub(super) fn is_browser_find_shortcut(&self, event: &KeyboardEvent) -> bool {
        if !self.search_supported() {
            return false;
        }
        let command = event
            .modifiers
            .intersects(Modifiers::CONTROL | Modifiers::META);
        (command
            && !event.modifiers.contains(Modifiers::ALT)
            && matches!(&event.key, Key::Character(text) if text.eq_ignore_ascii_case("f")))
            || (!self.browser.search.focused
                && !event
                    .modifiers
                    .intersects(Modifiers::CONTROL | Modifiers::META | Modifiers::ALT)
                && matches!(&event.key, Key::Character(text) if text == "/"))
    }

    pub(super) fn browser_search_key(
        &mut self,
        event: &KeyboardEvent,
        data: &mut MsegData,
    ) -> Option<MsegEdit> {
        if event
            .modifiers
            .intersects(Modifiers::CONTROL | Modifiers::META | Modifiers::ALT)
            && !matches!(&event.key, Key::Character(_))
        {
            return None;
        }
        match event.key {
            Key::Escape => self.clear_browser_search(true),
            Key::Enter => {
                return self.browser_action(
                    usize::from(event.modifiers.contains(Modifiers::SHIFT)),
                    data,
                );
            }
            Key::Tab => self.browser.search.focused = false,
            Key::ArrowUp => {
                self.select_browser_position(self.browser_selected_position().saturating_sub(4))
            }
            Key::ArrowDown => {
                let last = self.browser_visible_indices().len().saturating_sub(1);
                self.select_browser_position((self.browser_selected_position() + 4).min(last));
            }
            Key::PageUp | Key::PageDown if self.browser.page == Page::User => {
                self.move_user_page(if event.key == Key::PageUp { -1 } else { 1 })
            }
            _ => {
                let before = self.browser.search.text().to_owned();
                self.browser.search.field.key(event);
                if self.browser.search.text() != before {
                    self.update_search_view();
                }
            }
        }
        None
    }

    pub(super) fn browser_search_mouse_down(
        &mut self,
        x: f32,
        y: f32,
        data: &MsegData,
        layout: &BrowserLayout,
        rect: Rect,
        scale: f32,
    ) -> bool {
        if layout.find.is_some_and(|r| in_rect(r, x, y)) {
            self.open_browser_search(data, rect, scale);
            return true;
        }
        if let Some([field, clear, close]) = layout.search {
            if in_rect(field, x, y) {
                self.browser.search.focused = true;
                self.browser.last_click = None;
                return true;
            }
            if in_rect(clear, x, y) {
                self.clear_browser_search(false);
                return true;
            }
            if in_rect(close, x, y) {
                self.clear_browser_search(true);
                self.browser.search_close_click = Some((close, Instant::now()));
                return true;
            }
        }
        self.browser.search.focused = false;
        false
    }

    pub(super) fn draw_browser_find(
        &self,
        pm: &mut Pixmap,
        tr: &mut TextRenderer,
        layout: &BrowserLayout,
    ) {
        let Some(r) = layout.find else {
            return;
        };
        crate::controls::draw_button(
            pm,
            tr,
            r.0,
            r.1,
            r.2,
            r.3,
            "",
            self.browser.search.visible,
            false,
        );
        let font = (r.3 * 0.5).max(10.0);
        let width = tr.text_width("Find", font);
        draw_fitted(
            tr,
            pm,
            (r.0 + (r.2 - width) * 0.5, r.1 + (r.3 + font) * 0.5 - 2.0),
            "Find",
            font,
            r.2 - 8.0,
            if self.browser.search.visible {
                color_bg()
            } else {
                color_text()
            },
        );
    }

    pub(super) fn draw_browser_search(
        &self,
        pm: &mut Pixmap,
        tr: &mut TextRenderer,
        layout: &BrowserLayout,
        scale: f32,
    ) {
        let Some([field, clear, close]) = layout.search else {
            return;
        };
        draw_name_field(
            pm,
            tr,
            field,
            &self.browser.search.field,
            self.browser.search.focused,
            (12.0 * scale).max(10.0),
            if field.2 < 160.0 {
                "Query"
            } else {
                "Names or categories"
            },
        );
        for (r, label) in [(clear, "Clear"), (close, "X")] {
            crate::controls::draw_button(pm, tr, r.0, r.1, r.2, r.3, "", false, false);
            let font = (r.3 * 0.5).max(10.0);
            let width = tr.text_width(label, font);
            draw_fitted(
                tr,
                pm,
                (r.0 + (r.2 - width) * 0.5, r.1 + (r.3 + font) * 0.5 - 2.0),
                label,
                font,
                r.2 - 8.0,
                color_text(),
            );
        }
    }
}
