use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
    Frame,
};

pub struct Button<'a> {
    pub label: &'a str,
    pub focused: bool,
}

pub fn render_button(frame: &mut Frame, area: Rect, b: &Button) -> (u16, u16) {
    let text = format!("[ {} ]", b.label);
    let style = if b.focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    frame.render_widget(Paragraph::new(text).style(style), area);
    // Curseur sur la première lettre du label, après "[ "
    (area.x + 2, area.y)
}

#[allow(dead_code)]
pub fn button_width(label: &str) -> u16 {
    (label.chars().count() + 4) as u16
}
