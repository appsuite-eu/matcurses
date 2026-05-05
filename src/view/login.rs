use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
    Frame,
};
use widgets::{render_form, FormField};

pub const FIELD_COUNT: usize = 5;

pub const F_MXID: usize = 0;
pub const F_PASSWORD: usize = 1;
pub const F_SERVER: usize = 2;
pub const F_CONNECT: usize = 3;
pub const F_CANCEL: usize = 4;

pub struct LoginState {
    pub mxid: String,
    pub password: String,
    pub server: String,
    pub focus_idx: usize,
}

impl LoginState {
    pub fn new() -> Self {
        Self {
            mxid: String::new(),
            password: String::new(),
            server: String::new(),
            focus_idx: 0,
        }
    }

    pub fn next(&mut self) {
        self.focus_idx = (self.focus_idx + 1) % FIELD_COUNT;
    }

    pub fn prev(&mut self) {
        self.focus_idx = (self.focus_idx + FIELD_COUNT - 1) % FIELD_COUNT;
    }

    pub fn focused_text(&mut self) -> Option<&mut String> {
        match self.focus_idx {
            F_MXID => Some(&mut self.mxid),
            F_PASSWORD => Some(&mut self.password),
            F_SERVER => Some(&mut self.server),
            _ => None,
        }
    }
}

pub fn render(frame: &mut Frame, area: Rect, s: &LoginState) -> (u16, u16) {
    let title_area = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new("Connexion Matrix").style(Style::default().add_modifier(Modifier::BOLD)),
        title_area,
    );

    let fields = [
        FormField::Text {
            label: "MXID",
            value: &s.mxid,
            masked: false,
        },
        FormField::Spacer,
        FormField::Text {
            label: "Mot de passe",
            value: &s.password,
            masked: true,
        },
        FormField::Spacer,
        FormField::Text {
            label: "Serveur",
            value: &s.server,
            masked: false,
        },
        FormField::Spacer,
        FormField::Buttons2 {
            first: "Connexion",
            second: "Annuler",
        },
    ];

    let body = Rect {
        x: area.x + 2,
        y: area.y + 3,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(5),
    };
    let cursor = render_form(frame, body, &fields, s.focus_idx);

    let help =
        Paragraph::new("Tab/Maj+Tab: champ suivant/précédent · Entrée: valider · Esc: fermer")
            .style(Style::default().add_modifier(Modifier::DIM));
    frame.render_widget(
        help,
        Rect {
            x: area.x + 2,
            y: area.y + area.height.saturating_sub(2),
            width: area.width.saturating_sub(4),
            height: 1,
        },
    );

    cursor
}
