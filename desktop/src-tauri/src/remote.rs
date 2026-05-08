// Remote multiplayer subsystem.
//
// Mirrors the protocol that lives in eggs/scripts/egg_desktop.py:RemoteSession
// and talks to the Go server at server/main.go. A single tokio task owns the
// websocket lifetime, polls ~/.eggs/remote.json for config changes
// (reconnects on signature drift), polls state.json for pet/state changes
// (pushes to the wire), and emits Tauri events ("remote-status" /
// "remote-peers") so the webview can render presence.
//
// CLI subcommands (`eggs remote on/off/server/random/room/leave`) just mutate
// remote.json and exit; the running GUI's poller reacts automatically. This
// keeps the CLI <-> GUI contract identical to the legacy Python tool.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::bubbles;
use crate::client::read_client_config;
use crate::bubbles::SharedBubbleWindowManager;
use crate::peers::SharedPeerWindowManager;
use crate::state;

const DEFAULT_SERVER_URL: &str = "http://localhost:8787";
const DEFAULT_ROOM_LIMIT: u16 = 5;
const MIN_ROOM_LIMIT: u16 = 2;
const MAX_ROOM_LIMIT: u16 = 100;
const HEARTBEAT_SECS: u64 = 20;
/// Tear down + reconnect if no frame (incl. server PINGs every 15s) has
/// arrived in this window. Set to match the server's read timeout
/// (`wsReadTimeout = 45s`); the server will already have given up by then,
/// and TCP may not surface a dead socket on its own (e.g. after sleep/wake).
const LIVENESS_TIMEOUT_SECS: u64 = 45;
const RECONNECT_INITIAL_SECS: f64 = 2.0;
const RECONNECT_MAX_SECS: f64 = 60.0;
const RECONNECT_BACKOFF: f64 = 2.0;
const POLL_INTERVAL_MS: u64 = 200;

#[derive(Debug, Default)]
struct RemoteRuntimeSnapshot {
    connected: bool,
    peer_count: usize,
}

fn runtime_snapshot() -> &'static Mutex<RemoteRuntimeSnapshot> {
    static SNAPSHOT: OnceLock<Mutex<RemoteRuntimeSnapshot>> = OnceLock::new();
    SNAPSHOT.get_or_init(|| Mutex::new(RemoteRuntimeSnapshot::default()))
}

fn with_runtime_snapshot<R>(f: impl FnOnce(&mut RemoteRuntimeSnapshot) -> R) -> R {
    let mut guard = runtime_snapshot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&mut guard)
}

pub fn can_leave_room() -> bool {
    with_runtime_snapshot(|snap| snap.connected && snap.peer_count > 0)
}

// ---------- remote.json --------------------------------------------------

// The "active sprite" is intentionally NOT stored here -- state.json::pet is
// the single source of truth for the pet currently being rendered, and we
// announce that to the server. Keeping a duplicate in remote.json caused
// drift bugs when the user swapped pets without going through the path that
// updated both files.

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RemoteConfig {
    #[serde(default = "default_server_url")]
    pub server_url: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub room: String,
    #[serde(default = "default_room_limit")]
    pub room_limit: u16,
    #[serde(default)]
    pub session_nonce: u64,
}

fn default_server_url() -> String {
    std::env::var("EGGS_REMOTE_URL").unwrap_or_else(|_| DEFAULT_SERVER_URL.to_string())
}

fn default_mode() -> String {
    "random".to_string()
}

fn default_room_limit() -> u16 {
    DEFAULT_ROOM_LIMIT
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            server_url: default_server_url(),
            enabled: false,
            mode: default_mode(),
            room: String::new(),
            room_limit: default_room_limit(),
            session_nonce: 0,
        }
    }
}

fn remote_path() -> std::path::PathBuf {
    state::app_dir().join("remote.json")
}

pub fn read_remote_config() -> RemoteConfig {
    let path = remote_path();
    let mut cfg = if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<RemoteConfig>(&t).ok())
            .unwrap_or_default()
    } else {
        let cfg = RemoteConfig::default();
        let _ = write_remote_config(&cfg);
        cfg
    };
    cfg.server_url = cfg.server_url.trim_end_matches('/').trim().to_string();
    if cfg.server_url.is_empty() {
        cfg.server_url = default_server_url();
    }
    cfg.mode = normalize_mode(&cfg.mode, &cfg.room);
    cfg.room = cfg.room.trim().to_string();
    cfg.room_limit = normalize_room_limit(cfg.room_limit);
    cfg
}

fn normalize_mode(mode: &str, room: &str) -> String {
    let normalized_mode = mode.trim().to_ascii_lowercase();
    let has_room = !room.trim().is_empty();
    match normalized_mode.as_str() {
        "room" if has_room => "room".to_string(),
        "room" => "random".to_string(),
        "random" => "random".to_string(),
        _ if has_room => "room".to_string(),
        _ => default_mode(),
    }
}

fn normalize_room_limit(limit: u16) -> u16 {
    match limit {
        0 => DEFAULT_ROOM_LIMIT,
        n if n < MIN_ROOM_LIMIT => MIN_ROOM_LIMIT,
        n if n > MAX_ROOM_LIMIT => MAX_ROOM_LIMIT,
        n => n,
    }
}

pub fn write_remote_config(cfg: &RemoteConfig) -> std::io::Result<()> {
    let dir = state::app_dir();
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(cfg)?;
    std::fs::write(remote_path(), format!("{json}\n"))
}

/// Read-modify-write helper for partial updates from CLI subcommands.
pub fn update_remote_config<F: FnOnce(&mut RemoteConfig)>(f: F) -> std::io::Result<RemoteConfig> {
    let mut cfg = read_remote_config();
    f(&mut cfg);
    write_remote_config(&cfg)?;
    Ok(cfg)
}

pub fn leave_room() -> std::io::Result<RemoteConfig> {
    update_remote_config(|cfg| {
        cfg.session_nonce = cfg.session_nonce.wrapping_add(1);
    })
}

/// Reconnect signature. Includes server URL, mode, room, plus a nonce used by
/// "leave room" to force a reconnect cycle without disabling remote. Local
/// sprite changes are handled in-place via a `{"type":"sprite",...}` message
/// over the existing WS (the server's hub.updateClientSprite hot-swaps the
/// binding and broadcasts `peer_sprite_changed`), so swapping pets must NOT
/// tear down the room or matchmaking.
fn config_signature(cfg: &RemoteConfig) -> String {
    if !cfg.enabled {
        return "off".to_string();
    }
    format!(
        "on|{}|{}|{}|{}|{}",
        cfg.server_url, cfg.mode, cfg.room, cfg.room_limit, cfg.session_nonce
    )
}

// ---------- websocket session -------------------------------------------

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Session {
    write: futures_util::stream::SplitSink<WsStream, Message>,
    read: futures_util::stream::SplitStream<WsStream>,
    /// Last time we received any frame (text/binary/ping/pong/close).
    /// Bumped inside `recv_json` so the outer loop can detect a silent
    /// half-dead TCP connection (e.g. after the laptop wakes from sleep
    /// and the OS hasn't surfaced the dead socket yet) and force a
    /// reconnect. Compared against `LIVENESS_TIMEOUT_SECS`.
    last_activity: Instant,
}

impl Session {
    async fn connect(cfg: &RemoteConfig, device_id: &str, sprite: &str) -> Result<Self, String> {
        let url = build_ws_url(cfg, device_id, sprite).map_err(|e| e.to_string())?;
        let (stream, _resp) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .map_err(|e| e.to_string())?;
        let (write, read) = stream.split();
        Ok(Self {
            write,
            read,
            last_activity: Instant::now(),
        })
    }

    async fn send_json(&mut self, value: Value) -> Result<(), String> {
        let text = serde_json::to_string(&value).map_err(|e| e.to_string())?;
        self.write
            .send(Message::Text(text))
            .await
            .map_err(|e| e.to_string())
    }

    async fn recv_json(&mut self) -> Result<Value, String> {
        loop {
            match self.read.next().await {
                Some(Ok(Message::Text(t))) => {
                    self.last_activity = Instant::now();
                    return serde_json::from_str(&t).map_err(|e| e.to_string());
                }
                Some(Ok(Message::Binary(_))) => {
                    self.last_activity = Instant::now();
                    continue;
                }
                Some(Ok(Message::Ping(p))) => {
                    self.last_activity = Instant::now();
                    let _ = self.write.send(Message::Pong(p)).await;
                    continue;
                }
                Some(Ok(Message::Pong(_))) | Some(Ok(Message::Frame(_))) => {
                    self.last_activity = Instant::now();
                    continue;
                }
                Some(Ok(Message::Close(c))) => {
                    return Err(c
                        .map(|f| format!("server closed: {} {}", f.code, f.reason))
                        .unwrap_or_else(|| "server closed".to_string()));
                }
                Some(Err(e)) => return Err(e.to_string()),
                None => return Err("websocket stream ended".to_string()),
            }
        }
    }

    fn idle_for(&self) -> Duration {
        Instant::now().saturating_duration_since(self.last_activity)
    }

    async fn close(mut self) {
        let _ = self.write.send(Message::Close(None)).await;
    }
}

fn build_ws_url(cfg: &RemoteConfig, device_id: &str, sprite: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(&cfg.server_url).map_err(|e| e.to_string())?;
    let scheme = match parsed.scheme() {
        "https" => "wss",
        _ => "ws",
    };
    let host = parsed.host_str().ok_or("server_url is missing host")?;
    let port_part = match parsed.port() {
        Some(p) => format!(":{p}"),
        None => String::new(),
    };
    let prefix = parsed.path().trim_end_matches('/');
    let mut url = url::Url::parse(&format!("{scheme}://{host}{port_part}{prefix}/ws"))
        .map_err(|e| e.to_string())?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("device_id", device_id);
        q.append_pair("mode", &cfg.mode);
        q.append_pair("room", &cfg.room);
        q.append_pair("sprite", sprite);
        if cfg.mode == "room" {
            q.append_pair("room_limit", &cfg.room_limit.to_string());
        }
    }
    Ok(url)
}

fn is_permanent_error(reason: &str) -> bool {
    let text = reason.to_lowercase();
    if text.is_empty() {
        return false;
    }
    [
        "http error: 400",
        "http error: 401",
        "http error: 403",
        "http error: 404",
        "http error: 409",
        "http error: 422",
        // Legacy markers (string match the Go server's bodies)
        "unknown sprite for device",
        "device_id and sprite are required",
        "room code is required",
    ]
    .iter()
    .any(|m| text.contains(m))
}

// ---------- emitted events ----------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct RemoteStatus {
    pub enabled: bool,
    pub connected: bool,
    pub reconnecting: bool,
    pub error: String,
    pub server_url: String,
    pub mode: String,
    pub room: String,
    pub room_limit: u16,
    pub sprite: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerSnapshot {
    pub device_id: String,
    pub state: String,
    /// Pet/sprite name as reported by the peer (`sprite.name` from server).
    pub sprite: String,
    /// Where the webview can fetch the peer's atlas + manifest (+ optional
    /// config). The Rust side downloads these into the local remote-asset
    /// cache before opening the peer window.
    pub sprite_url: String,
    pub json_url: String,
    pub config_url: Option<String>,
}

fn build_status(
    cfg: &RemoteConfig,
    sprite: &str,
    connected: bool,
    reconnecting: bool,
    error: &str,
) -> RemoteStatus {
    RemoteStatus {
        enabled: cfg.enabled,
        connected,
        reconnecting,
        error: error.trim().to_string(),
        server_url: cfg.server_url.clone(),
        mode: cfg.mode.clone(),
        room: cfg.room.clone(),
        room_limit: cfg.room_limit,
        sprite: sprite.to_string(),
    }
}

fn emit_status(app: &AppHandle, status: &RemoteStatus) {
    with_runtime_snapshot(|snap| {
        snap.connected = status.connected;
    });
    let _ = app.emit("remote-status", status);
}

fn emit_peers(app: &AppHandle, peers: &HashMap<String, PeerSnapshot>) {
    with_runtime_snapshot(|snap| {
        snap.peer_count = peers.len();
    });
    let mut list: Vec<&PeerSnapshot> = peers.values().collect();
    list.sort_by(|a, b| a.device_id.cmp(&b.device_id));
    let _ = app.emit("remote-peers", &list);
}

// ---------- peer state ---------------------------------------------------

fn peer_from_message(msg: &Value) -> Option<PeerSnapshot> {
    let device_id = msg.get("device_id")?.as_str()?.to_string();
    if device_id.is_empty() {
        return None;
    }
    let state = msg
        .get("state")
        .or_else(|| msg.get("action"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let sprite_obj = msg.get("sprite").and_then(|v| v.as_object());
    let sprite = sprite_obj
        .and_then(|o| o.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // Server renamed `png_url` → `sprite_url`; accept both for back-compat.
    let sprite_url = sprite_obj
        .and_then(|o| o.get("sprite_url").or_else(|| o.get("png_url")))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let json_url = sprite_obj
        .and_then(|o| o.get("json_url"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let config_url = sprite_obj
        .and_then(|o| o.get("config_url"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Some(PeerSnapshot {
        device_id,
        state,
        sprite,
        sprite_url,
        json_url,
        config_url,
    })
}

fn merge_peer_update(existing: Option<&PeerSnapshot>, msg: &Value) -> Option<PeerSnapshot> {
    let mut next = match peer_from_message(msg) {
        Some(p) => p,
        None => existing.cloned()?,
    };
    if let Some(prev) = existing {
        if next.sprite.is_empty() {
            next.sprite = prev.sprite.clone();
        }
        if next.sprite_url.is_empty() {
            next.sprite_url = prev.sprite_url.clone();
        }
        if next.json_url.is_empty() {
            next.json_url = prev.json_url.clone();
        }
        if next.state.is_empty() {
            next.state = prev.state.clone();
        }
        if next.config_url.is_none() {
            next.config_url = prev.config_url.clone();
        }
    }
    Some(next)
}

fn handle_incoming(app: &AppHandle, peers: &mut HashMap<String, PeerSnapshot>, msg: Value) -> bool {
    let msg_type = msg
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut changed = false;
    match msg_type.as_str() {
        "room_snapshot" => {
            peers.clear();
            if let Some(items) = msg.get("peers").and_then(|v| v.as_array()) {
                for item in items {
                    if let Some(p) = peer_from_message(item) {
                        peers.insert(p.device_id.clone(), p);
                    }
                }
            }
            changed = true;
        }
        "peer_left" => {
            if let Some(id) = msg.get("device_id").and_then(|v| v.as_str()) {
                if peers.remove(id).is_some() {
                    changed = true;
                }
            }
        }
        "peer_joined" | "peer_state" | "peer_action" | "peer_sprite_changed" => {
            if let Some(id) = msg.get("device_id").and_then(|v| v.as_str()) {
                let updated = merge_peer_update(peers.get(id), &msg);
                if let Some(p) = updated {
                    peers.insert(p.device_id.clone(), p);
                    changed = true;
                }
            }
        }
        _ => {}
    }
    if changed {
        emit_peers(app, peers);
    }
    changed
}

fn room_code_for_history(cfg: &RemoteConfig) -> Option<String> {
    if cfg.mode == "room" {
        let room = cfg.room.trim();
        if !room.is_empty() {
            return Some(room.to_string());
        }
    }
    None
}

fn handle_peer_chat(cfg: &RemoteConfig, msg: &Value) {
    let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if msg_type != "peer_chat" {
        return;
    }
    let device_id = msg
        .get("device_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let text = msg.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
    if device_id.is_empty() || text.is_empty() {
        return;
    }
    let room_code = room_code_for_history(cfg);

    let event = match bubbles::queue_peer_user_message(
        &device_id,
        text,
        room_code.clone(),
    ) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("remote peer chat bubble queue failed: {e}");
            None
        }
    };

    if let (Some(room), Some(evt)) = (room_code, event) {
        let history = bubbles::ChatHistoryEntry {
            id: evt.id,
            source: bubbles::BubbleSource::PeerUserInput,
            owner: bubbles::HistoryOwner::from(&evt.owner),
            text: evt.text,
            created_ms: evt.created_ms,
            device_id: Some(device_id),
        };
        if let Err(e) = bubbles::append_room_history(&room, history) {
            eprintln!("remote peer chat history append failed: {e}");
        }
    }
}

// ---------- public entry --------------------------------------------------

pub fn start(
    app: AppHandle,
    shutdown: Arc<AtomicBool>,
    peer_windows: SharedPeerWindowManager,
    bubble_windows: SharedBubbleWindowManager,
) {
    tauri::async_runtime::spawn(async move {
        run_actor(app, shutdown, peer_windows, bubble_windows).await;
    });
}

async fn run_actor(
    app: AppHandle,
    shutdown: Arc<AtomicBool>,
    peer_windows: SharedPeerWindowManager,
    bubble_windows: SharedBubbleWindowManager,
) {
    let mut current_signature = String::new();
    let mut session: Option<Session> = None;
    let mut peers: HashMap<String, PeerSnapshot> = HashMap::new();
    let mut last_state = state::read_state().unwrap_or_else(|_| state::RuntimeState {
        pet: String::new(),
        state: "idle".to_string(),
        scale_millis: 1000,
        window_x: None,
        window_y: None,
    });
    let mut pending_sprite_announce = false;
    let mut pending_state_sync = false;
    let mut next_heartbeat: Option<Instant> = None;
    let mut reconnect_delay = RECONNECT_INITIAL_SECS;
    let mut last_status_payload: Option<String> = None;
    let mut pending_chat_outbox: VecDeque<bubbles::ChatOutboxFile> = VecDeque::new();
    let mut pending_chat_paths: HashSet<String> = HashSet::new();
    // Name of the sprite we last successfully ran ensure_pet_uploaded for on
    // the current `cfg.server_url`. Used to skip a redundant hash-skip GET
    // when the connect block already uploaded this exact sprite on the same
    // tick, and reset on signature drift since a new server may not have it.
    let mut last_uploaded_sprite: String = String::new();

    let emit_if_changed = |status: &RemoteStatus, last: &mut Option<String>, app: &AppHandle| {
        let payload = serde_json::to_string(status).unwrap_or_default();
        if last.as_deref() != Some(payload.as_str()) {
            emit_status(app, status);
            *last = Some(payload);
        }
    };

    loop {
        if shutdown.load(Ordering::Relaxed) {
            if let Some(s) = session.take() {
                s.close().await;
            }
            peer_windows.close_all(&app).await;
            bubble_windows
                .sync_peer_owners(&app, &HashSet::<String>::new())
                .await;
            return;
        }

        let cfg = read_remote_config();

        if let Ok(outbox_files) = bubbles::drain_chat_outbox() {
            for file in outbox_files {
                let key = file.path.to_string_lossy().to_string();
                if pending_chat_paths.insert(key) {
                    pending_chat_outbox.push_back(file);
                }
            }
        }

        // Read state.json once per tick: it's the source of truth for the
        // current sprite (state.pet) and the animation state we announce. The
        // remote actor never persists or caches sprite separately, so a
        // `eggs pet <id>` from the CLI is picked up automatically.
        if let Ok(cur) = state::read_state() {
            if cur.pet != last_state.pet {
                pending_sprite_announce = true;
            } else if cur.state != last_state.state {
                pending_state_sync = true;
            }
            last_state = cur;
        }
        let current_sprite = last_state.pet.clone();

        let signature = config_signature(&cfg);

        // --- handle config changes ---
        if signature != current_signature {
            if let Some(s) = session.take() {
                s.close().await;
            }
            peers.clear();
            emit_peers(&app, &peers);
            peer_windows.sync(&app, &peers).await;
            bubble_windows
                .sync_peer_owners(&app, &HashSet::<String>::new())
                .await;
            current_signature = signature.clone();
            reconnect_delay = RECONNECT_INITIAL_SECS;
            // The new server (if URL changed) may not have this sprite
            // uploaded; force a re-check on next connect.
            last_uploaded_sprite.clear();
            let status = build_status(&cfg, &current_sprite, false, cfg.enabled, "");
            emit_if_changed(&status, &mut last_status_payload, &app);
            if !cfg.enabled {
                tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                continue;
            }
        }

        if !cfg.enabled {
            tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
            continue;
        }

        // --- ensure connected ---
        if session.is_none() {
            let device_id = match read_client_config() {
                Ok(c) => c.device_id,
                Err(e) => {
                    let status = build_status(
                        &cfg,
                        &current_sprite,
                        false,
                        false,
                        &format!("client.json: {e}"),
                    );
                    emit_if_changed(&status, &mut last_status_payload, &app);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };
            if current_sprite.is_empty() {
                let status = build_status(
                    &cfg,
                    &current_sprite,
                    false,
                    false,
                    "no pet configured (run: eggs pet <id>)",
                );
                emit_if_changed(&status, &mut last_status_payload, &app);
                tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                continue;
            }

            // Hash-skip upload before connecting. The Go server's WS handshake
            // refuses unknown sprites for the device, so this is mandatory the
            // first time a pet is announced. On the warm path it's one GET.
            match crate::upload::ensure_pet_uploaded(&cfg.server_url, &device_id, &current_sprite)
                .await
            {
                Ok(_) => {
                    last_uploaded_sprite = current_sprite.clone();
                }
                Err(e) => {
                    let retryable = e.is_retryable();
                    let msg = format!("pet upload failed: {e}");
                    let status = build_status(&cfg, &current_sprite, false, retryable, &msg);
                    emit_if_changed(&status, &mut last_status_payload, &app);
                    if retryable {
                        tokio::time::sleep(Duration::from_secs_f64(reconnect_delay)).await;
                        reconnect_delay =
                            (reconnect_delay * RECONNECT_BACKOFF).min(RECONNECT_MAX_SECS);
                    } else {
                        // Permanent: wait for the user to fix config / install
                        // the pet. The config-signature poller will wake us.
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    continue;
                }
            }

            match Session::connect(&cfg, &device_id, &current_sprite).await {
                Ok(s) => {
                    session = Some(s);
                    pending_sprite_announce = true;
                    pending_state_sync = true;
                    next_heartbeat = Some(Instant::now());
                    reconnect_delay = RECONNECT_INITIAL_SECS;
                    let status = build_status(&cfg, &current_sprite, true, false, "");
                    emit_if_changed(&status, &mut last_status_payload, &app);
                }
                Err(e) => {
                    let permanent = is_permanent_error(&e);
                    let status = build_status(&cfg, &current_sprite, false, !permanent, &e);
                    emit_if_changed(&status, &mut last_status_payload, &app);
                    if permanent {
                        // Stay disabled-ish until the user changes config.
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    } else {
                        tokio::time::sleep(Duration::from_secs_f64(reconnect_delay)).await;
                        reconnect_delay =
                            (reconnect_delay * RECONNECT_BACKOFF).min(RECONNECT_MAX_SECS);
                    }
                    continue;
                }
            }
        }

        // --- in-place sprite swap: upload before announcing ---
        //
        // Mid-session pet switches no longer drop the WS (we just push a
        // `{"type":"sprite",...}` message below and let the server hot-swap
        // the binding via hub.updateClientSprite + broadcast
        // `peer_sprite_changed`). The server validates that the new sprite
        // is uploaded for this device first, so we must run the hash-skip
        // upload here too. `state::set_pet` (CLI + menu path) already does
        // this synchronously before writing state.json, so on the warm path
        // `last_uploaded_sprite` matches and this is a no-op; the branch is
        // defense-in-depth for legacy callers that write state.json directly
        // (e.g. the Python egg_desktop.py shipped with older skill versions).
        if pending_sprite_announce
            && session.is_some()
            && !current_sprite.is_empty()
            && current_sprite != last_uploaded_sprite
        {
            let device_id = match read_client_config() {
                Ok(c) => c.device_id,
                Err(e) => {
                    eprintln!("remote: client.json read failed mid-session: {e}");
                    tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                    continue;
                }
            };
            match crate::upload::ensure_pet_uploaded(&cfg.server_url, &device_id, &current_sprite)
                .await
            {
                Ok(_) => {
                    last_uploaded_sprite = current_sprite.clone();
                }
                Err(e) => {
                    let retryable = e.is_retryable();
                    let msg = format!("pet upload failed: {e}");
                    let status = build_status(&cfg, &current_sprite, true, retryable, &msg);
                    emit_if_changed(&status, &mut last_status_payload, &app);
                    if retryable {
                        tokio::time::sleep(Duration::from_secs_f64(reconnect_delay)).await;
                        reconnect_delay =
                            (reconnect_delay * RECONNECT_BACKOFF).min(RECONNECT_MAX_SECS);
                    } else {
                        // Permanent (e.g. pet folder missing). Drop the
                        // announce so the loop doesn't spin; peers stay on
                        // the previous sprite until the user fixes the
                        // underlying issue and switches pet again.
                        pending_sprite_announce = false;
                    }
                    continue;
                }
            }
        }

        // Detect a silently-dead WS (no PINGs, no messages from server in
        // ~45s) and force a reconnect. TCP alone may not surface a half-dead
        // socket — typical case: laptop sleeps, server times out and drops
        // the peer, but on wake the local socket still appears writable so
        // recv_json blocks forever and we never reconnect. The liveness
        // probe (set below) is checked at the bottom of the loop together
        // with disconnect_reason from send/recv.
        let mut disconnect_reason: Option<String> = None;
        if let Some(s) = session.as_ref() {
            if s.idle_for() > Duration::from_secs(LIVENESS_TIMEOUT_SECS) {
                disconnect_reason = Some(format!(
                    "no activity from server in {LIVENESS_TIMEOUT_SECS}s"
                ));
            }
        }

        // --- send pending outbound ---
        if disconnect_reason.is_none() {
            if let Some(s) = session.as_mut() {
                let outgoing_is_sprite = pending_sprite_announce;
                let outgoing_is_chat = !pending_sprite_announce
                    && !(pending_state_sync
                        || next_heartbeat.map(|t| Instant::now() >= t).unwrap_or(false))
                    && !pending_chat_outbox.is_empty();
                let outgoing = if pending_sprite_announce {
                    Some(json!({
                        "type": "sprite",
                        "sprite": last_state.pet,
                        "state": last_state.state,
                    }))
                } else if pending_state_sync
                    || next_heartbeat.map(|t| Instant::now() >= t).unwrap_or(false)
                {
                    Some(json!({
                        "type": "state",
                        "state": last_state.state,
                        "sprite": last_state.pet,
                    }))
                } else if outgoing_is_chat {
                    pending_chat_outbox.front().map(|out| {
                        json!({
                            "type": "chat",
                            "id": out.item.id,
                            "text": out.item.text,
                            "created_ms": out.item.created_ms,
                        })
                    })
                } else {
                    None
                };
                if let Some(payload) = outgoing {
                    match s.send_json(payload).await {
                        Ok(()) => {
                            pending_sprite_announce = false;
                            pending_state_sync = false;
                            next_heartbeat =
                                Some(Instant::now() + Duration::from_secs(HEARTBEAT_SECS));
                            // Re-emit status after a sprite swap so the frontend's
                            // `RemoteStatus.sprite` field reflects the new pet
                            // (without this, the in-place swap path no longer
                            // hits the signature-drift emit at the top of the
                            // loop, and the status would stay stale).
                            if outgoing_is_sprite {
                                let status =
                                    build_status(&cfg, &current_sprite, true, false, "");
                                emit_if_changed(&status, &mut last_status_payload, &app);
                            }
                            if outgoing_is_chat {
                                if let Some(sent) = pending_chat_outbox.pop_front() {
                                    let key = sent.path.to_string_lossy().to_string();
                                    pending_chat_paths.remove(&key);
                                    if let Err(e) = bubbles::acknowledge_chat_outbox(&sent.path) {
                                        eprintln!("remote chat outbox ack failed: {e}");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            // Send failure means the connection is broken — TCP
                            // gave us a hard error, so don't wait for the recv
                            // loop to time out. Surface it now so we tear down
                            // and reconnect on this iteration.
                            eprintln!("remote send failed: {e}");
                            disconnect_reason = Some(format!("send failed: {e}"));
                        }
                    }
                }
            }
        }

        // --- pump receive with a short timeout so the outer loop can keep
        //     polling config + state.json without blocking forever ---
        let mut peers_changed = false;
        if disconnect_reason.is_none() {
            if let Some(s) = session.as_mut() {
                let recv_fut = s.recv_json();
                tokio::pin!(recv_fut);
                let timeout = tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                tokio::pin!(timeout);
                tokio::select! {
                    msg = &mut recv_fut => match msg {
                        Ok(value) => {
                            handle_peer_chat(&cfg, &value);
                            peers_changed = handle_incoming(&app, &mut peers, value);
                        }
                        Err(e) => { disconnect_reason = Some(e); }
                    },
                    _ = &mut timeout => {}
                }
            }
        }
        if peers_changed {
            peer_windows.sync(&app, &peers).await;
            let active_device_ids: HashSet<String> = peers.keys().cloned().collect();
            bubble_windows.sync_peer_owners(&app, &active_device_ids).await;
        }

        if let Some(reason) = disconnect_reason {
            if let Some(s) = session.take() {
                s.close().await;
            }
            peers.clear();
            emit_peers(&app, &peers);
            peer_windows.sync(&app, &peers).await;
            bubble_windows
                .sync_peer_owners(&app, &HashSet::<String>::new())
                .await;
            let permanent = is_permanent_error(&reason);
            let status = build_status(&cfg, &current_sprite, false, !permanent, &reason);
            emit_if_changed(&status, &mut last_status_payload, &app);
            if permanent {
                tokio::time::sleep(Duration::from_secs(60)).await;
            } else {
                tokio::time::sleep(Duration::from_secs_f64(reconnect_delay)).await;
                reconnect_delay = (reconnect_delay * RECONNECT_BACKOFF).min(RECONNECT_MAX_SECS);
            }
        }
    }
}
