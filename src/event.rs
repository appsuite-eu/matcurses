use crate::app::{App, Focus, View};
use crate::modal::{ConfirmButton, Modal};
use crate::view::login::{self as login_view};
use crate::view::settings::{self as settings_view, SettingsState};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

pub enum EventOutcome {
    Continue,
    Quit,
}

pub fn handle_key(app: &mut App, key: KeyEvent) -> EventOutcome {
    if key.kind != KeyEventKind::Press {
        return EventOutcome::Continue;
    }
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return EventOutcome::Quit;
    }
    app.flash = None;
    if app.modal.is_some() {
        return handle_modal_key(app, key);
    }
    if app.search.active {
        return handle_search_key(app, key);
    }
    // Ctrl+N / Ctrl+P : reprendre la dernière recherche même si fermée.
    if app.is_searchable_view()
        && !app.search.query.is_empty()
        && key.modifiers.contains(KeyModifiers::CONTROL)
    {
        match key.code {
            KeyCode::Char('n') => {
                app.search_resume_and_next();
                return EventOutcome::Continue;
            }
            KeyCode::Char('p') => {
                app.search_resume_and_prev();
                return EventOutcome::Continue;
            }
            _ => {}
        }
    }
    match app.view {
        View::Conversation => match app.focus {
            Focus::Conversation => handle_conversation_key(app, key),
            Focus::Input => handle_input_key(app, key),
        },
        View::Settings => handle_settings_key(app, key),
        View::Login => handle_login_key(app, key),
        View::RoomList => handle_room_list_key(app, key),
        View::SpaceTree => handle_space_tree_key(app, key),
        View::Members => handle_members_key(app, key),
    }
}

fn handle_search_key(app: &mut App, key: KeyEvent) -> EventOutcome {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc | KeyCode::Enter => app.search_end(),
        KeyCode::Char('n') if ctrl => app.search_next(),
        KeyCode::Char('p') if ctrl => app.search_prev(),
        KeyCode::Backspace => app.search_backspace(),
        KeyCode::Char(c) if !ctrl => app.search_push(c),
        _ => {}
    }
    EventOutcome::Continue
}

fn handle_modal_key(app: &mut App, key: KeyEvent) -> EventOutcome {
    let modal = match app.modal.as_mut() {
        Some(m) => m,
        None => return EventOutcome::Continue,
    };
    match modal {
        Modal::Confirm(c) => match key.code {
            KeyCode::Esc => app.close_modal(),
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                c.focused = match c.focused {
                    ConfirmButton::Yes => ConfirmButton::No,
                    ConfirmButton::No => ConfirmButton::Yes,
                };
            }
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('o') | KeyCode::Char('O') => {
                app.confirm_modal_yes();
            }
            KeyCode::Char('n') | KeyCode::Char('N') => app.close_modal(),
            KeyCode::Enter => match c.focused {
                ConfirmButton::Yes => app.confirm_modal_yes(),
                ConfirmButton::No => app.close_modal(),
            },
            _ => {}
        },
        Modal::Details(d) => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.close_modal(),
            KeyCode::Up => d.scroll = d.scroll.saturating_sub(1),
            KeyCode::Down => {
                d.scroll = (d.scroll + 1).min(d.lines.len().saturating_sub(1));
            }
            KeyCode::PageUp => d.scroll = d.scroll.saturating_sub(5),
            KeyCode::PageDown => {
                d.scroll = (d.scroll + 5).min(d.lines.len().saturating_sub(1));
            }
            KeyCode::Home => d.scroll = 0,
            KeyCode::End => d.scroll = d.lines.len().saturating_sub(1),
            _ => {}
        },
        Modal::ReactionPicker(p) => match key.code {
            KeyCode::Esc => app.close_modal(),
            KeyCode::Up => p.selected = p.selected.saturating_sub(1),
            KeyCode::Down => {
                p.selected = (p.selected + 1).min(p.options.len().saturating_sub(1));
            }
            KeyCode::Home => p.selected = 0,
            KeyCode::End => p.selected = p.options.len().saturating_sub(1),
            KeyCode::Enter => app.pick_reaction(),
            _ => {}
        },
        Modal::ReactedBy(r) => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.close_modal(),
            KeyCode::Up => r.selected = r.selected.saturating_sub(1),
            KeyCode::Down => {
                r.selected = (r.selected + 1).min(r.entries.len().saturating_sub(1));
            }
            _ => {}
        },
    }
    EventOutcome::Continue
}

fn handle_conversation_key(app: &mut App, key: KeyEvent) -> EventOutcome {
    let page = (app.last_main_height as usize).max(1);
    if key.code == KeyCode::Char('l') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.open_login();
        return EventOutcome::Continue;
    }
    match key.code {
        KeyCode::Tab => app.toggle_focus(),
        KeyCode::Char('i') => app.set_focus(Focus::Input),
        KeyCode::Enter => app.set_focus(Focus::Input),
        KeyCode::Char('q') => app.open_quit_confirm(),
        KeyCode::Char('d') => app.open_details(),
        KeyCode::Char('D') => app.open_redact_confirm(),
        KeyCode::Char('r') => app.open_reaction_picker(),
        KeyCode::Char('R') => app.open_reacted_by(),
        KeyCode::Char('v') => app.play_current_voice(),
        KeyCode::Char('/') => app.search_start(),
        KeyCode::Char('?') => app.search_start_backward(),
        KeyCode::Char(',') => app.open_settings(),
        KeyCode::F(3) => app.open_spaces(),
        KeyCode::F(4) => app.open_rooms(),
        KeyCode::F(5) => app.open_members(),
        KeyCode::Up => app.select_prev(1),
        KeyCode::Down => app.select_next(1),
        KeyCode::PageUp => app.select_prev(page),
        KeyCode::PageDown => app.select_next(page),
        KeyCode::Home => app.select_first(),
        KeyCode::End => app.select_last(),
        KeyCode::Char('g') => app.select_first(),
        KeyCode::Char('G') => app.select_last(),
        KeyCode::Right | KeyCode::Char('+') => app.open_thread(),
        KeyCode::Left | KeyCode::Char('-') => app.close_thread(),
        _ => {}
    }
    EventOutcome::Continue
}

fn handle_room_list_key(app: &mut App, key: KeyEvent) -> EventOutcome {
    if key.code == KeyCode::Char('/') {
        app.search_start();
        return EventOutcome::Continue;
    }
    if key.code == KeyCode::Char('?') {
        app.search_start_backward();
        return EventOutcome::Continue;
    }
    if key.code == KeyCode::Esc {
        app.back_to_conversation();
        return EventOutcome::Continue;
    }
    if key.code == KeyCode::Enter {
        if let Some(name) = app.room_list_state.selected_room_name() {
            app.switch_room(&name);
        }
        return EventOutcome::Continue;
    }
    let s = &mut app.room_list_state;
    match key.code {
        KeyCode::Up => s.prev(1),
        KeyCode::Down => s.next(1),
        KeyCode::PageUp => s.prev(10),
        KeyCode::PageDown => s.next(10),
        KeyCode::Home | KeyCode::Char('g') => s.first(),
        KeyCode::End | KeyCode::Char('G') => s.last(),
        _ => {}
    }
    EventOutcome::Continue
}

fn handle_space_tree_key(app: &mut App, key: KeyEvent) -> EventOutcome {
    use crate::view::space_tree::Action;
    if key.code == KeyCode::Char('/') {
        app.search_start();
        return EventOutcome::Continue;
    }
    if key.code == KeyCode::Char('?') {
        app.search_start_backward();
        return EventOutcome::Continue;
    }
    if key.code == KeyCode::Esc {
        app.back_to_conversation();
        return EventOutcome::Continue;
    }
    if matches!(
        key.code,
        KeyCode::Right | KeyCode::Char('+') | KeyCode::Enter
    ) {
        if let Action::OpenRoom(name) = app.space_tree_state.open() {
            app.switch_room(&name);
        }
        return EventOutcome::Continue;
    }
    let s = &mut app.space_tree_state;
    match key.code {
        KeyCode::Up => s.prev(1),
        KeyCode::Down => s.next(1),
        KeyCode::PageUp => s.prev(10),
        KeyCode::PageDown => s.next(10),
        KeyCode::Home | KeyCode::Char('g') => s.first(),
        KeyCode::End | KeyCode::Char('G') => s.last(),
        KeyCode::Left | KeyCode::Char('-') => s.close(),
        _ => {}
    }
    EventOutcome::Continue
}

fn handle_members_key(app: &mut App, key: KeyEvent) -> EventOutcome {
    if key.code == KeyCode::Char('/') {
        app.search_start();
        return EventOutcome::Continue;
    }
    if key.code == KeyCode::Char('?') {
        app.search_start_backward();
        return EventOutcome::Continue;
    }
    if key.code == KeyCode::Esc {
        app.back_to_conversation();
        return EventOutcome::Continue;
    }
    if key.code == KeyCode::Enter {
        app.open_member_details();
        return EventOutcome::Continue;
    }
    let s = &mut app.members_state;
    match key.code {
        KeyCode::Up => s.prev(1),
        KeyCode::Down => s.next(1),
        KeyCode::PageUp => s.prev(10),
        KeyCode::PageDown => s.next(10),
        KeyCode::Home | KeyCode::Char('g') => s.first(),
        KeyCode::End | KeyCode::Char('G') => s.last(),
        _ => {}
    }
    EventOutcome::Continue
}

fn handle_input_key(app: &mut App, key: KeyEvent) -> EventOutcome {
    match key.code {
        KeyCode::Esc => app.set_focus(Focus::Conversation),
        KeyCode::Char(c) => app.input.push(c),
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Enter => {
            app.submit_input();
        }
        _ => {}
    }
    EventOutcome::Continue
}

fn handle_settings_key(app: &mut App, key: KeyEvent) -> EventOutcome {
    let s = &mut app.settings_state;
    match key.code {
        KeyCode::Esc => app.back_to_conversation(),
        KeyCode::Tab | KeyCode::Down => s.next(),
        KeyCode::BackTab | KeyCode::Up => s.prev(),
        KeyCode::Char(' ') => toggle_settings_field(s),
        KeyCode::Left | KeyCode::Right => cycle_settings_radio(s),
        KeyCode::Enter => match s.focus_idx {
            settings_view::F_DOC => {
                app.flash = Some("ouverture documentation (mock)".into());
            }
            settings_view::F_SAVE => {
                app.flash = Some("paramètres enregistrés (mock)".into());
                app.back_to_conversation();
            }
            settings_view::F_CANCEL => {
                app.back_to_conversation();
            }
            _ => toggle_settings_field(s),
        },
        _ => {}
    }
    EventOutcome::Continue
}

fn toggle_settings_field(s: &mut SettingsState) {
    match s.focus_idx {
        settings_view::F_TTS => s.tts = !s.tts,
        settings_view::F_NATO => s.nato = !s.nato,
        settings_view::F_SAS => s.sas_decimal = !s.sas_decimal,
        settings_view::F_VOICE => s.voice_toggle = !s.voice_toggle,
        _ => {}
    }
}

fn cycle_settings_radio(s: &mut SettingsState) {
    match s.focus_idx {
        settings_view::F_SAS => s.sas_decimal = !s.sas_decimal,
        settings_view::F_VOICE => s.voice_toggle = !s.voice_toggle,
        _ => {}
    }
}

fn handle_login_key(app: &mut App, key: KeyEvent) -> EventOutcome {
    let s = &mut app.login_state;
    match key.code {
        KeyCode::Esc => app.back_to_conversation(),
        KeyCode::Tab | KeyCode::Down => s.next(),
        KeyCode::BackTab | KeyCode::Up => s.prev(),
        KeyCode::Char(c) => {
            if let Some(field) = s.focused_text() {
                field.push(c);
            } else {
                handle_login_button(app, key);
            }
        }
        KeyCode::Backspace => {
            if let Some(field) = s.focused_text() {
                field.pop();
            }
        }
        KeyCode::Enter => match s.focus_idx {
            login_view::F_CONNECT => {
                app.submit_login();
            }
            login_view::F_CANCEL => {
                app.back_to_conversation();
            }
            _ => s.next(),
        },
        _ => {}
    }
    EventOutcome::Continue
}

fn handle_login_button(_app: &mut App, _key: KeyEvent) {
    // Placeholder: les boutons réagissent à Enter dans l'arm Enter du match.
}
