use crate::event::{handle_key, EventOutcome};
use crate::matrix::{Command as MxCommand, MatrixBridge, Update as MxUpdate};
use crate::message::{build_visible_items, Block, ItemKind, Message, ViewItem};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDirection {
    Forward,
    Backward,
}

#[derive(Debug, Clone)]
pub enum MatchTarget {
    ConvMsg(usize),
    ConvReply(usize, usize),
    Room(usize),
    Space(Vec<usize>),
    Member(usize),
}

pub struct SearchState {
    pub active: bool,
    pub query: String,
    pub direction: SearchDirection,
    pub matches: Vec<MatchTarget>,
    pub match_pos: usize,
    pub origin: Option<MatchTarget>,
    pub last_activity: Instant,
    pub timeout: Duration,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            active: false,
            query: String::new(),
            direction: SearchDirection::Forward,
            matches: Vec::new(),
            match_pos: 0,
            origin: None,
            last_activity: Instant::now(),
            timeout: Duration::from_secs(5),
        }
    }
}
use crate::modal::{
    ConfirmAction, ConfirmButton, ConfirmModal, DetailsModal, Modal, ReactedByModal,
    ReactionPickerModal, RecoveryDisplayFocus, RecoveryDisplayModal, RecoveryFocus,
    RecoveryInputModal, SasVerificationModal, WindowListEntry, WindowListModal,
};
use crate::ui::draw;
use crate::view::login::LoginState;
use crate::view::members::MembersState;
use crate::view::room_list::RoomListState;
use crate::view::settings::SettingsState;
use crate::view::space_tree::SpaceTreeState;
use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;
use std::collections::HashSet;
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Conversation,
    Settings,
    Login,
    RoomList,
    SpaceTree,
    Members,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Conversation,
    Input,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum InputMode {
    Normal,
    Reply,
    Edit,
    Thread,
}

impl InputMode {
    pub fn prefix(self) -> &'static str {
        match self {
            InputMode::Normal => ">",
            InputMode::Reply => ">reply:",
            InputMode::Edit => ">edit:",
            InputMode::Thread => ">thread:",
        }
    }
}

const REACTION_OPTIONS: &[&str] = &[
    "+1", "-1", "heart", "smile", "laugh", "thinking", "eyes", "fire",
];

/// Names accepted by `App::run_command`. Used by `Tab` autocomplete in
/// the chat input bar.
const SLASH_COMMANDS: &[&str] = &[
    "quit",
    "q",
    "help",
    "h",
    "version",
    "me",
    "join",
    "j",
    "leave",
    "part",
    "redact",
    "del",
    "react",
    "restore",
    "recovery",
    "setup",
    "enable-recovery",
    "verify",
    "logout",
    "window",
    "win",
    "w",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Slash,
    Mention,
}

pub struct CompletionState {
    #[allow(dead_code)]
    pub kind: CompletionKind,
    /// Char position of the trigger char (`/` or `@`).
    pub trigger: usize,
    /// Replacement strings (without the trigger char).
    pub candidates: Vec<String>,
    /// Index of the currently inserted candidate inside `candidates`.
    pub current: usize,
}

/// Activity flag for a non-focused window, irssi-style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityLevel {
    None,
    /// Plain new message in the room.
    Active,
    /// New message that mentions the local user.
    Mention,
}

/// Per-room scrollback / cursor / thread state. The app keeps one of
/// these for each open window (irssi-style); the active window's data
/// is mirrored into `App.messages` / `expanded_threads` / `selected` /
/// `scroll_top` / `current_room` / `current_room_id` while focused, and
/// saved back on window switch.
pub struct ChatWindow {
    pub room_id: Option<String>,
    pub room_name: String,
    pub messages: Vec<Message>,
    pub expanded_threads: HashSet<usize>,
    pub selected: usize,
    pub scroll_top: usize,
    pub activity: ActivityLevel,
}

impl ChatWindow {
    pub fn empty() -> Self {
        Self {
            room_id: None,
            room_name: String::new(),
            messages: Vec::new(),
            expanded_threads: HashSet::new(),
            selected: 0,
            scroll_top: 0,
            activity: ActivityLevel::None,
        }
    }
}

pub struct App {
    pub view: View,
    pub focus: Focus,
    pub input: String,
    /// Char-position of the editing cursor inside `input` (0..=chars().count()).
    pub input_cursor: usize,
    pub input_mode: InputMode,
    pub current_room: String,
    pub status_text: String,
    pub should_quit: bool,
    pub messages: Vec<Message>,
    pub expanded_threads: HashSet<usize>,
    pub selected: usize,
    pub scroll_top: usize,
    pub last_main_height: u16,
    pub modal: Option<Modal>,
    pub settings_state: SettingsState,
    pub login_state: LoginState,
    pub room_list_state: RoomListState,
    pub space_tree_state: SpaceTreeState,
    pub members_state: MembersState,
    pub me: String,
    pub flash: Option<String>,
    pub search: SearchState,
    /// Bridge to the Matrix SDK. None if the tokio runtime failed to start.
    pub matrix: Option<MatrixBridge>,
    /// Matrix room IDs, aligned with `room_list_state.rooms` (same length,
    /// same order). Empty until we receive an `Update::Rooms`.
    pub room_ids: Vec<String>,
    /// Room ID currently open on the Matrix side (None while on mocks).
    pub current_room_id: Option<String>,
    /// True once the Matrix login is confirmed.
    pub matrix_logged_in: bool,
    /// Recovery key that we just sent to `Command::RecoverFromKey` and
    /// are waiting to confirm worked. On `Update::RecoverySuccess`, if
    /// the keychain flag is on and no entry exists yet, we persist it.
    pub pending_recovery_key: Option<String>,
    /// In-flight Tab completion (slash command or `@user` mention).
    /// Reset to None on any key event other than Tab/BackTab.
    pub pending_completion: Option<CompletionState>,
    /// All open chat windows (irssi-style). The active one's state is
    /// mirrored into the top-level `messages` / `current_room*` fields
    /// while focused.
    pub windows: Vec<ChatWindow>,
    pub active_window: usize,
    /// Room IDs to re-open after the next sync completes (driven by
    /// `settings_state.reopen_windows`). Cleared once consumed.
    pub pending_reopen: Vec<String>,
    /// Active window index to restore after re-opening pending rooms.
    pub pending_reopen_active: usize,
}

impl App {
    pub fn new() -> Self {
        let mut s = Self {
            view: View::Conversation,
            focus: Focus::Conversation,
            input: String::new(),
            input_cursor: 0,
            input_mode: InputMode::Normal,
            current_room: String::new(),
            status_text: String::new(),
            should_quit: false,
            messages: Vec::new(),
            expanded_threads: HashSet::new(),
            selected: 0,
            scroll_top: 0,
            last_main_height: 0,
            modal: None,
            settings_state: SettingsState::new(),
            login_state: LoginState::new(),
            room_list_state: RoomListState::new(),
            space_tree_state: SpaceTreeState::new(),
            members_state: MembersState::new(),
            me: "moi".to_string(),
            flash: None,
            search: SearchState::new(),
            matrix: MatrixBridge::spawn().ok(),
            room_ids: Vec::new(),
            current_room_id: None,
            matrix_logged_in: false,
            pending_recovery_key: None,
            pending_completion: None,
            windows: vec![ChatWindow::empty()],
            active_window: 0,
            pending_reopen: Vec::new(),
            pending_reopen_active: 0,
        };
        if s.settings_state.reopen_windows && !s.settings_state.last_windows.is_empty() {
            s.pending_reopen = s.settings_state.last_windows.clone();
            s.pending_reopen_active = s.settings_state.last_active;
        }
        // Try to restore a previously-persisted session on startup, so the
        // user doesn't have to log in again every time.
        if let Some(b) = &s.matrix {
            b.send(MxCommand::TryRestore);
        }
        s.update_status();
        s
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| draw(frame, self))?;
            let timeout = self.next_timeout();
            if event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    match handle_key(self, key) {
                        EventOutcome::Continue => {}
                        EventOutcome::Quit => self.should_quit = true,
                        EventOutcome::OpenEditor(content) => {
                            let editor = self.settings_state.editor.clone();
                            if let Err(e) = suspend_for_editor(terminal, &content, &editor) {
                                self.flash = Some(format!("éditeur : {e}"));
                            }
                        }
                        EventOutcome::EditInput(content) => {
                            let editor = self.settings_state.editor.clone();
                            match suspend_for_input_editor(terminal, &content, &editor) {
                                Ok(Some(new_content)) => {
                                    self.input_set(new_content);
                                    self.flash = Some(
                                        "saisie chargée · Entrée envoie".into(),
                                    );
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    self.flash = Some(format!("éditeur : {e}"));
                                }
                            }
                        }
                    }
                }
            } else {
                self.tick();
            }
            // Fetch and apply pending Matrix updates.
            self.apply_matrix_updates();
        }
        // Persist the open-window list so the next start can restore it
        // (only when the corresponding setting is on; otherwise clear).
        self.persist_session();
        Ok(())
    }

    fn persist_session(&mut self) {
        if self.settings_state.reopen_windows {
            self.settings_state.last_windows = self
                .windows
                .iter()
                .filter_map(|w| w.room_id.clone())
                .collect();
            self.settings_state.last_active = self.active_window;
        } else {
            self.settings_state.last_windows.clear();
            self.settings_state.last_active = 0;
        }
        let _ = self.settings_state.save();
    }

    fn next_timeout(&self) -> Duration {
        if self.search.active {
            let elapsed = self.search.last_activity.elapsed();
            if elapsed >= self.search.timeout {
                Duration::from_millis(0)
            } else {
                self.search.timeout - elapsed
            }
        } else if self.matrix.is_some() {
            // Regular polling to fetch Matrix updates without blocking too long.
            Duration::from_millis(150)
        } else {
            Duration::from_secs(60)
        }
    }

    fn tick(&mut self) {
        if self.search.active && self.search.last_activity.elapsed() >= self.search.timeout {
            self.search_end();
        }
    }

    pub fn visible_items(&self) -> Vec<ViewItem> {
        build_visible_items(&self.messages, &self.expanded_threads)
    }

    pub fn current_item(&self) -> Option<ViewItem> {
        self.visible_items().get(self.selected).copied()
    }

    pub fn set_focus(&mut self, focus: Focus) {
        self.focus = focus;
        self.update_status();
    }

    pub fn toggle_focus(&mut self) {
        let next = match self.focus {
            Focus::Conversation => Focus::Input,
            Focus::Input => Focus::Conversation,
        };
        self.set_focus(next);
    }

    pub fn select_prev(&mut self, n: usize) {
        self.selected = self.selected.saturating_sub(n);
        self.update_status();
    }

    pub fn select_next(&mut self, n: usize) {
        let len = self.visible_items().len();
        let max = len.saturating_sub(1);
        self.selected = (self.selected + n).min(max);
        self.update_status();
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
        self.update_status();
    }

    pub fn select_last(&mut self) {
        let len = self.visible_items().len();
        self.selected = len.saturating_sub(1);
        self.update_status();
    }

    pub fn open_thread(&mut self) {
        let item = match self.current_item() {
            Some(it) => it,
            None => return,
        };
        if item.kind != ItemKind::Top {
            return;
        }
        let msg_idx = item.msg_idx;
        if self.messages[msg_idx].replies.is_empty() {
            return;
        }
        if self.expanded_threads.contains(&msg_idx) {
            return;
        }
        self.expanded_threads.insert(msg_idx);
        self.update_status();
    }

    pub fn close_thread(&mut self) {
        let item = match self.current_item() {
            Some(it) => it,
            None => return,
        };
        match item.kind {
            ItemKind::Top => {
                if self.expanded_threads.remove(&item.msg_idx) {
                    self.update_status();
                }
            }
            ItemKind::Reply => {
                let parent = item.msg_idx;
                self.expanded_threads.remove(&parent);
                let new_visible = self.visible_items();
                if let Some(pos) = new_visible
                    .iter()
                    .position(|v| v.kind == ItemKind::Top && v.msg_idx == parent)
                {
                    self.selected = pos;
                }
                self.update_status();
            }
        }
    }

    pub fn open_quit_confirm(&mut self) {
        self.modal = Some(Modal::Confirm(ConfirmModal {
            title: "Quitter".into(),
            message: "Quitter matcurses ?".into(),
            action: ConfirmAction::Quit,
            focused: ConfirmButton::No,
        }));
    }

    pub fn open_logout_confirm(&mut self) {
        if !self.matrix_logged_in {
            self.flash = Some("pas connecté".into());
            return;
        }
        self.modal = Some(Modal::Confirm(ConfirmModal {
            title: "Déconnexion".into(),
            message: format!("Te déconnecter de {} ?", self.me),
            action: ConfirmAction::Logout,
            focused: ConfirmButton::No,
        }));
    }

    pub fn open_redact_confirm(&mut self) {
        let item = match self.current_item() {
            Some(it) => it,
            None => return,
        };
        if item.kind != ItemKind::Top {
            return;
        }
        let summary = self.message_summary(item.msg_idx);
        self.modal = Some(Modal::Confirm(ConfirmModal {
            title: "Supprimer le message".into(),
            message: format!("Supprimer : {}", summary),
            action: ConfirmAction::Redact(item.msg_idx),
            focused: ConfirmButton::No,
        }));
    }

    pub fn open_details(&mut self) {
        let item = match self.current_item() {
            Some(it) => it,
            None => return,
        };
        let mut lines = Vec::new();
        match item.kind {
            ItemKind::Top => {
                let msg = &self.messages[item.msg_idx];
                lines.push("Type    : message".to_string());
                lines.push(format!("Heure   : {}", msg.time));
                lines.push(format!("Auteur  : {}", msg.author));
                lines.push(format!("Index   : {}", item.msg_idx));
                lines.push(format!("Blocs   : {}", msg.blocks.len()));
                lines.push(format!("Réponses: {}", msg.replies.len()));
                lines.push(format!("Réactions: {}", msg.reactions.len()));
                lines.push(String::new());
                lines.push("État    : envoyé".into());
                if !msg.reactions.is_empty() {
                    lines.push(String::new());
                    lines.push("Réactions :".into());
                    for r in &msg.reactions {
                        lines.push(format!("  {} — {}", r.key, r.users.join(", ")));
                    }
                }
                lines.push(String::new());
                lines.push("Aperçu  :".into());
                append_blocks_preview(&mut lines, &msg.blocks);
            }
            ItemKind::Reply => {
                let msg = &self.messages[item.msg_idx];
                let r = &msg.replies[item.reply_idx];
                lines.push("Type    : réponse de thread".into());
                lines.push(format!("Heure   : {}", r.time));
                lines.push(format!("Auteur  : {}", r.author));
                lines.push(format!(
                    "Parent  : message {} de <{}>",
                    item.msg_idx, msg.author
                ));
                lines.push(format!("Index   : réponse {}", item.reply_idx));
                lines.push(String::new());
                lines.push("État    : envoyé".into());
                lines.push(String::new());
                lines.push("Aperçu  :".into());
                append_blocks_preview(&mut lines, &r.blocks);
            }
        }
        self.modal = Some(Modal::Details(DetailsModal {
            title: "Détails".into(),
            lines,
            scroll: 0,
        }));
    }

    pub fn open_reaction_picker(&mut self) {
        let item = match self.current_item() {
            Some(it) => it,
            None => return,
        };
        if item.kind != ItemKind::Top {
            return;
        }
        self.modal = Some(Modal::ReactionPicker(ReactionPickerModal {
            msg_idx: item.msg_idx,
            options: REACTION_OPTIONS.iter().map(|s| s.to_string()).collect(),
            selected: 0,
        }));
    }

    pub fn open_reacted_by(&mut self) {
        let item = match self.current_item() {
            Some(it) => it,
            None => return,
        };
        if item.kind != ItemKind::Top {
            return;
        }
        let msg = &self.messages[item.msg_idx];
        let entries: Vec<String> = msg
            .reactions
            .iter()
            .map(|r| format!("{} — {}", r.key, r.users.join(", ")))
            .collect();
        self.modal = Some(Modal::ReactedBy(ReactedByModal {
            title: "Qui a réagi".into(),
            entries,
            selected: 0,
        }));
    }

    pub fn pick_reaction(&mut self) {
        let (msg_idx, key) = match &self.modal {
            Some(Modal::ReactionPicker(p)) => {
                let key = p.options.get(p.selected).cloned();
                (p.msg_idx, key)
            }
            _ => return,
        };
        self.modal = None;
        let key = match key {
            Some(k) => k,
            None => return,
        };
        if msg_idx >= self.messages.len() {
            return;
        }
        let parent_event_id = self.messages[msg_idx].event_id.clone();
        let my_existing = self.messages[msg_idx]
            .reactions
            .iter()
            .find(|r| r.key == key)
            .and_then(|r| r.my_event_id.clone());

        match (
            self.matrix_logged_in,
            self.matrix.as_ref(),
            self.current_room_id.clone(),
            parent_event_id.is_empty(),
        ) {
            (true, Some(b), Some(room_id), false) => {
                if let Some(reaction_event_id) = my_existing {
                    b.send(MxCommand::RedactEvent {
                        room_id,
                        event_id: reaction_event_id,
                    });
                    self.flash = Some(format!("réaction {} retirée", key));
                } else {
                    b.send(MxCommand::SendReaction {
                        room_id,
                        parent_event_id,
                        key: key.clone(),
                    });
                    self.flash = Some(format!("réaction {} envoyée", key));
                }
            }
            _ => {
                self.flash = Some("réactions indisponibles (hors session)".into());
            }
        }
    }

    /// Format the currently-focused message (or thread reply) as plain text
    /// so it can be opened in `$EDITOR` for leisurely reading.
    pub fn current_message_text(&self) -> String {
        let item = match self.current_item() {
            Some(it) => it,
            None => return String::new(),
        };
        let (time, author, blocks) = match item.kind {
            ItemKind::Top => {
                let m = &self.messages[item.msg_idx];
                (m.time.as_str(), m.author.as_str(), &m.blocks)
            }
            ItemKind::Reply => {
                let r = &self.messages[item.msg_idx].replies[item.reply_idx];
                (r.time.as_str(), r.author.as_str(), &r.blocks)
            }
        };
        let mut out = format!("{} <{}>\n\n", time, author);
        for block in blocks {
            match block {
                Block::Text(t) => {
                    out.push_str(t);
                    out.push_str("\n\n");
                }
                Block::Code(c) => {
                    out.push_str("```\n");
                    out.push_str(c);
                    out.push_str("\n```\n\n");
                }
                Block::Voice { duration_secs } => {
                    let mins = duration_secs / 60;
                    let secs = duration_secs % 60;
                    out.push_str(&format!("[voix {}:{:02}]\n\n", mins, secs));
                }
            }
        }
        out
    }

    pub fn play_current_voice(&mut self) {
        let item = match self.current_item() {
            Some(it) => it,
            None => return,
        };
        let (blocks, event_id) = match item.kind {
            ItemKind::Top => {
                let m = &self.messages[item.msg_idx];
                (&m.blocks, m.event_id.clone())
            }
            ItemKind::Reply => {
                let r = &self.messages[item.msg_idx].replies[item.reply_idx];
                (&r.blocks, r.event_id.clone())
            }
        };
        let has_voice = blocks.iter().any(|b| matches!(b, Block::Voice { .. }));
        if !has_voice {
            self.flash = Some("pas de note vocale ici".into());
            return;
        }
        match (
            self.matrix_logged_in,
            self.current_room_id.clone(),
            event_id.is_empty(),
        ) {
            (true, Some(room_id), false) => {
                if let Some(b) = self.matrix.as_ref() {
                    b.send(MxCommand::PlayVoice { room_id, event_id });
                    self.flash = Some("téléchargement de la note vocale…".into());
                }
            }
            _ => {
                self.flash = Some("lecture indisponible (hors session Matrix)".into());
            }
        }
    }

    pub fn stop_voice(&mut self) {
        if let Some(b) = self.matrix.as_ref() {
            b.send(MxCommand::StopVoice);
            self.flash = Some("lecture arrêtée".into());
        }
    }

    /// Send a notification sound to the audio thread, gated on the user's
    /// settings checkbox.
    pub fn play_event_sound(&self, kind: crate::sounds::SoundKind) {
        if !self.settings_state.sounds {
            return;
        }
        if let Some(b) = self.matrix.as_ref() {
            b.send(MxCommand::PlaySound { kind });
        }
    }

    pub fn open_settings(&mut self) {
        self.view = View::Settings;
        self.settings_state.focus_idx = 0;
        self.update_status();
    }

    pub fn open_login(&mut self) {
        self.view = View::Login;
        self.login_state.focus_idx = 0;
        self.update_status();
    }

    pub fn open_rooms(&mut self) {
        self.view = View::RoomList;
        self.update_status();
    }

    pub fn open_spaces(&mut self) {
        self.view = View::SpaceTree;
        self.update_status();
        if self.matrix_logged_in {
            if let Some(b) = &self.matrix {
                b.send(MxCommand::LoadSpaces);
            }
        }
    }

    pub fn open_members(&mut self) {
        self.view = View::Members;
        self.update_status();
        if self.matrix_logged_in {
            if let (Some(b), Some(rid)) = (&self.matrix, &self.current_room_id) {
                b.send(MxCommand::LoadMembers {
                    room_id: rid.clone(),
                });
            }
        }
    }

    /// Snapshot the focused window's per-room state from the top-level
    /// `App` fields. Called before activating a different window.
    fn save_active_window(&mut self) {
        if let Some(w) = self.windows.get_mut(self.active_window) {
            w.room_id = self.current_room_id.clone();
            w.room_name = self.current_room.clone();
            w.messages = std::mem::take(&mut self.messages);
            w.expanded_threads = std::mem::take(&mut self.expanded_threads);
            w.selected = self.selected;
            w.scroll_top = self.scroll_top;
        }
    }

    /// Restore the given window's state into the top-level `App` fields
    /// and mark it as active. Triggers a Matrix refetch when the window
    /// has a matrix room id.
    fn load_window(&mut self, idx: usize) {
        if idx >= self.windows.len() {
            return;
        }
        self.active_window = idx;
        let w = &mut self.windows[idx];
        self.current_room = w.room_name.clone();
        self.current_room_id = w.room_id.clone();
        self.messages = std::mem::take(&mut w.messages);
        self.expanded_threads = std::mem::take(&mut w.expanded_threads);
        self.selected = w.selected;
        self.scroll_top = w.scroll_top;
        // Entering the window dismisses any pending activity for it.
        w.activity = ActivityLevel::None;
        self.update_status();
    }

    /// Switch to window `idx` (0-based). No-op if out of range or already
    /// active. Refetches the room timeline so it stays current.
    pub fn switch_window(&mut self, idx: usize) {
        if idx == self.active_window || idx >= self.windows.len() {
            return;
        }
        self.save_active_window();
        self.load_window(idx);
        if let (Some(b), Some(rid)) =
            (self.matrix.as_ref(), self.current_room_id.clone())
        {
            b.send(MxCommand::OpenRoom { room_id: rid });
        }
    }

    pub fn open_window_list(&mut self) {
        let entries: Vec<WindowListEntry> = self
            .windows
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let activity = match w.activity {
                    ActivityLevel::None => ' ',
                    ActivityLevel::Active => '+',
                    ActivityLevel::Mention => '!',
                };
                let label = if w.room_name.is_empty() {
                    "(vide)".to_string()
                } else {
                    w.room_name.clone()
                };
                WindowListEntry {
                    idx: i,
                    label,
                    activity,
                    is_active: i == self.active_window,
                }
            })
            .collect();
        let selected = self.active_window;
        self.modal = Some(Modal::WindowList(WindowListModal {
            entries,
            selected,
        }));
    }

    pub fn pick_window_from_list(&mut self) {
        let target = match &self.modal {
            Some(Modal::WindowList(m)) => m
                .entries
                .get(m.selected)
                .map(|e| e.idx),
            _ => None,
        };
        self.modal = None;
        if let Some(idx) = target {
            self.switch_window(idx);
            self.view = View::Conversation;
        }
    }

    pub fn next_window(&mut self) {
        if self.windows.is_empty() {
            return;
        }
        let next = (self.active_window + 1) % self.windows.len();
        self.switch_window(next);
    }

    pub fn prev_window(&mut self) {
        if self.windows.is_empty() {
            return;
        }
        let n = self.windows.len();
        let prev = (self.active_window + n - 1) % n;
        self.switch_window(prev);
    }

    /// Switch to a room. The argument may be either a display name (from F4)
    /// or a Matrix room_id (from F3, where the tree stores ids).
    pub fn switch_room(&mut self, name_or_id: &str) {
        // Try match-by-name first, then match-by-id.
        let mut idx = self
            .room_list_state
            .rooms
            .iter()
            .position(|r| r.name == name_or_id);
        if idx.is_none() {
            idx = self.room_ids.iter().position(|id| id == name_or_id);
        }

        if !self.matrix_logged_in {
            self.current_room = name_or_id.to_string();
            self.flash = Some(format!("ouverture {} (mock)", name_or_id));
            self.view = View::Conversation;
            self.update_status();
            return;
        }

        let idx = match idx {
            Some(i) => i,
            None => {
                self.flash = Some(format!("room {} introuvable", name_or_id));
                self.view = View::Conversation;
                self.update_status();
                return;
            }
        };

        let display = self.room_list_state.rooms[idx].name.clone();
        let id = match self.room_ids.get(idx).cloned() {
            Some(i) => i,
            None => {
                self.flash = Some(format!("room {} : id manquant", display));
                return;
            }
        };

        // If a window already shows this room, just focus it.
        if let Some(existing) = self
            .windows
            .iter()
            .position(|w| w.room_id.as_deref() == Some(&id))
        {
            // Active window already on this room: just refetch.
            if existing == self.active_window {
                if let Some(b) = self.matrix.as_ref() {
                    b.send(MxCommand::OpenRoom { room_id: id });
                }
            } else {
                self.switch_window(existing);
            }
            self.flash = Some(format!("focus {}", display));
            self.view = View::Conversation;
            return;
        }

        // Decide whether to reuse the active window or create a new one.
        // We look at the LIVE state (`current_room_id`), not the stored
        // window slot — the active slot's `room_id` is only updated on a
        // save_active_window, which happens on switch.
        let active_is_unused = self.current_room_id.is_none();
        let target = if self.windows.is_empty() {
            self.windows.push(ChatWindow {
                room_id: Some(id.clone()),
                room_name: display.clone(),
                ..ChatWindow::empty()
            });
            0
        } else if active_is_unused {
            self.active_window
        } else {
            self.save_active_window();
            self.windows.push(ChatWindow {
                room_id: Some(id.clone()),
                room_name: display.clone(),
                ..ChatWindow::empty()
            });
            self.windows.len() - 1
        };

        if target != self.active_window {
            self.active_window = target;
        }
        self.current_room = display.clone();
        self.current_room_id = Some(id.clone());
        self.messages.clear();
        self.expanded_threads.clear();
        self.selected = 0;
        self.scroll_top = 0;
        // Persist the room into the active slot immediately so the next
        // switch_room sees this window as "occupied" and creates a new
        // one instead of overwriting.
        if let Some(w) = self.windows.get_mut(target) {
            w.room_id = Some(id.clone());
            w.room_name = display.clone();
        }

        if let Some(b) = self.matrix.as_ref() {
            b.send(MxCommand::OpenRoom {
                room_id: id.clone(),
            });
            // Pre-fetch the member list so @user Tab-completion works
            // without the user having to open F5 first. Stale members
            // from the previous room are dropped immediately.
            self.members_state.members.clear();
            self.members_state.set_selected(0);
            b.send(MxCommand::LoadMembers { room_id: id });
        }
        self.flash = Some(format!("ouverture {}", display));
        self.view = View::Conversation;
        self.update_status();
    }

    /// Starts the Matrix login with the current form values.
    pub fn submit_login(&mut self) {
        let (mxid, password, server) = {
            let s = &self.login_state;
            (s.mxid.clone(), s.password.clone(), s.server.clone())
        };
        if mxid.is_empty() || password.is_empty() {
            self.flash = Some("MXID et mot de passe requis".into());
            return;
        }
        let bridge = match self.matrix.as_ref() {
            Some(b) => b,
            None => {
                self.flash = Some("runtime Matrix indisponible".into());
                return;
            }
        };
        bridge.send(MxCommand::Login {
            mxid: mxid.clone(),
            password,
            server,
        });
        self.me = mxid
            .trim_start_matches('@')
            .split(':')
            .next()
            .unwrap_or("moi")
            .to_string();
        self.flash = Some("connexion en cours…".into());
    }

    /// Trigger an SSO login: opens the homeserver SSO redirect URL in the
    /// system browser. Only `server` from the login form is consulted —
    /// the MXID and password fields are ignored.
    pub fn submit_sso_login(&mut self) {
        let server = self.login_state.server.clone();
        if server.trim().is_empty() {
            self.flash = Some("SSO : renseigne le serveur (ex. matrix.org)".into());
            return;
        }
        let bridge = match self.matrix.as_ref() {
            Some(b) => b,
            None => {
                self.flash = Some("runtime Matrix indisponible".into());
                return;
            }
        };
        bridge.send(MxCommand::LoginSso {
            server,
            idp_id: None,
        });
        self.flash = Some("SSO : ouverture du navigateur…".into());
    }

    /// Sends the input buffer contents to the current room (if Matrix is active).
    fn byte_index_for_char(&self, char_idx: usize) -> usize {
        self.input
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.input.len())
    }

    pub fn input_insert_char(&mut self, c: char) {
        let pos = self.byte_index_for_char(self.input_cursor);
        self.input.insert(pos, c);
        self.input_cursor += 1;
    }

    pub fn input_backspace(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        self.input_cursor -= 1;
        let pos = self.byte_index_for_char(self.input_cursor);
        self.input.remove(pos);
    }

    pub fn input_delete_forward(&mut self) {
        let total = self.input.chars().count();
        if self.input_cursor >= total {
            return;
        }
        let pos = self.byte_index_for_char(self.input_cursor);
        self.input.remove(pos);
    }

    pub fn input_left(&mut self) {
        if self.input_cursor > 0 {
            self.input_cursor -= 1;
        }
    }

    pub fn input_right(&mut self) {
        let total = self.input.chars().count();
        if self.input_cursor < total {
            self.input_cursor += 1;
        }
    }

    pub fn input_home(&mut self) {
        self.input_cursor = 0;
    }

    pub fn input_end(&mut self) {
        self.input_cursor = self.input.chars().count();
    }

    /// Move the cursor to column 0 of the current line.
    pub fn input_line_home(&mut self) {
        let (line, _) = cursor_line_col(&self.input, self.input_cursor);
        self.input_cursor = char_pos_for_line_col(&self.input, line, 0);
    }

    /// Move the cursor to the end of the current line.
    pub fn input_line_end(&mut self) {
        let (line, _) = cursor_line_col(&self.input, self.input_cursor);
        let lines: Vec<&str> = self.input.split('\n').collect();
        let line_len = lines.get(line).map(|l| l.chars().count()).unwrap_or(0);
        self.input_cursor = char_pos_for_line_col(&self.input, line, line_len);
    }

    /// Move the input cursor up one line, preserving column when possible.
    /// Returns false when the cursor is already on the first line so the
    /// caller can decide what to do (e.g. exit input focus).
    pub fn input_up(&mut self) -> bool {
        let (line, col) = cursor_line_col(&self.input, self.input_cursor);
        if line == 0 {
            return false;
        }
        self.input_cursor = char_pos_for_line_col(&self.input, line - 1, col);
        true
    }

    /// Move the input cursor down one line, preserving column when possible.
    /// Returns false when the cursor is already on the last line.
    pub fn input_down(&mut self) -> bool {
        let (line, col) = cursor_line_col(&self.input, self.input_cursor);
        let total = self.input.split('\n').count();
        if line + 1 >= total {
            return false;
        }
        self.input_cursor = char_pos_for_line_col(&self.input, line + 1, col);
        true
    }

    /// Readline-style Ctrl+K: drop everything from the cursor up to (but
    /// not including) the next newline. At the end of a non-final line
    /// this is a no-op rather than swallowing the newline — matches what
    /// most readline-flavored editors do for blind-friendly predictability.
    pub fn input_kill_to_end(&mut self) {
        let (line, col) = cursor_line_col(&self.input, self.input_cursor);
        let lines: Vec<&str> = self.input.split('\n').collect();
        let line_len = lines.get(line).map(|l| l.chars().count()).unwrap_or(0);
        if col >= line_len {
            return;
        }
        let end_char = char_pos_for_line_col(&self.input, line, line_len);
        let start = self.byte_index_for_char(self.input_cursor);
        let end = self.byte_index_for_char(end_char);
        self.input.replace_range(start..end, "");
    }

    pub fn input_clear(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
        self.pending_completion = None;
    }

    /// Tab completion entry point. If a completion is already in flight,
    /// advance to the next candidate (cycle). Otherwise, detect whether
    /// the cursor is in a slash-command or `@user` mention context, build
    /// the candidate list, and apply the first match.
    pub fn input_tab_complete(&mut self, forward: bool) {
        if let Some(state) = &self.pending_completion {
            if state.candidates.is_empty() {
                return;
            }
            let n = state.candidates.len();
            let next = if forward {
                (state.current + 1) % n
            } else {
                (state.current + n - 1) % n
            };
            self.apply_completion(next);
            return;
        }

        let chars: Vec<char> = self.input.chars().collect();
        let cursor = self.input_cursor.min(chars.len());

        // Walk back from the cursor to find a trigger ('@' or '/'). Stop on
        // whitespace or newline — the trigger has to be in the same word.
        let mut trigger_pos: Option<usize> = None;
        let mut kind: Option<CompletionKind> = None;
        let mut i = cursor;
        while i > 0 {
            let c = chars[i - 1];
            if c == '\n' || c.is_whitespace() {
                break;
            }
            i -= 1;
            match chars[i] {
                '@' => {
                    trigger_pos = Some(i);
                    kind = Some(CompletionKind::Mention);
                    break;
                }
                '/' => {
                    // A slash command must sit at the very start of a line.
                    if i == 0 || chars[i - 1] == '\n' {
                        trigger_pos = Some(i);
                        kind = Some(CompletionKind::Slash);
                    }
                    break;
                }
                _ => {}
            }
        }

        let trigger = match trigger_pos {
            Some(p) => p,
            None => return,
        };
        let kind = kind.unwrap();
        let prefix: String = chars[trigger + 1..cursor].iter().collect();

        let candidates = match kind {
            CompletionKind::Slash => slash_candidates(&prefix),
            CompletionKind::Mention => self.mention_candidates(&prefix),
        };
        if candidates.is_empty() {
            return;
        }
        self.pending_completion = Some(CompletionState {
            kind,
            trigger,
            candidates,
            current: 0,
        });
        self.apply_completion(0);
    }

    fn apply_completion(&mut self, idx: usize) {
        let (trigger, candidate) = {
            let state = match self.pending_completion.as_mut() {
                Some(s) => s,
                None => return,
            };
            if idx >= state.candidates.len() {
                return;
            }
            state.current = idx;
            (state.trigger, state.candidates[idx].clone())
        };
        let start = self.byte_index_for_char(trigger + 1);
        let end = self.byte_index_for_char(self.input_cursor);
        self.input.replace_range(start..end, &candidate);
        self.input_cursor = trigger + 1 + candidate.chars().count();
    }

    fn mention_candidates(&self, prefix: &str) -> Vec<String> {
        let lower = prefix.to_lowercase();
        let mut out = Vec::new();
        for m in &self.members_state.members {
            let dn = m.displayname.to_lowercase();
            let mxid_lower = m.mxid.to_lowercase();
            if dn.starts_with(&lower) || mxid_lower.contains(&lower) {
                // Insert the full MXID (without leading '@', since the
                // trigger is already in the buffer). The full MXID is
                // what triggers a notification on the recipient side.
                out.push(m.mxid.trim_start_matches('@').to_string());
            }
        }
        out
    }

    pub fn input_set(&mut self, content: String) {
        self.input_cursor = content.chars().count();
        self.input = content;
    }

    pub fn submit_input(&mut self) {
        if self.input.is_empty() {
            return;
        }
        let raw = self.input.clone();
        self.input_clear();

        // `//foo` → escape: send literal "/foo" as a regular message.
        // `/foo` → slash command.
        // anything else → regular message.
        if let Some(rest) = raw.strip_prefix("//") {
            self.send_text(rest.to_string());
            return;
        }
        if let Some(after_slash) = raw.strip_prefix('/') {
            let trimmed = after_slash.trim_start();
            if trimmed.is_empty() {
                return;
            }
            let mut parts = trimmed.splitn(2, char::is_whitespace);
            let cmd = parts.next().unwrap_or("").to_string();
            let args = parts.next().unwrap_or("").trim().to_string();
            self.run_command(&cmd, &args);
            return;
        }
        self.send_text(raw);
    }

    fn send_text(&mut self, body: String) {
        if !self.matrix_logged_in {
            return;
        }
        if let (Some(id), Some(b)) = (self.current_room_id.clone(), self.matrix.as_ref()) {
            b.send(MxCommand::SendMessage { room_id: id, body });
        }
    }

    /// Dispatch a slash-command. The full set is documented by `/help`.
    pub fn run_command(&mut self, cmd: &str, args: &str) {
        match cmd {
            "quit" | "q" => self.open_quit_confirm(),
            "help" | "h" | "?" => self.open_help(),
            "version" => {
                self.flash = Some(format!(
                    "matcurses {}",
                    env!("CARGO_PKG_VERSION")
                ));
            }
            "me" => {
                if args.is_empty() {
                    self.flash = Some("/me <texte>".into());
                    return;
                }
                if let (true, Some(id), Some(b)) = (
                    self.matrix_logged_in,
                    self.current_room_id.clone(),
                    self.matrix.as_ref(),
                ) {
                    b.send(MxCommand::SendEmote {
                        room_id: id,
                        body: args.to_string(),
                    });
                } else {
                    self.flash = Some("/me indisponible (hors session)".into());
                }
            }
            "join" | "j" => {
                if args.is_empty() {
                    self.flash = Some("/join <#room:server>".into());
                    return;
                }
                if let (true, Some(b)) = (self.matrix_logged_in, self.matrix.as_ref()) {
                    b.send(MxCommand::JoinRoom {
                        alias_or_id: args.to_string(),
                    });
                } else {
                    self.flash = Some("/join indisponible (hors session)".into());
                }
            }
            "leave" | "part" => {
                if let (true, Some(id), Some(b)) = (
                    self.matrix_logged_in,
                    self.current_room_id.clone(),
                    self.matrix.as_ref(),
                ) {
                    b.send(MxCommand::LeaveRoom { room_id: id });
                } else {
                    self.flash = Some("/leave indisponible (hors session)".into());
                }
            }
            "redact" | "del" => self.open_redact_confirm(),
            "restore" | "recovery" => self.open_recovery_input(),
            "setup" | "enable-recovery" => self.enable_recovery(),
            "logout" => self.open_logout_confirm(),
            "window" | "win" | "w" => {
                let arg = args.trim();
                match arg {
                    "" | "list" => self.open_window_list(),
                    "next" | "n" => self.next_window(),
                    "prev" | "p" | "previous" => self.prev_window(),
                    other => match other.parse::<usize>() {
                        Ok(n) if n >= 1 && n <= self.windows.len() => {
                            self.switch_window(n - 1);
                        }
                        _ => {
                            self.flash =
                                Some(format!("/window : '{}' n'est pas un numéro valide", other));
                        }
                    },
                }
            }
            "verify" => {
                let target = if args.is_empty() { None } else { Some(args) };
                self.verify_user(target);
            }
            "react" => {
                if args.is_empty() {
                    self.open_reaction_picker();
                } else {
                    self.flash =
                        Some("/react sans argument ; ouvre le picker".into());
                }
            }
            other => {
                self.flash = Some(format!("commande inconnue : /{other}"));
            }
        }
    }

    pub fn verify_user(&mut self, target: Option<&str>) {
        if !self.matrix_logged_in {
            self.flash = Some("/verify indisponible (hors session)".into());
            return;
        }
        let user_id = match target {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => self.me.clone(),
        };
        if user_id.is_empty() {
            self.flash = Some("MXID inconnu — précise /verify @user:server".into());
            return;
        }
        if let Some(b) = self.matrix.as_ref() {
            b.send(MxCommand::VerifyUser { user_id });
            self.flash = Some("vérification : en attente d'acceptation côté pair…".into());
        }
    }

    pub fn sas_confirm(&mut self) {
        if let Some(b) = self.matrix.as_ref() {
            b.send(MxCommand::SasConfirm);
        }
    }

    pub fn sas_mismatch(&mut self) {
        if let Some(b) = self.matrix.as_ref() {
            b.send(MxCommand::SasMismatch);
        }
    }

    pub fn sas_cancel(&mut self) {
        if let Some(b) = self.matrix.as_ref() {
            b.send(MxCommand::SasCancel);
        }
        self.modal = None;
    }

    pub fn enable_recovery(&mut self) {
        if !self.matrix_logged_in {
            self.flash = Some("E2EE setup indisponible (hors session)".into());
            return;
        }
        if let Some(b) = self.matrix.as_ref() {
            b.send(MxCommand::EnableRecovery);
            self.flash = Some("génération de la clé E2EE…".into());
        }
    }

    pub fn open_recovery_input(&mut self) {
        self.modal = Some(Modal::RecoveryInput(RecoveryInputModal {
            key: String::new(),
            focused: RecoveryFocus::Input,
        }));
    }

    pub fn submit_recovery_input(&mut self) {
        let key = match &self.modal {
            Some(Modal::RecoveryInput(m)) => m.key.clone(),
            _ => return,
        };
        self.modal = None;
        let mut resolved = key.trim().to_string();

        // Resolution chain when input is empty:
        //   1. OS keychain entry for this MXID
        //   2. user-configured PM command
        if resolved.is_empty() && self.settings_state.keychain_recovery && !self.me.is_empty() {
            match crate::secrets::load_recovery_key(&self.me) {
                Ok(Some(k)) => {
                    resolved = k;
                    self.flash = Some("clé chargée depuis le keychain".into());
                }
                Ok(None) => {}
                Err(e) => {
                    self.flash = Some(format!("keychain : {e}"));
                }
            }
        }
        if resolved.is_empty() && !self.settings_state.pm_cmd.trim().is_empty() {
            match crate::secrets::run_pm_command(&self.settings_state.pm_cmd) {
                Ok(k) => {
                    resolved = k;
                    self.flash = Some("clé chargée depuis le password manager".into());
                }
                Err(e) => {
                    self.flash = Some(format!("PM : {e}"));
                }
            }
        }
        if resolved.is_empty() {
            self.flash = Some("clé vide (saisie + keychain + PM tous KO)".into());
            return;
        }

        if let Some(b) = self.matrix.as_ref() {
            // Stash the key so we can persist it to the keychain only after
            // RecoverySuccess confirms it actually worked.
            self.pending_recovery_key = Some(resolved.clone());
            b.send(MxCommand::RecoverFromKey { key: resolved });
            self.flash = Some("restauration en cours…".into());
        } else {
            self.flash = Some("Matrix indisponible".into());
        }
    }

    /// Copy the recovery key currently shown in the display modal to the
    /// system clipboard.
    pub fn copy_recovery_to_clipboard(&mut self) {
        let key = match &self.modal {
            Some(Modal::RecoveryDisplay(m)) => m.key.clone(),
            _ => return,
        };
        match crate::secrets::copy_to_clipboard(&key) {
            Ok(()) => {
                self.flash = Some("clé copiée dans le presse-papier".into());
            }
            Err(e) => {
                self.flash = Some(format!("copie KO : {e}"));
            }
        }
    }

    /// Called when the user confirms they have saved the recovery key.
    /// Optionally persists it to the OS keychain for later auto-restore.
    pub fn confirm_recovery_displayed(&mut self) {
        let key = match &self.modal {
            Some(Modal::RecoveryDisplay(m)) => m.key.clone(),
            _ => return,
        };
        self.modal = None;
        if self.settings_state.keychain_recovery && !self.me.is_empty() {
            match crate::secrets::store_recovery_key(&self.me, &key) {
                Ok(()) => {
                    self.flash = Some("clé E2EE prête · sauvée dans le keychain".into());
                }
                Err(e) => {
                    self.flash = Some(format!("clé prête, keychain KO : {e}"));
                }
            }
        } else {
            self.flash = Some("clé E2EE prête · note-la bien".into());
        }
    }

    fn open_help(&mut self) {
        let lines: Vec<String> = vec![
            "Slash-commands :".into(),
            String::new(),
            "/quit, /q              quitter (confirmation)".into(),
            "/help, /h, /?          cette aide".into(),
            "/version               version de matcurses".into(),
            "/me <texte>            action / emote (m.emote)".into(),
            "/join <#room:server>   rejoindre une room".into(),
            "/leave, /part          quitter la room courante".into(),
            "/redact, /del          supprimer le message courant".into(),
            "/react                 ouvrir le picker de réactions".into(),
            "/setup                 générer la clé E2EE (1re fois sur ce compte)".into(),
            "/restore, /recovery    importer une clé de récupération E2EE".into(),
            "/verify [@user:srv]    vérification SAS (défaut : soi-même)".into(),
            "/logout                déconnexion + wipe local".into(),
            "/window N | n | p     basculer de fenêtre · Alt+1..9 / Alt+n / Alt+p".into(),
            String::new(),
            "Échapper un slash : commencer le message par //".into(),
        ];
        self.modal = Some(Modal::Details(DetailsModal {
            title: "Aide".into(),
            lines,
            scroll: 0,
        }));
    }

    /// Fetches and applies pending Matrix updates. Called on each
    /// UI loop iteration.
    pub fn apply_matrix_updates(&mut self) {
        let updates = match self.matrix.as_mut() {
            Some(b) => b.drain_updates(),
            None => return,
        };
        for u in updates {
            self.apply_one_update(u);
        }
    }

    fn apply_one_update(&mut self, u: MxUpdate) {
        match u {
            MxUpdate::LoggedOut => {
                self.matrix_logged_in = false;
                self.me.clear();
                self.current_room.clear();
                self.current_room_id = None;
                self.messages.clear();
                self.expanded_threads.clear();
                self.selected = 0;
                self.scroll_top = 0;
                self.room_list_state.rooms.clear();
                self.room_list_state.set_selected(0);
                self.members_state.members.clear();
                self.members_state.set_selected(0);
                self.space_tree_state.roots.clear();
                self.space_tree_state.set_selected(0);
                self.room_ids.clear();
                self.pending_recovery_key = None;
                self.modal = None;
                self.flash = Some("déconnecté · Ctrl+L pour retenter".into());
                self.update_status();
            }
            MxUpdate::LoggedIn { mxid } => {
                self.matrix_logged_in = true;
                self.me = mxid.clone();
                self.flash = Some(format!("connecté · {}", mxid));
                // Return to the conversation if we were on Login.
                if matches!(self.view, View::Login) {
                    self.view = View::Conversation;
                    self.update_status();
                }
            }
            MxUpdate::LoginFailed { reason } => {
                self.matrix_logged_in = false;
                self.flash = Some(format!("login KO : {}", reason));
            }
            MxUpdate::Rooms { rooms, ids } => {
                // Preserve the selection if possible (by name).
                let prev_name = self.room_list_state.selected_room_name();
                self.room_list_state.rooms = rooms;
                self.room_ids = ids;
                // Coupled sort of rooms+ids (preserves the name↔id alignment).
                let mut combined: Vec<(crate::view::room_list::Room, String)> = self
                    .room_list_state
                    .rooms
                    .drain(..)
                    .zip(self.room_ids.drain(..))
                    .collect();
                combined.sort_by(|a, b| {
                    let a_unread = a.0.unread > 0;
                    let b_unread = b.0.unread > 0;
                    b_unread.cmp(&a_unread).then_with(|| {
                        b.0.mentions
                            .cmp(&a.0.mentions)
                            .then_with(|| b.0.unread.cmp(&a.0.unread))
                            .then_with(|| a.0.name.cmp(&b.0.name))
                    })
                });
                for (r, id) in combined {
                    self.room_list_state.rooms.push(r);
                    self.room_ids.push(id);
                }
                if let Some(name) = prev_name {
                    if let Some(pos) = self
                        .room_list_state
                        .rooms
                        .iter()
                        .position(|r| r.name == name)
                    {
                        self.room_list_state.set_selected(pos);
                    }
                }
            }
            MxUpdate::RoomMessages { room_id, messages } => {
                if self.current_room_id.as_deref() == Some(&room_id) {
                    self.messages = messages;
                    self.expanded_threads.clear();
                    let visible = self.visible_items();
                    self.selected = visible.len().saturating_sub(1);
                    self.update_status();
                }
            }
            MxUpdate::NewMessage { room_id, message } => {
                if self.current_room_id.as_deref() == Some(&room_id) {
                    let mentioned = is_mention(&message, &self.me);
                    let was_at_bottom = {
                        let v = self.visible_items();
                        v.is_empty() || self.selected + 1 >= v.len()
                    };
                    self.messages.push(message);
                    if was_at_bottom {
                        let v = self.visible_items();
                        self.selected = v.len().saturating_sub(1);
                    }
                    self.play_event_sound(if mentioned {
                        crate::sounds::SoundKind::Mention
                    } else {
                        crate::sounds::SoundKind::Message
                    });
                    self.update_status();
                } else if let Some(idx) = self
                    .windows
                    .iter()
                    .position(|w| w.room_id.as_deref() == Some(&room_id))
                {
                    let mentioned = is_mention(&message, &self.me);
                    if let Some(w) = self.windows.get_mut(idx) {
                        let was_at_bottom = w.selected + 1 >= w.messages.len();
                        w.messages.push(message);
                        if was_at_bottom {
                            w.selected = w.messages.len().saturating_sub(1);
                        }
                        // Bump activity, escalate to Mention but never
                        // downgrade an existing Mention.
                        w.activity = match (w.activity, mentioned) {
                            (_, true) => ActivityLevel::Mention,
                            (ActivityLevel::Mention, _) => ActivityLevel::Mention,
                            _ => ActivityLevel::Active,
                        };
                    }
                    if mentioned {
                        self.play_event_sound(crate::sounds::SoundKind::Mention);
                    }
                }
            }
            MxUpdate::Error { reason } => {
                self.flash = Some(reason);
            }
            MxUpdate::SyncComplete => {
                // Drain any pending re-open list so the user finds their
                // last session's windows back. This runs once: the list
                // is moved out before iterating.
                if !self.pending_reopen.is_empty() {
                    let to_open = std::mem::take(&mut self.pending_reopen);
                    let target_active = self.pending_reopen_active.min(to_open.len().saturating_sub(1));
                    for id in &to_open {
                        self.switch_room(id);
                    }
                    if target_active < self.windows.len() {
                        self.switch_window(target_active);
                    }
                    self.flash = Some(format!(
                        "{} fenêtre(s) restaurée(s)",
                        to_open.len()
                    ));
                }
            }
            MxUpdate::Members { room_id, members } => {
                // Only accept the update if it matches the room
                // currently displayed in the Members view.
                if self.current_room_id.as_deref() == Some(&room_id) {
                    self.members_state.members = members;
                    self.members_state.set_selected(0);
                }
            }
            MxUpdate::MemberPresence {
                room_id,
                mxid,
                presence,
            } => {
                if self.current_room_id.as_deref() == Some(&room_id) {
                    if let Some(m) = self
                        .members_state
                        .members
                        .iter_mut()
                        .find(|m| m.mxid == mxid)
                    {
                        m.presence = presence;
                    }
                }
            }
            MxUpdate::SasReady { decimal, emoji } => {
                self.modal = Some(Modal::SasVerification(SasVerificationModal {
                    decimal,
                    emoji,
                    focused: ConfirmButton::No,
                }));
                self.flash = Some(
                    "SAS prêt — compare et valide (y/o ou n).".into(),
                );
            }
            MxUpdate::SasDone { ok } => {
                self.modal = None;
                self.flash = Some(if ok {
                    "vérification réussie".into()
                } else {
                    "vérification échouée ou annulée".into()
                });
            }
            MxUpdate::RecoveryKeyGenerated { key } => {
                self.modal = Some(Modal::RecoveryDisplay(RecoveryDisplayModal {
                    key,
                    show_nato: false,
                    focused: RecoveryDisplayFocus::Confirm,
                }));
            }
            MxUpdate::RecoverySuccess => {
                // The recovery worked: if the user wants their key cached
                // in the OS keychain and we don't have an entry yet for
                // this MXID, persist it now.
                if let Some(key) = self.pending_recovery_key.take() {
                    if self.settings_state.keychain_recovery && !self.me.is_empty() {
                        let already = matches!(
                            crate::secrets::load_recovery_key(&self.me),
                            Ok(Some(_))
                        );
                        if !already {
                            if let Err(e) =
                                crate::secrets::store_recovery_key(&self.me, &key)
                            {
                                self.flash = Some(format!("keychain : {e}"));
                            }
                        }
                    }
                }
                self.flash = Some("clés restaurées · refetch en cours".into());
                if let (Some(b), Some(id)) = (self.matrix.as_ref(), self.current_room_id.clone())
                {
                    b.send(MxCommand::OpenRoom { room_id: id });
                }
            }
            MxUpdate::Spaces { roots } => {
                // Preserve user state across reloads: keep the currently
                // selected path and re-expand any space that was open before.
                let prev_path = self
                    .space_tree_state
                    .flat()
                    .get(self.space_tree_state.selected())
                    .map(|it| it.path.clone());
                let prev_expanded = collect_expanded_labels(&self.space_tree_state.roots);

                self.space_tree_state.roots = roots;

                for label_path in &prev_expanded {
                    expand_by_labels(&mut self.space_tree_state.roots, label_path);
                }

                let pos = prev_path
                    .as_deref()
                    .and_then(|p| self.space_tree_state.find_pos(p))
                    .unwrap_or(0);
                self.space_tree_state.set_selected(pos);
            }
        }
    }

    pub fn open_member_details(&mut self) {
        let m = match self.members_state.current() {
            Some(m) => m,
            None => return,
        };
        let lines = vec![
            format!("MXID    : {}", m.mxid),
            format!("Nom     : {}", m.displayname),
            format!("Rôle    : {} (level {})", m.power_label(), m.power_level),
            format!("Présence: {}", m.presence.label()),
            String::new(),
            "Devices : (non chargé)".into(),
            "Vérifié : (non chargé)".into(),
        ];
        self.modal = Some(Modal::Details(DetailsModal {
            title: "Membre".into(),
            lines,
            scroll: 0,
        }));
    }

    pub fn back_to_conversation(&mut self) {
        self.view = View::Conversation;
        self.update_status();
    }

    pub fn search_start(&mut self) {
        self.start_search(SearchDirection::Forward);
    }

    pub fn search_start_backward(&mut self) {
        self.start_search(SearchDirection::Backward);
    }

    fn start_search(&mut self, direction: SearchDirection) {
        if !self.is_searchable_view() {
            return;
        }
        self.search.active = true;
        self.search.query.clear();
        self.search.matches.clear();
        self.search.match_pos = 0;
        self.search.direction = direction;
        self.search.origin = self.current_target();
        self.search.last_activity = Instant::now();
        self.update_status();
    }

    pub fn search_end(&mut self) {
        self.search.active = false;
        self.update_status();
    }

    pub fn search_push(&mut self, c: char) {
        self.search.query.push(c);
        self.search.last_activity = Instant::now();
        self.recompute_matches_and_jump();
    }

    pub fn search_backspace(&mut self) {
        self.search.query.pop();
        self.search.last_activity = Instant::now();
        if self.search.query.is_empty() {
            // Empty query: no matches, cursor returns to the origin position
            self.search.matches.clear();
            self.search.match_pos = 0;
            if let Some(origin) = self.search.origin.clone() {
                self.jump_to_target(&origin);
            }
            self.update_status();
        } else {
            self.recompute_matches_and_jump();
        }
    }

    pub fn search_resume_and_next(&mut self) {
        if self.resume_search() {
            self.search_next();
        }
    }

    pub fn search_resume_and_prev(&mut self) {
        if self.resume_search() {
            self.search_prev();
        }
    }

    fn resume_search(&mut self) -> bool {
        if !self.is_searchable_view() || self.search.query.is_empty() {
            return false;
        }
        self.search.active = true;
        self.search.last_activity = Instant::now();
        self.search.matches = self.compute_matches();
        if self.search.matches.is_empty() {
            self.search.match_pos = 0;
            self.update_status();
            return false;
        }
        if self.search.match_pos >= self.search.matches.len() {
            self.search.match_pos = 0;
        }
        self.update_status();
        true
    }

    pub fn search_next(&mut self) {
        self.search.last_activity = Instant::now();
        if self.search.matches.is_empty() {
            return;
        }
        let n = self.search.matches.len();
        self.search.match_pos = (self.search.match_pos + 1) % n;
        self.jump_to_current_match();
    }

    pub fn search_prev(&mut self) {
        self.search.last_activity = Instant::now();
        if self.search.matches.is_empty() {
            return;
        }
        let n = self.search.matches.len();
        self.search.match_pos = (self.search.match_pos + n - 1) % n;
        self.jump_to_current_match();
    }

    pub fn is_searchable_view(&self) -> bool {
        matches!(
            self.view,
            View::Conversation | View::RoomList | View::SpaceTree | View::Members
        )
    }

    fn current_target(&self) -> Option<MatchTarget> {
        match self.view {
            View::Conversation => {
                let it = self.current_item()?;
                Some(match it.kind {
                    ItemKind::Top => MatchTarget::ConvMsg(it.msg_idx),
                    ItemKind::Reply => MatchTarget::ConvReply(it.msg_idx, it.reply_idx),
                })
            }
            View::RoomList => Some(MatchTarget::Room(self.room_list_state.selected())),
            View::SpaceTree => {
                let f = self.space_tree_state.flat();
                let it = f.get(self.space_tree_state.selected())?;
                Some(MatchTarget::Space(it.path.clone()))
            }
            View::Members => Some(MatchTarget::Member(self.members_state.selected())),
            _ => None,
        }
    }

    fn recompute_matches_and_jump(&mut self) {
        self.search.matches = self.compute_matches();
        if self.search.matches.is_empty() {
            self.update_status();
            return;
        }
        self.search.match_pos = self.pick_initial_match();
        self.jump_to_current_match();
        self.update_status();
    }

    fn pick_initial_match(&self) -> usize {
        let n = self.search.matches.len();
        if n == 0 {
            return 0;
        }
        let origin = match &self.search.origin {
            Some(o) => o,
            None => return 0,
        };
        let origin_key = self.target_sort_key(origin);
        match self.search.direction {
            SearchDirection::Forward => self
                .search
                .matches
                .iter()
                .position(|t| self.target_sort_key(t) >= origin_key)
                .unwrap_or(0),
            SearchDirection::Backward => self
                .search
                .matches
                .iter()
                .rposition(|t| self.target_sort_key(t) <= origin_key)
                .unwrap_or(n - 1),
        }
    }

    fn target_sort_key(&self, target: &MatchTarget) -> Vec<usize> {
        match target {
            MatchTarget::ConvMsg(i) => vec![*i, 0, 0],
            MatchTarget::ConvReply(i, j) => vec![*i, 1, *j],
            MatchTarget::Room(i) => vec![*i],
            MatchTarget::Space(path) => path.clone(),
            MatchTarget::Member(i) => vec![*i],
        }
    }

    fn compute_matches(&self) -> Vec<MatchTarget> {
        let q = self.search.query.to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        match self.view {
            View::Conversation => {
                let mut out = Vec::new();
                for (i, msg) in self.messages.iter().enumerate() {
                    if message_text(msg).to_lowercase().contains(&q) {
                        out.push(MatchTarget::ConvMsg(i));
                    }
                    for (j, r) in msg.replies.iter().enumerate() {
                        if reply_text(r).to_lowercase().contains(&q) {
                            out.push(MatchTarget::ConvReply(i, j));
                        }
                    }
                }
                out
            }
            View::RoomList => self
                .room_list_state
                .rooms
                .iter()
                .enumerate()
                .filter(|(_, r)| r.name.to_lowercase().contains(&q))
                .map(|(i, _)| MatchTarget::Room(i))
                .collect(),
            View::SpaceTree => self
                .space_tree_state
                .all_paths()
                .into_iter()
                .filter(|(_, label)| label.to_lowercase().contains(&q))
                .map(|(p, _)| MatchTarget::Space(p))
                .collect(),
            View::Members => self
                .members_state
                .members
                .iter()
                .enumerate()
                .filter(|(_, m)| {
                    m.displayname.to_lowercase().contains(&q)
                        || m.mxid.to_lowercase().contains(&q)
                })
                .map(|(i, _)| MatchTarget::Member(i))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn jump_to_current_match(&mut self) {
        if self.search.matches.is_empty() {
            return;
        }
        let target = self.search.matches[self.search.match_pos].clone();
        self.jump_to_target(&target);
    }

    fn jump_to_target(&mut self, target: &MatchTarget) {
        match target {
            MatchTarget::ConvMsg(i) => {
                let visible = self.visible_items();
                if let Some(pos) = visible
                    .iter()
                    .position(|v| v.kind == ItemKind::Top && v.msg_idx == *i)
                {
                    self.selected = pos;
                }
            }
            MatchTarget::ConvReply(i, j) => {
                self.expanded_threads.insert(*i);
                let visible = self.visible_items();
                if let Some(pos) = visible.iter().position(|v| {
                    v.kind == ItemKind::Reply && v.msg_idx == *i && v.reply_idx == *j
                }) {
                    self.selected = pos;
                }
            }
            MatchTarget::Room(i) => self.room_list_state.set_selected(*i),
            MatchTarget::Space(path) => {
                self.space_tree_state.expand_to(path);
                if let Some(pos) = self.space_tree_state.find_pos(path) {
                    self.space_tree_state.set_selected(pos);
                }
            }
            MatchTarget::Member(i) => self.members_state.set_selected(*i),
        }
    }

    pub fn close_modal(&mut self) {
        self.modal = None;
    }

    pub fn confirm_modal_yes(&mut self) {
        let action = match &self.modal {
            Some(Modal::Confirm(m)) => m.action,
            _ => return,
        };
        self.modal = None;
        match action {
            ConfirmAction::Quit => self.should_quit = true,
            ConfirmAction::Logout => {
                if let Some(b) = self.matrix.as_ref() {
                    b.send(MxCommand::Logout);
                    self.flash = Some("déconnexion en cours…".into());
                }
            }
            ConfirmAction::Redact(idx) => {
                if idx < self.messages.len() {
                    self.messages.remove(idx);
                    self.expanded_threads.remove(&idx);
                    let len = self.visible_items().len();
                    if self.selected >= len && len > 0 {
                        self.selected = len - 1;
                    } else if len == 0 {
                        self.selected = 0;
                    }
                    self.update_status();
                }
            }
        }
    }

    fn message_summary(&self, idx: usize) -> String {
        let msg = &self.messages[idx];
        let body: String = msg
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Text(t) => Some(t.clone()),
                Block::Code(_) => Some("(code)".to_string()),
                Block::Voice { .. } => Some("(note vocale)".to_string()),
            })
            .unwrap_or_else(|| "(vide)".to_string());
        let trimmed: String = body.chars().take(40).collect();
        format!("<{}> {}", msg.author, trimmed)
    }

    fn update_status(&mut self) {
        let view_label = match self.view {
            View::Conversation => "Conv",
            View::Settings => "Paramètres",
            View::Login => "Connexion",
            View::RoomList => "Rooms",
            View::SpaceTree => "Spaces",
            View::Members => "Membres",
        };
        let visible = self.visible_items();
        let pos = if visible.is_empty() {
            "0/0".to_string()
        } else {
            format!("{}/{}", self.selected + 1, visible.len())
        };
        let suffix = match visible.get(self.selected) {
            Some(it) if it.kind == ItemKind::Reply => " · thread",
            Some(it)
                if it.kind == ItemKind::Top
                    && !self.messages[it.msg_idx].replies.is_empty()
                    && self.expanded_threads.contains(&it.msg_idx) =>
            {
                " · thread ouvert"
            }
            Some(it)
                if it.kind == ItemKind::Top
                    && !self.messages[it.msg_idx].replies.is_empty() =>
            {
                " · thread fermé"
            }
            _ => "",
        };
        let reactions_marker = match visible.get(self.selected) {
            Some(it)
                if it.kind == ItemKind::Top
                    && !self.messages[it.msg_idx].reactions.is_empty() =>
            {
                let n: usize = self.messages[it.msg_idx]
                    .reactions
                    .iter()
                    .map(|r| r.users.len())
                    .sum();
                format!(" · ♥{}", n)
            }
            _ => String::new(),
        };
        if matches!(self.view, View::Conversation) {
            let focus = match self.focus {
                Focus::Conversation => "Conv",
                Focus::Input => "Saisie",
            };
            let win = if self.windows.len() > 1 {
                format!("[w{}/{}] ", self.active_window + 1, self.windows.len())
            } else {
                String::new()
            };
            self.status_text =
                format!("{}{} · {}{}{}", win, focus, pos, suffix, reactions_marker);
        } else {
            self.status_text = view_label.to_string();
        }
    }
}

fn message_text(m: &Message) -> String {
    let mut s = format!("{} {}", m.author, m.time);
    for b in &m.blocks {
        match b {
            Block::Text(t) => {
                s.push(' ');
                s.push_str(t);
            }
            Block::Code(t) => {
                s.push(' ');
                s.push_str(t);
            }
            Block::Voice { .. } => s.push_str(" voice note"),
        }
    }
    s
}

fn reply_text(r: &crate::message::ThreadReply) -> String {
    let mut s = format!("{} {}", r.author, r.time);
    for b in &r.blocks {
        match b {
            Block::Text(t) => {
                s.push(' ');
                s.push_str(t);
            }
            Block::Code(t) => {
                s.push(' ');
                s.push_str(t);
            }
            Block::Voice { .. } => s.push_str(" voice note"),
        }
    }
    s
}

fn collect_expanded_labels(
    nodes: &[crate::view::space_tree::Node],
) -> Vec<Vec<String>> {
    fn walk(
        node: &crate::view::space_tree::Node,
        path: &[String],
        out: &mut Vec<Vec<String>>,
    ) {
        if let crate::view::space_tree::NodeKind::Space {
            expanded, children, ..
        } = &node.kind
        {
            let mut p = path.to_vec();
            p.push(node.label.clone());
            if *expanded {
                out.push(p.clone());
            }
            for c in children {
                walk(c, &p, out);
            }
        }
    }
    let mut out = Vec::new();
    for n in nodes {
        walk(n, &[], &mut out);
    }
    out
}

fn expand_by_labels(
    nodes: &mut [crate::view::space_tree::Node],
    label_path: &[String],
) {
    if label_path.is_empty() {
        return;
    }
    let head = &label_path[0];
    let rest = &label_path[1..];
    for node in nodes.iter_mut() {
        if &node.label != head {
            continue;
        }
        if let crate::view::space_tree::NodeKind::Space {
            expanded, children, ..
        } = &mut node.kind
        {
            *expanded = true;
            if !rest.is_empty() {
                expand_by_labels(children, rest);
            }
        }
        return;
    }
}

/// Like `suspend_for_editor` but returns the post-edit content so the
/// caller can replace e.g. `app.input` with what the user wrote in the
/// editor. Returns `Ok(None)` if the file ends up empty.
fn suspend_for_input_editor(
    terminal: &mut DefaultTerminal,
    content: &str,
    editor: &str,
) -> io::Result<Option<String>> {
    use crossterm::terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    };

    let path = std::env::temp_dir().join(format!("matcurses-input-{}.txt", std::process::id()));
    std::fs::write(&path, content)?;

    crossterm::execute!(std::io::stdout(), LeaveAlternateScreen)?;
    disable_raw_mode()?;

    let cmd = if !editor.trim().is_empty() {
        editor.to_string()
    } else {
        std::env::var("EDITOR").unwrap_or_else(|_| "vi".into())
    };
    let _ = std::process::Command::new(&cmd).arg(&path).status();

    enable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen)?;
    terminal.clear()?;

    let result = std::fs::read_to_string(&path).ok().map(|s| {
        // Trim a single trailing newline (vi adds one) but keep internal
        // line breaks. The user explicitly went into the editor to type
        // multi-line content.
        let mut s = s;
        if s.ends_with('\n') {
            s.pop();
            if s.ends_with('\r') {
                s.pop();
            }
        }
        s
    });
    let _ = std::fs::remove_file(&path);
    Ok(result.filter(|s| !s.is_empty()))
}

/// Leave raw mode + the alternate screen, run the configured editor on a
/// temp file containing `content`, then re-enter the TUI. Resolution
/// order for the editor command: explicit `editor` argument (settings),
/// then the `$EDITOR` env var, then `vi` as a final fallback.
fn suspend_for_editor(
    terminal: &mut DefaultTerminal,
    content: &str,
    editor: &str,
) -> io::Result<()> {
    use crossterm::terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    };

    let path = std::env::temp_dir().join(format!("matcurses-msg-{}.txt", std::process::id()));
    std::fs::write(&path, content)?;

    crossterm::execute!(std::io::stdout(), LeaveAlternateScreen)?;
    disable_raw_mode()?;

    let cmd = if !editor.trim().is_empty() {
        editor.to_string()
    } else {
        std::env::var("EDITOR").unwrap_or_else(|_| "vi".into())
    };
    let _ = std::process::Command::new(&cmd).arg(&path).status();

    enable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen)?;
    terminal.clear()?;

    let _ = std::fs::remove_file(&path);
    Ok(())
}

fn slash_candidates(prefix: &str) -> Vec<String> {
    let lower = prefix.to_lowercase();
    SLASH_COMMANDS
        .iter()
        .filter(|c| c.to_lowercase().starts_with(&lower))
        .map(|c| (*c).to_string())
        .collect()
}

/// Walk `s` and return the (line, column) char-coordinates of the
/// `cursor` index (in chars).
fn cursor_line_col(s: &str, cursor: usize) -> (usize, usize) {
    let mut line = 0usize;
    let mut col = 0usize;
    for (i, c) in s.chars().enumerate() {
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

/// Convert a (line, column) target back to a flat char index in `s`,
/// clamping the column to the chosen line's length.
fn char_pos_for_line_col(s: &str, target_line: usize, target_col: usize) -> usize {
    let mut idx = 0usize;
    let mut line = 0usize;
    let mut col = 0usize;
    for (i, c) in s.chars().enumerate() {
        if line == target_line && col == target_col {
            return i;
        }
        if c == '\n' {
            if line == target_line {
                // Reached end of the target line: clamp here.
                return i;
            }
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
        idx = i + 1;
    }
    if line == target_line && col >= target_col {
        let _ = idx;
        return s.chars().count();
    }
    s.chars().count()
}

/// Heuristic detection of a mention of `me` in `message`. Matches the full
/// MXID or the localpart against any text/code block. Good enough for the
/// notification-sound gate; not authoritative for any security-bearing
/// decision.
fn is_mention(message: &Message, me: &str) -> bool {
    if me.is_empty() {
        return false;
    }
    let me_lower = me.to_lowercase();
    let local = me
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or("")
        .to_lowercase();
    for b in &message.blocks {
        let text = match b {
            Block::Text(t) => t,
            Block::Code(t) => t,
            Block::Voice { .. } => continue,
        };
        let t = text.to_lowercase();
        if t.contains(&me_lower) {
            return true;
        }
        if !local.is_empty() && t.contains(&local) {
            return true;
        }
    }
    false
}

fn append_blocks_preview(lines: &mut Vec<String>, blocks: &[Block]) {
    for block in blocks {
        match block {
            Block::Text(t) => {
                for l in t.lines() {
                    lines.push(format!("  {}", l));
                }
            }
            Block::Code(c) => {
                lines.push("  ─── code ───".into());
                for l in c.lines() {
                    lines.push(format!("  {}", l));
                }
                lines.push("  ─── fin code ───".into());
            }
            Block::Voice { duration_secs } => {
                let mins = duration_secs / 60;
                let secs = duration_secs % 60;
                lines.push(format!("  [note vocale · {}:{:02}]", mins, secs));
            }
        }
    }
}

