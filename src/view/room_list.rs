use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
    Frame,
};
use widgets::{render_list, ListRow, ListState};

pub struct Room {
    pub name: String,
    pub unread: usize,
    pub mentions: usize,
    pub pinned: bool,
    pub muted: bool,
}

pub struct RoomListState {
    pub rooms: Vec<Room>,
    pub list: ListState,
}

impl RoomListState {
    pub fn new() -> Self {
        let mut s = Self {
            rooms: mock_rooms(),
            list: ListState::new(),
        };
        s.sort();
        s
    }

    pub fn sort(&mut self) {
        self.rooms.sort_by(|a, b| {
            let a_unread = a.unread > 0;
            let b_unread = b.unread > 0;
            b_unread.cmp(&a_unread).then_with(|| {
                b.mentions
                    .cmp(&a.mentions)
                    .then_with(|| b.unread.cmp(&a.unread))
                    .then_with(|| a.name.cmp(&b.name))
            })
        });
    }

    pub fn next(&mut self, n: usize) {
        self.list.next(n, self.rooms.len());
    }

    pub fn prev(&mut self, n: usize) {
        self.list.prev(n);
    }

    pub fn first(&mut self) {
        self.list.first();
    }

    pub fn last(&mut self) {
        self.list.last(self.rooms.len());
    }

    pub fn selected(&self) -> usize {
        self.list.selected
    }

    pub fn set_selected(&mut self, idx: usize) {
        self.list.selected = idx;
    }

    pub fn selected_room_name(&self) -> Option<String> {
        self.rooms.get(self.list.selected).map(|r| r.name.clone())
    }
}

const NAME_COL: u16 = 5; // "  pin mute " before the name

pub fn render(
    frame: &mut Frame,
    area: Rect,
    s: &mut RoomListState,
    search_query: Option<&str>,
) -> (u16, u16) {
    let title_area = Rect {
        x: area.x + 2,
        y: area.y,
        width: area.width.saturating_sub(4),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new("Rooms").style(Style::default().add_modifier(Modifier::BOLD)),
        title_area,
    );

    let body = Rect {
        x: area.x,
        y: area.y + 2,
        width: area.width,
        height: area.height.saturating_sub(3),
    };

    let rows: Vec<ListRow> = s
        .rooms
        .iter()
        .map(|room| {
            let pin = if room.pinned { '*' } else { ' ' };
            let mute = if room.muted { 'm' } else { ' ' };
            let counts = if room.unread == 0 {
                String::new()
            } else if room.mentions > 0 {
                format!(" [{}@{}]", room.unread, room.mentions)
            } else {
                format!(" [{}]", room.unread)
            };
            let text = format!("  {}{} {}{}", pin, mute, room.name, counts);
            ListRow::new(text)
                .cursor_col(NAME_COL)
                .bold(room.unread > 0)
                .dim(room.muted)
        })
        .collect();

    let cursor = render_list(frame, body, &rows, &mut s.list, search_query);

    let help = Paragraph::new(
        "↑↓: parcourir · Entrée: ouvrir · Esc: retour · * pinned · m muted · [N] non lus · [N@M] mentions",
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

fn mock_rooms() -> Vec<Room> {
    vec![
        Room {
            name: "#dev".into(),
            unread: 3,
            mentions: 1,
            pinned: true,
            muted: false,
        },
        Room {
            name: "#general".into(),
            unread: 0,
            mentions: 0,
            pinned: true,
            muted: false,
        },
        Room {
            name: "#alerts".into(),
            unread: 12,
            mentions: 0,
            pinned: false,
            muted: false,
        },
        Room {
            name: "#postmortems".into(),
            unread: 0,
            mentions: 0,
            pinned: false,
            muted: false,
        },
        Room {
            name: "#random".into(),
            unread: 47,
            mentions: 0,
            pinned: false,
            muted: true,
        },
        Room {
            name: "#frontend".into(),
            unread: 2,
            mentions: 0,
            pinned: false,
            muted: false,
        },
        Room {
            name: "#backend".into(),
            unread: 0,
            mentions: 0,
            pinned: false,
            muted: false,
        },
        Room {
            name: "#infra".into(),
            unread: 1,
            mentions: 1,
            pinned: false,
            muted: false,
        },
        Room {
            name: "#a11y".into(),
            unread: 0,
            mentions: 0,
            pinned: true,
            muted: false,
        },
        Room {
            name: "@alice:matrix.org".into(),
            unread: 4,
            mentions: 4,
            pinned: false,
            muted: false,
        },
        Room {
            name: "@bob:matrix.org".into(),
            unread: 0,
            mentions: 0,
            pinned: false,
            muted: false,
        },
        Room {
            name: "@carol:matrix.org".into(),
            unread: 1,
            mentions: 0,
            pinned: false,
            muted: false,
        },
        Room {
            name: "#archives".into(),
            unread: 0,
            mentions: 0,
            pinned: false,
            muted: true,
        },
    ]
}
