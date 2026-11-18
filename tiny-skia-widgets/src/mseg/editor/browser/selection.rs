use super::FAVORITE_COUNT;

#[derive(Default)]
pub(super) struct BrowserView {
    indices: Vec<usize>,
    selected_position: Option<usize>,
}

impl BrowserView {
    pub(super) fn rebuild(
        &mut self,
        indices: Vec<usize>,
        preferred: Option<usize>,
        fallback_position: usize,
    ) {
        let position = preferred
            .and_then(|index| indices.iter().position(|&visible| visible == index))
            .unwrap_or(fallback_position);
        self.indices = indices;
        self.select_position(position);
    }

    pub(super) fn indices(&self) -> &[usize] {
        &self.indices
    }

    pub(super) fn selected_index(&self) -> Option<usize> {
        self.selected_position
            .map(|position| self.indices[position])
    }

    pub(super) fn position_or_start(&self) -> usize {
        self.selected_position.unwrap_or(0)
    }

    pub(super) fn select_position(&mut self, position: usize) {
        self.selected_position =
            (!self.indices.is_empty()).then(|| position.min(self.indices.len() - 1));
    }

    pub(super) fn page_start(&self) -> usize {
        self.position_or_start() / FAVORITE_COUNT * FAVORITE_COUNT
    }

    pub(super) fn page_indices(&self) -> &[usize] {
        let start = self.page_start();
        &self.indices[start..(start + FAVORITE_COUNT).min(self.indices.len())]
    }

    pub(super) fn select_card(&mut self, card: usize) -> Option<usize> {
        let index = *self.page_indices().get(card)?;
        self.select_position(self.page_start() + card);
        Some(index)
    }

    pub(super) fn move_page(&mut self, delta: i32) -> bool {
        let Some(position) = self.selected_position else {
            return false;
        };
        let page = position / FAVORITE_COUNT;
        let last = (self.indices.len() - 1) / FAVORITE_COUNT;
        let next = if delta < 0 {
            page.saturating_sub(1)
        } else {
            (page + 1).min(last)
        };
        if page == next {
            return false;
        }
        self.select_position(next * FAVORITE_COUNT + position % FAVORITE_COUNT);
        true
    }
}
