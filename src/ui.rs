use crate::app::{App, Focus, View};
use crate::message::wrap_view;
use crate::modal::draw_modal;
use crate::view::{login, members, room_list, settings, space_tree};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    widgets::{Clear, Paragraph},
    Frame,
};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let main = chunks[1];
    app.last_main_height = main.height;

    draw_status_bar(frame, chunks[0], app);

    // Force-clear the main area each frame. Without this, cells that no
    // longer carry content (e.g., a list with fewer rows than before, or a
    // form with non-rectangular field placement) keep leftover glyphs from
    // earlier frames in some terminals.
    frame.render_widget(Clear, main);

    let mut conv_cursor: Option<(u16, u16)> = None;
    let mut other_cursor: Option<(u16, u16)> = None;

    match app.view {
        View::Conversation => {
            let items = app.visible_items();
            let wrapped = wrap_view(
                &app.messages,
                &items,
                &app.expanded_threads,
                main.width,
                main.height as usize,
            );
            let q = search_query_owned(app);
            let pos = widgets::render_wrapped_list(
                frame,
                main,
                &wrapped,
                app.selected,
                &mut app.scroll_top,
                q.as_deref(),
            );
            conv_cursor = Some(pos);
        }
        View::Settings => {
            let pos = settings::render(frame, main, &app.settings_state);
            other_cursor = Some(pos);
        }
        View::Login => {
            let pos = login::render(frame, main, &app.login_state);
            other_cursor = Some(pos);
        }
        View::RoomList => {
            let q = search_query_owned(app);
            let pos = room_list::render(frame, main, &mut app.room_list_state, q.as_deref());
            other_cursor = Some(pos);
        }
        View::SpaceTree => {
            let q = search_query_owned(app);
            let pos = space_tree::render(frame, main, &mut app.space_tree_state, q.as_deref());
            other_cursor = Some(pos);
        }
        View::Members => {
            let q = search_query_owned(app);
            let pos = members::render(frame, main, &mut app.members_state, q.as_deref());
            other_cursor = Some(pos);
        }
    }

    draw_input_bar(frame, chunks[2], app);

    if let Some(modal) = &app.modal {
        let cursor = draw_modal(frame, frame.area(), modal);
        frame.set_cursor_position((cursor.x, cursor.y));
    } else {
        match app.view {
            View::Conversation => match app.focus {
                Focus::Input => place_input_cursor(frame, chunks[2], app),
                Focus::Conversation => {
                    if let Some((x, y)) = conv_cursor {
                        frame.set_cursor_position((x, y));
                    } else {
                        frame.set_cursor_position((main.x, main.y));
                    }
                }
            },
            _ => {
                if let Some((x, y)) = other_cursor {
                    frame.set_cursor_position((x, y));
                }
            }
        }
    }
}

fn draw_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    widgets::render_status_bar(
        frame,
        area,
        &widgets::StatusBar {
            left: &app.current_room,
            right: &app.status_text,
        },
    );
}

fn draw_input_bar(frame: &mut Frame, area: Rect, app: &App) {
    // Clear the row first so a shorter flash / input does not leave behind
    // characters from a longer previous render.
    frame.render_widget(Clear, area);
    if app.search.active {
        let n = app.search.matches.len();
        let pos_label = if app.search.query.is_empty() {
            String::new()
        } else if n == 0 {
            " (aucun)".to_string()
        } else {
            format!(" ({}/{})", app.search.match_pos + 1, n)
        };
        let line = format!(
            "/{}{}  · Ctrl+N: suivant · Ctrl+P: précédent · Esc: fin",
            app.search.query, pos_label
        );
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    if let Some(flash) = &app.flash {
        let p = Paragraph::new(flash.clone())
            .style(Style::default().add_modifier(Modifier::REVERSED));
        frame.render_widget(p, area);
        return;
    }
    if !matches!(app.view, View::Conversation) {
        return;
    }
    let prefix = app.input_mode.prefix();
    let prefix_cells = prefix.chars().count() + 1; // prefix + separator space
    let avail = (area.width as usize).saturating_sub(prefix_cells);
    let total = app.input.chars().count();
    // Horizontal scroll: when the typed input is longer than the visible
    // area, show its tail so the cursor stays on the last character. Skip
    // is in chars (not bytes) to handle UTF-8 cleanly.
    let skip = total.saturating_sub(avail);
    let visible: String = app.input.chars().skip(skip).collect();
    let line = format!("{} {}", prefix, visible);
    let style = if app.focus == Focus::Input {
        Style::default()
    } else {
        Style::default().add_modifier(Modifier::DIM)
    };
    frame.render_widget(Paragraph::new(line).style(style), area);
}

fn place_input_cursor(frame: &mut Frame, area: Rect, app: &App) {
    let prefix_cells = app.input_mode.prefix().chars().count() as u16 + 1;
    let avail = area.width.saturating_sub(prefix_cells);
    let total = app.input.chars().count() as u16;
    let visible_len = total.min(avail);
    let x = area.x + prefix_cells + visible_len;
    let max_x = area.right().saturating_sub(1);
    frame.set_cursor_position((x.min(max_x), area.y));
}

fn search_query_owned(app: &App) -> Option<String> {
    if app.search.active && !app.search.query.is_empty() {
        Some(app.search.query.clone())
    } else {
        None
    }
}
