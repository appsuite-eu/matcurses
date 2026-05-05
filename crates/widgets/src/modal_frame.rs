use ratatui::{
    layout::{Margin, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Returns a `Rect` of size `width × height` centered inside `area`, clamped
/// to fit if any dimension exceeds `area`.
pub fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

pub struct ModalFrame<'a> {
    pub title: &'a str,
    pub footer: Option<&'a str>,
}

/// Renders a popup chrome (background clear, border, title, optional footer
/// hint line) and returns the inner `Rect` available for content.
pub fn render_modal_frame(frame: &mut Frame, popup: Rect, mf: &ModalFrame) -> Rect {
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(format!(" {} ", mf.title))
        .borders(Borders::ALL);
    frame.render_widget(block, popup);

    let inner = popup.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });

    if let Some(footer) = mf.footer {
        let footer_area = Rect {
            x: popup.x + 1,
            y: popup.y + popup.height.saturating_sub(1),
            width: popup.width.saturating_sub(2),
            height: 1,
        };
        let line = Line::from(format!(" {} ", footer));
        let p = Paragraph::new(line).style(Style::default().add_modifier(Modifier::DIM));
        frame.render_widget(p, footer_area);
    }

    inner
}
