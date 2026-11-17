use super::*;
use crate::mseg::library::{MAX_USER_PRESET_NAME_CHARS, UserPreset};
use std::cell::Cell;

#[derive(Default)]
pub(super) struct UserBrowser {
    pub entries: Vec<UserPreset>,
    pub error: Option<String>,
    pub dialog: Option<Dialog>,
}

pub(super) struct Dialog {
    action: Action,
    document: MsegData,
    field: NameField,
    category: NameField,
    error: String,
    focus: u8,
    name_scroll: usize,
    name_scroll_max: Cell<usize>,
    name_rows: Cell<usize>,
}

impl Dialog {
    fn scroll_name(&mut self, delta: isize) {
        let max = self.name_scroll_max.get();
        self.name_scroll = self
            .name_scroll
            .min(max)
            .saturating_add_signed(delta)
            .min(max);
    }
}

enum Action {
    Save(CurveShape),
    Rename(UserPreset),
    Delete(UserPreset),
}

impl Action {
    fn label(&self) -> &'static str {
        match self {
            Self::Save(_) => "Save",
            Self::Rename(_) => "Rename",
            Self::Delete(_) => "Delete",
        }
    }
    fn expected(&self) -> Option<&UserPreset> {
        match self {
            Self::Save(_) => None,
            Self::Rename(preset) | Self::Delete(preset) => Some(preset),
        }
    }
}

pub(super) struct NameField {
    pub text: String,
    caret: usize,
    anchor: usize,
    limit: usize,
}

impl NameField {
    pub(super) fn new(text: String) -> Self {
        Self::with_limit(text, MAX_USER_PRESET_NAME_CHARS)
    }
    pub(super) fn with_limit(text: String, limit: usize) -> Self {
        let text: String = text.chars().take(limit).collect();
        Self {
            caret: text.len(),
            anchor: 0,
            text,
            limit,
        }
    }
    pub(super) fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }
    fn selection(&self) -> std::ops::Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }
    fn previous(&self) -> usize {
        self.text[..self.caret]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }
    fn next(&self) -> usize {
        self.text[self.caret..]
            .chars()
            .next()
            .map_or(self.caret, |ch| self.caret + ch.len_utf8())
    }
    fn erase_selection(&mut self) -> bool {
        let selected = self.selection();
        if selected.is_empty() {
            return false;
        }
        self.text.replace_range(selected.clone(), "");
        self.caret = selected.start;
        self.anchor = self.caret;
        true
    }
    pub(super) fn key(&mut self, event: &KeyboardEvent) {
        let command = event
            .modifiers
            .intersects(Modifiers::CONTROL | Modifiers::META);
        if command {
            if matches!(&event.key, Key::Character(text) if text.eq_ignore_ascii_case("a")) {
                self.select_all();
            }
            return;
        }
        if event.modifiers.contains(Modifiers::ALT) && !matches!(&event.key, Key::Character(_)) {
            return;
        }
        let shift = event.modifiers.contains(Modifiers::SHIFT);
        let selection = self.selection();
        let next = match event.key {
            Key::Home => Some(0),
            Key::End => Some(self.text.len()),
            Key::ArrowLeft => Some(if !shift && !selection.is_empty() {
                selection.start
            } else {
                self.previous()
            }),
            Key::ArrowRight => Some(if !shift && !selection.is_empty() {
                selection.end
            } else {
                self.next()
            }),
            _ => None,
        };
        if let Some(next) = next {
            self.caret = next;
            if !shift {
                self.anchor = next;
            }
            return;
        }
        match &event.key {
            Key::Backspace | Key::Delete => {
                if !self.erase_selection() {
                    if event.key == Key::Backspace {
                        let previous = self.previous();
                        self.text.replace_range(previous..self.caret, "");
                        self.caret = previous;
                    } else {
                        self.text.replace_range(self.caret..self.next(), "");
                    }
                    self.anchor = self.caret;
                }
            }
            Key::Character(text) => {
                let text: String = text.chars().filter(|ch| !ch.is_control()).collect();
                if text.is_empty() {
                    return;
                }
                self.erase_selection();
                let room = self.limit.saturating_sub(self.text.chars().count());
                let text: String = text.chars().take(room).collect();
                self.text.insert_str(self.caret, &text);
                self.caret += text.len();
                self.anchor = self.caret;
            }
            _ => {}
        }
    }
}

struct DialogLayout {
    panel: Rect,
    field: Rect,
    category: Rect,
    category_picker: Rect,
    accept: Rect,
    cancel: Rect,
}

fn dialog_layout(layout: &BrowserLayout, scale: f32) -> DialogLayout {
    let s = scale.max(0.01);
    let pad = (12.0 * s).max(6.0);
    let top = (34.0 * s).max(28.0);
    let (x, y, w, h) = layout.panel;
    let panel = (x + pad, y + top, w - 2.0 * pad, h - top - pad);
    let button_h = (28.0 * s).max(22.0);
    let button_w = (100.0 * s).max(68.0);
    let button_y = panel.1 + panel.3 - button_h - pad;
    let field = (
        panel.0 + pad,
        panel.1 + (40.0 * s).max(34.0),
        panel.2 - 2.0 * pad,
        button_h,
    );
    let gap = (6.0 * s).max(4.0);
    let label_w = (70.0 * s).max(50.0);
    let picker_w = (24.0 * s).max(20.0);
    let category_y = field.1 + field.3 + gap;
    DialogLayout {
        panel,
        field,
        category: (
            field.0 + label_w + gap,
            category_y,
            field.2 - label_w - picker_w - 2.0 * gap,
            button_h,
        ),
        category_picker: (field.0 + field.2 - picker_w, category_y, picker_w, button_h),
        accept: (panel.0 + pad, button_y, button_w, button_h),
        cancel: (
            panel.0 + panel.2 - button_w - pad,
            button_y,
            button_w,
            button_h,
        ),
    }
}

pub(super) fn dialog_fits(layout: &BrowserLayout, scale: f32) -> bool {
    let lay = dialog_layout(layout, scale);
    let font = (12.0 * scale.max(0.01)).max(10.0);
    // Three name lines, instructions and two retryable-error lines.
    lay.field.1 + font + 6.0 * font * 1.5 < lay.accept.1 - 4.0
}

pub(super) fn draw_name_field(
    pm: &mut Pixmap,
    tr: &mut TextRenderer,
    rect: Rect,
    field: &NameField,
    focused: bool,
    font: f32,
    placeholder: &str,
) {
    draw_rect(pm, rect.0, rect.1, rect.2, rect.3, color_control_bg());
    draw_rect_outline(
        pm,
        rect.0,
        rect.1,
        rect.2,
        rect.3,
        if focused {
            color_accent()
        } else {
            color_border()
        },
        1.0,
    );
    let mut start = 0;
    for (index, _) in field.text[..field.caret].char_indices() {
        if tr.text_width(&field.text[start..field.caret], font) <= rect.2 - 16.0 {
            break;
        }
        start = index;
    }
    let selection = field.selection();
    if !selection.is_empty() && selection.end > start {
        let left = tr
            .text_width(&field.text[start..selection.start.max(start)], font)
            .min(rect.2 - 10.0);
        let right = tr
            .text_width(&field.text[start..selection.end], font)
            .min(rect.2 - 10.0);
        draw_rect(
            pm,
            rect.0 + 5.0 + left,
            rect.1 + 3.0,
            (right - left).max(0.0),
            rect.3 - 6.0,
            color_border(),
        );
    }
    let visible = if field.text.is_empty() {
        placeholder
    } else {
        &field.text[start..]
    };
    draw_fitted(
        tr,
        pm,
        (rect.0 + 5.0, rect.1 + (rect.3 + font) * 0.5 - 2.0),
        visible,
        font,
        rect.2 - 10.0,
        if field.text.is_empty() {
            color_muted()
        } else {
            color_text()
        },
    );
    if focused {
        let caret = tr
            .text_width(&field.text[start..field.caret], font)
            .min(rect.2 - 12.0);
        draw_rect(
            pm,
            rect.0 + 5.0 + caret,
            rect.1 + 4.0,
            1.0,
            rect.3 - 8.0,
            color_accent(),
        );
    }
}

impl MsegEditState {
    pub(super) fn user_dialog_fits(&self, layout: &BrowserLayout, scale: f32) -> bool {
        if !dialog_fits(layout, scale) {
            return false;
        }
        if self
            .browser
            .user
            .dialog
            .as_ref()
            .is_some_and(|dialog| !matches!(dialog.action, Action::Delete(_)))
        {
            let lay = dialog_layout(layout, scale);
            let line = (12.0 * scale.max(0.01)).max(10.0) * 1.5;
            return lay.category.1 + lay.category.3 + 4.0 * line < lay.accept.1 - 4.0;
        }
        true
    }

    pub(super) fn user_category_text(&self) -> Option<&str> {
        self.browser
            .user
            .dialog
            .as_ref()
            .filter(|dialog| !matches!(dialog.action, Action::Delete(_)))
            .map(|dialog| dialog.category.text.as_str())
    }

    pub(super) fn user_category_focused(&self) -> bool {
        self.browser
            .user
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.focus == 1 && !matches!(dialog.action, Action::Delete(_)))
    }

    pub(super) fn user_category_anchor(&self, layout: &BrowserLayout, scale: f32) -> Rect {
        let lay = dialog_layout(layout, scale);
        (
            lay.category.0,
            lay.category.1,
            lay.category.2 + lay.category_picker.2,
            lay.category.3,
        )
    }

    pub(super) fn set_user_category_text(&mut self, category: String) {
        if let Some(dialog) = self.browser.user.dialog.as_mut() {
            dialog.category = NameField::new(category);
            dialog.focus = 1;
            dialog.error.clear();
        }
    }

    pub(super) fn validate_user_document(&mut self, data: &MsegData) -> bool {
        if self
            .browser
            .user
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.document != *data)
        {
            self.close_curve_browser();
            return false;
        }
        true
    }

    pub(super) fn refresh_browser_user(&mut self, selected: Option<u64>) {
        let position = self.browser_selected_position();
        let selected = selected.or_else(|| {
            self.browser
                .user
                .entries
                .get(self.browser.selected)
                .map(|entry| entry.id)
        });
        match self
            .curve_library()
            .and_then(|library| library.user_presets())
        {
            Ok(entries) => {
                let preferred =
                    selected.and_then(|id| entries.iter().position(|entry| entry.id == id));
                self.browser.user.entries = entries;
                self.browser.user.error = None;
                self.rebuild_browser_view(preferred, position);
            }
            Err(error) => {
                self.browser.user.entries.clear();
                self.browser.selected = 0;
                self.browser.user.error = Some(error.to_string());
                self.rebuild_browser_view(None, 0);
            }
        }
        self.browser.last_click = None;
        self.browser.message.clear();
    }

    fn verified_user_selection(&mut self) -> Option<UserPreset> {
        let expected = self
            .browser
            .user
            .entries
            .get(self.browser.selected)?
            .clone();
        match self
            .curve_library()
            .and_then(|library| library.load_user_preset(&expected))
        {
            Ok(_) => Some(expected),
            Err(error) => {
                self.refresh_browser_user(None);
                self.browser.message = error.to_string();
                None
            }
        }
    }

    pub(super) fn user_browser_action(
        &mut self,
        action: usize,
        data: &mut MsegData,
    ) -> Option<MsegEdit> {
        if !self.browser_actions().get(action).copied().unwrap_or(false) {
            return None;
        }
        self.browser.last_click = None;
        if action == 5 {
            self.refresh_browser_user(None);
            return None;
        }
        let action = if action == 2 {
            Action::Save(CurveShape::capture(data))
        } else {
            let expected = self.verified_user_selection()?;
            match action {
                0 | 1 => return self.apply_shape(data, expected.shape, action == 1),
                3 => Action::Rename(expected),
                4 => Action::Delete(expected),
                _ => return None,
            }
        };
        let field = NameField::new(
            action
                .expected()
                .map_or_else(|| "New curve".to_owned(), |entry| entry.name.clone()),
        );
        let category = NameField::new(action.expected().map_or_else(
            || self.active_user_category(),
            |entry| entry.category.clone(),
        ));
        let focus = if matches!(action, Action::Delete(_)) {
            2
        } else {
            0
        };
        self.browser.user.dialog = Some(Dialog {
            action,
            document: *data,
            field,
            category,
            error: String::new(),
            focus,
            name_scroll: 0,
            name_scroll_max: Cell::new(0),
            name_rows: Cell::new(1),
        });
        None
    }

    fn commit_user_dialog(&mut self) {
        let Some(mut dialog) = self.browser.user.dialog.take() else {
            return;
        };
        let result = self
            .curve_library()
            .and_then(|library| match &dialog.action {
                Action::Save(shape) => library
                    .save_user_preset_with_category(
                        &dialog.field.text,
                        &dialog.category.text,
                        shape,
                    )
                    .map(Some),
                Action::Rename(expected) => library
                    .update_user_preset(expected, &dialog.field.text, &dialog.category.text)
                    .map(Some),
                Action::Delete(expected) => library.remove_user_preset(expected).map(|()| None),
            });
        match result {
            Ok(selected) => {
                if let Some(entry) = &selected {
                    self.reveal_user_category(&entry.category);
                    self.reveal_browser_search(&entry.name, &entry.category);
                }
                self.refresh_browser_user(selected.map(|entry| entry.id));
                self.browser.message = format!(
                    "User preset {}",
                    match dialog.action {
                        Action::Save(_) => "saved",
                        Action::Rename(_) => "renamed",
                        Action::Delete(_) => "deleted",
                    }
                );
            }
            Err(error) => {
                self.refresh_browser_user(None);
                let stale = dialog
                    .action
                    .expected()
                    .is_some_and(|expected| !self.browser.user.entries.contains(expected));
                if stale || self.browser.user.error.is_some() {
                    self.browser.message = error.to_string();
                } else {
                    dialog.error = error.to_string();
                    self.browser.user.dialog = Some(dialog);
                }
            }
        }
    }

    pub(super) fn user_dialog_key(&mut self, event: &KeyboardEvent) {
        let Some(dialog) = self.browser.user.dialog.as_mut() else {
            return;
        };
        if matches!(dialog.action, Action::Delete(_)) {
            match event.key {
                Key::ArrowUp => dialog.scroll_name(-1),
                Key::ArrowDown => dialog.scroll_name(1),
                Key::PageUp => dialog.scroll_name(-(dialog.name_rows.get() as isize)),
                Key::PageDown => dialog.scroll_name(dialog.name_rows.get() as isize),
                Key::Home => dialog.name_scroll = 0,
                Key::End => dialog.name_scroll = usize::MAX,
                _ => return self.user_dialog_non_scroll_key(event),
            }
            return;
        }
        self.user_dialog_non_scroll_key(event);
    }

    fn user_dialog_non_scroll_key(&mut self, event: &KeyboardEvent) {
        let Some(dialog) = self.browser.user.dialog.as_mut() else {
            return;
        };
        if event.key == Key::Escape {
            self.browser.user.dialog = None;
        } else if event.key == Key::Tab {
            let first = if matches!(dialog.action, Action::Delete(_)) {
                2
            } else {
                0
            };
            let count = 4 - first;
            let delta = if event.modifiers.contains(Modifiers::SHIFT) {
                count - 1
            } else {
                1
            };
            dialog.focus = first + (dialog.focus - first + delta) % count;
        } else if event.key == Key::Enter
            || (event.key == Key::Character(" ".into()) && dialog.focus >= 2)
        {
            if dialog.focus == 3 {
                self.browser.user.dialog = None;
            } else {
                self.commit_user_dialog();
            }
        } else if dialog.focus <= 1 {
            if dialog.focus == 0 {
                dialog.field.key(event);
            } else {
                dialog.category.key(event);
            }
            dialog.error.clear();
        }
    }

    pub(super) fn scroll_user_name(&mut self, delta: f32) {
        if delta.is_finite()
            && let Some(dialog) = self.browser.user.dialog.as_mut()
            && matches!(dialog.action, Action::Delete(_))
        {
            dialog.scroll_name((-delta.clamp(-10.0, 10.0) * 3.0).round() as isize);
        }
    }

    pub(super) fn user_dialog_mouse_down(
        &mut self,
        x: f32,
        y: f32,
        layout: &BrowserLayout,
        scale: f32,
    ) -> Option<MsegEdit> {
        if in_rect(layout.close, x, y) || !in_rect(layout.panel, x, y) {
            self.close_curve_browser();
            return None;
        }
        let dialog_layout = dialog_layout(layout, scale);
        let delete = self
            .browser
            .user
            .dialog
            .as_ref()
            .is_some_and(|dialog| matches!(dialog.action, Action::Delete(_)));
        if !delete && in_rect(dialog_layout.category_picker, x, y) {
            if let Some(dialog) = self.browser.user.dialog.as_mut() {
                dialog.focus = 1;
            }
            self.open_category_menu(
                CategoryMenu::Assign,
                self.user_category_anchor(layout, scale),
                self.browser_window_size(layout.panel),
            );
        } else if !delete && in_rect(dialog_layout.category, x, y) {
            if let Some(dialog) = self.browser.user.dialog.as_mut() {
                dialog.focus = 1;
            }
        } else if in_rect(dialog_layout.accept, x, y) {
            self.commit_user_dialog();
        } else if in_rect(dialog_layout.cancel, x, y) || !in_rect(dialog_layout.panel, x, y) {
            self.browser.user.dialog = None;
        } else if in_rect(dialog_layout.field, x, y)
            && let Some(dialog) = self.browser.user.dialog.as_mut()
            && !matches!(dialog.action, Action::Delete(_))
        {
            dialog.focus = 0;
        }
        None
    }

    pub(super) fn user_page_start(&self) -> usize {
        self.browser_selected_position() / FAVORITE_COUNT * FAVORITE_COUNT
    }

    pub(super) fn move_user_page(&mut self, delta: i32) {
        let selected = self.browser_selected_position();
        let len = self.browser_visible_indices().len();
        let page = selected / FAVORITE_COUNT;
        let last = len.saturating_sub(1) / FAVORITE_COUNT;
        let next = if delta < 0 {
            page.saturating_sub(1)
        } else {
            (page + 1).min(last)
        };
        self.browser.last_click = None;
        if page == next {
            return;
        }
        self.select_browser_position(
            (next * FAVORITE_COUNT + selected % FAVORITE_COUNT).min(len.saturating_sub(1)),
        );
        self.browser.message.clear();
    }

    pub(super) fn user_browser_key(
        &mut self,
        event: &KeyboardEvent,
        data: &mut MsegData,
    ) -> Option<MsegEdit> {
        let selected = self.browser_selected_position();
        let last = self.browser_visible_indices().len().saturating_sub(1);
        match &event.key {
            Key::ArrowLeft => self.select_browser_position(selected.saturating_sub(1)),
            Key::ArrowRight => self.select_browser_position((selected + 1).min(last)),
            Key::ArrowUp => self.select_browser_position(selected.saturating_sub(4)),
            Key::ArrowDown => self.select_browser_position((selected + 4).min(last)),
            Key::Home => self.select_browser_position(0),
            Key::End => self.select_browser_position(last),
            Key::PageUp => self.move_user_page(-1),
            Key::PageDown => self.move_user_page(1),
            Key::Enter => {
                return self.user_browser_action(
                    usize::from(event.modifiers.contains(Modifiers::SHIFT)),
                    data,
                );
            }
            Key::F2 => return self.user_browser_action(3, data),
            Key::Delete | Key::Backspace => return self.user_browser_action(4, data),
            Key::Character(text) if text.eq_ignore_ascii_case("s") => {
                return self.user_browser_action(2, data);
            }
            Key::Character(text) if text.eq_ignore_ascii_case("r") => {
                return self.user_browser_action(5, data);
            }
            _ => return None,
        }
        self.browser.message.clear();
        None
    }

    pub(super) fn draw_user_pager(
        &self,
        pm: &mut Pixmap,
        tr: &mut TextRenderer,
        layout: &BrowserLayout,
    ) {
        let Some([previous, next]) = layout.pager else {
            return;
        };
        for (r, label, enabled) in [
            (previous, "<", self.user_page_start() > 0),
            (
                next,
                ">",
                self.user_page_start() + FAVORITE_COUNT < self.browser_visible_indices().len(),
            ),
        ] {
            crate::controls::draw_button(pm, tr, r.0, r.1, r.2, r.3, "", false, false);
            draw_fitted(
                tr,
                pm,
                (r.0 + r.2 * 0.3, r.1 + r.3 * 0.75),
                label,
                r.3 * 0.6,
                r.2 * 0.6,
                if enabled { color_text() } else { color_muted() },
            );
        }
        let pages = self
            .browser_visible_indices()
            .len()
            .div_ceil(FAVORITE_COUNT)
            .max(1);
        let text = format!(
            "{}/{}",
            self.browser_selected_position() / FAVORITE_COUNT + 1,
            pages
        );
        draw_fitted(
            tr,
            pm,
            (
                previous.0 + previous.2 + 4.0,
                previous.1 + previous.3 * 0.75,
            ),
            &text,
            (previous.3 * 0.55).max(10.0),
            next.0 - previous.0 - previous.2 - 8.0,
            color_muted(),
        );
    }

    pub(super) fn draw_user_dialog(
        &self,
        pm: &mut Pixmap,
        tr: &mut TextRenderer,
        layout: &BrowserLayout,
        scale: f32,
    ) {
        let Some(dialog) = &self.browser.user.dialog else {
            return;
        };
        let lay = dialog_layout(layout, scale);
        let (x, y, w, h) = lay.panel;
        let s = scale.max(0.01);
        let pad = (12.0 * s).max(6.0);
        let font = (12.0 * s).max(10.0);
        let line = font * 1.5;
        draw_rect(pm, x, y, w, h, color_bg());
        draw_rect_outline(pm, x, y, w, h, color_accent(), 1.0);
        draw_fitted(
            tr,
            pm,
            (x + pad, y + line),
            &format!("{} user preset", dialog.action.label()),
            (16.0 * s).max(11.0),
            w - 2.0 * pad,
            color_text(),
        );
        let field = lay.field;
        let delete = matches!(dialog.action, Action::Delete(_));
        let info = if delete {
            "Up/Down: name; Enter: delete; Esc: cancel"
        } else {
            "Name/category: Enter applies; Esc cancels; Tab: focus"
        };
        let info_lines = wrap(tr, info, font, field.2);
        let name_rows =
            (((lay.accept.1 - 5.0 - (info_lines.len().min(2) + 2) as f32 * line - field.1 - font)
                / line)
                .floor() as usize)
                .saturating_add(1)
                .max(1);
        let mut name_lines = if delete {
            wrap(tr, &dialog.field.text, font, field.2)
        } else {
            Vec::new()
        };
        if name_lines.len() > name_rows {
            name_lines = wrap(tr, &dialog.field.text, font, field.2 - 12.0);
        }
        let max_scroll = name_lines.len().saturating_sub(name_rows);
        dialog.name_scroll_max.set(max_scroll);
        dialog.name_rows.set(name_rows);
        let start = dialog.name_scroll.min(max_scroll);
        let visible_name = &name_lines[start..(start + name_rows).min(name_lines.len())];
        if delete {
            for (i, text) in visible_name.iter().enumerate() {
                draw_fitted(
                    tr,
                    pm,
                    (field.0, field.1 + font + i as f32 * line),
                    text,
                    font,
                    field.2,
                    color_text(),
                );
            }
            if max_scroll > 0 {
                let height = name_rows as f32 * line;
                let thumb = height * name_rows as f32 / name_lines.len() as f32;
                let top = field.1 + (height - thumb) * start as f32 / max_scroll as f32;
                draw_rect(
                    pm,
                    field.0 + field.2 - 4.0,
                    field.1,
                    3.0,
                    height,
                    color_border(),
                );
                draw_rect(pm, field.0 + field.2 - 4.0, top, 3.0, thumb, color_accent());
            }
        } else {
            draw_name_field(
                pm,
                tr,
                field,
                &dialog.field,
                dialog.focus == 0,
                font,
                "Name",
            );
            draw_fitted(
                tr,
                pm,
                (
                    field.0,
                    lay.category.1 + (lay.category.3 + font) * 0.5 - 2.0,
                ),
                "Category",
                font,
                lay.category.0 - field.0 - 4.0,
                color_muted(),
            );
            draw_name_field(
                pm,
                tr,
                lay.category,
                &dialog.category,
                dialog.focus == 1,
                font,
                "Uncategorized",
            );
            crate::dropdown::draw_dropdown_trigger(
                pm,
                tr,
                lay.category_picker,
                "",
                self.browser
                    .categories
                    .menu
                    .is_open_for(CategoryMenu::Assign),
            );
        }
        let info_y = if delete {
            field.1 + font + line * visible_name.len() as f32
        } else {
            lay.category.1 + lay.category.3 + line
        };
        for (i, text) in info_lines.iter().take(2).enumerate() {
            draw_fitted(
                tr,
                pm,
                (field.0, info_y + i as f32 * line),
                text,
                font,
                field.2,
                color_muted(),
            );
        }
        for (i, text) in wrap(tr, &dialog.error, font, field.2)
            .iter()
            .take(2)
            .enumerate()
        {
            let y = info_y + (i as f32 + 2.0) * line;
            if y < lay.accept.1 - 4.0 {
                draw_fitted(tr, pm, (field.0, y), text, font, field.2, color_accent());
            }
        }
        for (r, label, focus) in [
            (lay.accept, dialog.action.label(), 2),
            (lay.cancel, "Cancel", 3),
        ] {
            crate::controls::draw_button(
                pm,
                tr,
                r.0,
                r.1,
                r.2,
                r.3,
                label,
                dialog.focus == focus,
                false,
            );
        }
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn deletion(bounds: (f32, f32), name: &str) -> MsegEditState {
        let mut state = MsegEditState::new_curve_only();
        state.set_overlay_bounds(bounds);
        state.browser.page = Page::User;
        state.browser.open = true;
        let expected = UserPreset {
            id: 1,
            name: name.to_owned(),
            category: String::new(),
            shape: factory_shape(FactoryShape::Duck),
        };
        state.browser.user.entries.push(expected.clone());
        state.browser.user.dialog = Some(Dialog {
            field: NameField::new(expected.name.clone()),
            category: NameField::new(String::new()),
            action: Action::Delete(expected),
            document: MsegData::default(),
            error: String::new(),
            focus: 2,
            name_scroll: 0,
            name_scroll_max: Cell::new(0),
            name_rows: Cell::new(1),
        });
        state
    }

    #[test]
    fn maximum_width_name_glyphs_remain_intact_beneath_delete_instructions() {
        for (name, count) in [("W".repeat(64), 3), ("‱".repeat(64), 4)] {
            let state = deletion((520.0, 360.0), &name);
            let rect = (10.0, 35.0, 500.0, 210.0);
            let layout = state.browser_layout(rect, 0.5).unwrap();
            let field = dialog_layout(&layout, 0.5).field;
            let mut tr = TextRenderer::new(crate::DEFAULT_FONT);
            let lines = wrap(&mut tr, &name, 10.0, field.2);
            assert_eq!(lines.len(), count);
            let mut actual = Pixmap::new(520, 360).unwrap();
            actual.fill(color_bg());
            state.draw_curve_browser(&mut actual, &mut tr, rect, 0.5);
            let mut expected = Pixmap::new(520, 360).unwrap();
            expected.fill(color_bg());
            for (i, text) in lines.iter().enumerate() {
                tr.draw_text(
                    &mut expected,
                    field.0,
                    field.1 + 10.0 + i as f32 * 15.0,
                    text,
                    10.0,
                    color_text(),
                );
            }
            let mut compared = 0;
            for (index, (name, pixel)) in expected.pixels().iter().zip(actual.pixels()).enumerate()
            {
                if name.red().saturating_sub(color_bg().to_color_u8().red()) > 16 {
                    assert_eq!(
                        name, pixel,
                        "Name glyph missing or overwritten at pixel {index}"
                    );
                    compared += 1;
                }
            }
            assert!(compared > 100);
        }
    }

    #[test]
    fn admitted_user_layouts_reserve_delete_name_instructions_and_error_space() {
        for height in 180..261 {
            let state = deletion((520.0, height as f32), &"W".repeat(64));
            let rect = (10.0, 10.0, 500.0, height as f32 - 20.0);
            let Some(layout) = state.browser_layout(rect, 0.5) else {
                continue;
            };
            let lay = dialog_layout(&layout, 0.5);
            let last_error_baseline = lay.field.1 + 10.0 + 6.0 * 15.0;
            assert!(
                last_error_baseline < lay.accept.1 - 4.0,
                "Delete text has insufficient room at height {height}"
            );
        }
    }

    #[test]
    fn overflowing_delete_name_scrolls_to_its_final_unicode_characters() {
        let name = "‱".repeat(64);
        let mut state = deletion((290.0, 205.0), &name);
        let rect = (10.0, 10.0, 270.0, 185.0);
        let layout = state.browser_layout(rect, 0.5).unwrap();
        let field = dialog_layout(&layout, 0.5).field;
        let mut data = MsegData::default();
        let mut tr = TextRenderer::new(crate::DEFAULT_FONT);
        let mut first = Pixmap::new(290, 205).unwrap();
        state.draw_curve_browser(&mut first, &mut tr, rect, 0.5);
        assert_eq!(
            state.on_key(
                &KeyboardEvent {
                    key: Key::End,
                    state: KeyState::Down,
                    ..KeyboardEvent::default()
                },
                &mut data,
                rect,
                0.5
            ),
            (true, None)
        );
        let mut last = Pixmap::new(290, 205).unwrap();
        state.draw_curve_browser(&mut last, &mut tr, rect, 0.5);
        assert!(
            last.pixels() != first.pixels(),
            "End did not reveal the end of the name"
        );
        let lines = wrap(&mut tr, &name, 10.0, field.2 - 12.0);
        assert_eq!(lines.len(), 5);
        let mut expected = Pixmap::new(290, 205).unwrap();
        expected.fill(color_bg());
        for (i, text) in lines[1..].iter().enumerate() {
            tr.draw_text(
                &mut expected,
                field.0,
                field.1 + 10.0 + i as f32 * 15.0,
                text,
                10.0,
                color_text(),
            );
        }
        for (index, (name, pixel)) in expected.pixels().iter().zip(last.pixels()).enumerate() {
            if name.red().saturating_sub(color_bg().to_color_u8().red()) > 16 {
                assert_eq!(name, pixel, "Final name glyph missing at {index}");
            }
        }
        assert!(state.on_wheel(1.0, rect));
        let mut restored = Pixmap::new(290, 205).unwrap();
        state.draw_curve_browser(&mut restored, &mut tr, rect, 0.5);
        assert!(
            restored.pixels() == first.pixels(),
            "Wheel up did not restore the first name lines"
        );
        assert!(state.browser.user.dialog.is_some());
        assert_eq!(data, MsegData::default());
    }
}
