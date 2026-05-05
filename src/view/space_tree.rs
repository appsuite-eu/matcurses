use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
    Frame,
};
use widgets::{render_tree, ListState, TreeRow, TreeRowKind};

#[derive(Clone)]
pub enum NodeKind {
    Space {
        children: Vec<Node>,
        expanded: bool,
    },
    Room {
        name: String,
        unread: usize,
    },
}

#[derive(Clone)]
pub struct Node {
    pub label: String,
    pub kind: NodeKind,
}

pub struct SpaceTreeState {
    pub roots: Vec<Node>,
    pub list: ListState,
}

impl SpaceTreeState {
    pub fn new() -> Self {
        Self {
            roots: Vec::new(),
            list: ListState::new(),
        }
    }

    pub fn flat(&self) -> Vec<FlatItem> {
        let mut out = Vec::new();
        for (i, n) in self.roots.iter().enumerate() {
            collect_flat(n, &[i], 0, &mut out);
        }
        out
    }

    pub fn all_paths(&self) -> Vec<(Vec<usize>, String)> {
        let mut out = Vec::new();
        for (i, n) in self.roots.iter().enumerate() {
            collect_all_paths(n, &[i], &mut out);
        }
        out
    }

    pub fn find_pos(&self, target: &[usize]) -> Option<usize> {
        self.flat().iter().position(|it| it.path == target)
    }

    pub fn expand_to(&mut self, target: &[usize]) {
        if target.len() < 2 {
            return;
        }
        expand_along(&mut self.roots, &target[..target.len() - 1]);
    }

    pub fn selected(&self) -> usize {
        self.list.selected
    }

    pub fn set_selected(&mut self, idx: usize) {
        self.list.selected = idx;
    }

    pub fn next(&mut self, n: usize) {
        let total = self.flat().len();
        self.list.next(n, total);
    }

    pub fn prev(&mut self, n: usize) {
        self.list.prev(n);
    }

    pub fn first(&mut self) {
        self.list.first();
    }

    pub fn last(&mut self) {
        let total = self.flat().len();
        self.list.last(total);
    }

    pub fn open(&mut self) -> Action {
        let path = match self.flat().get(self.list.selected) {
            Some(it) => it.path.clone(),
            None => return Action::None,
        };
        let node = match resolve_mut(&mut self.roots, &path) {
            Some(n) => n,
            None => return Action::None,
        };
        match &mut node.kind {
            NodeKind::Space { expanded, .. } => {
                if !*expanded {
                    *expanded = true;
                }
                Action::None
            }
            NodeKind::Room { name, .. } => Action::OpenRoom(name.clone()),
        }
    }

    pub fn close(&mut self) {
        let path = match self.flat().get(self.list.selected) {
            Some(it) => it.path.clone(),
            None => return,
        };
        let node = match resolve_mut(&mut self.roots, &path) {
            Some(n) => n,
            None => return,
        };
        match &mut node.kind {
            NodeKind::Space { expanded, .. } => {
                if *expanded {
                    *expanded = false;
                }
            }
            NodeKind::Room { .. } => {
                if path.len() > 1 {
                    let parent_path = &path[..path.len() - 1];
                    let parent_pos = self
                        .flat()
                        .iter()
                        .position(|it| it.path == parent_path)
                        .unwrap_or(self.list.selected);
                    self.list.selected = parent_pos;
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct FlatItem {
    pub path: Vec<usize>,
    pub depth: u16,
    pub label: String,
    pub kind_repr: KindRepr,
    pub unread: usize,
}

#[derive(Clone, Copy)]
pub enum KindRepr {
    SpaceClosed,
    SpaceOpen,
    Room,
}

pub enum Action {
    None,
    OpenRoom(String),
}

fn collect_flat(node: &Node, path: &[usize], depth: u16, out: &mut Vec<FlatItem>) {
    let (kind_repr, unread) = match &node.kind {
        NodeKind::Space { expanded, .. } => (
            if *expanded {
                KindRepr::SpaceOpen
            } else {
                KindRepr::SpaceClosed
            },
            0,
        ),
        NodeKind::Room { unread, .. } => (KindRepr::Room, *unread),
    };
    out.push(FlatItem {
        path: path.to_vec(),
        depth,
        label: node.label.clone(),
        kind_repr,
        unread,
    });
    if let NodeKind::Space {
        children,
        expanded: true,
    } = &node.kind
    {
        for (i, child) in children.iter().enumerate() {
            let mut child_path = path.to_vec();
            child_path.push(i);
            collect_flat(child, &child_path, depth + 1, out);
        }
    }
}

fn collect_all_paths(node: &Node, path: &[usize], out: &mut Vec<(Vec<usize>, String)>) {
    out.push((path.to_vec(), node.label.clone()));
    if let NodeKind::Space { children, .. } = &node.kind {
        for (i, c) in children.iter().enumerate() {
            let mut p = path.to_vec();
            p.push(i);
            collect_all_paths(c, &p, out);
        }
    }
}

fn expand_along(roots: &mut [Node], path: &[usize]) {
    if path.is_empty() {
        return;
    }
    let node = match roots.get_mut(path[0]) {
        Some(n) => n,
        None => return,
    };
    if let NodeKind::Space { expanded, children } = &mut node.kind {
        *expanded = true;
        if path.len() > 1 {
            expand_along(children, &path[1..]);
        }
    }
}

fn resolve_mut<'a>(roots: &'a mut [Node], path: &[usize]) -> Option<&'a mut Node> {
    if path.is_empty() {
        return None;
    }
    let mut node = roots.get_mut(path[0])?;
    for &idx in &path[1..] {
        let children = match &mut node.kind {
            NodeKind::Space { children, .. } => children,
            _ => return None,
        };
        node = children.get_mut(idx)?;
    }
    Some(node)
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    s: &mut SpaceTreeState,
    search_query: Option<&str>,
) -> (u16, u16) {
    frame.render_widget(
        Paragraph::new("Spaces").style(Style::default().add_modifier(Modifier::BOLD)),
        Rect {
            x: area.x + 2,
            y: area.y,
            width: area.width.saturating_sub(4),
            height: 1,
        },
    );

    let body = Rect {
        x: area.x,
        y: area.y + 2,
        width: area.width,
        height: area.height.saturating_sub(3),
    };

    let flat = s.flat();
    let rows: Vec<TreeRow> = flat
        .iter()
        .map(|it| {
            let kind = match it.kind_repr {
                KindRepr::SpaceClosed => TreeRowKind::Closed,
                KindRepr::SpaceOpen => TreeRowKind::Open,
                KindRepr::Room => TreeRowKind::Leaf,
            };
            let trailing = if it.unread > 0 {
                format!(" [{}]", it.unread)
            } else {
                String::new()
            };
            TreeRow::new(it.depth, kind, &it.label)
                .trailing(trailing)
                .bold(matches!(kind, TreeRowKind::Closed | TreeRowKind::Open) || it.unread > 0)
        })
        .collect();

    let cursor = render_tree(frame, body, &rows, &mut s.list, search_query);

    let help = Paragraph::new(
        "↑↓: parcourir · → ou + : déplier space · ← ou - : replier · Entrée: ouvrir room · Esc: retour",
    )
    .style(Style::default().add_modifier(Modifier::DIM));
    frame.render_widget(
        help,
        Rect {
            x: area.x + 2,
            y: area.y + area.height.saturating_sub(1),
            width: area.width.saturating_sub(4),
            height: 1,
        },
    );

    cursor
}

