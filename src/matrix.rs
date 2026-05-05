//! Pont entre l'UI synchrone (boucle crossterm) et le SDK Matrix asynchrone.
//!
//! Le `MatrixBridge` détient un runtime tokio en arrière-plan, deux channels
//! `mpsc` (UI → bg = Command, bg → UI = Update) et fait tourner une tâche
//! qui pilote `matrix_sdk::Client` (login, sync, send, etc.).
//!
//! Le crate `widgets/` ne dépend pas de ce module — c'est `app.rs` qui se
//! charge de mapper les Updates vers l'état UI.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::config::SyncSettings;
use matrix_sdk::room::{MessagesOptions, RoomMember};
use matrix_sdk::ruma::api::client::filter::RoomEventFilter;
use matrix_sdk::ruma::events::room::message::{
    MessageType, RoomMessageEventContent, SyncRoomMessageEvent,
};
use matrix_sdk::ruma::events::AnyMessageLikeEventContent;
use matrix_sdk::ruma::OwnedRoomId;
use matrix_sdk::{Client, RoomMemberships};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::message::{Block, Message};
use crate::view::members::{Member as UiMember, Presence as UiPresence};
use crate::view::room_list::Room as UiRoom;
use crate::view::space_tree::{Node as UiNode, NodeKind as UiNodeKind};

/// Commandes envoyées de l'UI vers la tâche Matrix.
#[derive(Debug, Clone)]
pub enum Command {
    /// Login password : MXID + password + serveur (peut être vide → autodiscover via MXID)
    Login {
        mxid: String,
        password: String,
        server: String,
    },
    /// L'utilisateur a sélectionné une room dans la liste — charger son contenu.
    OpenRoom { room_id: String },
    /// Envoyer un message texte dans la room active (l'utilisateur a tapé Entrée).
    SendMessage { room_id: String, body: String },
    /// Forcer un refresh de la liste des rooms.
    #[allow(dead_code)]
    RefreshRooms,
    /// Charger les membres joinés d'une room.
    LoadMembers { room_id: String },
    /// Charger l'arbre des spaces (top-level + leurs enfants).
    LoadSpaces,
}

/// Mises à jour poussées par la tâche Matrix vers l'UI.
pub enum Update {
    /// Login OK : MXID effectif (en cas d'autodiscovery).
    LoggedIn { mxid: String },
    /// Login KO : message d'erreur lisible.
    LoginFailed { reason: String },
    /// Liste de rooms mise à jour (sync ou refresh manuel).
    Rooms {
        rooms: Vec<UiRoom>,
        ids: Vec<String>,
    },
    /// Historique d'une room chargé / rafraîchi.
    RoomMessages {
        room_id: String,
        messages: Vec<Message>,
    },
    /// Nouvel event arrivé sur une room (pendant un sync live).
    NewMessage {
        room_id: String,
        message: Message,
    },
    /// Message d'erreur générique (sync, send, etc.) — à afficher en flash.
    Error { reason: String },
    /// Sync initial terminé.
    SyncComplete,
    /// Membres d'une room (en réponse à LoadMembers).
    Members {
        room_id: String,
        members: Vec<UiMember>,
    },
    /// Arbre des spaces (en réponse à LoadSpaces).
    Spaces { roots: Vec<UiNode> },
}

/// Pont côté UI. Détient les sender/receiver et le runtime tokio.
pub struct MatrixBridge {
    pub cmd_tx: Sender<Command>,
    update_rx: Receiver<Update>,
    /// Le runtime est gardé vivant tant que le bridge existe.
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

        runtime.spawn(matrix_main(cmd_rx, update_tx));

        Ok(Self {
            cmd_tx,
            update_rx,
            _runtime: runtime,
        })
    }

    /// Envoi non-bloquant d'une commande. Si le channel est plein, on log et on jette.
    pub fn send(&self, cmd: Command) {
        if let Err(e) = self.cmd_tx.try_send(cmd) {
            // Pas de log proper ici, on ne veut pas casser l'UI.
            // La commande perdue est généralement bénigne (refresh).
            let _ = e;
        }
    }

    /// Drain des updates en attente, sans bloquer.
    pub fn drain_updates(&mut self) -> Vec<Update> {
        let mut out = Vec::new();
        while let Ok(u) = self.update_rx.try_recv() {
            out.push(u);
        }
        out
    }
}

/// Boucle principale de la tâche Matrix : reçoit des commandes, gère le client.
async fn matrix_main(mut cmd_rx: Receiver<Command>, update_tx: Sender<Update>) {
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
                        let arc = Arc::new(c);
                        client = Some(arc.clone());
                        let _ = update_tx
                            .send(Update::LoggedIn { mxid: mxid.clone() })
                            .await;

                        // Lancer un sync_once pour peupler les rooms, puis un sync continu.
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

/// Charge les membres joinés d'une room et envoie un Update::Members.
async fn load_members(
    client: &Client,
    room_id: &str,
    tx: &Sender<Update>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parsed: OwnedRoomId = room_id.parse()?;
    let room = client.get_room(&parsed).ok_or("room introuvable")?;
    let members = room.members(RoomMemberships::JOIN).await?;
    let mut out: Vec<UiMember> = members.iter().map(map_member).collect();
    // Tri : admin d'abord, puis modérateurs, puis alphabétique.
    out.sort_by(|a, b| {
        b.power_level
            .cmp(&a.power_level)
            .then_with(|| a.displayname.to_lowercase().cmp(&b.displayname.to_lowercase()))
    });
    let _ = tx
        .send(Update::Members {
            room_id: room_id.to_string(),
            members: out,
        })
        .await;
    Ok(())
}

fn map_member(m: &RoomMember) -> UiMember {
    let mxid = m.user_id().to_string();
    let displayname = m
        .display_name()
        .map(|s| s.to_string())
        .unwrap_or_else(|| m.user_id().localpart().to_string());
    let power_level = m.power_level().clamp(0, 100) as u8;
    // La presence n'est pas exposée directement sur RoomMember en matrix-sdk 0.7 :
    // elle dépend d'une route séparée et n'est pas systématiquement remplie.
    // On reste sur Unavailable par défaut ; on pourra brancher la presence API plus tard.
    let presence = UiPresence::Unavailable;
    UiMember {
        mxid,
        displayname,
        power_level,
        presence,
    }
}

/// Charge l'arbre des spaces : pour chaque space joinable, récupère ses
/// enfants directs (m.space.child state events), récursif sur les sub-spaces.
async fn load_spaces(
    client: &Client,
    tx: &Sender<Update>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::collections::HashSet;
    let mut visited: HashSet<String> = HashSet::new();
    let mut roots = Vec::new();
    // On considère comme racines tous les spaces joinables (matrix-sdk ne donne pas
    // directement la notion de "top-level"; en pratique l'utilisateur peut joindre
    // un sub-space directement, donc on liste tout et l'utilisateur trie).
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
    // Les rooms hors de tout space restent accessibles via F4 (room list).
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
        // state_key est l'ID de la room enfant
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

/// Effectue le login + restore éventuel de session, retourne un Client connecté.
async fn do_login(
    mxid: &str,
    password: &str,
    server: &str,
) -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
    if mxid.is_empty() || password.is_empty() {
        return Err("MXID ou mot de passe vide".into());
    }

    let store_path = session_store_path(mxid)?;
    std::fs::create_dir_all(&store_path)?;

    // Si l'utilisateur a fourni une URL explicite, on l'utilise telle quelle.
    // Sinon (champ vide ou simple domaine), on passe par `server_name` qui
    // déclenche l'auto-discovery .well-known/matrix/client — plus robuste pour
    // les Synapse qui sont derrière un reverse proxy.
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

    // Le MXID complet (avec @ si fourni sans, on le préfixe). login_username
    // accepte localpart ou MXID complet — passer le MXID complet est le plus
    // explicite côté serveur.
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

/// Lance le sync continu et pousse des updates Rooms à chaque tick.
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

    // Sync continu.
    if let Err(e) = client.sync(settings).await {
        let _ = tx
            .send(Update::Error {
                reason: format!("sync : {e}"),
            })
            .await;
    }
}

/// Snapshot synchrone (côté tokio) des rooms.
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

/// Charge ~50 derniers messages d'une room et envoie un Update::RoomMessages.
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

    let mut opts = MessagesOptions::backward();
    opts.limit = matrix_sdk::ruma::UInt::from(50u32);
    let mut filter = RoomEventFilter::default();
    filter.types = Some(vec!["m.room.message".to_owned()]);
    opts.filter = filter;

    let chunk = room.messages(opts).await?;

    let mut out = Vec::new();
    for tev in chunk.chunk.iter().rev() {
        // tev: matrix_sdk::deserialized_responses::TimelineEvent
        let raw = tev.event.deserialize();
        let raw = match raw {
            Ok(e) => e,
            Err(_) => continue,
        };
        use matrix_sdk::ruma::events::AnyTimelineEvent;
        if let AnyTimelineEvent::MessageLike(ml) = raw {
            if let Some(content) = ml.original_content() {
                if let AnyMessageLikeEventContent::RoomMessage(rmc) = content {
                    let sender = ml.sender().to_string();
                    let ts = ml.origin_server_ts();
                    let msg = event_content_to_message(&sender, &rmc.msgtype, ts.0.into());
                    out.push(msg);
                }
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

/// Envoie un message texte plain dans la room.
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

/// Convertit un MessageType (room message) vers notre `Message` UI.
fn event_content_to_message(sender: &str, msgtype: &MessageType, ts_ms: u64) -> Message {
    let time = format_time(ts_ms);
    let author = short_author(sender);
    let blocks = msgtype_to_blocks(msgtype);
    Message {
        time,
        author,
        blocks,
        replies: Vec::new(),
        reactions: Vec::new(),
    }
}

fn msgtype_to_blocks(msgtype: &MessageType) -> Vec<Block> {
    match msgtype {
        MessageType::Text(t) => {
            // Heuristique : si le formatted body contient <pre><code> ou si le body
            // est entouré de ```, on considère que c'est du code.
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
            // Voice notes : durée si dispo, sinon 0
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
    // ts_ms = millisecondes Unix. On formatte en HH:MM local.
    use std::time::{SystemTime, UNIX_EPOCH};
    let _ = (SystemTime::now(), UNIX_EPOCH); // évite warning si non utilisé
    let secs = ts_ms / 1000;
    // Calcul minimaliste (pas de chrono pour rester light) : HH:MM UTC.
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
