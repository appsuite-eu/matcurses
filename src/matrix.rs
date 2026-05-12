//! Bridge between the synchronous UI (crossterm loop) and the async Matrix SDK.
//!
//! The `MatrixBridge` holds a tokio runtime in the background, two `mpsc`
//! channels (UI → bg = Command, bg → UI = Update) and runs a task that
//! drives `matrix_sdk::Client` (login, sync, send, etc.).
//!
//! The `widgets/` crate does not depend on this module — `app.rs` is in
//! charge of mapping Updates to UI state.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::authentication::matrix::MatrixSession;
use matrix_sdk::config::SyncSettings;
use matrix_sdk::media::{MediaFormat, MediaRequestParameters};
use matrix_sdk::store::RoomLoadSettings;
use matrix_sdk::room::{MessagesOptions, RoomMember};
use matrix_sdk::ruma::events::room::message::{
    MessageType, RoomMessageEventContent, SyncRoomMessageEvent,
};
use matrix_sdk::ruma::events::AnyMessageLikeEventContent;
use matrix_sdk::ruma::{OwnedEventId, OwnedRoomId};
use matrix_sdk::{Client, RoomMemberships, RoomState};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::message::{Block, Message};
use crate::view::members::{Member as UiMember, Presence as UiPresence};
use crate::view::room_list::Room as UiRoom;
use crate::view::space_tree::{Node as UiNode, NodeKind as UiNodeKind};

/// Which slice of a homeserver's public directory to surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicKind {
    /// Regular rooms (`m.room` / no `room_type`); excludes spaces.
    Rooms,
    /// Spaces only (`room_type = "m.space"`).
    Spaces,
}

/// One entry from a public room directory query, flattened down to what
/// the UI list needs.
#[derive(Debug, Clone)]
pub struct PublicRoomEntry {
    pub name: String,
    pub topic: Option<String>,
    pub members: u64,
    /// Preferred join target: `#alias:server` if the room has a canonical
    /// alias, otherwise the bare room id (works for federated joins too).
    pub join_target: String,
}

/// Commands sent from the UI to the Matrix task.
#[derive(Debug, Clone)]
pub enum Command {
    /// Password login: MXID + password + server (may be empty → autodiscover via MXID)
    Login {
        mxid: String,
        password: String,
        server: String,
    },
    /// SSO login: opens the homeserver's SSO redirect URL in the system
    /// browser, then exchanges the resulting login token for a session.
    /// `server` is required (we don't have an MXID to autodiscover from).
    LoginSso { server: String, idp_id: Option<String> },
    /// The user selected a room in the list — load its contents.
    OpenRoom { room_id: String },
    /// Send a text message to the active room (user pressed Enter).
    /// `reply_to` and `thread_root` populate the corresponding
    /// `m.relates_to` relation when set.
    SendMessage {
        room_id: String,
        body: String,
        reply_to: Option<String>,
        thread_root: Option<String>,
    },
    /// Force a refresh of the rooms list.
    #[allow(dead_code)]
    RefreshRooms,
    /// Load joined members of a room.
    LoadMembers { room_id: String },
    /// Load the spaces tree (top-level + their children).
    LoadSpaces,
    /// Lazy-load the children of a single space (the user expanded a
    /// node whose subtree was not in the initial hierarchy fetch).
    LoadSpaceChildren { room_id: String },
    /// Try to restore a previous session from the SQLite store.
    /// On success → Update::LoggedIn + continuous sync. Otherwise → silence.
    TryRestore,
    /// Download the audio media for the given event and play it through
    /// the in-process rodio player (with fallback to the system player if
    /// the format is not supported, e.g. Opus).
    PlayVoice { room_id: String, event_id: String },
    /// Stop the currently-playing voice note, if any.
    StopVoice,
    /// Play a short event notification sound (new message, mention,
    /// call ringing, …). Plays on a detached sink so it does not
    /// preempt voice-note playback.
    PlaySound { kind: crate::sounds::SoundKind },
    /// Send an `m.reaction` to a parent event in a room.
    SendReaction {
        room_id: String,
        parent_event_id: String,
        key: String,
    },
    /// Redact (delete) an event we own — typically used to toggle off a
    /// reaction we previously sent.
    RedactEvent { room_id: String, event_id: String },
    /// Send an `m.emote` (the IRC `/me` action).
    SendEmote { room_id: String, body: String },
    /// Edit a previously-sent text message (`m.replace`). Only valid for
    /// events whose original sender is the current user; the homeserver
    /// will reject otherwise.
    EditMessage {
        room_id: String,
        event_id: String,
        body: String,
    },
    /// Join a room by alias (`#name:server`) or id. `via` are server
    /// hints needed to federated-join a remote room by id; safe to pass
    /// empty when the target is an alias or already known locally.
    JoinRoom {
        alias_or_id: String,
        via: Vec<String>,
    },
    /// Create a new room. When `is_direct` is true the homeserver flags
    /// the room as a 1:1 DM and tracks it in `m.direct`. `invite` is the
    /// initial list of MXIDs to invite (typically the DM partner for the
    /// DM case, may be empty for a regular `/create`).
    CreateRoom {
        name: Option<String>,
        is_direct: bool,
        invite: Vec<String>,
    },
    /// Invite a user to the given room. Requires the local user to have
    /// at least the `invite` power level there.
    InviteUser { room_id: String, user_id: String },
    /// Accept a pending invitation for the given room (RoomState::Invited).
    AcceptInvite { room_id: String },
    /// Decline a pending invitation. The room is left and dropped from
    /// the local rooms list on the next sync.
    RejectInvite { room_id: String },
    /// Kick a user from a room (revocable join). Requires `kick` power.
    KickUser {
        room_id: String,
        user_id: String,
        reason: Option<String>,
    },
    /// Ban a user from a room (durable). Requires `ban` power.
    BanUser {
        room_id: String,
        user_id: String,
        reason: Option<String>,
    },
    /// Lift a previous ban. Requires `ban` power.
    UnbanUser { room_id: String, user_id: String },
    /// Set or reset `user_id`'s power level in `room_id`. Spec levels:
    /// 0 = normal, 50 = moderator, 100 = admin. Requires being above the
    /// target level yourself.
    SetPowerLevel {
        room_id: String,
        user_id: String,
        level: i64,
    },
    /// Replace the room topic (an `m.room.topic` state event).
    SetTopic { room_id: String, topic: String },
    /// Replace the room name (an `m.room.name` state event).
    SetRoomName { room_id: String, name: String },
    /// Update the local user's display name. `name = ""` clears it.
    SetDisplayName { name: String },
    /// Upload the given local file as the local user's avatar and point
    /// the account at the resulting MXC URI.
    SetAvatar { path: String },
    /// Send a file from disk as an attachment. The msgtype (m.image /
    /// m.audio / m.video / m.file) is derived from the MIME type. In an
    /// encrypted room the SDK encrypts the upload transparently.
    SendAttachment { room_id: String, path: String },
    /// Fetch the public room directory of `server` (or the local server
    /// when empty), filtered by kind. Surfaces an `Update::PublicRooms`.
    DiscoverPublicRooms { server: String, kind: PublicKind },
    /// Leave the given room.
    LeaveRoom { room_id: String },
    /// Restore E2EE keys (cross-signing + Megolm key backup) from a
    /// recovery key string. Used to read historical encrypted messages
    /// after logging in on a fresh device.
    RecoverFromKey { key: String },
    /// Bootstrap E2EE: set up cross-signing + a server-side key backup
    /// secured by a freshly-generated recovery key. The key is returned
    /// via `Update::RecoveryKeyGenerated` for the user to save.
    EnableRecovery,
    /// Log out from the homeserver, revoke the access token, wipe the
    /// local session.json + last_mxid + SQLite store for this account,
    /// and clear the client. After this the bridge is back to the
    /// "no session" state.
    Logout,
    /// Initiate a SAS verification request against `user_id`. Typically
    /// used to verify another device of the same user.
    VerifyUser { user_id: String },
    /// Acknowledge that the SAS values match (mark the device verified).
    SasConfirm,
    /// Acknowledge a SAS mismatch (abort the verification, alert).
    SasMismatch,
    /// Cancel the in-flight SAS verification.
    SasCancel,
}

/// Updates pushed from the Matrix task to the UI.
pub enum Update {
    /// Login OK: effective MXID (in case of autodiscovery).
    LoggedIn { mxid: String },
    /// Logout completed (or attempted). The UI should clear all
    /// session-bound state (rooms, messages, members, spaces…).
    LoggedOut,
    /// Login failed: human-readable error message.
    LoginFailed { reason: String },
    /// Rooms list updated (sync or manual refresh).
    Rooms {
        rooms: Vec<UiRoom>,
        ids: Vec<String>,
    },
    /// Room history loaded / refreshed.
    RoomMessages {
        room_id: String,
        messages: Vec<Message>,
    },
    /// New event received on a room (during live sync).
    NewMessage {
        room_id: String,
        message: Message,
    },
    /// Generic error message (sync, send, etc.) — to display as flash.
    Error { reason: String },
    /// Initial sync complete.
    SyncComplete,
    /// Members of a room (in response to LoadMembers).
    Members {
        room_id: String,
        members: Vec<UiMember>,
    },
    /// Spaces tree (in response to LoadSpaces).
    Spaces { roots: Vec<UiNode> },
    /// Children of a single space, in response to `LoadSpaceChildren`.
    /// `parent_id` is the room id the user expanded; `children` are the
    /// nodes to splice in place of its current (empty) children list.
    SpaceChildren {
        parent_id: String,
        children: Vec<UiNode>,
    },
    /// Result of a `DiscoverPublicRooms` query.
    PublicRooms {
        server: String,
        kind: PublicKind,
        entries: Vec<PublicRoomEntry>,
    },
    /// E2EE recovery succeeded: keys imported, the UI may want to
    /// refetch the current room so previously-undecryptable messages
    /// show up in clear.
    RecoverySuccess,
    /// Recovery has been freshly enabled and a new recovery key was
    /// generated. The UI must show this to the user — they will not
    /// be able to retrieve it again later.
    RecoveryKeyGenerated { key: String },
    /// SAS verification has reached the comparison phase: show the
    /// decimal triple and emoji words to the user, who confirms match
    /// or mismatch.
    SasReady {
        decimal: (u16, u16, u16),
        emoji: Vec<(String, String)>,
    },
    /// SAS verification finished. `ok = true` means it was confirmed
    /// matching on both sides; `false` means mismatch / cancel / error.
    SasDone { ok: bool },
    /// Presence update for a single member, scoped to the room it was
    /// requested from. Fired after `LoadMembers` once each per-user
    /// `GET /presence/{user}/status` response comes back.
    MemberPresence {
        room_id: String,
        mxid: String,
        presence: UiPresence,
    },
}

/// Audio commands sent to the dedicated audio thread.
enum AudioCommand {
    Play {
        bytes: Vec<u8>,
        ack: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Stop,
    Pause,
    Resume,
    /// Play a short event notification sound on a detached sink. Multiple
    /// concurrent event sounds may stack; voice-note playback is unaffected.
    PlaySound {
        bytes: &'static [u8],
    },
}

/// Shared handle on the currently-playing voice-note sink. The audio thread
/// owns the rodio `OutputStream` (which is `!Send`) but the `Sink` itself is
/// `Send + Sync` and exposes `pause()`, `play()`, `get_pos()`, etc., so the
/// UI thread can both control playback and read its position through this
/// mutex. The `tempo` atomic is read each refill by `StretchSource` so the
/// UI thread can change pitch-preserving playback speed mid-playback.
pub struct AudioControl {
    current: std::sync::Mutex<Option<rodio::Sink>>,
    tempo: crate::audio::SharedTempo,
    /// Source-time playback position in seconds, updated by `StretchSource`
    /// as it pulls frames from the decoder. Independent of tempo, so
    /// changing playback speed does not skew the displayed position.
    position: crate::audio::SharedPosition,
}

impl Default for AudioControl {
    fn default() -> Self {
        Self {
            current: std::sync::Mutex::new(None),
            tempo: crate::audio::make_shared_tempo(),
            position: crate::audio::make_shared_position(),
        }
    }
}

/// Snapshot of the active voice-note playback, sampled by the UI each frame.
pub struct VoiceStatus {
    pub pos_secs: f32,
    pub speed: f32,
    pub paused: bool,
    pub finished: bool,
}

/// UI-side bridge. Owns the sender/receiver and the tokio runtime.
pub struct MatrixBridge {
    pub cmd_tx: Sender<Command>,
    update_rx: Receiver<Update>,
    audio_tx: std::sync::mpsc::Sender<AudioCommand>,
    audio_control: std::sync::Arc<AudioControl>,
    /// The runtime is kept alive as long as the bridge exists.
    _runtime: Runtime,
}

impl MatrixBridge {
    pub fn spawn() -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()?;
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(32);
        let (update_tx, update_rx) = mpsc::channel::<Update>(64);

        // Dedicated audio thread (rodio's OutputStream is !Send so it must
        // live on a single thread that owns it).
        let (audio_tx, audio_rx) = std::sync::mpsc::channel::<AudioCommand>();
        let audio_control = std::sync::Arc::new(AudioControl::default());
        let audio_control_thread = audio_control.clone();
        std::thread::spawn(move || audio_thread(audio_rx, audio_control_thread));

        runtime.spawn(matrix_main(cmd_rx, update_tx, audio_tx.clone()));

        Ok(Self {
            cmd_tx,
            update_rx,
            audio_tx,
            audio_control,
            _runtime: runtime,
        })
    }

    /// Pause the active voice playback. No-op if nothing is playing.
    pub fn voice_pause(&self) {
        let _ = self.audio_tx.send(AudioCommand::Pause);
    }

    /// Resume the active (paused) voice playback. No-op if nothing is paused.
    pub fn voice_resume(&self) {
        let _ = self.audio_tx.send(AudioCommand::Resume);
    }

    /// Adjust the playback speed of the active voice (typically 0.5..=2.0).
    /// Pitch-preserving: the change is applied through SoundTouch inside
    /// `StretchSource`, not through `Sink::set_speed` which only resamples.
    pub fn voice_set_speed(&self, speed: f32) {
        crate::audio::store_tempo(&self.audio_control.tempo, speed);
    }

    /// Snapshot the current voice playback state, or `None` if no voice
    /// note is currently loaded into the sink.
    pub fn voice_status(&self) -> Option<VoiceStatus> {
        let guard = self.audio_control.current.lock().ok()?;
        let sink = guard.as_ref()?;
        Some(VoiceStatus {
            // Source-time position published by `StretchSource`, not the
            // sink's wall-clock counter — wall-clock would diverge from
            // the displayed source duration as soon as tempo ≠ 1.0.
            pos_secs: crate::audio::load_position(&self.audio_control.position),
            speed: crate::audio::load_tempo(&self.audio_control.tempo),
            paused: sink.is_paused(),
            finished: sink.empty(),
        })
    }

    /// Non-blocking command send. If the channel is full, we log and drop it.
    pub fn send(&self, cmd: Command) {
        if let Err(e) = self.cmd_tx.try_send(cmd) {
            // No proper log here — we don't want to break the UI.
            // A dropped command is usually benign (refresh).
            let _ = e;
        }
    }

    /// Drain pending updates without blocking.
    pub fn drain_updates(&mut self) -> Vec<Update> {
        let mut out = Vec::new();
        while let Ok(u) = self.update_rx.try_recv() {
            out.push(u);
        }
        out
    }
}

/// Audio playback thread: owns the rodio OutputStream and processes
/// AudioCommands one at a time. Runs until the channel is closed.
fn audio_thread(
    rx: std::sync::mpsc::Receiver<AudioCommand>,
    control: std::sync::Arc<AudioControl>,
) {
    let stream_pair = match rodio::OutputStream::try_default() {
        Ok(p) => p,
        Err(_) => return, // No audio device — silently drop further commands.
    };
    let (_stream, handle) = stream_pair;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            AudioCommand::Play { bytes, ack } => {
                if let Ok(mut g) = control.current.lock() {
                    if let Some(s) = g.take() {
                        s.stop();
                    }
                }
                // Reset tempo so a new voice starts at 1.0× regardless of
                // what the previous playback was set to.
                crate::audio::store_tempo(&control.tempo, 1.0);
                crate::audio::store_position(&control.position, 0.0);
                match try_play(
                    &handle,
                    bytes,
                    control.tempo.clone(),
                    control.position.clone(),
                ) {
                    Ok(sink) => {
                        if let Ok(mut g) = control.current.lock() {
                            *g = Some(sink);
                        }
                        let _ = ack.send(Ok(()));
                    }
                    Err(e) => {
                        let _ = ack.send(Err(e));
                    }
                }
            }
            AudioCommand::Stop => {
                if let Ok(mut g) = control.current.lock() {
                    if let Some(s) = g.take() {
                        s.stop();
                    }
                }
            }
            AudioCommand::Pause => {
                if let Ok(g) = control.current.lock() {
                    if let Some(s) = g.as_ref() {
                        s.pause();
                    }
                }
            }
            AudioCommand::Resume => {
                if let Ok(g) = control.current.lock() {
                    if let Some(s) = g.as_ref() {
                        s.play();
                    }
                }
            }
            AudioCommand::PlaySound { bytes } => {
                let cursor = std::io::Cursor::new(bytes);
                if let Ok(source) = rodio::Decoder::new(cursor) {
                    if let Ok(sink) = rodio::Sink::try_new(&handle) {
                        sink.append(source);
                        // Detach: the sink keeps playing after we drop it,
                        // so multiple event sounds can overlap without us
                        // tracking lifetimes.
                        sink.detach();
                    }
                }
            }
        }
    }
}

/// Try to decode `bytes` and produce a playing `Sink`. Tries rodio's built-in
/// decoders first (MP3/M4A/FLAC/WAV/Vorbis via Symphonia), then falls back
/// to our custom OGG/Opus path. The decoded source is wrapped in a
/// `StretchSource` so the UI thread can adjust tempo (pitch-preserving)
/// through the shared atomic.
fn try_play(
    handle: &rodio::OutputStreamHandle,
    bytes: Vec<u8>,
    tempo: crate::audio::SharedTempo,
    position: crate::audio::SharedPosition,
) -> Result<rodio::Sink, String> {
    // First attempt: rodio + Symphonia.
    let bytes_for_rodio = bytes.clone();
    if let Ok(source) = rodio::Decoder::new(std::io::Cursor::new(bytes_for_rodio)) {
        let sink = rodio::Sink::try_new(handle).map_err(|e| format!("sink : {e}"))?;
        sink.append(crate::audio::StretchSource::new(source, tempo, position));
        return Ok(sink);
    }
    // Second attempt: OGG/Opus (covers the common voice-note case).
    match crate::audio::OpusSource::try_from_bytes(bytes) {
        Ok(source) => {
            let sink = rodio::Sink::try_new(handle).map_err(|e| format!("sink : {e}"))?;
            sink.append(crate::audio::StretchSource::new(source, tempo, position));
            Ok(sink)
        }
        Err(opus_err) => Err(format!("aucun décodeur supporté ({opus_err})")),
    }
}

#[derive(Debug)]
enum SasUserDecision {
    Confirm,
    Mismatch,
    Cancel,
}

/// Main loop of the Matrix task: receives commands, drives the client.
async fn matrix_main(
    mut cmd_rx: Receiver<Command>,
    update_tx: Sender<Update>,
    audio_tx: std::sync::mpsc::Sender<AudioCommand>,
) {
    use std::sync::Arc;
    use tokio::sync::Mutex as AsyncMutex;
    let pending_sas: Arc<AsyncMutex<Option<tokio::sync::oneshot::Sender<SasUserDecision>>>> =
        Arc::new(AsyncMutex::new(None));
    // Tracks the currently open room so the sync callback can refetch its
    // timeline whenever new events arrive — a safety net for cases where
    // the SyncRoomMessageEvent live handler does not fire (e.g. delayed
    // Megolm decryption on bridge-encrypted rooms).
    let current_room: Arc<AsyncMutex<Option<OwnedRoomId>>> =
        Arc::new(AsyncMutex::new(None));
    let mut client: Option<Arc<Client>> = None;

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            Command::Login {
                mxid,
                password,
                server,
            } => {
                match do_login(&mxid, &password, &server).await {
                    Ok(c) => {
                        // Persist MXID + serialized session so we can auto-restore.
                        let normalized_mxid = if mxid.starts_with('@') {
                            mxid.clone()
                        } else {
                            format!("@{mxid}")
                        };
                        if let Ok(p) = last_mxid_file() {
                            let _ = std::fs::write(&p, &normalized_mxid);
                        }
                        if let Some(session) = c.matrix_auth().session() {
                            if let Ok(store_path) = session_store_path(&normalized_mxid) {
                                if let Ok(json) = serde_json::to_string(&session) {
                                    let _ = std::fs::write(store_path.join("session.json"), json);
                                }
                            }
                        }

                        let arc = Arc::new(c);
                        client = Some(arc.clone());
                        let _ = update_tx
                            .send(Update::LoggedIn {
                                mxid: normalized_mxid,
                            })
                            .await;

                        let tx = update_tx.clone();
                        let arc2 = arc.clone();
                        let pending = pending_sas.clone();
                        let cur = current_room.clone();
                        tokio::spawn(async move {
                            run_sync(arc2, tx, pending, cur).await;
                        });
                    }
                    Err(e) => {
                        let _ = update_tx
                            .send(Update::LoginFailed {
                                reason: format!("{e}"),
                            })
                            .await;
                    }
                }
            }
            Command::LoginSso { server, idp_id } => {
                // Same inline shape as Command::Login: the dispatcher blocks
                // while the browser-based authentication completes. That is
                // acceptable because we have nothing else useful to do until
                // login has produced a Client to store in `client`.
                let _ = update_tx
                    .send(Update::Error {
                        reason: "ouverture du navigateur pour SSO…".into(),
                    })
                    .await;
                match do_login_sso(&server, idp_id.as_deref()).await {
                    Ok(c) => {
                        let mxid = c
                            .user_id()
                            .map(|u| u.to_string())
                            .unwrap_or_default();
                        if let Ok(p) = last_mxid_file() {
                            let _ = std::fs::write(&p, &mxid);
                        }
                        if let Some(session) = c.matrix_auth().session() {
                            if let Ok(store_path) = session_store_path(&mxid) {
                                if let Ok(json) = serde_json::to_string(&session) {
                                    let _ = std::fs::write(
                                        store_path.join("session.json"),
                                        json,
                                    );
                                }
                            }
                        }
                        let arc = Arc::new(c);
                        client = Some(arc.clone());
                        let _ = update_tx.send(Update::LoggedIn { mxid }).await;

                        let tx = update_tx.clone();
                        let arc2 = arc.clone();
                        let pending = pending_sas.clone();
                        let cur = current_room.clone();
                        tokio::spawn(async move {
                            run_sync(arc2, tx, pending, cur).await;
                        });
                    }
                    Err(e) => {
                        let _ = update_tx
                            .send(Update::LoginFailed {
                                reason: format!("{e}"),
                            })
                            .await;
                    }
                }
            }
            Command::TryRestore => {
                match try_restore_session().await {
                    Ok(Some((c, mxid))) => {
                        let arc = Arc::new(c);
                        client = Some(arc.clone());
                        let _ = update_tx.send(Update::LoggedIn { mxid }).await;

                        let tx = update_tx.clone();
                        let arc2 = arc.clone();
                        let pending = pending_sas.clone();
                        let cur = current_room.clone();
                        tokio::spawn(async move {
                            run_sync(arc2, tx, pending, cur).await;
                        });
                    }
                    Ok(None) => {
                        // No session: wait for a Command::Login. We don't push a flash
                        // here to avoid polluting the UI on a normal cold start.
                    }
                    Err(e) => {
                        let _ = update_tx
                            .send(Update::Error {
                                reason: format!("restore session : {e}"),
                            })
                            .await;
                    }
                }
            }
            Command::OpenRoom { room_id } => {
                // Record the active room so the sync callback can refetch
                // its timeline whenever new events arrive on it.
                if let Ok(parsed) = room_id.parse::<OwnedRoomId>() {
                    *current_room.lock().await = Some(parsed);
                }
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) = load_room_messages(&c, &room_id, &tx).await {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("ouverture room : {e}"),
                                })
                                .await;
                        }
                    });
                }
            }
            Command::SendMessage { room_id, body, reply_to, thread_root } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) = do_send(&c, &room_id, &body, reply_to, thread_root).await {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("envoi : {e}"),
                                })
                                .await;
                        }
                    });
                }
            }
            Command::RefreshRooms => {
                if let Some(c) = &client {
                    let _ = update_tx.send(snapshot_rooms(c).await).await;
                }
            }
            Command::LoadMembers { room_id } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) = load_members(&c, &room_id, &tx).await {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("membres : {e}"),
                                })
                                .await;
                        }
                    });
                }
            }
            Command::PlayVoice { room_id, event_id } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    let audio_tx = audio_tx.clone();
                    tokio::spawn(async move {
                        match download_voice(&c, &room_id, &event_id).await {
                            Ok((bytes, mime)) => {
                                let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
                                let bytes_clone = bytes.clone();
                                if audio_tx
                                    .send(AudioCommand::Play {
                                        bytes: bytes_clone,
                                        ack: ack_tx,
                                    })
                                    .is_err()
                                {
                                    let _ = tx
                                        .send(Update::Error {
                                            reason: "audio thread indisponible".into(),
                                        })
                                        .await;
                                    return;
                                }
                                match ack_rx.await {
                                    Ok(Ok(())) => {
                                        let _ = tx
                                            .send(Update::Error {
                                                reason: "lecture en cours".into(),
                                            })
                                            .await;
                                    }
                                    Ok(Err(decode_err)) => {
                                        // Format not decodable in-process (commonly Opus).
                                        // Fall back to writing a temp file and opening it
                                        // with the system audio player.
                                        match save_and_open(
                                            &bytes,
                                            &event_id,
                                            ext_for_mime(mime.as_deref()),
                                        ) {
                                            Ok(_) => {
                                                let _ = tx.send(Update::Error {
                                                    reason: format!(
                                                        "format non géré ({}); ouverture player externe",
                                                        decode_err
                                                    ),
                                                })
                                                .await;
                                            }
                                            Err(e2) => {
                                                let _ = tx.send(Update::Error {
                                                    reason: format!(
                                                        "lecture KO : {decode_err} / {e2}"
                                                    ),
                                                })
                                                .await;
                                            }
                                        }
                                    }
                                    Err(_) => {
                                        let _ = tx
                                            .send(Update::Error {
                                                reason: "audio thread mort".into(),
                                            })
                                            .await;
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("lecture voix : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::StopVoice => {
                let _ = audio_tx.send(AudioCommand::Stop);
            }
            Command::PlaySound { kind } => {
                let _ = audio_tx.send(AudioCommand::PlaySound {
                    bytes: crate::sounds::bytes_for(kind),
                });
            }
            Command::SendReaction {
                room_id,
                parent_event_id,
                key,
            } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        match send_reaction(&c, &room_id, &parent_event_id, &key).await {
                            Ok(()) => {
                                // Brief delay then refetch so the new reaction
                                // shows up in the timeline.
                                tokio::time::sleep(Duration::from_millis(300)).await;
                                let _ = load_room_messages(&c, &room_id, &tx).await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("envoi réaction : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::SendEmote { room_id, body } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) = send_emote(&c, &room_id, &body).await {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("envoi /me : {e}"),
                                })
                                .await;
                        }
                    });
                }
            }
            Command::EditMessage { room_id, event_id, body } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        match do_edit(&c, &room_id, &event_id, &body).await {
                            Ok(()) => {
                                tokio::time::sleep(Duration::from_millis(300)).await;
                                let _ = load_room_messages(&c, &room_id, &tx).await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("édition : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::DiscoverPublicRooms { server, kind } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        match discover_public_rooms(&c, &server, kind).await {
                            Ok(entries) => {
                                let _ = tx
                                    .send(Update::PublicRooms { server, kind, entries })
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("/discover : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::CreateRoom { name, is_direct, invite } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        match create_room(&c, name.as_deref(), is_direct, &invite).await {
                            Ok(label) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("room créée : {label}"),
                                    })
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("/create : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::KickUser { room_id, user_id, reason } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            kick_user(&c, &room_id, &user_id, reason.as_deref()).await
                        {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("/kick : {e}"),
                                })
                                .await;
                        } else {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("kické : {user_id}"),
                                })
                                .await;
                        }
                    });
                }
            }
            Command::BanUser { room_id, user_id, reason } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            ban_user(&c, &room_id, &user_id, reason.as_deref()).await
                        {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("/ban : {e}"),
                                })
                                .await;
                        } else {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("banni : {user_id}"),
                                })
                                .await;
                        }
                    });
                }
            }
            Command::UnbanUser { room_id, user_id } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) = unban_user(&c, &room_id, &user_id).await {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("/unban : {e}"),
                                })
                                .await;
                        } else {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("débanni : {user_id}"),
                                })
                                .await;
                        }
                    });
                }
            }
            Command::SetPowerLevel { room_id, user_id, level } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            set_power_level(&c, &room_id, &user_id, level).await
                        {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("/op : {e}"),
                                })
                                .await;
                        } else {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("{user_id} → power level {level}"),
                                })
                                .await;
                        }
                    });
                }
            }
            Command::SetTopic { room_id, topic } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) = set_topic(&c, &room_id, &topic).await {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("/topic : {e}"),
                                })
                                .await;
                        }
                    });
                }
            }
            Command::SetRoomName { room_id, name } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) = set_room_name(&c, &room_id, &name).await {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("/name : {e}"),
                                })
                                .await;
                        }
                    });
                }
            }
            Command::SendAttachment { room_id, path } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        match send_attachment(&c, &room_id, &path).await {
                            Ok(name) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("envoyé : {name}"),
                                    })
                                    .await;
                                // Refresh so the new event lands in the UI.
                                tokio::time::sleep(Duration::from_millis(300)).await;
                                let _ = load_room_messages(&c, &room_id, &tx).await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("/upload : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::SetDisplayName { name } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        let new_name = if name.trim().is_empty() {
                            None
                        } else {
                            Some(name.trim().to_string())
                        };
                        match c.account().set_display_name(new_name.as_deref()).await {
                            Ok(()) => {
                                let label = new_name
                                    .clone()
                                    .unwrap_or_else(|| "(vide)".to_string());
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("display name : {label}"),
                                    })
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("/nick : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::SetAvatar { path } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        match upload_avatar(&c, &path).await {
                            Ok(()) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: "avatar mis à jour".into(),
                                    })
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("/avatar : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::AcceptInvite { room_id } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        match accept_invite(&c, &room_id).await {
                            Ok(()) => {
                                let _ = tx.send(snapshot_rooms(&c).await).await;
                                let _ = load_room_messages(&c, &room_id, &tx).await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("/accept : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::RejectInvite { room_id } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        match reject_invite(&c, &room_id).await {
                            Ok(()) => {
                                let _ = tx.send(snapshot_rooms(&c).await).await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("/reject : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::InviteUser { room_id, user_id } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) = invite_user(&c, &room_id, &user_id).await {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("/invite : {e}"),
                                })
                                .await;
                        } else {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("invité : {user_id}"),
                                })
                                .await;
                        }
                    });
                }
            }
            Command::JoinRoom { alias_or_id, via } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        match join_room(&c, &alias_or_id, &via).await {
                            Ok(name) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("rejoint : {name}"),
                                    })
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("/join : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::VerifyUser { user_id } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    let pending = pending_sas.clone();
                    tokio::spawn(async move {
                        let (decision_tx, decision_rx) = tokio::sync::oneshot::channel();
                        *pending.lock().await = Some(decision_tx);
                        let result = run_sas_verification(&c, &user_id, &tx, decision_rx).await;
                        // Drop any pending decision sender that wasn't claimed.
                        let _ = pending.lock().await.take();
                        match result {
                            Ok(ok) => {
                                let _ = tx.send(Update::SasDone { ok }).await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("verify : {e}"),
                                    })
                                    .await;
                                let _ = tx.send(Update::SasDone { ok: false }).await;
                            }
                        }
                    });
                }
            }
            Command::SasConfirm => {
                if let Some(s) = pending_sas.lock().await.take() {
                    let _ = s.send(SasUserDecision::Confirm);
                }
            }
            Command::SasMismatch => {
                if let Some(s) = pending_sas.lock().await.take() {
                    let _ = s.send(SasUserDecision::Mismatch);
                }
            }
            Command::SasCancel => {
                if let Some(s) = pending_sas.lock().await.take() {
                    let _ = s.send(SasUserDecision::Cancel);
                }
            }
            Command::Logout => {
                // Best-effort: try to revoke the token server-side, then wipe
                // local persistence regardless of network success so we end
                // up in a clean "logged out" state.
                let mxid = client
                    .as_ref()
                    .and_then(|c| c.user_id().map(|u| u.to_string()))
                    .unwrap_or_default();
                if let Some(c) = &client {
                    let _ = c.matrix_auth().logout().await;
                }
                if !mxid.is_empty() {
                    if let Ok(p) = session_store_path(&mxid) {
                        let _ = std::fs::remove_dir_all(&p);
                    }
                }
                if let Ok(p) = last_mxid_file() {
                    let _ = std::fs::remove_file(&p);
                }
                client = None;
                let _ = update_tx.send(Update::LoggedOut).await;
            }
            Command::EnableRecovery => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        match enable_recovery(&c).await {
                            Ok(key) => {
                                let _ = tx
                                    .send(Update::RecoveryKeyGenerated { key })
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("setup E2EE : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::RecoverFromKey { key } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        match recover_from_key(&c, &key).await {
                            Ok(()) => {
                                let _ = tx.send(Update::RecoverySuccess).await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("recovery : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::LeaveRoom { room_id } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) = leave_room(&c, &room_id).await {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("/leave : {e}"),
                                })
                                .await;
                        } else {
                            let _ = tx
                                .send(Update::Error {
                                    reason: "room quittée".into(),
                                })
                                .await;
                        }
                    });
                }
            }
            Command::RedactEvent { room_id, event_id } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        match redact_event(&c, &room_id, &event_id).await {
                            Ok(()) => {
                                tokio::time::sleep(Duration::from_millis(300)).await;
                                let _ = load_room_messages(&c, &room_id, &tx).await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("suppression : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
            Command::LoadSpaces => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) = load_spaces(&c, &tx).await {
                            let _ = tx
                                .send(Update::Error {
                                    reason: format!("spaces : {e}"),
                                })
                                .await;
                        }
                    });
                }
            }
            Command::LoadSpaceChildren { room_id } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        match load_space_children_for(&c, &room_id).await {
                            Ok(children) => {
                                let _ = tx
                                    .send(Update::SpaceChildren {
                                        parent_id: room_id,
                                        children,
                                    })
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Update::Error {
                                        reason: format!("spaces (lazy) : {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                }
            }
        }
    }
}

/// Lazy-load children for a single space (the user expanded a node we
/// hadn't fetched at initial-tree time). Same hierarchy + pagination as
/// the bulk load, rooted at `room_id`.
async fn load_space_children_for(
    client: &Client,
    room_id: &str,
) -> Result<Vec<UiNode>, Box<dyn std::error::Error + Send + Sync>> {
    let parsed: OwnedRoomId = room_id.parse()?;
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    visited.insert(parsed.to_string());
    // Build a fake `Room`-like context isn't necessary — we only need the
    // hierarchy fetch. Reuse `collect_space_children` by fetching the
    // SDK's local Room when available; otherwise drive the hierarchy
    // call directly.
    if let Some(local) = client.get_room(&parsed) {
        Ok(collect_space_children(client, &local, &mut visited).await)
    } else {
        Ok(collect_space_children_unjoined(client, &parsed, &mut visited).await)
    }
}

/// Fallback `collect_space_children` for a space the SDK doesn't have a
/// local Room for (typically because the user hasn't joined it yet).
/// Drives the hierarchy endpoint directly off the room id.
async fn collect_space_children_unjoined(
    client: &Client,
    room_id: &OwnedRoomId,
    visited: &mut std::collections::HashSet<String>,
) -> Vec<UiNode> {
    use matrix_sdk::ruma::api::client::space::get_hierarchy;
    use matrix_sdk::ruma::api::client::space::SpaceHierarchyRoomsChunk;
    use std::collections::HashMap;

    const HIERARCHY_HARD_CAP: usize = 1500;
    let mut chunks: HashMap<OwnedRoomId, SpaceHierarchyRoomsChunk> = HashMap::new();
    let mut from: Option<String> = None;
    loop {
        let mut request = get_hierarchy::v1::Request::new(room_id.clone());
        request.limit = Some(100u32.into());
        request.max_depth = Some(2u32.into());
        request.from = from.clone();
        let response = match client.send(request).await {
            Ok(r) => r,
            Err(_) => break,
        };
        let next = response.next_batch.clone();
        for c in response.rooms {
            chunks.insert(c.summary.room_id.clone(), c);
        }
        if next.is_none() || chunks.len() >= HIERARCHY_HARD_CAP {
            break;
        }
        from = next;
    }
    if chunks.is_empty() {
        return Vec::new();
    }
    build_children_from_hierarchy(client, &chunks, room_id, visited).await
}

/// Try to restore a previously persisted session from the SQLite store.
/// Returns `Some((client, mxid))` if a logged-in client was restored.
async fn try_restore_session(
) -> Result<Option<(Client, String)>, Box<dyn std::error::Error + Send + Sync>> {
    let last_mxid_path = match last_mxid_file() {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    if !last_mxid_path.exists() {
        return Ok(None);
    }
    let mxid = std::fs::read_to_string(&last_mxid_path)?;
    let mxid = mxid.trim().to_string();
    if mxid.is_empty() {
        return Ok(None);
    }
    let store_path = session_store_path(&mxid)?;
    if !store_path.exists() {
        return Ok(None);
    }

    let domain = mxid
        .trim_start_matches('@')
        .split(':')
        .nth(1)
        .ok_or("MXID invalide (pas de domaine)")?;
    let server_name: matrix_sdk::ruma::OwnedServerName = domain.parse()?;

    let session_path = store_path.join("session.json");
    if !session_path.exists() {
        return Ok(None);
    }

    match build_and_restore(&server_name, &store_path, &session_path).await {
        Ok(client) => {
            if client.matrix_auth().logged_in() {
                Ok(Some((client, mxid)))
            } else {
                Ok(None)
            }
        }
        Err(e) if is_account_mismatch(e.as_ref()) => {
            // Stale crypto store. Wipe and force the user back to the Login
            // view by returning Ok(None).
            let _ = std::fs::remove_dir_all(&store_path);
            let _ = std::fs::remove_file(last_mxid_file()?);
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

async fn build_and_restore(
    server_name: &matrix_sdk::ruma::OwnedServerName,
    store_path: &std::path::Path,
    session_path: &std::path::Path,
) -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::builder()
        .server_name(server_name)
        .sqlite_store(store_path, None)
        .build()
        .await?;
    let json = std::fs::read_to_string(session_path)?;
    let session: MatrixSession = serde_json::from_str(&json)?;
    client
        .matrix_auth()
        .restore_session(session, RoomLoadSettings::default())
        .await?;
    Ok(client)
}

/// Path used to remember the last successfully-logged-in MXID, used by
/// `try_restore_session` on next startup.
fn last_mxid_file() -> std::io::Result<PathBuf> {
    let base = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no data directory")
        })?;
    let dir = base.join("matcurses");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("last_mxid"))
}

/// Load joined members of a room and emit `Update::Members`. Then spawn
/// per-user presence fetches that emit `Update::MemberPresence` as their
/// responses come back.
async fn load_members(
    client: &Client,
    room_id: &str,
    tx: &Sender<Update>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parsed: OwnedRoomId = room_id.parse()?;
    let room = client.get_room(&parsed).ok_or("room introuvable")?;
    let members = room.members(RoomMemberships::JOIN).await?;
    let mut out: Vec<UiMember> = members.iter().map(map_member).collect();
    // Sort: admin first, then moderators, then alphabetical.
    out.sort_by(|a, b| {
        b.power_level
            .cmp(&a.power_level)
            .then_with(|| a.displayname.to_lowercase().cmp(&b.displayname.to_lowercase()))
    });
    let mxids: Vec<String> = out.iter().map(|m| m.mxid.clone()).collect();
    let _ = tx
        .send(Update::Members {
            room_id: room_id.to_string(),
            members: out,
        })
        .await;

    // Fetch presence per user in parallel; each result lands on the UI as a
    // separate Update::MemberPresence so the list refreshes progressively.
    for mxid in mxids {
        let client = client.clone();
        let tx = tx.clone();
        let room_id = room_id.to_string();
        tokio::spawn(async move {
            if let Ok(presence) = fetch_presence(&client, &mxid).await {
                let _ = tx
                    .send(Update::MemberPresence {
                        room_id,
                        mxid,
                        presence,
                    })
                    .await;
            }
        });
    }

    Ok(())
}

async fn fetch_presence(
    client: &Client,
    mxid: &str,
) -> Result<UiPresence, Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::ruma::api::client::presence::get_presence;
    use matrix_sdk::ruma::OwnedUserId;
    let user_id: OwnedUserId = mxid.parse()?;
    let request = get_presence::v3::Request::new(user_id);
    let response = client.send(request).await?;
    Ok(map_presence(&response.presence))
}

fn map_presence(p: &matrix_sdk::ruma::presence::PresenceState) -> UiPresence {
    use matrix_sdk::ruma::presence::PresenceState;
    match p {
        PresenceState::Online => UiPresence::Online,
        PresenceState::Unavailable => UiPresence::Idle,
        PresenceState::Offline => UiPresence::Offline,
        _ => UiPresence::Unavailable,
    }
}

fn map_member(m: &RoomMember) -> UiMember {
    use matrix_sdk::ruma::events::room::power_levels::UserPowerLevel;
    let mxid = m.user_id().to_string();
    let displayname = m
        .display_name()
        .map(|s| s.to_string())
        .unwrap_or_else(|| m.user_id().localpart().to_string());
    // `power_level()` now returns `UserPowerLevel` (Infinite | Int(Int))
    // instead of a raw integer; clamp the integer variant to [0, 100] so it
    // fits in the UI byte. "Infinite" (room creator under room v12) → 100.
    let power_level = match m.power_level() {
        UserPowerLevel::Infinite => 100u8,
        UserPowerLevel::Int(n) => {
            let raw: i64 = n.into();
            raw.clamp(0, 100) as u8
        }
        // The enum is `#[non_exhaustive]`; future ruma additions get a
        // sane "user-tier" mapping until we know what they mean.
        _ => 0,
    };
    // Presence is not exposed directly on RoomMember in matrix-sdk 0.7:
    // it depends on a separate API and isn't always populated. Default to
    // Unavailable for now; we can wire the presence API later.
    let presence = UiPresence::Unavailable;
    UiMember {
        mxid,
        displayname,
        power_level,
        presence,
    }
}

/// Load the spaces tree: for each joinable space, fetch its direct
/// children (m.space.child state events), recursing into sub-spaces.
async fn load_spaces(
    client: &Client,
    tx: &Sender<Update>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::collections::HashSet;
    let mut visited: HashSet<String> = HashSet::new();
    let mut roots = Vec::new();
    // We treat every joinable space as a root: matrix-sdk doesn't expose a
    // "top-level" notion, and in practice the user can join a sub-space
    // directly, so we list them all and let the user sort it out.
    for r in client.rooms() {
        if !r.is_space() {
            continue;
        }
        let space_id = r.room_id().to_string();
        if visited.contains(&space_id) {
            continue;
        }
        let node = build_space_node(client, &r, &mut visited).await;
        roots.push(node);
    }
    // Rooms that don't belong to any space remain accessible via F4 (room list).
    let _ = tx.send(Update::Spaces { roots }).await;
    Ok(())
}

async fn build_space_node(
    client: &Client,
    space: &matrix_sdk::Room,
    visited: &mut std::collections::HashSet<String>,
) -> UiNode {
    let space_id = space.room_id().to_string();
    visited.insert(space_id.clone());

    let label = space
        .display_name()
        .await
        .map(|n| n.to_string())
        .unwrap_or_else(|_| {
            space
                .name()
                .unwrap_or_else(|| space.room_id().to_string())
        });

    let children = collect_space_children(client, space, visited).await;

    UiNode {
        label,
        kind: UiNodeKind::Space {
            room_id: space.room_id().to_string(),
            expanded: false,
            children,
            loaded: true,
            // Top-level (joined) spaces don't need via hints to be
            // re-joined; they're already in the user's local state.
            via: Vec::new(),
        },
    }
}

async fn collect_space_children(
    client: &Client,
    space: &matrix_sdk::Room,
    visited: &mut std::collections::HashSet<String>,
) -> Vec<UiNode> {
    use matrix_sdk::ruma::api::client::space::get_hierarchy;
    use matrix_sdk::ruma::api::client::space::SpaceHierarchyRoomsChunk;
    use std::collections::HashMap;

    // The space hierarchy endpoint (`GET /rooms/{id}/hierarchy`, MSC2946 /
    // spec 1.2) returns RoomSummary chunks for the space and all
    // descendants the homeserver knows about — even ones the user has
    // not joined. We ask for several levels at once so unjoined
    // sub-spaces also come back populated; otherwise pressing Right on
    // them would land on an empty children vec.
    // Paginate the hierarchy endpoint so we collect every direct + grand-
    // child of the space, not just the first 100 entries the server feels
    // like sending. With `max_depth=2` we cover sub-space → its rooms,
    // which is what the user actually wants to expand. Cap the total
    // chunks at something sane in case a homeserver returns ridiculous
    // numbers of descendants (Matrix.org Community sits around 600).
    const HIERARCHY_HARD_CAP: usize = 1500;
    let mut chunks: HashMap<OwnedRoomId, SpaceHierarchyRoomsChunk> = HashMap::new();
    let mut from: Option<String> = None;
    loop {
        let mut request = get_hierarchy::v1::Request::new(space.room_id().to_owned());
        request.limit = Some(100u32.into());
        request.max_depth = Some(2u32.into());
        request.from = from.clone();
        let response = match client.send(request).await {
            Ok(r) => r,
            Err(_) => break,
        };
        let next = response.next_batch.clone();
        for c in response.rooms {
            chunks.insert(c.summary.room_id.clone(), c);
        }
        if next.is_none() || chunks.len() >= HIERARCHY_HARD_CAP {
            break;
        }
        from = next;
    }
    if chunks.is_empty() {
        return Vec::new();
    }

    let root_id = space.room_id().to_owned();
    build_children_from_hierarchy(client, &chunks, &root_id, visited).await
}

fn build_children_from_hierarchy<'a>(
    client: &'a Client,
    chunks: &'a std::collections::HashMap<
        OwnedRoomId,
        matrix_sdk::ruma::api::client::space::SpaceHierarchyRoomsChunk,
    >,
    parent: &'a OwnedRoomId,
    visited: &'a mut std::collections::HashSet<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<UiNode>> + Send + 'a>> {
    Box::pin(async move {
        use matrix_sdk::ruma::room::RoomType;
        let chunk = match chunks.get(parent) {
            Some(c) => c,
            None => return Vec::new(),
        };
        let mut out = Vec::new();
        for raw in &chunk.children_state {
            let parsed = match raw.deserialize() {
                Ok(p) => p,
                Err(_) => continue,
            };
            let child_id = parsed.state_key.clone();
            let child_id_str = child_id.to_string();
            if visited.contains(&child_id_str) {
                continue;
            }
            // The m.space.child event content carries `via` server hints
            // — required to federate-join a child by id. Drop them and
            // the join later fails with "no servers ... have been provided".
            let via: Vec<String> = parsed
                .content
                .via
                .iter()
                .map(|s| s.to_string())
                .collect();
            let summary = chunks.get(&child_id).map(|c| &c.summary);
            let local = client.get_room(&child_id);

            let is_space = if let Some(r) = &local {
                r.is_space()
            } else if let Some(s) = summary {
                matches!(s.room_type, Some(RoomType::Space))
            } else {
                false
            };
            let label = if let Some(r) = &local {
                r.display_name()
                    .await
                    .map(|n| n.to_string())
                    .unwrap_or_else(|_| {
                        summary
                            .and_then(|s| s.name.clone())
                            .unwrap_or_else(|| child_id_str.clone())
                    })
            } else if let Some(s) = summary {
                s.name.clone().unwrap_or_else(|| child_id_str.clone())
            } else {
                child_id_str.clone()
            };

            if is_space {
                visited.insert(child_id_str.clone());
                // A child sub-space is "loaded" only if its own chunk
                // came back in this hierarchy fetch — otherwise we know
                // it's a space but not what's inside, and the UI marks
                // it as needing a lazy fetch on expand.
                let loaded = chunks.contains_key(&child_id);
                let children = if loaded {
                    build_children_from_hierarchy(client, chunks, &child_id, visited).await
                } else {
                    Vec::new()
                };
                out.push(UiNode {
                    label,
                    kind: UiNodeKind::Space {
                        room_id: child_id_str.clone(),
                        expanded: false,
                        children,
                        loaded,
                        via: via.clone(),
                    },
                });
            } else {
                visited.insert(child_id_str.clone());
                let unread = local
                    .as_ref()
                    .map(|r| r.unread_notification_counts().notification_count as usize)
                    .unwrap_or(0);
                out.push(UiNode {
                    label,
                    kind: UiNodeKind::Room {
                        name: child_id_str,
                        unread,
                        via,
                    },
                });
            }
        }
        out
    })
}

/// Perform the login (and optional session restore), return a connected Client.
///
/// If the first attempt fails with a crypto-store account mismatch (a stale
/// store from a previous login pointing to a now-defunct device), the store
/// is wiped and the login is retried once with a clean store.
async fn do_login(
    mxid: &str,
    password: &str,
    server: &str,
) -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
    if mxid.is_empty() || password.is_empty() {
        return Err("MXID ou mot de passe vide".into());
    }
    match login_once(mxid, password, server).await {
        Ok(c) => Ok(c),
        Err(e) if is_account_mismatch(e.as_ref()) => {
            // Stale crypto store: wipe and retry once with a clean store.
            let store_path = session_store_path(mxid)?;
            let _ = std::fs::remove_dir_all(&store_path);
            login_once(mxid, password, server).await
        }
        Err(e) => Err(e),
    }
}

/// SSO login flow: opens the homeserver SSO URL via the OS default
/// browser (cross-platform via the `open` crate) and waits for the
/// callback to deliver a login token. The matrix-sdk takes care of
/// running the local callback HTTP server.
async fn do_login_sso(
    server: &str,
    idp_id: Option<&str>,
) -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
    if server.trim().is_empty() {
        return Err("le champ Serveur est requis pour SSO".into());
    }

    // For SSO we do not yet know the MXID, so the store path is keyed off
    // the homeserver until the actual MXID lands; we move the store later
    // if needed (no-op for now: we use a server-keyed path).
    let server_key: String = server
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let store_path = session_store_path(&format!("__sso__{server_key}"))?;
    std::fs::create_dir_all(&store_path)?;

    let client = if server.starts_with("http://") || server.starts_with("https://") {
        Client::builder()
            .homeserver_url(server)
            .sqlite_store(&store_path, None)
            .build()
            .await
            .map_err(|e| format!("connexion à {server} : {e}"))?
    } else {
        let server_name: matrix_sdk::ruma::OwnedServerName = server
            .parse()
            .map_err(|e| format!("nom de serveur invalide '{server}' : {e}"))?;
        Client::builder()
            .server_name(&server_name)
            .sqlite_store(&store_path, None)
            .build()
            .await
            .map_err(|e| format!("auto-discovery {server_name} : {e}"))?
    };

    let mut sso = client
        .matrix_auth()
        .login_sso(|url| async move {
            // Best-effort cross-platform browser open. If it fails the
            // user still sees the URL printed below; matrix-sdk will time
            // out on its own if no callback arrives.
            let _ = open::that(&url);
            Ok(())
        })
        .initial_device_display_name("matcurses");
    if let Some(id) = idp_id {
        sso = sso.identity_provider_id(id);
    }
    sso.send().await?;
    Ok(client)
}

/// Single login attempt with the existing (or freshly created) store.
async fn login_once(
    mxid: &str,
    password: &str,
    server: &str,
) -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
    let store_path = session_store_path(mxid)?;
    std::fs::create_dir_all(&store_path)?;

    // If the user provided an explicit URL, use it as is. Otherwise (empty
    // field or bare domain) we go through `server_name`, which triggers
    // .well-known/matrix/client autodiscovery — more robust for Synapse
    // instances sitting behind a reverse proxy.
    let server_input = if server.is_empty() {
        mxid.trim_start_matches('@')
            .split(':')
            .nth(1)
            .ok_or("MXID invalide (pas de domaine)")?
            .to_string()
    } else {
        server.to_string()
    };

    let client = if server_input.starts_with("http://") || server_input.starts_with("https://") {
        Client::builder()
            .homeserver_url(&server_input)
            .sqlite_store(&store_path, None)
            .build()
            .await
            .map_err(|e| format!("connexion à {server_input} : {e}"))?
    } else {
        let server_name: matrix_sdk::ruma::OwnedServerName = server_input
            .parse()
            .map_err(|e| format!("nom de serveur invalide '{server_input}' : {e}"))?;
        Client::builder()
            .server_name(&server_name)
            .sqlite_store(&store_path, None)
            .build()
            .await
            .map_err(|e| format!("auto-discovery {server_name} : {e}"))?
    };

    // The full MXID (we prepend @ if the user omitted it). `login_username`
    // accepts either a localpart or a full MXID — passing the full MXID is
    // the most explicit choice server-side.
    let user_id = if mxid.starts_with('@') {
        mxid.to_string()
    } else {
        format!("@{mxid}")
    };

    client
        .matrix_auth()
        .login_username(&user_id, password)
        .initial_device_display_name("matcurses")
        .send()
        .await
        .map_err(|e| format!("auth user_id={user_id} : {e}"))?;

    Ok(client)
}

/// Detect the matrix-sdk crypto store error that indicates the store was
/// created for a different account (stale device after server-side logout
/// or device deletion). Pattern-matches the formatted error message — not
/// pretty, but matrix-sdk doesn't expose a typed variant we can match on
/// portably across error chains.
fn is_account_mismatch(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = current {
        let s = e.to_string().to_lowercase();
        if s.contains("account in the store doesn't match")
            || s.contains("account in the store does not match")
            || s.contains("the account in the store")
        {
            return true;
        }
        current = e.source();
    }
    false
}

/// Path: ~/.local/share/matcurses/<account_sanitized>/
fn session_store_path(mxid: &str) -> std::io::Result<PathBuf> {
    let base = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "pas de répertoire data")
        })?;
    let safe: String = mxid
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(base.join("matcurses").join(safe))
}

/// Run the continuous sync and push Rooms updates on every tick.
async fn run_sync(
    client: Arc<Client>,
    tx: Sender<Update>,
    pending_sas: std::sync::Arc<
        tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<SasUserDecision>>>,
    >,
    current_room: std::sync::Arc<tokio::sync::Mutex<Option<OwnedRoomId>>>,
) {
    // Live messages.
    let tx_handler = tx.clone();
    let client_handler = client.clone();
    client.add_event_handler(
        move |ev: SyncRoomMessageEvent, room: matrix_sdk::room::Room| {
            let tx = tx_handler.clone();
            let c = client_handler.clone();
            async move {
                let original = match ev.as_original() {
                    Some(o) => o,
                    None => return, // redaction
                };
                // Edits (`m.replace`) and thread replies require the whole
                // room timeline to be re-rendered: an edit overwrites a
                // previous message's blocks, a thread reply attaches under
                // its root. Reload the room instead of pushing a new line.
                if matches!(
                    &original.content.relates_to,
                    Some(matrix_sdk::ruma::events::room::message::Relation::Replacement(_))
                ) || matches!(
                    &original.content.relates_to,
                    Some(matrix_sdk::ruma::events::room::message::Relation::Thread(_))
                ) {
                    let _ = load_room_messages(&c, &room.room_id().to_string(), &tx).await;
                    return;
                }
                let msg = event_content_to_message(
                    &original.event_id.to_string(),
                    &original.sender.to_string(),
                    &original.content.msgtype,
                    original.origin_server_ts.0.into(),
                );
                let _ = tx
                    .send(Update::NewMessage {
                        room_id: room.room_id().to_string(),
                        message: msg,
                    })
                    .await;
            }
        },
    );

    // Incoming verification requests (other devices initiating SAS toward us).
    // We auto-accept and run the same flow as outgoing, opening the SAS modal.
    let tx_v = tx.clone();
    let pending_v = pending_sas.clone();
    client.add_event_handler(
        move |ev: matrix_sdk::ruma::events::key::verification::request::ToDeviceKeyVerificationRequestEvent,
              c: matrix_sdk::Client| {
            let tx = tx_v.clone();
            let pending = pending_v.clone();
            async move {
                let request = match c
                    .encryption()
                    .get_verification_request(&ev.sender, &ev.content.transaction_id)
                    .await
                {
                    Some(r) => r,
                    None => return,
                };
                let (decision_tx, decision_rx) = tokio::sync::oneshot::channel();
                *pending.lock().await = Some(decision_tx);
                let _ = tx
                    .send(Update::Error {
                        reason: format!(
                            "vérification entrante de {} — accepte sur l'autre device",
                            ev.sender
                        ),
                    })
                    .await;
                let result = run_sas_incoming(request, &tx, decision_rx).await;
                let _ = pending.lock().await.take();
                match result {
                    Ok(ok) => {
                        let _ = tx.send(Update::SasDone { ok }).await;
                    }
                    Err(e) => {
                        let _ = tx
                            .send(Update::Error {
                                reason: format!("verify entrant : {e}"),
                            })
                            .await;
                        let _ = tx.send(Update::SasDone { ok: false }).await;
                    }
                }
            }
        },
    );

    // Surface the cached rooms (restored from the SQLite store by
    // `restore_session`) immediately. Otherwise the UI sees zero rooms
    // until the initial network sync returns — which on a delta sync with
    // an outdated next_batch token can take the full 30 s timeout.
    let _ = tx.send(snapshot_rooms(&client).await).await;

    // Premier sync pour peupler.
    let settings = SyncSettings::new().timeout(Duration::from_secs(30));
    if let Err(e) = client.sync_once(settings.clone()).await {
        let _ = tx
            .send(Update::Error {
                reason: format!("sync initial : {e}"),
            })
            .await;
        return;
    }
    let _ = tx.send(snapshot_rooms(&client).await).await;
    let _ = tx.send(Update::SyncComplete).await;

    // Continuous sync: push a rooms snapshot after each iteration so the UI
    // reflects new rooms in real time (including those created by bridges
    // like mautrix-whatsapp), unread count changes, name changes, etc.
    let tx_cb = tx.clone();
    let client_cb = client.clone();
    let cur_cb = current_room.clone();
    let result = client
        .sync_with_callback(settings, move |response| {
            let tx = tx_cb.clone();
            let c = client_cb.clone();
            let cur = cur_cb.clone();
            async move {
                let _ = tx.send(snapshot_rooms(&c).await).await;
                // Safety net for the live SyncRoomMessageEvent handler:
                // if the active room received any timeline events this
                // iteration, force a refetch. Catches messages that the
                // SDK could not yet decrypt at handler time, or that the
                // bridge re-encrypted asynchronously.
                let cur_id = cur.lock().await.clone();
                if let Some(rid) = cur_id {
                    if let Some(joined) = response.rooms.joined.get(&rid) {
                        if !joined.timeline.events.is_empty() {
                            let _ = load_room_messages(&c, &rid.to_string(), &tx).await;
                        }
                    }
                }
                matrix_sdk::LoopCtrl::Continue
            }
        })
        .await;
    if let Err(e) = result {
        let _ = tx
            .send(Update::Error {
                reason: format!("sync : {e}"),
            })
            .await;
    }
}

/// Synchronous rooms snapshot (run on the tokio side).
async fn snapshot_rooms(client: &Client) -> Update {
    let mut rooms = Vec::new();
    let mut ids = Vec::new();
    for r in client.rooms() {
        let name = match r.display_name().await {
            Ok(n) => n.to_string(),
            Err(_) => r
                .name()
                .unwrap_or_else(|| r.room_id().to_string()),
        };

        let counts = r.unread_notification_counts();
        let unread = counts.notification_count as usize;
        let mentions = counts.highlight_count as usize;

        let invited = matches!(r.state(), RoomState::Invited);
        rooms.push(UiRoom {
            name,
            unread,
            mentions,
            pinned: false,
            muted: false,
            invited,
        });
        ids.push(r.room_id().to_string());
    }
    Update::Rooms { rooms, ids }
}

/// Load the ~50 latest messages of a room and emit an `Update::RoomMessages`.
async fn load_room_messages(
    client: &Client,
    room_id: &str,
    tx: &Sender<Update>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parsed: OwnedRoomId = room_id.parse()?;
    let room = match client.get_room(&parsed) {
        Some(r) => r,
        None => return Err("room introuvable".into()),
    };

    // Auto-joining invited rooms here would silently consent to any
    // incoming invitation — a privacy / spam footgun. Surface the state
    // instead and let the UI offer /accept or /reject.
    match room.state() {
        RoomState::Joined => {}
        RoomState::Invited => {
            return Err("invitation en attente · /accept ou /reject".into());
        }
        RoomState::Left | RoomState::Knocked | RoomState::Banned => {
            return Err("room non accessible".into());
        }
    }

    let mut opts = MessagesOptions::backward();
    opts.limit = matrix_sdk::ruma::UInt::from(50u32);
    // No server-side type filter: encrypted rooms ship `m.room.encrypted`
    // events that the SDK decrypts client-side. A `m.room.message` filter
    // would strip them before we get a chance.

    let chunk = room.messages(opts).await?;
    let me = client
        .user_id()
        .map(|u| u.to_string())
        .unwrap_or_default();

    use matrix_sdk::ruma::events::AnySyncTimelineEvent;
    use std::collections::HashMap;

    // Walk events in chronological (oldest → newest) order.
    // Since matrix-sdk 0.8 the timeline-event wrapper exposes the raw JSON
    // through `.raw()` (instead of an `event` field) and only types it as
    // `AnySyncTimelineEvent`; the `room_id` field that distinguishes
    // `AnyTimelineEvent` is unused on this path.
    let events: Vec<AnySyncTimelineEvent> = chunk
        .chunk
        .iter()
        .rev()
        .filter_map(|tev| tev.raw().deserialize().ok())
        .collect();

    // Pass 1: extract top-level RoomMessages and undecryptable placeholders.
    // Skip thread replies (attached to their root in pass 2) and edits
    // (`m.replace`, applied in pass 3).
    let mut out: Vec<Message> = Vec::new();
    let mut idx_by_event: HashMap<String, usize> = HashMap::new();

    for raw in &events {
        if let AnySyncTimelineEvent::MessageLike(ml) = raw {
            let event_id = ml.event_id().to_string();
            let sender = ml.sender().to_string();
            let ts = ml.origin_server_ts();
            match ml.original_content() {
                Some(AnyMessageLikeEventContent::RoomMessage(rmc)) => {
                    if matches!(
                        &rmc.relates_to,
                        Some(matrix_sdk::ruma::events::room::message::Relation::Thread(_))
                    ) {
                        continue;
                    }
                    if matches!(
                        &rmc.relates_to,
                        Some(matrix_sdk::ruma::events::room::message::Relation::Replacement(_))
                    ) {
                        continue;
                    }
                    let msg = event_content_to_message(
                        &event_id,
                        &sender,
                        &rmc.msgtype,
                        ts.0.into(),
                    );
                    idx_by_event.insert(event_id.clone(), out.len());
                    out.push(msg);
                }
                Some(AnyMessageLikeEventContent::RoomEncrypted(_)) => {
                    let m = Message {
                        time: format_time(ts.0.into()),
                        author: short_author(&sender),
                        blocks: vec![Block::Text("[chiffré · clé manquante]".into())],
                        replies: Vec::new(),
                        reactions: Vec::new(),
                        event_id: event_id.clone(),
                        timestamp_ms: ts.0.into(),
                        read: false,
                    };
                    idx_by_event.insert(event_id.clone(), out.len());
                    out.push(m);
                }
                _ => {}
            }
        }
    }

    // Pass 2: attach thread replies and reactions to their parent message.
    for raw in &events {
        if let AnySyncTimelineEvent::MessageLike(ml) = raw {
            let event_id = ml.event_id().to_string();
            let sender = ml.sender().to_string();
            let ts = ml.origin_server_ts();
            match ml.original_content() {
                Some(AnyMessageLikeEventContent::RoomMessage(rmc)) => {
                    if let Some(matrix_sdk::ruma::events::room::message::Relation::Thread(t)) =
                        &rmc.relates_to
                    {
                        let root = t.event_id.to_string();
                        if let Some(&idx) = idx_by_event.get(&root) {
                            let reply = crate::message::ThreadReply {
                                time: format_time(ts.0.into()),
                                author: short_author(&sender),
                                blocks: msgtype_to_blocks(&rmc.msgtype),
                                event_id,
                                timestamp_ms: ts.0.into(),
                                read: false,
                            };
                            out[idx].replies.push(reply);
                        }
                    }
                }
                Some(AnyMessageLikeEventContent::Reaction(rc)) => {
                    let parent = rc.relates_to.event_id.to_string();
                    let key = rc.relates_to.key.clone();
                    if let Some(&idx) = idx_by_event.get(&parent) {
                        let display = short_author(&sender);
                        let is_me = sender == me;
                        let parent_msg = &mut out[idx];
                        if let Some(r) =
                            parent_msg.reactions.iter_mut().find(|r| r.key == key)
                        {
                            if !r.users.contains(&display) {
                                r.users.push(display);
                            }
                            if is_me {
                                r.my_event_id = Some(event_id.clone());
                            }
                        } else {
                            parent_msg.reactions.push(crate::message::Reaction {
                                key,
                                users: vec![display],
                                my_event_id: if is_me { Some(event_id.clone()) } else { None },
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Pass 3: apply edits (`m.replace`). For each target event, keep the
    // latest replacement (by origin_server_ts) and overwrite the parent's
    // blocks with the new content. Edits authored by anyone other than the
    // original sender are dropped (servers usually filter, but be defensive).
    {
        use std::collections::HashMap as Map;
        // (parent_event_id) -> (latest_ts, new_msgtype)
        let mut latest: Map<String, (u64, MessageType)> = Map::new();
        for raw in &events {
            if let AnySyncTimelineEvent::MessageLike(ml) = raw {
                let sender = ml.sender().to_string();
                let ts: u64 = ml.origin_server_ts().0.into();
                if let Some(AnyMessageLikeEventContent::RoomMessage(rmc)) =
                    ml.original_content()
                {
                    if let Some(matrix_sdk::ruma::events::room::message::Relation::Replacement(
                        r,
                    )) = &rmc.relates_to
                    {
                        let parent = r.event_id.to_string();
                        if let Some(&idx) = idx_by_event.get(&parent) {
                            // Author check: short_author equality is enough
                            // here since both come from the same MXID space.
                            let parent_author = &out[idx].author;
                            if short_author(&sender) != *parent_author {
                                continue;
                            }
                            let new_msgtype = r.new_content.msgtype.clone();
                            match latest.get(&parent) {
                                Some((prev_ts, _)) if *prev_ts >= ts => {}
                                _ => {
                                    latest.insert(parent, (ts, new_msgtype));
                                }
                            }
                        }
                    }
                }
            }
        }
        for (parent, (_ts, mt)) in latest {
            if let Some(&idx) = idx_by_event.get(&parent) {
                out[idx].blocks = msgtype_to_blocks(&mt);
            }
        }
    }

    // Historical batch: mark every message + reply as already read so
    // the user's `u` (next-unread) only walks messages that arrived
    // live during the current matcurses session.
    for msg in &mut out {
        msg.read = true;
        for r in &mut msg.replies {
            r.read = true;
        }
    }

    let _ = tx
        .send(Update::RoomMessages {
            room_id: room_id.to_string(),
            messages: out,
        })
        .await;
    Ok(())
}

async fn send_reaction(
    client: &Client,
    room_id: &str,
    parent_event_id: &str,
    key: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::ruma::events::reaction::ReactionEventContent;
    use matrix_sdk::ruma::events::relation::Annotation;
    let parsed_room: OwnedRoomId = room_id.parse()?;
    let room = client.get_room(&parsed_room).ok_or("room introuvable")?;
    let parsed_event: OwnedEventId = parent_event_id.parse()?;
    let content = ReactionEventContent::new(Annotation::new(parsed_event, key.to_string()));
    room.send(content).await?;
    Ok(())
}

async fn redact_event(
    client: &Client,
    room_id: &str,
    event_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parsed_room: OwnedRoomId = room_id.parse()?;
    let room = client.get_room(&parsed_room).ok_or("room introuvable")?;
    let parsed_event: OwnedEventId = event_id.parse()?;
    room.redact(&parsed_event, None, None).await?;
    Ok(())
}

async fn send_emote(
    client: &Client,
    room_id: &str,
    body: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parsed: OwnedRoomId = room_id.parse()?;
    let room = client.get_room(&parsed).ok_or("room introuvable")?;
    let content = RoomMessageEventContent::emote_plain(body);
    room.send(content).await?;
    Ok(())
}

async fn discover_public_rooms(
    client: &Client,
    server: &str,
    kind: PublicKind,
) -> Result<Vec<PublicRoomEntry>, Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::ruma::api::client::directory::get_public_rooms_filtered::v3 as f;
    use matrix_sdk::ruma::directory::{Filter, RoomTypeFilter};
    use matrix_sdk::ruma::OwnedServerName;

    let mut request = f::Request::new();
    if !server.trim().is_empty() {
        let parsed: OwnedServerName = server.trim().parse()?;
        request.server = Some(parsed);
    }
    request.limit = Some(100u32.into());
    let mut filter = Filter::new();
    filter.room_types = vec![match kind {
        PublicKind::Rooms => RoomTypeFilter::Default,
        PublicKind::Spaces => RoomTypeFilter::Space,
    }];
    request.filter = filter;

    let response = client.public_rooms_filtered(request).await?;
    let entries = response
        .chunk
        .into_iter()
        .map(|c| {
            let join_target = c
                .canonical_alias
                .as_ref()
                .map(|a| a.to_string())
                .unwrap_or_else(|| c.room_id.to_string());
            let name = c
                .name
                .clone()
                .unwrap_or_else(|| join_target.clone());
            PublicRoomEntry {
                name,
                topic: c.topic.clone(),
                members: u64::from(c.num_joined_members),
                join_target,
            }
        })
        .collect();
    Ok(entries)
}

async fn join_room(
    client: &Client,
    alias_or_id: &str,
    via: &[String],
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::ruma::{OwnedRoomOrAliasId, OwnedServerName};
    let parsed: OwnedRoomOrAliasId = alias_or_id.parse()?;
    let via_parsed: Vec<OwnedServerName> = via
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect();
    let room = client
        .join_room_by_id_or_alias(&parsed, &via_parsed)
        .await?;
    // Right after a remote join, the room's state has not been synced yet,
    // so `display_name()` returns `RoomDisplayName::Empty` ("Empty Room").
    // Fall back to whatever the user typed so the flash is meaningful;
    // the rooms list will pick up the proper name on the next sync.
    use matrix_sdk::RoomDisplayName;
    let name = match room.display_name().await {
        Ok(RoomDisplayName::Empty) | Ok(RoomDisplayName::EmptyWas(_)) | Err(_) => {
            alias_or_id.to_string()
        }
        Ok(n) => n.to_string(),
    };
    Ok(name)
}

async fn leave_room(
    client: &Client,
    room_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parsed: OwnedRoomId = room_id.parse()?;
    let room = client.get_room(&parsed).ok_or("room introuvable")?;
    room.leave().await?;
    Ok(())
}

/// Create a new room. `is_direct` flips the spec-level flag the homeserver
/// uses to track DMs (it also adds the room to the inviter's `m.direct`
/// account data via the SDK). For `/dm`, `invite` is the partner; for a
/// plain `/create`, `invite` may be empty.
async fn create_room(
    client: &Client,
    name: Option<&str>,
    is_direct: bool,
    invite: &[String],
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::ruma::api::client::room::create_room::v3::Request as CreateRoomRequest;
    use matrix_sdk::ruma::api::client::room::Visibility;
    use matrix_sdk::ruma::OwnedUserId;

    let mut req = CreateRoomRequest::new();
    if let Some(n) = name {
        if !n.is_empty() {
            req.name = Some(n.to_string());
        }
    }
    let invite_parsed: Vec<OwnedUserId> = invite
        .iter()
        .map(|s| s.parse::<OwnedUserId>())
        .collect::<Result<Vec<_>, _>>()?;
    req.invite = invite_parsed;
    req.is_direct = is_direct;
    if is_direct {
        use matrix_sdk::ruma::api::client::room::create_room::v3::RoomPreset;
        req.preset = Some(RoomPreset::TrustedPrivateChat);
        req.visibility = Visibility::Private;
    }

    let room = client.create_room(req).await?;
    Ok(name.map(|s| s.to_string()).unwrap_or_else(|| room.room_id().to_string()))
}

/// Accept a pending invitation. Idempotent: joining an already-joined
/// room returns Ok silently.
async fn accept_invite(
    client: &Client,
    room_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parsed: OwnedRoomId = room_id.parse()?;
    let room = client.get_room(&parsed).ok_or("invite introuvable")?;
    if matches!(room.state(), RoomState::Joined) {
        return Ok(());
    }
    room.join().await?;
    Ok(())
}

/// Decline a pending invitation. Implemented as `leave`, which the spec
/// allows for invited rooms and drops them from the local rooms list.
async fn reject_invite(
    client: &Client,
    room_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parsed: OwnedRoomId = room_id.parse()?;
    let room = client.get_room(&parsed).ok_or("invite introuvable")?;
    room.leave().await?;
    Ok(())
}

/// Invite `user_id` to `room_id`. The local user must have at least the
/// room's `invite` power level (default 0 for most rooms).
async fn invite_user(
    client: &Client,
    room_id: &str,
    user_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::ruma::OwnedUserId;
    let parsed_room: OwnedRoomId = room_id.parse()?;
    let parsed_user: OwnedUserId = user_id.parse()?;
    let room = client.get_room(&parsed_room).ok_or("room introuvable")?;
    room.invite_user_by_id(&parsed_user).await?;
    Ok(())
}

async fn kick_user(
    client: &Client,
    room_id: &str,
    user_id: &str,
    reason: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::ruma::OwnedUserId;
    let parsed_room: OwnedRoomId = room_id.parse()?;
    let parsed_user: OwnedUserId = user_id.parse()?;
    let room = client.get_room(&parsed_room).ok_or("room introuvable")?;
    room.kick_user(&parsed_user, reason).await?;
    Ok(())
}

async fn ban_user(
    client: &Client,
    room_id: &str,
    user_id: &str,
    reason: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::ruma::OwnedUserId;
    let parsed_room: OwnedRoomId = room_id.parse()?;
    let parsed_user: OwnedUserId = user_id.parse()?;
    let room = client.get_room(&parsed_room).ok_or("room introuvable")?;
    room.ban_user(&parsed_user, reason).await?;
    Ok(())
}

async fn unban_user(
    client: &Client,
    room_id: &str,
    user_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::ruma::OwnedUserId;
    let parsed_room: OwnedRoomId = room_id.parse()?;
    let parsed_user: OwnedUserId = user_id.parse()?;
    let room = client.get_room(&parsed_room).ok_or("room introuvable")?;
    room.unban_user(&parsed_user, None).await?;
    Ok(())
}

/// Set `user_id`'s power level. Delegates to the SDK's
/// `update_power_levels`, which fetches the current `m.room.power_levels`,
/// patches the `users` map, and re-sends the state event. Setting back to
/// the room's `users_default` removes the override entirely.
async fn set_power_level(
    client: &Client,
    room_id: &str,
    user_id: &str,
    level: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::ruma::int;
    use matrix_sdk::ruma::Int;
    use matrix_sdk::ruma::OwnedUserId;
    let parsed_room: OwnedRoomId = room_id.parse()?;
    let parsed_user: OwnedUserId = user_id.parse()?;
    let room = client.get_room(&parsed_room).ok_or("room introuvable")?;
    let level_int: Int = Int::try_from(level).unwrap_or_else(|_| int!(0));
    room.update_power_levels(vec![(parsed_user.as_ref(), level_int)])
        .await?;
    Ok(())
}

async fn set_topic(
    client: &Client,
    room_id: &str,
    topic: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parsed: OwnedRoomId = room_id.parse()?;
    let room = client.get_room(&parsed).ok_or("room introuvable")?;
    room.set_room_topic(topic).await?;
    Ok(())
}

async fn set_room_name(
    client: &Client,
    room_id: &str,
    name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parsed: OwnedRoomId = room_id.parse()?;
    let room = client.get_room(&parsed).ok_or("room introuvable")?;
    room.set_name(name.to_string()).await?;
    Ok(())
}

/// Read `path`, infer the MIME type from the extension, upload to the
/// homeserver's media repo, and bind the resulting MXC to the local
/// account's avatar URL.
async fn upload_avatar(
    client: &Client,
    path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        match std::env::var("HOME") {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => path.to_string(),
        }
    } else {
        path.to_string()
    };
    let bytes = std::fs::read(&expanded)
        .map_err(|e| format!("lecture {expanded} : {e}"))?;
    let mime = mime_from_ext(&expanded);
    let parsed_mime: mime::Mime = mime
        .parse()
        .map_err(|_| format!("type MIME inconnu pour {expanded}"))?;
    client
        .account()
        .upload_avatar(&parsed_mime, bytes)
        .await?;
    Ok(())
}

/// Minimal MIME sniffer based on the file extension. Covers the formats
/// the homeserver / Element will treat as inline media; anything else
/// falls back to `application/octet-stream` (sent as `m.file`).
fn mime_from_ext(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".mp3") {
        "audio/mpeg"
    } else if lower.ends_with(".ogg") || lower.ends_with(".opus") {
        "audio/ogg"
    } else if lower.ends_with(".wav") {
        "audio/wav"
    } else if lower.ends_with(".flac") {
        "audio/flac"
    } else if lower.ends_with(".m4a") || lower.ends_with(".aac") {
        "audio/mp4"
    } else if lower.ends_with(".mp4") {
        "video/mp4"
    } else if lower.ends_with(".webm") {
        "video/webm"
    } else if lower.ends_with(".mov") {
        "video/quicktime"
    } else if lower.ends_with(".pdf") {
        "application/pdf"
    } else if lower.ends_with(".txt") || lower.ends_with(".log") {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

/// Read a local file, infer its MIME type, and post it to the room. The
/// SDK picks the right `m.file` / `m.image` / `m.audio` / `m.video`
/// msgtype from the MIME prefix. Encrypted rooms encrypt the upload
/// automatically when the `e2e-encryption` feature is enabled (we have
/// it on, see Cargo.toml).
async fn send_attachment(
    client: &Client,
    room_id: &str,
    path: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::attachment::AttachmentConfig;
    let parsed: OwnedRoomId = room_id.parse()?;
    let room = client.get_room(&parsed).ok_or("room introuvable")?;

    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        match std::env::var("HOME") {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => path.to_string(),
        }
    } else {
        path.to_string()
    };
    let bytes = std::fs::read(&expanded)
        .map_err(|e| format!("lecture {expanded} : {e}"))?;
    let filename = std::path::Path::new(&expanded)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("fichier")
        .to_string();
    let mime_str = mime_from_ext(&expanded);
    let mime_parsed: mime::Mime = mime_str
        .parse()
        .map_err(|_| format!("type MIME inconnu pour {expanded}"))?;
    room.send_attachment(filename.clone(), &mime_parsed, bytes, AttachmentConfig::new())
        .await?;
    Ok(filename)
}

async fn recover_from_key(
    client: &Client,
    key: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("clé de récupération vide".into());
    }
    client.encryption().recovery().recover(trimmed).await?;
    Ok(())
}

/// Outgoing SAS verification: initiate the request, then run the flow.
async fn run_sas_verification(
    client: &Client,
    user_id: &str,
    tx: &Sender<Update>,
    decision_rx: tokio::sync::oneshot::Receiver<SasUserDecision>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::ruma::OwnedUserId;

    let parsed: OwnedUserId = user_id.parse()?;
    let identity = client
        .encryption()
        .get_user_identity(&parsed)
        .await?
        .ok_or("user identity not found (re-sync may help)")?;

    let request = identity.request_verification().await?;
    drive_verification_flow(request, tx, decision_rx).await
}

/// Incoming SAS verification: another device requested verification.
/// We accept the request and run the flow from there.
async fn run_sas_incoming(
    request: matrix_sdk::encryption::verification::VerificationRequest,
    tx: &Sender<Update>,
    decision_rx: tokio::sync::oneshot::Receiver<SasUserDecision>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    request.accept().await?;
    drive_verification_flow(request, tx, decision_rx).await
}

/// Shared verification driver: wait for ready, start SAS, present, apply
/// the user decision, wait for completion. Used by both the outgoing
/// (`/verify`) and incoming (peer-initiated) paths.
async fn drive_verification_flow(
    request: matrix_sdk::encryption::verification::VerificationRequest,
    tx: &Sender<Update>,
    decision_rx: tokio::sync::oneshot::Receiver<SasUserDecision>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt;
    use matrix_sdk::encryption::verification::SasState;

    // Wait for the request to be ready (both ends agreed on methods).
    {
        let mut changes = request.changes();
        loop {
            if request.is_ready() {
                break;
            }
            if request.is_cancelled() || request.is_done() {
                return Err("verification request closed".into());
            }
            if changes.next().await.is_none() {
                break;
            }
        }
    }

    let sas = request
        .start_sas()
        .await?
        .ok_or("SAS not negotiable for this peer")?;

    {
        let mut changes = sas.changes();
        loop {
            if sas.can_be_presented() {
                break;
            }
            if sas.is_cancelled() || sas.is_done() {
                return Err("SAS closed before presentation".into());
            }
            if changes.next().await.is_none() {
                break;
            }
        }
    }

    let decimal = sas.decimals().ok_or("SAS produced no decimals")?;
    let emoji_strings: Vec<(String, String)> = sas
        .emoji()
        .map(|emoji| {
            emoji
                .iter()
                .map(|e| (e.symbol.to_string(), e.description.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let _ = tx
        .send(Update::SasReady {
            decimal,
            emoji: emoji_strings,
        })
        .await;

    let decision = decision_rx
        .await
        .map_err(|_| "no SAS decision delivered")?;

    match decision {
        SasUserDecision::Confirm => sas.confirm().await?,
        SasUserDecision::Mismatch => sas.mismatch().await?,
        SasUserDecision::Cancel => sas.cancel().await?,
    }

    let mut changes = sas.changes();
    while !sas.is_done() && !sas.is_cancelled() {
        if changes.next().await.is_none() {
            break;
        }
    }

    Ok(matches!(sas.state(), SasState::Done { .. }))
}

async fn enable_recovery(
    client: &Client,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Check the recovery state first so we can give a useful error instead
    // of the SDK's opaque "Secret storage already exists" failure.
    use matrix_sdk::encryption::recovery::RecoveryState;
    match client.encryption().recovery().state() {
        RecoveryState::Disabled => {}
        RecoveryState::Enabled => {
            return Err(
                "E2EE déjà configuré sur ce compte. Utilise /restore avec ta clé existante \
                 (depuis Element ou un autre client). Pour générer une nouvelle clé, il faut \
                 d'abord la désactiver depuis un client qui la connaît."
                    .into(),
            );
        }
        RecoveryState::Incomplete => {
            // Partial state: the SDK's `enable()` should be able to finish the
            // setup. Proceed.
        }
        RecoveryState::Unknown => {
            return Err(
                "État E2EE pas encore connu — attends la fin de la synchro initiale et réessaie."
                    .into(),
            );
        }
    }
    // `enable()` provisions cross-signing keys + a server-side key backup
    // (Megolm session keys), generating a fresh recovery key string that
    // the user MUST save: there is no way to retrieve it later.
    let key = client.encryption().recovery().enable().await?;
    Ok(key)
}

/// Send a plain text message into the room.
///
/// `reply_to` and `thread_root` populate `m.relates_to` so Element / other
/// clients render the bubble as a reply or as a thread message. Thread takes
/// precedence (it embeds the reply relation under `is_falling_back`).
async fn do_send(
    client: &Client,
    room_id: &str,
    body: &str,
    reply_to: Option<String>,
    thread_root: Option<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::ruma::events::relation::{InReplyTo, Thread};
    use matrix_sdk::ruma::events::room::message::Relation;
    let parsed: OwnedRoomId = room_id.parse()?;
    let room = client
        .get_room(&parsed)
        .ok_or("room introuvable pour send")?;
    let mut content = RoomMessageEventContent::text_plain(body);
    if let Some(root) = thread_root {
        let root_id: OwnedEventId = root.parse()?;
        let reply_target: OwnedEventId = match reply_to {
            Some(id) => id.parse()?,
            None => root_id.clone(),
        };
        // `Thread::plain` constructs a thread relation whose `in_reply_to`
        // points at the previous message; we then flip `is_falling_back`
        // so thread-aware clients (Element) treat the rich-reply purely
        // as a fallback for legacy clients.
        let mut thread = Thread::plain(root_id, reply_target);
        thread.is_falling_back = true;
        content.relates_to = Some(Relation::Thread(thread));
    } else if let Some(id) = reply_to {
        let event_id: OwnedEventId = id.parse()?;
        content.relates_to = Some(Relation::Reply {
            in_reply_to: InReplyTo::new(event_id),
        });
    }
    room.send(content).await?;
    Ok(())
}

/// Send an edit (`m.replace`) for a previously-sent text message. The
/// homeserver enforces that only the original sender can edit.
async fn do_edit(
    client: &Client,
    room_id: &str,
    event_id: &str,
    body: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use matrix_sdk::ruma::events::relation::Replacement;
    use matrix_sdk::ruma::events::room::message::{
        Relation, RoomMessageEventContentWithoutRelation,
    };
    let parsed_room: OwnedRoomId = room_id.parse()?;
    let room = client
        .get_room(&parsed_room)
        .ok_or("room introuvable pour edit")?;
    let parsed_event: OwnedEventId = event_id.parse()?;

    // Fallback body shown by legacy clients that ignore m.new_content,
    // per spec: prefix with "* ".
    let mut content = RoomMessageEventContent::text_plain(format!("* {body}"));
    let new_content: RoomMessageEventContentWithoutRelation =
        RoomMessageEventContent::text_plain(body).into();
    content.relates_to = Some(Relation::Replacement(Replacement::new(
        parsed_event,
        new_content,
    )));
    room.send(content).await?;
    Ok(())
}

/// Convert an `m.room.message` event content to our UI `Message`. Live
/// arrivals start `read: false`; historical loads mark them `true`
/// after the batch is built (see `load_room_messages`).
fn event_content_to_message(
    event_id: &str,
    sender: &str,
    msgtype: &MessageType,
    ts_ms: u64,
) -> Message {
    Message {
        time: format_time(ts_ms),
        author: short_author(sender),
        blocks: msgtype_to_blocks(msgtype),
        replies: Vec::new(),
        reactions: Vec::new(),
        event_id: event_id.to_string(),
        timestamp_ms: ts_ms,
        read: false,
    }
}

/// Fetch the audio bytes for a given event (auto-decrypts E2EE attachments
/// via the SDK). Returns the raw bytes and the optional mime type.
async fn download_voice(
    client: &Client,
    room_id: &str,
    event_id: &str,
) -> Result<(Vec<u8>, Option<String>), Box<dyn std::error::Error + Send + Sync>> {
    let parsed_room: OwnedRoomId = room_id.parse()?;
    let room = client.get_room(&parsed_room).ok_or("room introuvable")?;
    let parsed_event: OwnedEventId = event_id.parse()?;

    let timeline_event = room.event(&parsed_event, None).await?;
    let raw = timeline_event.raw().deserialize()?;

    use matrix_sdk::ruma::events::AnySyncTimelineEvent;
    let (source, mime) = match raw {
        AnySyncTimelineEvent::MessageLike(ml) => match ml.original_content() {
            Some(AnyMessageLikeEventContent::RoomMessage(rmc)) => match rmc.msgtype {
                MessageType::Audio(audio) => {
                    let mime = audio.info.as_ref().and_then(|i| i.mimetype.clone());
                    (audio.source.clone(), mime)
                }
                _ => return Err("event n'est pas un m.audio".into()),
            },
            _ => return Err("event sans contenu décryptable".into()),
        },
        _ => return Err("event inattendu".into()),
    };

    let request = MediaRequestParameters {
        source,
        format: MediaFormat::File,
    };
    let bytes = client.media().get_media_content(&request, true).await?;
    Ok((bytes, mime))
}

/// Fallback path: write the bytes to a temp file and spawn the OS audio
/// player. Used when in-process decoding fails (common case: Opus).
fn save_and_open(
    bytes: &[u8],
    event_id: &str,
    ext: &str,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let safe_id: String = event_id
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let path = std::env::temp_dir().join(format!("matcurses-voice-{safe_id}.{ext}"));
    std::fs::write(&path, bytes)?;

    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let opener = "xdg-open";
    #[cfg(target_os = "windows")]
    let opener = "cmd";

    #[cfg(target_os = "windows")]
    std::process::Command::new(opener)
        .args(["/C", "start", "", path.to_str().unwrap_or("")])
        .spawn()?;
    #[cfg(not(target_os = "windows"))]
    std::process::Command::new(opener).arg(&path).spawn()?;

    Ok(path)
}

fn ext_for_mime(mime: Option<&str>) -> &'static str {
    match mime {
        Some(m) if m.contains("ogg") => "ogg",
        Some(m) if m.contains("opus") => "opus",
        Some(m) if m.contains("mpeg") || m.contains("mp3") => "mp3",
        Some(m) if m.contains("mp4") || m.contains("aac") || m.contains("m4a") => "m4a",
        Some(m) if m.contains("wav") => "wav",
        Some(m) if m.contains("flac") => "flac",
        Some(m) if m.contains("webm") => "webm",
        _ => "ogg",
    }
}

fn msgtype_to_blocks(msgtype: &MessageType) -> Vec<Block> {
    match msgtype {
        MessageType::Text(t) => {
            // Heuristique : si le formatted body contient <pre><code> ou si le body
            // is wrapped in ```, treat it as a code block.
            let body = t.body.clone();
            if let Some(stripped) = body.strip_prefix("```").and_then(|b| b.strip_suffix("```")) {
                vec![Block::Code(stripped.trim().to_string())]
            } else if body.contains("\n```") {
                // Mixte prose + code : on garde tout en texte pour l'instant.
                vec![Block::Text(body)]
            } else {
                vec![Block::Text(body)]
            }
        }
        MessageType::Notice(n) => vec![Block::Text(n.body.clone())],
        MessageType::Emote(e) => vec![Block::Text(format!("* {}", e.body))],
        MessageType::Audio(a) => {
            // Voice notes: duration if available, else 0.
            let secs = a
                .info
                .as_ref()
                .and_then(|i| i.duration)
                .map(|d| d.as_secs() as u32)
                .unwrap_or(0);
            vec![Block::Voice {
                duration_secs: secs,
            }]
        }
        MessageType::File(f) => vec![Block::Text(format!("[fichier · {}]", f.body))],
        MessageType::Image(i) => vec![Block::Text(format!("[image · {}]", i.body))],
        MessageType::Video(v) => vec![Block::Text(format!("[vidéo · {}]", v.body))],
        _ => vec![Block::Text("[message non supporté]".into())],
    }
}

fn format_time(ts_ms: u64) -> String {
    // ts_ms = Unix milliseconds. Format as local HH:MM.
    use std::time::{SystemTime, UNIX_EPOCH};
    let _ = (SystemTime::now(), UNIX_EPOCH); // avoid unused-import warning
    let secs = ts_ms / 1000;
    // Minimal computation (no chrono dep, stay light): HH:MM UTC.
    let total_min = secs / 60;
    let h = (total_min / 60) % 24;
    let m = total_min % 60;
    format!("{:02}:{:02}", h, m)
}

fn short_author(sender: &str) -> String {
    sender
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or(sender)
        .to_string()
}
