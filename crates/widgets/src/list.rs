use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::Paragraph,
    Frame,
};

pub struct ListRow {
    pub text: String,
    pub cursor_col: u16,
    pub bold: bool,
    pub dim: bool,
}

impl ListRow {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cursor_col: 0,
            bold: false,
            dim: false,
        }
    }

    pub fn cursor_col(mut self, col: u16) -> Self {
        self.cursor_col = col;
        self
    }

    pub fn bold(mut self, b: bool) -> Self {
        self.bold = b;
        self
    }

    pub fn dim(mut self, d: bool) -> Self {
        self.dim = d;
        self
    }
}

pub struct ListState {
    pub selected: usize,
    pub scroll_top: usize,
}

impl Default for ListState {
    fn default() -> Self {
        Self::new()
    }
}

impl ListState {
    pub fn new() -> Self {
        Self {
            selected: 0,
            scroll_top: 0,
        }
    }

    pub fn next(&mut self, n: usize, total: usize) {
        if total == 0 {
            self.selected = 0;
        } else {
            self.selected = (self.selected + n).min(total - 1);
        }
    }

    pub fn prev(&mut self, n: usize) {
        self.selected = self.selected.saturating_sub(n);
    }

    pub fn first(&mut self) {
        self.selected = 0;
    }

    pub fn last(&mut self, total: usize) {
        self.selected = total.saturating_sub(1);
    }
}

/// Renders a vertical list inside `area`. Manages scroll, selection style and
/// returns the cursor position to set on the focused row.
///
/// If `search_query` is `Some(q)`, the cursor on the selected row is moved to
/// the column of the first occurrence of `q` (case-insensitive). Otherwise,
/// the row's `cursor_col` is used.
pub fn render_list(
    frame: &mut Frame,
    area: Rect,
    rows: &[ListRow],
    state: &mut ListState,
    search_query: Option<&str>,
) -> (u16, u16) {
    let total = rows.len();
    if total == 0 || area.height == 0 {
        return (area.x, area.y);
    }
    if state.selected >= total {
        state.selected = total - 1;
    }
    let height = area.height as usize;
    if state.selected < state.scroll_top {
        state.scroll_top = state.selected;
    } else if state.selected >= state.scroll_top + height {
        state.scroll_top = state.selected + 1 - height;
    }

    let mut cursor = (area.x, area.y);
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for (i, row) in rows.iter().enumerate().skip(state.scroll_top).take(height) {
        let mut style = Style::default();
        if row.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if row.dim {
            style = style.add_modifier(Modifier::DIM);
        }
        if i == state.selected {
            style = style.add_modifier(Modifier::REVERSED);
            let row_y = area.y + (i - state.scroll_top) as u16;
            let mut col = row.cursor_col;
            if let Some(q) = search_query {
                if !q.is_empty() {
                    let lower = row.text.to_lowercase();
                    if let Some(byte_pos) = lower.find(&q.to_lowercase()) {
                        col = row.text[..byte_pos].chars().count() as u16;
                    }
                }
            }
            cursor = (area.x + col, row_y);
        }
        lines.push(Line::styled(row.text.clone(), style));
    }
    frame.render_widget(Paragraph::new(lines), area);
    cursor
}
