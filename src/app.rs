use crate::event::{handle_key, EventOutcome};
use crate::matrix::{Command as MxCommand, MatrixBridge, Update as MxUpdate};
use crate::message::{
    build_visible_items, mock_messages, Block, ItemKind, Message, Reaction, ViewItem,
};
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
    ReactionPickerModal,
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
    /// Pont vers le SDK Matrix. None si le runtime tokio n'a pas pu démarrer.
    pub matrix: Option<MatrixBridge>,
    /// IDs Matrix des rooms, alignés avec `room_list_state.rooms` (même longueur,
    /// même ordre). Vide tant qu'on n'a pas reçu de `Update::Rooms`.
    pub room_ids: Vec<String>,
    /// Room ID actuellement ouverte côté Matrix (None tant qu'on est sur les mocks).
    pub current_room_id: Option<String>,
    /// True une fois le login Matrix confirmé.
    pub matrix_logged_in: bool,
}

impl App {
    pub fn new() -> Self {
        let messages = mock_messages_with_extras();
        let expanded = HashSet::new();
        let visible = build_visible_items(&messages, &expanded);
        let selected = visible.len().saturating_sub(1);
        let mut s = Self {
            view: View::Conversation,
            focus: Focus::Conversation,
            input: String::new(),
            input_mode: InputMode::Normal,
            current_room: "#dev".to_string(),
            status_text: String::new(),
            should_quit: false,
            messages,
            expanded_threads: expanded,
            selected,
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
        };
        s.update_status();
        s
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| draw(frame, self))?;
            let timeout = self.next_timeout();
            if event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    if let EventOutcome::Quit = handle_key(self, key) {
                        self.should_quit = true;
                    }
                }
            } else {
                self.tick();
            }
            // Récupère et applique les updates Matrix en attente.
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
            // Sondage régulier pour récupérer les updates Matrix sans bloquer trop longtemps.
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
                lines.push("État    : envoyé · non édité · non chiffré (mock)".into());
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
                lines.push("État    : envoyé · non édité · non chiffré (mock)".into());
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
        let me = self.me.clone();
        let msg = &mut self.messages[msg_idx];
        if let Some(r) = msg.reactions.iter_mut().find(|r| r.key == key) {
            if let Some(pos) = r.users.iter().position(|u| u == &me) {
                r.users.remove(pos);
            } else {
                r.users.push(me.clone());
            }
        } else {
            msg.reactions.push(Reaction {
                key: key.clone(),
                users: vec![me],
            });
        }
        msg.reactions.retain(|r| !r.users.is_empty());
        self.flash = Some(format!("réaction {} basculée", key));
    }

    pub fn play_current_voice(&mut self) {
        let item = match self.current_item() {
            Some(it) => it,
            None => return,
        };
        let blocks = match item.kind {
            ItemKind::Top => &self.messages[item.msg_idx].blocks,
            ItemKind::Reply => {
                &self.messages[item.msg_idx].replies[item.reply_idx].blocks
            }
        };
        for b in blocks {
            if let Block::Voice { duration_secs } = b {
                let mins = duration_secs / 60;
                let secs = duration_secs % 60;
                self.flash = Some(format!("lecture voix {}:{:02} (mock)", mins, secs));
                return;
            }
        }
        self.flash = Some("pas de note vocale ici".into());
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
    }

    pub fn open_members(&mut self) {
        self.view = View::Members;
        self.update_status();
    }

    pub fn switch_room(&mut self, name: &str) {
        self.current_room = name.to_string();
        // Si Matrix est connecté, on récupère l'id correspondant et on déclenche le load.
        if self.matrix_logged_in {
            let idx = self
                .room_list_state
                .rooms
                .iter()
                .position(|r| r.name == name);
            if let Some(idx) = idx {
                if let Some(id) = self.room_ids.get(idx).cloned() {
                    self.current_room_id = Some(id.clone());
                    if let Some(b) = self.matrix.as_ref() {
                        b.send(MxCommand::OpenRoom { room_id: id });
                    }
                    self.flash = Some(format!("ouverture {}", name));
                } else {
                    self.flash = Some(format!("room {} : id manquant", name));
                }
            } else {
                self.flash = Some(format!("room {} introuvable", name));
            }
        } else {
            self.flash = Some(format!("ouverture {} (mock)", name));
        }
        self.view = View::Conversation;
        self.update_status();
    }

    /// Lance le login Matrix avec les valeurs courantes du formulaire.
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

    /// Envoie le contenu du buffer de saisie dans la room courante (si Matrix actif).
    pub fn submit_input(&mut self) {
        if self.input.is_empty() {
            return;
        }
        if self.matrix_logged_in {
            if let (Some(id), Some(b)) = (self.current_room_id.clone(), self.matrix.as_ref()) {
                let body = self.input.clone();
                b.send(MxCommand::SendMessage {
                    room_id: id,
                    body,
                });
                self.input.clear();
                return;
            }
        }
        // Fallback : on vide juste le buffer (comportement mock historique).
        self.input.clear();
    }

    /// Récupère et applique les updates Matrix en attente. Appelé à chaque
    /// tour de boucle UI.
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
                self.flash = Some(format!("connecté · {}", mxid));
                // Retour à la conversation si on était sur Login.
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
                // Préserver la sélection si possible (par nom).
                let prev_name = self.room_list_state.selected_room_name();
                self.room_list_state.rooms = rooms;
                self.room_ids = ids;
                // Tri couplé rooms+ids (garde l'alignement nom↔id).
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
                // rien à faire pour l'instant — on a déjà reçu Rooms juste avant.
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
            "Devices : 2 vérifiés (mock)".into(),
            "Vérifié : oui (mock)".into(),
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
            // Empty query : pas de matches, curseur revient à la position d'origine
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

fn mock_messages_with_extras() -> Vec<Message> {
    let mut messages = mock_messages();
    // Voice notes
    messages.insert(5, Message::voice("09:04", "carol", 28));
    messages.push(Message::voice("10:06", "dave", 73));
    // Reactions sur quelques messages
    let alice_postmortem_idx = messages
        .iter()
        .position(|m| m.author == "alice" && m.time == "10:02")
        .unwrap_or(0);
    messages[alice_postmortem_idx]
        .reactions
        .push(Reaction {
            key: "+1".into(),
            users: vec!["bob".into(), "carol".into(), "dave".into()],
        });
    messages[alice_postmortem_idx]
        .reactions
        .push(Reaction {
            key: "heart".into(),
            users: vec!["carol".into()],
        });
    let go_idx = messages
        .iter()
        .position(|m| m.author == "alice" && m.time == "10:05")
        .unwrap_or(0);
    messages[go_idx].reactions.push(Reaction {
        key: "fire".into(),
        users: vec!["bob".into(), "dave".into()],
    });
    messages
}
