use super::*;

const MAX_POPUP_ROWS: usize = 8;

#[derive(Clone, Debug)]
pub(super) struct SlashCommandEntry {
    pub(super) name: String,
    pub(super) description: String,
}

pub(super) struct SlashPopup {
    all_entries: Vec<SlashCommandEntry>,
    filtered: Vec<usize>,
    selected: usize,
    pub(super) visible: bool,
}

impl SlashPopup {
    pub(super) fn new(entries: Vec<SlashCommandEntry>) -> Self {
        let filtered: Vec<usize> = (0..entries.len()).collect();
        Self {
            all_entries: entries,
            filtered,
            selected: 0,
            visible: false,
        }
    }

    pub(super) fn update_filter(&mut self, filter: &str) {
        let filter_lower = filter.to_lowercase();
        self.filtered = self
            .all_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                if filter_lower.is_empty() {
                    true
                } else {
                    entry.name.to_lowercase().starts_with(&filter_lower)
                }
            })
            .map(|(i, _)| i)
            .collect();
        // Clamp selection
        if self.filtered.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered.len() {
            self.selected = 0;
        }
    }

    pub(super) fn move_up(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.filtered.len() - 1; // wrap to bottom
        } else {
            self.selected -= 1;
        }
    }

    pub(super) fn move_down(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        if self.selected + 1 >= self.filtered.len() {
            self.selected = 0; // wrap to top
        } else {
            self.selected += 1;
        }
    }

    pub(super) fn selected_entry(&self) -> Option<&SlashCommandEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|&idx| self.all_entries.get(idx))
    }

    pub(super) fn visible_rows(&self) -> usize {
        self.filtered.len().min(MAX_POPUP_ROWS)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }

    pub(super) fn render_popup(&self, frame: &mut ratatui::Frame, area: Rect) {
        if !self.visible || self.filtered.is_empty() {
            return;
        }

        let visible_count = self.visible_rows();
        let height = (visible_count as u16) + 2; // +2 for borders

        // Position the popup ABOVE the given area (composer area)
        if area.y < height {
            return; // not enough room
        }
        let popup_rect = Rect {
            x: area.x,
            y: area.y - height,
            width: area.width,
            height,
        };

        // Clear the area
        frame.render_widget(Clear, popup_rect);

        // Build list items with scrolling window
        let scroll_top = if self.selected >= visible_count {
            self.selected - visible_count + 1
        } else {
            0
        };

        let items: Vec<ListItem> = self
            .filtered
            .iter()
            .skip(scroll_top)
            .take(visible_count)
            .enumerate()
            .map(|(display_idx, &entry_idx)| {
                let entry = &self.all_entries[entry_idx];
                let is_selected = display_idx + scroll_top == self.selected;

                let name_span = Span::styled(
                    format!("/{}", entry.name),
                    if is_selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Yellow)
                    },
                );

                let desc_span = Span::styled(
                    format!("  {}", entry.description),
                    if is_selected {
                        Style::default().fg(Color::Black).bg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                );

                ListItem::new(Line::from(vec![name_span, desc_span]))
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" Commands ")
                .title_style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
        );

        frame.render_widget(list, popup_rect);
    }
}
