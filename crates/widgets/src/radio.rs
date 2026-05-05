use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
    Frame,
};

pub struct Radio<'a> {
    pub label: &'a str,
    pub selected: bool,
    pub focused: bool,
}

pub fn render_radio(frame: &mut Frame, area: Rect, r: &Radio) -> (u16, u16) {
    let mark = if r.selected { "x" } else { " " };
    let text = format!("({}) {}", mark, r.label);
    let style = if r.focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    frame.render_widget(Paragraph::new(text).style(style), area);
    // Curseur entre les parenthèses : juste après "("
    (area.x + 1, area.y)
}
