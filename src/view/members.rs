use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
    Frame,
};
use widgets::{render_list, ListRow, ListState};

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Presence {
    Online,
    Idle,
    Offline,
    Unavailable,
}

impl Presence {
    pub fn glyph(self) -> char {
        match self {
            Presence::Online => '·',
            Presence::Idle => 'i',
            Presence::Offline => 'o',
            Presence::Unavailable => '?',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Presence::Online => "en ligne",
            Presence::Idle => "inactif",
            Presence::Offline => "hors ligne",
            Presence::Unavailable => "indéterminé",
        }
    }
}

pub struct Member {
    pub mxid: String,
    pub displayname: String,
    pub power_level: u8,
    pub presence: Presence,
}

impl Member {
    pub fn power_glyph(&self) -> char {
        match self.power_level {
            100 => '@',
            50..=99 => '+',
            _ => ' ',
        }
    }

    pub fn power_label(&self) -> &'static str {
        match self.power_level {
            100 => "admin",
            50..=99 => "modérateur",
            _ => "membre",
        }
    }
}

pub struct MembersState {
    pub members: Vec<Member>,
    pub list: ListState,
}

impl MembersState {
    pub fn new() -> Self {
        Self {
            members: Vec::new(),
            list: ListState::new(),
        }
    }

    pub fn next(&mut self, n: usize) {
        self.list.next(n, self.members.len());
    }

    pub fn prev(&mut self, n: usize) {
        self.list.prev(n);
    }

    pub fn first(&mut self) {
        self.list.first();
    }

    pub fn last(&mut self) {
        self.list.last(self.members.len());
    }

    pub fn selected(&self) -> usize {
        self.list.selected
    }

    pub fn set_selected(&mut self, idx: usize) {
        self.list.selected = idx;
    }

    pub fn current(&self) -> Option<&Member> {
        self.members.get(self.list.selected)
    }
}

const NAME_COL: u16 = 5; // "  presence power " before displayname

pub fn render(
    frame: &mut Frame,
    area: Rect,
    s: &mut MembersState,
    search_query: Option<&str>,
) -> (u16, u16) {
    frame.render_widget(
        Paragraph::new(format!("Membres ({})", s.members.len()))
            .style(Style::default().add_modifier(Modifier::BOLD)),
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

    let rows: Vec<ListRow> = s
        .members
        .iter()
        .map(|m| {
            let text = format!("  {}{} {}", m.presence.glyph(), m.power_glyph(), m.displayname);
            ListRow::new(text)
                .cursor_col(NAME_COL)
                .bold(m.power_level >= 50)
                .dim(m.presence == Presence::Offline)
        })
        .collect();

    let cursor = render_list(frame, body, &rows, &mut s.list, search_query);

    let help = Paragraph::new(
        "↑↓: parcourir · Entrée: détails · Esc: retour · · en ligne · i inactif · o hors ligne · @ admin · + modérateur",
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

