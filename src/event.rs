use crate::app::{App, Focus, View};
use crate::modal::{ConfirmButton, Modal, RecoveryDisplayFocus, RecoveryFocus};
use crate::view::login::{self as login_view};
use crate::view::settings::{self as settings_view, SettingsState};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

pub enum EventOutcome {
    Continue,
    Quit,
    /// Suspend the TUI and open the given content in `$EDITOR` for
    /// leisurely reading. Resumes the TUI when the editor exits.
    OpenEditor(String),
    /// Suspend the TUI and open the given content in `$EDITOR` for
    /// editing. The edited content replaces `app.input` on return.
    EditInput(String),
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
    // Ctrl+N / Ctrl+P: resume the last search even if closed.
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
    // Alt + digit / Alt+n / Alt+p: window navigation. Works in any view
    // since a window switch always lands in the conversation view.
    if key.modifiers.contains(KeyModifiers::ALT) {
        match key.code {
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                let idx = (c as u8 - b'1') as usize;
                if idx < app.windows.len() {
                    app.switch_window(idx);
                    app.view = crate::app::View::Conversation;
                }
                return EventOutcome::Continue;
            }
            KeyCode::Char('n') => {
                app.next_window();
                app.view = crate::app::View::Conversation;
                return EventOutcome::Continue;
            }
            KeyCode::Char('p') => {
                app.prev_window();
                app.view = crate::app::View::Conversation;
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
        Modal::WindowList(w) => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.close_modal(),
            KeyCode::Up => w.selected = w.selected.saturating_sub(1),
            KeyCode::Down => {
                w.selected = (w.selected + 1).min(w.entries.len().saturating_sub(1));
            }
            KeyCode::Home | KeyCode::Char('g') => w.selected = 0,
            KeyCode::End | KeyCode::Char('G') => {
                w.selected = w.entries.len().saturating_sub(1);
            }
            KeyCode::Enter => app.pick_window_from_list(),
            _ => {}
        },
        Modal::SasVerification(s) => match key.code {
            KeyCode::Esc => app.sas_cancel(),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                s.focused = match s.focused {
                    ConfirmButton::Yes => ConfirmButton::No,
                    ConfirmButton::No => ConfirmButton::Yes,
                };
            }
            KeyCode::Char('y') | KeyCode::Char('o') => app.sas_confirm(),
            KeyCode::Char('n') => app.sas_mismatch(),
            KeyCode::Enter => match s.focused {
                ConfirmButton::Yes => app.sas_confirm(),
                ConfirmButton::No => app.sas_mismatch(),
            },
            _ => {}
        },
        Modal::RecoveryDisplay(d) => match key.code {
            KeyCode::Esc => app.close_modal(),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                d.focused = match d.focused {
                    RecoveryDisplayFocus::Confirm => RecoveryDisplayFocus::Cancel,
                    RecoveryDisplayFocus::Cancel => RecoveryDisplayFocus::Confirm,
                };
            }
            KeyCode::Char('n') => d.show_nato = !d.show_nato,
            KeyCode::Char('c') => app.copy_recovery_to_clipboard(),
            KeyCode::Enter => match d.focused {
                RecoveryDisplayFocus::Confirm => app.confirm_recovery_displayed(),
                RecoveryDisplayFocus::Cancel => {
                    app.close_modal();
                    app.flash =
                        Some("clé non confirmée — note-la AVANT toute restauration".into());
                }
            },
            _ => {}
        },
        Modal::RecoveryInput(r) => match key.code {
            KeyCode::Esc => app.close_modal(),
            KeyCode::Tab | KeyCode::BackTab => {
                r.focused = match r.focused {
                    RecoveryFocus::Input => RecoveryFocus::Submit,
                    RecoveryFocus::Submit => RecoveryFocus::Cancel,
                    RecoveryFocus::Cancel => RecoveryFocus::Input,
                };
            }
            KeyCode::Enter => match r.focused {
                RecoveryFocus::Input => r.focused = RecoveryFocus::Submit,
                RecoveryFocus::Submit => app.submit_recovery_input(),
                RecoveryFocus::Cancel => app.close_modal(),
            },
            KeyCode::Char(c) if r.focused == RecoveryFocus::Input => {
                r.key.push(c);
            }
            KeyCode::Backspace if r.focused == RecoveryFocus::Input => {
                r.key.pop();
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
        // `r` = reply to selected message. `R` (Shift+r) = reaction picker.
        // The "who reacted" view is reachable from the details popup (`d`).
        KeyCode::Char('r') => app.start_reply(),
        KeyCode::Char('R') => app.open_reaction_picker(),
        // `t` = create a new thread (or post into the existing thread of
        // the selected message).
        KeyCode::Char('t') => app.start_thread(),
        KeyCode::Char('v') => app.play_current_voice(),
        KeyCode::Char('V') => app.stop_voice(),
        KeyCode::Char('e') => return EventOutcome::OpenEditor(app.current_message_text()),
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
        // Window switching from the conversation focus.
        KeyCode::Char('<') => app.prev_window(),
        KeyCode::Char('>') => app.next_window(),
        // Date navigation.
        KeyCode::Char('[') => app.select_prev_date(),
        KeyCode::Char(']') => app.select_next_date(),
        // Next unread message in the current room.
        KeyCode::Char('u') => app.select_next_unread(),
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
        match app.space_tree_state.open() {
            Action::OpenRoom(name) => app.switch_room(&name),
            Action::None => {
                // The user expanded a space. Trigger a LoadSpaces refresh so
                // children populated by recent sync iterations show up.
                if app.matrix_logged_in {
                    if let Some(b) = &app.matrix {
                        b.send(crate::matrix::Command::LoadSpaces);
                    }
                }
            }
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
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // Drop the in-flight completion as soon as the user does anything
    // other than cycling through it.
    if !matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
        app.pending_completion = None;
    }
    match key.code {
        KeyCode::Esc => {
            // Drop any in-flight reply / thread target so the user is
            // back to a clean top-level compose next time.
            app.clear_compose_target();
            app.set_focus(Focus::Conversation);
        }
        // F-keys for view switches still work while typing.
        KeyCode::F(3) => app.open_spaces(),
        KeyCode::F(4) => app.open_rooms(),
        KeyCode::F(5) => app.open_members(),
        // ↑ : move cursor to previous line; if already on the first line
        // of the input, leave input focus to browse the conversation.
        KeyCode::Up => {
            if !app.input_up() {
                app.set_focus(Focus::Conversation);
            }
        }
        // ↓ : move cursor to next line of the input. No-op when already on
        // the last line (we don't have a "field below" yet).
        KeyCode::Down => {
            app.input_down();
        }
        // Ctrl+G — pop $EDITOR with the current input as initial content.
        KeyCode::Char('g') if ctrl => {
            return EventOutcome::EditInput(app.input.clone());
        }
        // Readline-style cursor movement and editing.
        KeyCode::Left => app.input_left(),
        KeyCode::Right => app.input_right(),
        // Home / End / Ctrl+A / Ctrl+E act on the current line.
        KeyCode::Home if !ctrl => app.input_line_home(),
        KeyCode::End if !ctrl => app.input_line_end(),
        KeyCode::Char('a') if ctrl => app.input_line_home(),
        KeyCode::Char('e') if ctrl => app.input_line_end(),
        // Ctrl+Home / Ctrl+End reach the start / end of the whole buffer.
        KeyCode::Home if ctrl => app.input_home(),
        KeyCode::End if ctrl => app.input_end(),
        KeyCode::Char('k') if ctrl => app.input_kill_to_end(),
        KeyCode::Char('s') if ctrl => app.submit_input(),
        KeyCode::Tab => app.input_tab_complete(true),
        KeyCode::BackTab => app.input_tab_complete(false),
        KeyCode::Delete => app.input_delete_forward(),
        KeyCode::Char(c) if !ctrl => app.input_insert_char(c),
        KeyCode::Backspace => app.input_backspace(),
        KeyCode::Enter => {
            if app.settings_state.multi_line_input {
                app.input_insert_char('\n');
            } else {
                app.submit_input();
            }
        }
        _ => {}
    }
    EventOutcome::Continue
}

fn handle_settings_key(app: &mut App, key: KeyEvent) -> EventOutcome {
    let s = &mut app.settings_state;
    let on_text =
        s.focus_idx == settings_view::F_EDITOR || s.focus_idx == settings_view::F_PM_CMD;
    match key.code {
        KeyCode::Esc => app.back_to_conversation(),
        KeyCode::Tab | KeyCode::Down => s.next(),
        KeyCode::BackTab | KeyCode::Up => s.prev(),
        KeyCode::Char(' ') if !on_text => toggle_settings_field(s),
        KeyCode::Left | KeyCode::Right if !on_text => cycle_settings_radio(s),
        KeyCode::Char(c) if on_text => match s.focus_idx {
            settings_view::F_EDITOR => s.editor.push(c),
            settings_view::F_PM_CMD => s.pm_cmd.push(c),
            _ => {}
        },
        KeyCode::Backspace if on_text => match s.focus_idx {
            settings_view::F_EDITOR => {
                s.editor.pop();
            }
            settings_view::F_PM_CMD => {
                s.pm_cmd.pop();
            }
            _ => {}
        },
        KeyCode::Enter => match s.focus_idx {
            settings_view::F_DOC => {
                app.flash = Some("ouverture documentation (mock)".into());
            }
            settings_view::F_SAVE => {
                match app.settings_state.save() {
                    Ok(path) => {
                        app.flash =
                            Some(format!("paramètres enregistrés : {}", path.display()));
                    }
                    Err(e) => {
                        app.flash = Some(format!("enregistrement KO : {e}"));
                    }
                }
                app.back_to_conversation();
            }
            settings_view::F_CANCEL => {
                app.back_to_conversation();
            }
            _ if on_text => s.next(),
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
        settings_view::F_KEYCHAIN => s.keychain_recovery = !s.keychain_recovery,
        settings_view::F_SOUNDS => s.sounds = !s.sounds,
        settings_view::F_MULTILINE => s.multi_line_input = !s.multi_line_input,
        settings_view::F_REOPEN => s.reopen_windows = !s.reopen_windows,
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
            login_view::F_SSO => {
                app.submit_sso_login();
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
    // Placeholder: buttons react to Enter in the Enter arm of the match.
}
