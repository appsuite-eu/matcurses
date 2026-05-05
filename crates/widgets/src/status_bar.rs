use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
    Frame,
};

pub struct StatusBar<'a> {
    pub left: &'a str,
    pub right: &'a str,
}

/// Renders a 1-line status bar with `left` left-aligned, `right`
/// right-aligned, padded between with spaces. Reverses video for visibility.
pub fn render_status_bar(frame: &mut Frame, area: Rect, sb: &StatusBar) {
    let left = format!(" {} ", sb.left);
    let right = format!(" {} ", sb.right);
    let pad = (area.width as usize)
        .saturating_sub(left.chars().count() + right.chars().count());
    let line = format!("{}{}{}", left, " ".repeat(pad), right);
    let p = Paragraph::new(line).style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_widget(p, area);
}
