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

/// Maximum number of lines the chat input bar can grow to before it
/// scrolls vertically inside its own area.
const INPUT_MAX_LINES: u16 = 5;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let input_height = input_visible_height(app);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // top status: room title + flash
            Constraint::Min(1),    // main view
            Constraint::Length(1), // window status (irssi-style)
            Constraint::Length(input_height),
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

    draw_window_status(frame, chunks[2], app);
    draw_input_bar(frame, chunks[3], app);

    if let Some(modal) = &app.modal {
        let cursor = draw_modal(frame, frame.area(), modal);
        frame.set_cursor_position((cursor.x, cursor.y));
    } else {
        match app.view {
            View::Conversation => match app.focus {
                Focus::Input => place_input_cursor(frame, chunks[3], app),
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

/// Irssi-style status line between the conversation area and the input
/// bar. Shows the active window number, any non-active windows that
/// gained activity since the user last looked at them (with a `*`
/// prefix on the ones that mention the user), the local time, and a
/// connection state glyph.
fn draw_window_status(frame: &mut Frame, area: Rect, app: &App) {
    use crate::app::ActivityLevel;
    let active = format!("[{}]", app.active_window + 1);

    let mut act: Vec<String> = Vec::new();
    let mut mention: Vec<String> = Vec::new();
    for (i, w) in app.windows.iter().enumerate() {
        if i == app.active_window {
            continue;
        }
        match w.activity {
            ActivityLevel::None => {}
            ActivityLevel::Active => act.push(format!("{}", i + 1)),
            ActivityLevel::Mention => mention.push(format!("{}", i + 1)),
        }
    }

    let mut left_parts = vec![active];
    if !act.is_empty() {
        left_parts.push(format!("[Act: {}]", act.join(",")));
    }
    if !mention.is_empty() {
        left_parts.push(format!("[Mention: {}]", mention.join(",")));
    }
    let left = left_parts.join(" ");

    let now = chrono::Local::now().format("%H:%M").to_string();
    let conn = if app.matrix_logged_in { "●" } else { "○" };
    let right = format!("{} {}", now, conn);

    widgets::render_status_bar(
        frame,
        area,
        &widgets::StatusBar {
            left: &left,
            right: &right,
        },
    );
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
    let style = if app.focus == Focus::Input {
        Style::default()
    } else {
        Style::default().add_modifier(Modifier::DIM)
    };

    let lines: Vec<&str> = app.input.split('\n').collect();
    let (cursor_line, cursor_col) = cursor_line_col(&app.input, app.input_cursor);
    let height = area.height as usize;
    let v_scroll = vertical_scroll(cursor_line, height, lines.len());

    for row in 0..height {
        let line_idx = v_scroll + row;
        let line_text = lines.get(line_idx).copied().unwrap_or("");
        let prefix_str = if line_idx == 0 {
            format!("{} ", prefix)
        } else {
            "  ".to_string()
        };
        let prefix_cells = prefix_str.chars().count();
        // Reserve one cell at the end for the cursor itself.
        let avail = (area.width as usize)
            .saturating_sub(prefix_cells)
            .saturating_sub(1);
        let h_scroll = if line_idx == cursor_line {
            horizontal_scroll(cursor_col, avail)
        } else {
            0
        };
        let visible: String = line_text.chars().skip(h_scroll).take(avail).collect();
        let row_area = Rect {
            x: area.x,
            y: area.y + row as u16,
            width: area.width,
            height: 1,
        };
        let display = format!("{}{}", prefix_str, visible);
        frame.render_widget(Paragraph::new(display).style(style), row_area);
    }
}

fn place_input_cursor(frame: &mut Frame, area: Rect, app: &App) {
    let lines: Vec<&str> = app.input.split('\n').collect();
    let (cursor_line, cursor_col) = cursor_line_col(&app.input, app.input_cursor);
    let height = area.height as usize;
    let v_scroll = vertical_scroll(cursor_line, height, lines.len());
    let row_in_view = cursor_line.saturating_sub(v_scroll);
    let prefix_cells = if cursor_line == 0 {
        app.input_mode.prefix().chars().count() + 1
    } else {
        2
    };
    let avail = (area.width as usize)
        .saturating_sub(prefix_cells)
        .saturating_sub(1);
    let h_scroll = horizontal_scroll(cursor_col, avail);
    let visible_col = cursor_col.saturating_sub(h_scroll);
    let x = area.x + prefix_cells as u16 + visible_col as u16;
    let y = area.y + row_in_view.min(height.saturating_sub(1)) as u16;
    let max_x = area.right().saturating_sub(1);
    frame.set_cursor_position((x.min(max_x), y));
}

fn input_visible_height(app: &App) -> u16 {
    let n = app.input.split('\n').count() as u16;
    n.max(1).min(INPUT_MAX_LINES)
}

fn cursor_line_col(input: &str, cursor: usize) -> (usize, usize) {
    let mut line = 0usize;
    let mut col = 0usize;
    for (i, c) in input.chars().enumerate() {
        if i == cursor {
            return (line, col);
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn horizontal_scroll(cursor_col: usize, avail: usize) -> usize {
    if avail == 0 {
        return cursor_col;
    }
    if cursor_col < avail {
        0
    } else {
        cursor_col - avail
    }
}

fn vertical_scroll(cursor_line: usize, height: usize, total: usize) -> usize {
    if height == 0 {
        return cursor_line;
    }
    if total <= height {
        return 0;
    }
    if cursor_line < height {
        0
    } else {
        cursor_line + 1 - height
    }
}

fn search_query_owned(app: &App) -> Option<String> {
    if app.search.active && !app.search.query.is_empty() {
        Some(app.search.query.clone())
    } else {
        None
    }
}
