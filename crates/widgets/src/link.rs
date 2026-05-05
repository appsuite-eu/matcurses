use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
    Frame,
};

pub struct Link<'a> {
    pub label: &'a str,
    pub focused: bool,
}

pub fn render_link(frame: &mut Frame, area: Rect, l: &Link) -> (u16, u16) {
    let style = if l.focused {
        Style::default()
            .add_modifier(Modifier::UNDERLINED)
            .add_modifier(Modifier::REVERSED)
    } else {
        Style::default().add_modifier(Modifier::UNDERLINED)
    };
    frame.render_widget(Paragraph::new(l.label).style(style), area);
    // Curseur sur le premier caractère du texte cliquable
    (area.x, area.y)
}
