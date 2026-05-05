use ratatui::{layout::Rect, Frame};

use crate::list::{render_list, ListRow, ListState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeRowKind {
    /// `+` indicator (collapsed parent)
    Closed,
    /// `-` indicator (expanded parent)
    Open,
    /// blank indicator (non-parent)
    Leaf,
}

pub struct TreeRow {
    pub depth: u16,
    pub kind: TreeRowKind,
    pub label: String,
    pub trailing: String,
    pub bold: bool,
    pub dim: bool,
}

impl TreeRow {
    pub fn new(depth: u16, kind: TreeRowKind, label: impl Into<String>) -> Self {
        Self {
            depth,
            kind,
            label: label.into(),
            trailing: String::new(),
            bold: false,
            dim: false,
        }
    }

    pub fn trailing(mut self, t: impl Into<String>) -> Self {
        self.trailing = t.into();
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

const LEFT_PAD: u16 = 2;
const DEPTH_PAD: u16 = 2;

fn to_list_row(t: &TreeRow) -> ListRow {
    let indicator = match t.kind {
        TreeRowKind::Closed => '+',
        TreeRowKind::Open => '-',
        TreeRowKind::Leaf => ' ',
    };
    let pad = " ".repeat((LEFT_PAD + t.depth * DEPTH_PAD) as usize);
    let text = format!("{}{}{}{}", pad, indicator, t.label, t.trailing);
    let cursor_col = LEFT_PAD + t.depth * DEPTH_PAD;
    ListRow::new(text)
        .cursor_col(cursor_col)
        .bold(t.bold)
        .dim(t.dim)
}

pub fn render_tree(
    frame: &mut Frame,
    area: Rect,
    rows: &[TreeRow],
    state: &mut ListState,
    search_query: Option<&str>,
) -> (u16, u16) {
    let list_rows: Vec<ListRow> = rows.iter().map(to_list_row).collect();
    render_list(frame, area, &list_rows, state, search_query)
}
