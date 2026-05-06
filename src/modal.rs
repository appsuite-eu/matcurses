// Cursor convention: see the `widgets` crate.
// Modals use these conventions to place the cursor on the focused element
// and propagate the position via `ModalCursor`.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use widgets::{
    centered_rect, render_button, render_list, render_modal_frame, render_text_input, Button,
    ListRow, ListState, ModalFrame, TextInput,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    Quit,
    Redact(usize),
    Logout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmButton {
    Yes,
    No,
}

pub struct ConfirmModal {
    pub title: String,
    pub message: String,
    pub action: ConfirmAction,
    pub focused: ConfirmButton,
}

pub struct DetailsModal {
    pub title: String,
    pub lines: Vec<String>,
    pub scroll: usize,
}

pub struct ReactionPickerModal {
    pub msg_idx: usize,
    pub options: Vec<String>,
    pub selected: usize,
}

pub struct ReactedByModal {
    pub title: String,
    pub entries: Vec<String>,
    pub selected: usize,
}

pub enum Modal {
    Confirm(ConfirmModal),
    Details(DetailsModal),
    ReactionPicker(ReactionPickerModal),
    ReactedBy(ReactedByModal),
    RecoveryInput(RecoveryInputModal),
    RecoveryDisplay(RecoveryDisplayModal),
    SasVerification(SasVerificationModal),
    WindowList(WindowListModal),
}

pub struct WindowListEntry {
    pub idx: usize,
    pub label: String,
    /// One char to indicate background activity: ' ' none, '+' new
    /// messages, '!' mention.
    pub activity: char,
    pub is_active: bool,
}

pub struct WindowListModal {
    pub entries: Vec<WindowListEntry>,
    pub selected: usize,
}

pub struct SasVerificationModal {
    pub decimal: (u16, u16, u16),
    pub emoji: Vec<(String, String)>,
    pub focused: ConfirmButton,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RecoveryDisplayFocus {
    Confirm,
    Cancel,
}

pub struct RecoveryDisplayModal {
    pub key: String,
    pub show_nato: bool,
    pub focused: RecoveryDisplayFocus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RecoveryFocus {
    Input,
    Submit,
    Cancel,
}

pub struct RecoveryInputModal {
    pub key: String,
    pub focused: RecoveryFocus,
}

pub struct ModalCursor {
    pub x: u16,
    pub y: u16,
}

pub fn draw_modal(frame: &mut Frame, area: Rect, modal: &Modal) -> ModalCursor {
    match modal {
        Modal::Confirm(m) => draw_confirm(frame, area, m),
        Modal::Details(m) => draw_details(frame, area, m),
        Modal::ReactionPicker(m) => draw_reaction_picker(frame, area, m),
        Modal::ReactedBy(m) => draw_reacted_by(frame, area, m),
        Modal::RecoveryInput(m) => draw_recovery_input(frame, area, m),
        Modal::RecoveryDisplay(m) => draw_recovery_display(frame, area, m),
        Modal::SasVerification(m) => draw_sas_verification(frame, area, m),
        Modal::WindowList(m) => draw_window_list(frame, area, m),
    }
}

fn draw_window_list(frame: &mut Frame, area: Rect, m: &WindowListModal) -> ModalCursor {
    let max_w = m
        .entries
        .iter()
        .map(|e| e.label.chars().count())
        .max()
        .unwrap_or(20)
        + 8;
    let h = (m.entries.len() as u16 + 4).min(area.height);
    let popup = centered_rect(area, (max_w as u16).max(36), h.max(6));
    let inner = render_modal_frame(
        frame,
        popup,
        &ModalFrame {
            title: "Fenêtres",
            footer: Some("↑↓: choisir · Entrée: ouvrir · Esc: fermer"),
        },
    );

    let rows: Vec<ListRow> = m
        .entries
        .iter()
        .map(|e| {
            let active_marker = if e.is_active { '>' } else { ' ' };
            // " >NN<act> name" — column 4 is the first letter of the label.
            let text = format!("{}{:>2}{} {}", active_marker, e.idx + 1, e.activity, e.label);
            ListRow::new(text)
                .cursor_col(5)
                .bold(e.activity == '!')
        })
        .collect();
    let mut state = ListState {
        selected: m.selected,
        scroll_top: 0,
    };
    let (cx, cy) = render_list(frame, inner, &rows, &mut state, None);
    ModalCursor { x: cx, y: cy }
}

fn draw_confirm(frame: &mut Frame, area: Rect, m: &ConfirmModal) -> ModalCursor {
    let inner_width = m
        .message
        .chars()
        .count()
        .max(m.title.chars().count())
        .max(20) as u16
        + 4;
    let popup = centered_rect(area, inner_width.min(area.width), 7);
    let inner = render_modal_frame(
        frame,
        popup,
        &ModalFrame {
            title: &m.title,
            footer: None,
        },
    );

    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::raw(m.message.clone()),
            Line::raw(""),
        ]),
        inner,
    );

    let yes_label = "[ Oui ]";
    let no_label = "[ Non ]";
    let yes_style = if m.focused == ConfirmButton::Yes {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    let no_style = if m.focused == ConfirmButton::No {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };

    let gap = 4u16;
    let yes_w = yes_label.chars().count() as u16;
    let no_w = no_label.chars().count() as u16;
    let buttons_total = yes_w + gap + no_w;
    let buttons_y = inner.y + 3;
    let buttons_x = inner.x + inner.width.saturating_sub(buttons_total) / 2;
    let buttons_area = Rect {
        x: buttons_x,
        y: buttons_y,
        width: buttons_total.min(inner.width),
        height: 1,
    };
    let buttons_line = Line::from(vec![
        Span::styled(yes_label, yes_style),
        Span::raw(" ".repeat(gap as usize)),
        Span::styled(no_label, no_style),
    ]);
    frame.render_widget(Paragraph::new(buttons_line), buttons_area);

    let label_offset = 2u16;
    let cursor_x = match m.focused {
        ConfirmButton::Yes => buttons_x + label_offset,
        ConfirmButton::No => buttons_x + yes_w + gap + label_offset,
    };
    ModalCursor {
        x: cursor_x,
        y: buttons_y,
    }
}

fn draw_details(frame: &mut Frame, area: Rect, m: &DetailsModal) -> ModalCursor {
    let max_w = m
        .lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(20)
        .max(m.title.chars().count())
        + 4;
    let max_h = (m.lines.len() + 4).min(area.height as usize) as u16;
    let popup = centered_rect(
        area,
        (max_w as u16).min(area.width).max(30),
        max_h.max(7),
    );
    let inner = render_modal_frame(
        frame,
        popup,
        &ModalFrame {
            title: &m.title,
            footer: Some("Esc: fermer · ↑↓: défiler"),
        },
    );

    let visible: Vec<Line> = m
        .lines
        .iter()
        .skip(m.scroll)
        .take(inner.height as usize)
        .map(|l| Line::raw(l.clone()))
        .collect();
    frame.render_widget(Paragraph::new(visible), inner);

    ModalCursor {
        x: inner.x,
        y: inner.y,
    }
}

fn draw_reaction_picker(frame: &mut Frame, area: Rect, m: &ReactionPickerModal) -> ModalCursor {
    let popup = centered_rect(area, 36, (m.options.len() as u16 + 4).min(area.height));
    let inner = render_modal_frame(
        frame,
        popup,
        &ModalFrame {
            title: "Réagir",
            footer: Some("↑↓: choisir · Entrée: valider · Esc: annuler"),
        },
    );

    let rows: Vec<ListRow> = m
        .options
        .iter()
        .map(|opt| ListRow::new(format!("  {}", opt)).cursor_col(2))
        .collect();
    let mut state = ListState {
        selected: m.selected,
        scroll_top: 0,
    };
    let (cx, cy) = render_list(frame, inner, &rows, &mut state, None);
    ModalCursor { x: cx, y: cy }
}

fn draw_sas_verification(
    frame: &mut Frame,
    area: Rect,
    m: &SasVerificationModal,
) -> ModalCursor {
    let (a, b, c) = m.decimal;
    let mut content_lines: Vec<Line> = Vec::new();
    content_lines.push(Line::raw(
        "Compare ces 3 nombres avec l'autre device :",
    ));
    content_lines.push(Line::raw(""));
    content_lines.push(Line::raw(format!("    {}    {}    {}", a, b, c)));
    if !m.emoji.is_empty() {
        content_lines.push(Line::raw(""));
        content_lines.push(Line::raw("Ou via les 7 mots emoji :"));
        let words: Vec<String> = m
            .emoji
            .iter()
            .map(|(_, name)| name.clone())
            .collect();
        content_lines.push(Line::raw(format!("    {}", words.join(" · "))));
    }
    content_lines.push(Line::raw(""));
    content_lines.push(Line::raw(
        "y / o : ça correspond  ·  n : ça ne correspond pas (alerte)",
    ));

    let body_w = 70.min(area.width.saturating_sub(2));
    let popup_h = (content_lines.len() as u16 + 4).min(area.height);
    let popup = centered_rect(area, body_w.max(40), popup_h.max(10));
    let inner = render_modal_frame(
        frame,
        popup,
        &ModalFrame {
            title: "Vérification SAS",
            footer: Some("Tab: alterner · Entrée: valider · Esc: annuler"),
        },
    );

    let content_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.saturating_sub(2),
    };
    frame.render_widget(Paragraph::new(content_lines), content_area);

    let yes = "Correspond";
    let no = "Ne correspond pas";
    let yw = (yes.chars().count() + 4) as u16;
    let nw = (no.chars().count() + 4) as u16;
    let row_y = inner.y + inner.height.saturating_sub(1);
    let yes_area = Rect {
        x: inner.x,
        y: row_y,
        width: yw,
        height: 1,
    };
    let no_area = Rect {
        x: inner.x + yw + 2,
        y: row_y,
        width: nw,
        height: 1,
    };
    let (cx_y, cy_y) = render_button(
        frame,
        yes_area,
        &Button {
            label: yes,
            focused: m.focused == ConfirmButton::Yes,
        },
    );
    let (cx_n, cy_n) = render_button(
        frame,
        no_area,
        &Button {
            label: no,
            focused: m.focused == ConfirmButton::No,
        },
    );
    let (x, y) = match m.focused {
        ConfirmButton::Yes => (cx_y, cy_y),
        ConfirmButton::No => (cx_n, cy_n),
    };
    ModalCursor { x, y }
}

fn draw_recovery_display(
    frame: &mut Frame,
    area: Rect,
    m: &RecoveryDisplayModal,
) -> ModalCursor {
    let key_compact = format_key_groups_of_4(&m.key);
    let mut content_lines: Vec<Line> = Vec::new();
    content_lines.push(Line::raw(
        "Note cette clé en lieu sûr (gestionnaire de mots de passe).",
    ));
    content_lines.push(Line::raw(
        "Sans elle, tu ne pourras pas restaurer les anciens messages chiffrés.",
    ));
    content_lines.push(Line::raw(""));
    content_lines.push(Line::raw(key_compact));
    if m.show_nato {
        content_lines.push(Line::raw(""));
        content_lines.push(Line::raw("Dictée OTAN (case-sensitive) :"));
        for l in nato_lines(&m.key) {
            content_lines.push(Line::raw(l));
        }
    }

    let body_w = 70.min(area.width.saturating_sub(2));
    let popup_h = (content_lines.len() as u16 + 4).min(area.height);
    let popup = centered_rect(area, body_w.max(40), popup_h.max(8));
    let inner = render_modal_frame(
        frame,
        popup,
        &ModalFrame {
            title: "Clé de récupération E2EE",
            footer: Some("n: dictée OTAN · Tab: bouton suivant · Entrée: valider · Esc: annuler"),
        },
    );

    let content_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.saturating_sub(2),
    };
    frame.render_widget(Paragraph::new(content_lines), content_area);

    let confirm = "J'ai noté";
    let cancel = "Annuler";
    let cw = (confirm.chars().count() + 4) as u16;
    let xw = (cancel.chars().count() + 4) as u16;
    let row_y = inner.y + inner.height.saturating_sub(1);
    let confirm_area = Rect {
        x: inner.x,
        y: row_y,
        width: cw,
        height: 1,
    };
    let cancel_area = Rect {
        x: inner.x + cw + 2,
        y: row_y,
        width: xw,
        height: 1,
    };
    let (cx_c, cy_c) = render_button(
        frame,
        confirm_area,
        &Button {
            label: confirm,
            focused: m.focused == RecoveryDisplayFocus::Confirm,
        },
    );
    let (cx_x, cy_x) = render_button(
        frame,
        cancel_area,
        &Button {
            label: cancel,
            focused: m.focused == RecoveryDisplayFocus::Cancel,
        },
    );

    let (x, y) = match m.focused {
        RecoveryDisplayFocus::Confirm => (cx_c, cy_c),
        RecoveryDisplayFocus::Cancel => (cx_x, cy_x),
    };
    ModalCursor { x, y }
}

fn format_key_groups_of_4(key: &str) -> String {
    let stripped: String = key.chars().filter(|c| !c.is_whitespace()).collect();
    let mut out = String::new();
    for (i, c) in stripped.chars().enumerate() {
        if i > 0 && i % 4 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn nato_lines(key: &str) -> Vec<String> {
    let stripped: String = key.chars().filter(|c| !c.is_whitespace()).collect();
    let chars: Vec<char> = stripped.chars().collect();
    let mut out = Vec::new();
    for chunk in chars.chunks(4) {
        let words: Vec<String> = chunk
            .iter()
            .map(|&c| {
                let lower = c.to_ascii_lowercase();
                let word = nato_for(lower);
                if c.is_ascii_uppercase() {
                    format!("MAJ-{word}")
                } else {
                    word.to_string()
                }
            })
            .collect();
        out.push(format!("  {}", words.join(" · ")));
    }
    out
}

fn nato_for(c: char) -> &'static str {
    match c {
        '0' => "zéro",
        '1' => "un",
        '2' => "deux",
        '3' => "trois",
        '4' => "quatre",
        '5' => "cinq",
        '6' => "six",
        '7' => "sept",
        '8' => "huit",
        '9' => "neuf",
        'a' => "alpha",
        'b' => "bravo",
        'c' => "charlie",
        'd' => "delta",
        'e' => "echo",
        'f' => "foxtrot",
        'g' => "golf",
        'h' => "hotel",
        'i' => "india",
        'j' => "juliett",
        'k' => "kilo",
        'l' => "lima",
        'm' => "mike",
        'n' => "november",
        'o' => "oscar",
        'p' => "papa",
        'q' => "quebec",
        'r' => "romeo",
        's' => "sierra",
        't' => "tango",
        'u' => "uniform",
        'v' => "victor",
        'w' => "whiskey",
        'x' => "x-ray",
        'y' => "yankee",
        'z' => "zulu",
        _ => "?",
    }
}

fn draw_recovery_input(
    frame: &mut Frame,
    area: Rect,
    m: &RecoveryInputModal,
) -> ModalCursor {
    let popup = centered_rect(area, 70.min(area.width), 8);
    let inner = render_modal_frame(
        frame,
        popup,
        &ModalFrame {
            title: "Restaurer les clés (clé de récupération)",
            footer: Some("Tab: champ suivant · Entrée: valider · Esc: annuler"),
        },
    );

    let input_area = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: 1,
    };
    let (cx_in, cy_in) = render_text_input(
        frame,
        input_area,
        &TextInput {
            label: "Clé",
            value: &m.key,
            focused: m.focused == RecoveryFocus::Input,
            masked: false,
        },
    );

    let submit = "Restaurer";
    let cancel = "Annuler";
    let submit_w = (submit.chars().count() + 4) as u16;
    let cancel_w = (cancel.chars().count() + 4) as u16;
    let row_y = inner.y + 3;
    let submit_area = Rect {
        x: inner.x,
        y: row_y,
        width: submit_w.min(inner.width),
        height: 1,
    };
    let cancel_area = Rect {
        x: inner.x + submit_w + 2,
        y: row_y,
        width: cancel_w.min(inner.width.saturating_sub(submit_w + 2)),
        height: 1,
    };
    let (cx_s, cy_s) = render_button(
        frame,
        submit_area,
        &Button {
            label: submit,
            focused: m.focused == RecoveryFocus::Submit,
        },
    );
    let (cx_c, cy_c) = render_button(
        frame,
        cancel_area,
        &Button {
            label: cancel,
            focused: m.focused == RecoveryFocus::Cancel,
        },
    );

    let (x, y) = match m.focused {
        RecoveryFocus::Input => (cx_in, cy_in),
        RecoveryFocus::Submit => (cx_s, cy_s),
        RecoveryFocus::Cancel => (cx_c, cy_c),
    };
    ModalCursor { x, y }
}

fn draw_reacted_by(frame: &mut Frame, area: Rect, m: &ReactedByModal) -> ModalCursor {
    let max_w = m
        .entries
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(20)
        .max(m.title.chars().count())
        + 4;
    let h = (m.entries.len() as u16 + 4).min(area.height);
    let popup = centered_rect(area, (max_w as u16).max(30), h.max(5));
    let inner = render_modal_frame(
        frame,
        popup,
        &ModalFrame {
            title: &m.title,
            footer: Some("↑↓: parcourir · Esc: fermer"),
        },
    );

    if m.entries.is_empty() {
        frame.render_widget(
            Paragraph::new("Aucune réaction sur ce message."),
            inner,
        );
        return ModalCursor {
            x: inner.x,
            y: inner.y,
        };
    }

    let rows: Vec<ListRow> = m
        .entries
        .iter()
        .map(|e| ListRow::new(format!("  {}", e)).cursor_col(2))
        .collect();
    let mut state = ListState {
        selected: m.selected,
        scroll_top: 0,
    };
    let (cx, cy) = render_list(frame, inner, &rows, &mut state, None);
    ModalCursor { x: cx, y: cy }
}
