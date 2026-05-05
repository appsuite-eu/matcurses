use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
    Frame,
};
use widgets::{render_form, FormField};

pub const FIELD_COUNT: usize = 7;

pub const F_TTS: usize = 0;
pub const F_NATO: usize = 1;
pub const F_SAS: usize = 2;
pub const F_VOICE: usize = 3;
pub const F_DOC: usize = 4;
pub const F_SAVE: usize = 5;
pub const F_CANCEL: usize = 6;

const SAS_OPTIONS: &[&str] = &["Décimal", "Emoji (noms)"];
const VOICE_OPTIONS: &[&str] = &[
    "Toggle (start/stop par touche)",
    "Push-to-talk (maintien)",
];

pub struct SettingsState {
    pub tts: bool,
    pub nato: bool,
    pub sas_decimal: bool,
    pub voice_toggle: bool,
    pub focus_idx: usize,
}

impl SettingsState {
    pub fn new() -> Self {
        Self {
            tts: true,
            nato: true,
            sas_decimal: true,
            voice_toggle: true,
            focus_idx: 0,
        }
    }

    pub fn next(&mut self) {
        self.focus_idx = (self.focus_idx + 1) % FIELD_COUNT;
    }

    pub fn prev(&mut self) {
        self.focus_idx = (self.focus_idx + FIELD_COUNT - 1) % FIELD_COUNT;
    }
}

pub fn render(frame: &mut Frame, area: Rect, s: &SettingsState) -> (u16, u16) {
    let title_area = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new("Paramètres").style(Style::default().add_modifier(Modifier::BOLD)),
        title_area,
    );

    let fields = [
        FormField::Checkbox {
            label: "TTS activé",
            checked: s.tts,
        },
        FormField::Checkbox {
            label: "Alphabet OTAN pour clés / SAS",
            checked: s.nato,
        },
        FormField::Spacer,
        FormField::Radio {
            label: "Format SAS",
            options: SAS_OPTIONS,
            selected: if s.sas_decimal { 0 } else { 1 },
        },
        FormField::Spacer,
        FormField::Radio {
            label: "Mode voice notes",
            options: VOICE_OPTIONS,
            selected: if s.voice_toggle { 0 } else { 1 },
        },
        FormField::Spacer,
        FormField::Link {
            label: "Voir la documentation en ligne",
        },
        FormField::Spacer,
        FormField::Buttons2 {
            first: "Enregistrer",
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

    let help = Paragraph::new(
        "Tab/Maj+Tab: champ suivant/précédent · Espace: toggle · ←→: option · Entrée: activer · Esc: fermer",
    )
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
