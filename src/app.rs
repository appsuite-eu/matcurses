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
    RecoveryInputModal, SasVerificationModal,
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

pub struct App {
    pub view: View,
    pub focus: Focus,
    pub input: String,
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
}

impl App {
    pub fn new() -> Self {
        let mut s = Self {
            view: View::Conversation,
            focus: Focus::Conversation,
            input: String::new(),
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
        };
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
                    }
                }
            } else {
                self.tick();
            }
            // Fetch and apply pending Matrix updates.
            self.apply_matrix_updates();
        }
        Ok(())
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

        self.current_room = display.clone();
        self.current_room_id = Some(id.clone());
        // Clear any previous messages (mock or previous room) so the user
        // doesn't see stale data while real messages load.
        self.messages.clear();
        self.expanded_threads.clear();
        self.selected = 0;
        self.scroll_top = 0;

        if let Some(b) = self.matrix.as_ref() {
            b.send(MxCommand::OpenRoom { room_id: id });
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
    pub fn submit_input(&mut self) {
        if self.input.is_empty() {
            return;
        }
        let raw = self.input.clone();
        self.input.clear();

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
                    let was_at_bottom = {
                        let v = self.visible_items();
                        v.is_empty() || self.selected + 1 >= v.len()
                    };
                    self.messages.push(message);
                    if was_at_bottom {
                        let v = self.visible_items();
                        self.selected = v.len().saturating_sub(1);
                    }
                    self.update_status();
                }
            }
            MxUpdate::Error { reason } => {
                self.flash = Some(reason);
            }
            MxUpdate::SyncComplete => {
                // Nothing to do for now — we already received Rooms just before.
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
            self.status_text =
                format!("{} · {}{}{}", focus, pos, suffix, reactions_marker);
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

