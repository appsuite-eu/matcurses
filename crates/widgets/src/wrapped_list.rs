use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::Paragraph,
    Frame,
};

/// One pre-wrapped line of content. Multiple `WrappedLine`s can share the
/// same `item_idx` when an item (e.g., a chat message) wraps over several
/// physical lines. The widget uses `item_idx` to know which lines belong to
/// the same logical item and to highlight them as a group.
#[derive(Default)]
pub struct WrappedLine {
    pub item_idx: usize,
    pub is_first: bool,
    pub text: String,
    pub bold: bool,
    pub dim: bool,
}

impl WrappedLine {
    pub fn new(item_idx: usize, is_first: bool, text: impl Into<String>) -> Self {
        Self {
            item_idx,
            is_first,
            text: text.into(),
            bold: false,
            dim: false,
        }
    }
}

/// Renders a flat list of pre-wrapped lines where multiple lines may belong
/// to the same `item`. Handles scrolling so that `selected_item` is fully
/// visible when possible. Returns the cursor position to place on the
/// focused item:
///
/// - If `search_query` is `Some(q)` and a wrapped line of the selected item
///   contains `q` (case-insensitive), the cursor lands at the column of the
///   match in that line.
/// - Otherwise the cursor is at the start (column 0) of the selected item's
///   first wrapped line.
///
/// All lines belonging to the selected item are rendered in BOLD, plus the
/// per-line `bold` / `dim` modifiers.
pub fn render_wrapped_list(
    frame: &mut Frame,
    area: Rect,
    lines: &[WrappedLine],
    selected_item: usize,
    scroll_top: &mut usize,
    search_query: Option<&str>,
) -> (u16, u16) {
    if lines.is_empty() || area.height == 0 {
        return (area.x, area.y);
    }
    let height = area.height as usize;

    // Compute scroll so that the selected item is visible.
    let first_line = lines
        .iter()
        .position(|l| l.item_idx == selected_item && l.is_first)
        .unwrap_or(0);
    let last_line = lines
        .iter()
        .rposition(|l| l.item_idx == selected_item)
        .unwrap_or(first_line);

    let mut scroll = (*scroll_top).min(lines.len().saturating_sub(1));
    if first_line < scroll {
        scroll = first_line;
    } else if last_line >= scroll + height {
        scroll = last_line + 1 - height;
    }
    let max_scroll = lines.len().saturating_sub(height);
    scroll = scroll.min(max_scroll);
    *scroll_top = scroll;

    // Render visible window.
    let mut rendered: Vec<Line> = Vec::with_capacity(height);
    for wl in lines.iter().skip(scroll).take(height) {
        let mut style = Style::default();
        if wl.bold || wl.item_idx == selected_item {
            style = style.add_modifier(Modifier::BOLD);
        }
        if wl.dim {
            style = style.add_modifier(Modifier::DIM);
        }
        rendered.push(Line::styled(wl.text.clone(), style));
    }
    while rendered.len() < height {
        rendered.push(Line::raw(""));
    }
    frame.render_widget(Paragraph::new(rendered), area);

    // Cursor: prefer match position when search is active.
    if let Some(q) = search_query {
        if !q.is_empty() {
            let lower_q = q.to_lowercase();
            for (idx, line) in lines.iter().enumerate() {
                if line.item_idx != selected_item {
                    continue;
                }
                if let Some(byte_pos) = line.text.to_lowercase().find(&lower_q) {
                    if idx < scroll {
                        continue;
                    }
                    let row_offset = (idx - scroll) as u16;
                    if row_offset >= area.height {
                        continue;
                    }
                    let col = line.text[..byte_pos].chars().count() as u16;
                    return (area.x + col, area.y + row_offset);
                }
            }
        }
    }
    // Fallback: start of selected item's first visible wrapped line.
    if let Some(line_idx) = lines
        .iter()
        .position(|l| l.item_idx == selected_item && l.is_first)
    {
        if line_idx >= scroll {
            let row_offset = (line_idx - scroll) as u16;
            if row_offset < area.height {
                return (area.x, area.y + row_offset);
            }
        }
    }
    (area.x, area.y)
}
