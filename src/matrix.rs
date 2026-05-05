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

use matrix_sdk::matrix_auth::MatrixSession;
use matrix_sdk::config::SyncSettings;
use matrix_sdk::media::{MediaFormat, MediaRequest};
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

/// Commands sent from the UI to the Matrix task.
#[derive(Debug, Clone)]
pub enum Command {
    /// Password login: MXID + password + server (may be empty → autodiscover via MXID)
    Login {
        mxid: String,
        password: String,
        server: String,
    },
    /// The user selected a room in the list — load its contents.
    OpenRoom { room_id: String },
    /// Send a text message to the active room (user pressed Enter).
    SendMessage { room_id: String, body: String },
    /// Force a refresh of the rooms list.
    #[allow(dead_code)]
    RefreshRooms,
    /// Load joined members of a room.
    LoadMembers { room_id: String },
    /// Load the spaces tree (top-level + their children).
    LoadSpaces,
    /// Try to restore a previous session from the SQLite store.
    /// On success → Update::LoggedIn + continuous sync. Otherwise → silence.
    TryRestore,
    /// Download the audio media for the given event and play it through
    /// the in-process rodio player (with fallback to the system player if
    /// the format is not supported, e.g. Opus).
    PlayVoice { room_id: String, event_id: String },
    /// Stop the currently-playing voice note, if any.
    StopVoice,
    /// Send an `m.reaction` to a parent event in a room.
    SendReaction {
        room_id: String,
        parent_event_id: String,
        key: String,
    },
    /// Redact (delete) an event we own — typically used to toggle off a
    /// reaction we previously sent.
    RedactEvent { room_id: String, event_id: String },
}

/// Updates pushed from the Matrix task to the UI.
pub enum Update {
    /// Login OK: effective MXID (in case of autodiscovery).
    LoggedIn { mxid: String },
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
}

/// UI-side bridge. Owns the sender/receiver and the tokio runtime.
pub struct MatrixBridge {
    pub cmd_tx: Sender<Command>,
    update_rx: Receiver<Update>,
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
        std::thread::spawn(move || audio_thread(audio_rx));

        runtime.spawn(matrix_main(cmd_rx, update_tx, audio_tx));

        Ok(Self {
            cmd_tx,
            update_rx,
            _runtime: runtime,
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
fn audio_thread(rx: std::sync::mpsc::Receiver<AudioCommand>) {
    let stream_pair = match rodio::OutputStream::try_default() {
        Ok(p) => p,
        Err(_) => return, // No audio device — silently drop further commands.
    };
    let (_stream, handle) = stream_pair;
    let mut current: Option<rodio::Sink> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            AudioCommand::Play { bytes, ack } => {
                if let Some(s) = current.take() {
                    s.stop();
                }
                match try_play(&handle, bytes) {
                    Ok(sink) => {
                        current = Some(sink);
                        let _ = ack.send(Ok(()));
                    }
                    Err(e) => {
                        let _ = ack.send(Err(e));
                    }
                }
            }
            AudioCommand::Stop => {
                if let Some(s) = current.take() {
                    s.stop();
                }
            }
        }
    }
}

/// Try to decode `bytes` and produce a playing `Sink`. Tries rodio's built-in
/// decoders first (MP3/M4A/FLAC/WAV/Vorbis via Symphonia), then falls back
/// to our custom OGG/Opus path.
fn try_play(
    handle: &rodio::OutputStreamHandle,
    bytes: Vec<u8>,
) -> Result<rodio::Sink, String> {
    // First attempt: rodio + Symphonia.
    let bytes_for_rodio = bytes.clone();
    if let Ok(source) = rodio::Decoder::new(std::io::Cursor::new(bytes_for_rodio)) {
        let sink = rodio::Sink::try_new(handle).map_err(|e| format!("sink : {e}"))?;
        sink.append(source);
        return Ok(sink);
    }
    // Second attempt: OGG/Opus (covers the common voice-note case).
    match crate::audio::OpusSource::try_from_bytes(bytes) {
        Ok(source) => {
            let sink = rodio::Sink::try_new(handle).map_err(|e| format!("sink : {e}"))?;
            sink.append(source);
            Ok(sink)
        }
        Err(opus_err) => Err(format!("aucun décodeur supporté ({opus_err})")),
    }
}

/// Main loop of the Matrix task: receives commands, drives the client.
async fn matrix_main(
    mut cmd_rx: Receiver<Command>,
    update_tx: Sender<Update>,
    audio_tx: std::sync::mpsc::Sender<AudioCommand>,
) {
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
                        tokio::spawn(async move {
                            run_sync(arc2, tx).await;
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
                        tokio::spawn(async move {
                            run_sync(arc2, tx).await;
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
            Command::SendMessage { room_id, body } => {
                if let Some(c) = &client {
                    let tx = update_tx.clone();
                    let c = c.clone();
                    tokio::spawn(async move {
                        if let Err(e) = do_send(&c, &room_id, &body).await {
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
        }
    }
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
    client.matrix_auth().restore_session(session).await?;
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
    let response = client.send(request, None).await?;
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
    let mxid = m.user_id().to_string();
    let displayname = m
        .display_name()
        .map(|s| s.to_string())
        .unwrap_or_else(|| m.user_id().localpart().to_string());
    let power_level = m.power_level().clamp(0, 100) as u8;
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
            expanded: false,
            children,
        },
    }
}

async fn collect_space_children(
    client: &Client,
    space: &matrix_sdk::Room,
    visited: &mut std::collections::HashSet<String>,
) -> Vec<UiNode> {
    use matrix_sdk::ruma::events::space::child::SpaceChildEventContent;
    let mut out = Vec::new();
    let raw_events = match space
        .get_state_events_static::<SpaceChildEventContent>()
        .await
    {
        Ok(v) => v,
        Err(_) => return out,
    };
    for raw in raw_events {
        let parsed = match raw.deserialize() {
            Ok(p) => p,
            Err(_) => continue,
        };
        // state_key is the child room ID
        let child_id_str = parsed.state_key().to_string();
        if visited.contains(&child_id_str) {
            continue;
        }
        let child_id: OwnedRoomId = match child_id_str.parse() {
            Ok(id) => id,
            Err(_) => continue,
        };
        let child_room = match client.get_room(&child_id) {
            Some(r) => r,
            None => continue,
        };
        if child_room.is_space() {
            let node = Box::pin(build_space_node(client, &child_room, visited)).await;
            out.push(node);
        } else {
            visited.insert(child_id_str.clone());
            let label = child_room
                .display_name()
                .await
                .map(|n| n.to_string())
                .unwrap_or_else(|_| child_id_str.clone());
            let counts = child_room.unread_notification_counts();
            out.push(UiNode {
                label,
                kind: UiNodeKind::Room {
                    name: child_id_str,
                    unread: counts.notification_count as usize,
                },
            });
        }
    }
    out
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
async fn run_sync(client: Arc<Client>, tx: Sender<Update>) {
    // Handler pour live messages.
    let tx_handler = tx.clone();
    client.add_event_handler(
        move |ev: SyncRoomMessageEvent, room: matrix_sdk::room::Room| {
            let tx = tx_handler.clone();
            async move {
                let original = match ev.as_original() {
                    Some(o) => o,
                    None => return, // redaction
                };
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
    let result = client
        .sync_with_callback(settings, move |_response| {
            let tx = tx_cb.clone();
            let c = client_cb.clone();
            async move {
                let _ = tx.send(snapshot_rooms(&c).await).await;
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

        rooms.push(UiRoom {
            name,
            unread,
            mentions,
            pinned: false,
            muted: false,
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

    // If the room is an open invite, accept it before fetching messages.
    // Otherwise the homeserver returns 403 (user not in room).
    match room.state() {
        RoomState::Joined => {}
        RoomState::Invited => {
            room.join().await?;
        }
        RoomState::Left => return Err("room quittée".into()),
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

    use matrix_sdk::ruma::events::AnyTimelineEvent;
    use std::collections::HashMap;

    // Walk events in chronological (oldest → newest) order.
    let events: Vec<AnyTimelineEvent> = chunk
        .chunk
        .iter()
        .rev()
        .filter_map(|tev| tev.event.deserialize().ok())
        .collect();

    // Pass 1: extract top-level RoomMessages and undecryptable placeholders.
    // Skip thread replies, they are attached to their root in pass 2.
    let mut out: Vec<Message> = Vec::new();
    let mut idx_by_event: HashMap<String, usize> = HashMap::new();

    for raw in &events {
        if let AnyTimelineEvent::MessageLike(ml) = raw {
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
        if let AnyTimelineEvent::MessageLike(ml) = raw {
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

/// Send a plain text message into the room.
async fn do_send(
    client: &Client,
    room_id: &str,
    body: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parsed: OwnedRoomId = room_id.parse()?;
    let room = client
        .get_room(&parsed)
        .ok_or("room introuvable pour send")?;
    let content = RoomMessageEventContent::text_plain(body);
    room.send(content).await?;
    Ok(())
}

/// Convert an `m.room.message` event content to our UI `Message`.
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

    let timeline_event = room.event(&parsed_event).await?;
    let raw = timeline_event.event.deserialize()?;

    use matrix_sdk::ruma::events::AnyTimelineEvent;
    let (source, mime) = match raw {
        AnyTimelineEvent::MessageLike(ml) => match ml.original_content() {
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

    let request = MediaRequest {
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
