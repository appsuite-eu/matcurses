use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
    Frame,
};

use crate::button::{render_button, Button};
use crate::checkbox::{render_checkbox, Checkbox};
use crate::link::{render_link, Link};
use crate::radio::{render_radio, Radio};
use crate::text_input::{render_text_input, TextInput};

pub enum FormField<'a> {
    Text {
        label: &'a str,
        value: &'a str,
        masked: bool,
    },
    Checkbox {
        label: &'a str,
        checked: bool,
    },
    /// Group of radio buttons. `selected` is the option currently chosen.
    /// When the group is focused, the cursor lands between the parens of
    /// the selected option.
    Radio {
        label: &'a str,
        options: &'a [&'a str],
        selected: usize,
    },
    Button {
        label: &'a str,
    },
    Link {
        label: &'a str,
    },
    /// Section header (non-focusable).
    Header(&'a str),
    /// One blank line (non-focusable).
    Spacer,
    /// Two buttons side by side (e.g., OK / Cancel). Counts as TWO focusable
    /// fields: focus index N for the first, N+1 for the second.
    Buttons2 {
        first: &'a str,
        second: &'a str,
    },
}

impl<'a> FormField<'a> {
    pub fn focusable_count(&self) -> usize {
        match self {
            FormField::Header(_) | FormField::Spacer => 0,
            FormField::Buttons2 { .. } => 2,
            _ => 1,
        }
    }

    pub fn height(&self) -> u16 {
        match self {
            FormField::Radio { options, .. } => 1 + options.len() as u16,
            _ => 1,
        }
    }
}

pub fn focusable_count(fields: &[FormField]) -> usize {
    fields.iter().map(|f| f.focusable_count()).sum()
}

/// Renders a vertical stack of form fields starting at `area`. Returns the
/// cursor position of the focused element, computed via per-widget
/// conventions.
pub fn render_form(
    frame: &mut Frame,
    area: Rect,
    fields: &[FormField],
    focus_idx: usize,
) -> (u16, u16) {
    let mut y = area.y;
    let mut focusable_seen = 0usize;
    let mut cursor = (area.x, area.y);

    for field in fields {
        if y >= area.y + area.height {
            break;
        }
        let row_w = area.width;

        match field {
            FormField::Text {
                label,
                value,
                masked,
            } => {
                let focused = focusable_seen == focus_idx;
                let row = rect(area.x, y, row_w, 1);
                let pos = render_text_input(
                    frame,
                    row,
                    &TextInput {
                        label,
                        value,
                        focused,
                        masked: *masked,
                    },
                );
                if focused {
                    cursor = pos;
                }
                focusable_seen += 1;
            }
            FormField::Checkbox { label, checked } => {
                let focused = focusable_seen == focus_idx;
                let row = rect(area.x, y, row_w, 1);
                let pos = render_checkbox(
                    frame,
                    row,
                    &Checkbox {
                        label,
                        checked: *checked,
                        focused,
                    },
                );
                if focused {
                    cursor = pos;
                }
                focusable_seen += 1;
            }
            FormField::Radio {
                label,
                options,
                selected,
            } => {
                let group_focused = focusable_seen == focus_idx;
                let header_area = rect(area.x, y, row_w, 1);
                frame.render_widget(Paragraph::new(format!("{} :", label)), header_area);
                for (i, opt) in options.iter().enumerate() {
                    let opt_y = y + 1 + i as u16;
                    let opt_area = rect(area.x + 4, opt_y, row_w.saturating_sub(4), 1);
                    let opt_focused = group_focused && *selected == i;
                    let pos = render_radio(
                        frame,
                        opt_area,
                        &Radio {
                            label: opt,
                            selected: *selected == i,
                            focused: opt_focused,
                        },
                    );
                    if opt_focused {
                        cursor = pos;
                    }
                }
                focusable_seen += 1;
            }
            FormField::Button { label } => {
                let focused = focusable_seen == focus_idx;
                let row = rect(area.x, y, row_w, 1);
                let pos = render_button(frame, row, &Button { label, focused });
                if focused {
                    cursor = pos;
                }
                focusable_seen += 1;
            }
            FormField::Link { label } => {
                let focused = focusable_seen == focus_idx;
                let row = rect(area.x, y, row_w, 1);
                let pos = render_link(frame, row, &Link { label, focused });
                if focused {
                    cursor = pos;
                }
                focusable_seen += 1;
            }
            FormField::Header(text) => {
                let row = rect(area.x, y, row_w, 1);
                frame.render_widget(
                    Paragraph::new(*text).style(Style::default().add_modifier(Modifier::BOLD)),
                    row,
                );
            }
            FormField::Spacer => {}
            FormField::Buttons2 { first, second } => {
                let first_focused = focusable_seen == focus_idx;
                let second_focused = focusable_seen + 1 == focus_idx;
                let first_w = (first.chars().count() + 4) as u16;
                let second_w = (second.chars().count() + 4) as u16;
                let first_area = rect(area.x, y, first_w, 1);
                let second_area = rect(area.x + first_w + 2, y, second_w, 1);
                let p1 = render_button(
                    frame,
                    first_area,
                    &Button {
                        label: first,
                        focused: first_focused,
                    },
                );
                let p2 = render_button(
                    frame,
                    second_area,
                    &Button {
                        label: second,
                        focused: second_focused,
                    },
                );
                if first_focused {
                    cursor = p1;
                }
                if second_focused {
                    cursor = p2;
                }
                focusable_seen += 2;
            }
        }

        y += field.height();
    }

    cursor
}

fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}
