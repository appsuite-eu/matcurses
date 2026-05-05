// Conventions du curseur logique : voir le crate `widgets`.
// Les modales utilisent ces conventions pour placer le curseur sur l'élément
// focusé et propager la position via `ModalCursor`.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use widgets::{
    centered_rect, render_list, render_modal_frame, ListRow, ListState, ModalFrame,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    Quit,
    Redact(usize),
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
    }
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
