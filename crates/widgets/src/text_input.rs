use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
    Frame,
};

pub struct TextInput<'a> {
    pub label: &'a str,
    pub value: &'a str,
    pub focused: bool,
    pub masked: bool,
}

pub fn render_text_input(frame: &mut Frame, area: Rect, t: &TextInput) -> (u16, u16) {
    let displayed: String = if t.masked {
        "*".repeat(t.value.chars().count())
    } else {
        t.value.to_string()
    };
    let label_part = format!("{}: ", t.label);
    let label_len = label_part.chars().count() as u16;
    let text = format!("{}{}", label_part, displayed);
    let style = if t.focused {
        Style::default().add_modifier(Modifier::UNDERLINED)
    } else {
        Style::default().add_modifier(Modifier::DIM)
    };
    frame.render_widget(Paragraph::new(text).style(style), area);
    // Curseur juste après la valeur saisie (en fin de saisie)
    let cursor_x = area.x + label_len + displayed.chars().count() as u16;
    let max_x = area.right().saturating_sub(1);
    (cursor_x.min(max_x), area.y)
}
