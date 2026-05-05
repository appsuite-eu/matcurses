use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
    Frame,
};

pub struct Checkbox<'a> {
    pub label: &'a str,
    pub checked: bool,
    pub focused: bool,
}

pub fn render_checkbox(frame: &mut Frame, area: Rect, c: &Checkbox) -> (u16, u16) {
    let mark = if c.checked { "x" } else { " " };
    let text = format!("[{}] {}", mark, c.label);
    let style = if c.focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    frame.render_widget(Paragraph::new(text).style(style), area);
    // Curseur entre les crochets : juste après "["
    (area.x + 1, area.y)
}
