use super::*;
use crate::dropdown::{DropdownEvent, DropdownKey, DropdownList, DropdownState};
use std::collections::BTreeMap;

const ALL_CARDS: [usize; FAVORITE_COUNT] = [0, 1, 2, 3, 4, 5, 6, 7];

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub(super) enum CategoryFilter {
    #[default]
    All,
    Category(String),
}

impl CategoryFilter {
    pub fn matches(&self, category: &str) -> bool {
        match self {
            Self::All => true,
            Self::Category(selected) => selected.to_lowercase() == category.to_lowercase(),
        }
    }
    fn same_key(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::All, Self::All) => true,
            (Self::Category(a), Self::Category(b)) => a.to_lowercase() == b.to_lowercase(),
            _ => false,
        }
    }
    fn label(&self) -> String {
        match self {
            Self::All => "All categories".to_owned(),
            Self::Category(category) if category.is_empty() => "Uncategorized".to_owned(),
            Self::Category(category) => format!("Category: {category}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CategoryMenu {
    Filter,
    Assign,
}

struct Choice {
    filter: CategoryFilter,
    label: String,
}

#[derive(Default)]
pub(super) struct Categories {
    pub filter: CategoryFilter,
    pub visible: Vec<usize>,
    pub menu: DropdownState<CategoryMenu>,
    choices: Vec<Choice>,
}

impl MsegEditState {
    pub(super) fn browser_visible_indices(&self) -> &[usize] {
        if matches!(self.browser.page, Page::Factory | Page::User) {
            &self.browser.categories.visible
        } else {
            &ALL_CARDS
        }
    }

    pub(super) fn browser_selected_position(&self) -> usize {
        self.browser_visible_indices()
            .iter()
            .position(|index| *index == self.browser.selected)
            .unwrap_or(0)
    }

    pub(super) fn select_browser_position(&mut self, position: usize) {
        self.browser.selected = self
            .browser_visible_indices()
            .get(position)
            .copied()
            .unwrap_or(0);
    }

    pub(super) fn rebuild_browser_view(
        &mut self,
        preferred: Option<usize>,
        fallback_position: usize,
    ) {
        self.close_category_menu();
        self.browser.categories.visible = match self.browser.page {
            Page::Factory => FactoryShape::ALL
                .iter()
                .enumerate()
                .filter(|(_, shape)| self.browser.categories.filter.matches(shape.category()))
                .map(|(index, _)| index)
                .collect(),
            Page::User => self
                .browser
                .user
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| self.browser.categories.filter.matches(&entry.category))
                .map(|(index, _)| index)
                .collect(),
            _ => ALL_CARDS.to_vec(),
        };
        self.browser.selected = preferred
            .filter(|index| self.browser.categories.visible.contains(index))
            .or_else(|| {
                self.browser
                    .categories
                    .visible
                    .get(
                        fallback_position
                            .min(self.browser.categories.visible.len().saturating_sub(1)),
                    )
                    .copied()
            })
            .unwrap_or(0);
        self.browser.last_click = None;
    }

    pub(super) fn select_browser_category(&mut self, filter: CategoryFilter) {
        let selected = self.browser.selected;
        self.browser.categories.filter = filter;
        self.rebuild_browser_view(Some(selected), 0);
        self.browser.message.clear();
    }

    pub(super) fn active_user_category(&self) -> String {
        match &self.browser.categories.filter {
            CategoryFilter::All => String::new(),
            CategoryFilter::Category(category) => category.clone(),
        }
    }

    pub(super) fn reveal_user_category(&mut self, category: &str) {
        if !self.browser.categories.filter.matches(category) {
            self.browser.categories.filter = CategoryFilter::Category(category.to_owned());
        }
    }

    pub(super) fn close_category_menu(&mut self) {
        self.browser.categories.menu.close();
        self.browser.categories.choices.clear();
    }

    pub(super) fn browser_window_size(&self, rect: Rect) -> (f32, f32) {
        self.overlay_bounds()
            .unwrap_or((rect.0 + rect.2, rect.1 + rect.3))
    }

    pub(super) fn open_category_menu(
        &mut self,
        action: CategoryMenu,
        anchor: Rect,
        window: (f32, f32),
    ) {
        let mut labels = BTreeMap::<String, String>::new();
        let mut insert = |category: &str| {
            if category.is_empty() {
                return;
            }
            labels
                .entry(category.to_lowercase())
                .and_modify(|label| {
                    if category < label.as_str() {
                        *label = category.to_owned();
                    }
                })
                .or_insert_with(|| category.to_owned());
        };
        if action == CategoryMenu::Assign || self.browser.page == Page::Factory {
            for shape in FactoryShape::ALL {
                insert(shape.category());
            }
        }
        if action == CategoryMenu::Assign || self.browser.page == Page::User {
            for entry in &self.browser.user.entries {
                insert(&entry.category);
            }
        }
        let current = if action == CategoryMenu::Assign {
            let Some(category) = self.user_category_text() else {
                return;
            };
            insert(category.trim());
            CategoryFilter::Category(category.trim().to_owned())
        } else {
            if let CategoryFilter::Category(category) = &self.browser.categories.filter {
                insert(category);
            }
            self.browser.categories.filter.clone()
        };
        let mut filters = Vec::new();
        if action == CategoryMenu::Filter {
            filters.push(CategoryFilter::All);
        }
        if action == CategoryMenu::Assign || self.browser.page == Page::User {
            filters.push(CategoryFilter::Category(String::new()));
        }
        filters.extend(labels.into_values().map(CategoryFilter::Category));
        let selected = filters
            .iter()
            .position(|filter| filter.same_key(&current))
            .unwrap_or(0);
        self.browser.categories.choices = filters
            .into_iter()
            .map(|filter| Choice {
                label: filter.label(),
                filter,
            })
            .collect();
        let refs: Vec<_> = self
            .browser
            .categories
            .choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect();
        let text_size = (anchor.3 * 0.5).max(10.0);
        let columns = refs
            .iter()
            .map(|label| {
                label
                    .chars()
                    .map(|ch| if ch.is_ascii() { 1 } else { 2 })
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0)
            .max(24);
        // Reserve label room independently of the compact trigger, plus border/padding/scrollbar.
        let popup_width = anchor
            .2
            .max(columns as f32 * text_size + 19.0)
            .min(window.0);
        let popup_anchor = (anchor.0, anchor.1, popup_width, anchor.3);
        self.browser.categories.menu.open(
            action,
            popup_anchor,
            DropdownList::flat(&refs),
            selected,
            false,
            window,
        );
        self.browser.last_click = None;
    }

    fn finish_category_event(&mut self, event: Option<DropdownEvent<CategoryMenu>>) {
        match event {
            Some(DropdownEvent::Selected(action, index)) => {
                let selected = self
                    .browser
                    .categories
                    .choices
                    .get(index)
                    .map(|choice| choice.filter.clone());
                self.close_category_menu();
                if let Some(selected) = selected {
                    match action {
                        CategoryMenu::Filter => self.select_browser_category(selected),
                        CategoryMenu::Assign => {
                            if let CategoryFilter::Category(category) = selected {
                                self.set_user_category_text(category);
                            }
                        }
                    }
                }
            }
            Some(DropdownEvent::Closed(_)) => self.close_category_menu(),
            _ => {}
        }
    }

    pub(super) fn category_menu_key(&mut self, event: &KeyboardEvent, window: (f32, f32)) {
        if event.state != KeyState::Down {
            return;
        }
        let key = match event.key {
            Key::ArrowUp => DropdownKey::Up,
            Key::ArrowDown => DropdownKey::Down,
            Key::Home => DropdownKey::Home,
            Key::End => DropdownKey::End,
            Key::PageUp => DropdownKey::PageUp,
            Key::PageDown => DropdownKey::PageDown,
            Key::Enter => DropdownKey::Enter,
            Key::Escape | Key::Tab | Key::F4 => DropdownKey::Esc,
            _ => return,
        };
        let refs: Vec<_> = self
            .browser
            .categories
            .choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect();
        let result = self.browser.categories.menu.on_key(key, &refs, window);
        self.finish_category_event(result);
    }

    pub(super) fn category_menu_mouse_down(&mut self, x: f32, y: f32, window: (f32, f32)) {
        let refs: Vec<_> = self
            .browser
            .categories
            .choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect();
        let result = self
            .browser
            .categories
            .menu
            .on_mouse_down(x, y, &refs, window);
        self.finish_category_event(result);
    }

    pub(super) fn category_menu_move(&mut self, x: f32, y: f32, window: (f32, f32)) {
        let refs: Vec<_> = self
            .browser
            .categories
            .choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect();
        self.browser
            .categories
            .menu
            .on_mouse_move(x, y, &refs, window);
    }

    pub(super) fn category_menu_wheel(&mut self, delta: f32, window: (f32, f32)) {
        let refs: Vec<_> = self
            .browser
            .categories
            .choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect();
        self.browser.categories.menu.on_wheel(delta, &refs, window);
    }

    pub(super) fn draw_category_header(
        &self,
        pm: &mut Pixmap,
        tr: &mut TextRenderer,
        layout: &BrowserLayout,
    ) {
        if let Some(rect) = layout.category {
            let label = if rect.2 < 72.0 {
                "Cat".to_owned()
            } else {
                self.browser.categories.filter.label()
            };
            crate::dropdown::draw_dropdown_trigger(
                pm,
                tr,
                rect,
                &label,
                self.browser
                    .categories
                    .menu
                    .is_open_for(CategoryMenu::Filter),
            );
        }
    }

    pub(super) fn draw_category_menu(&self, pm: &mut Pixmap, tr: &mut TextRenderer) {
        let refs: Vec<_> = self
            .browser
            .categories
            .choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect();
        crate::dropdown::draw_dropdown_popup(
            pm,
            tr,
            &self.browser.categories.menu,
            &refs,
            (pm.width() as f32, pm.height() as f32),
        );
    }
}
