use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{
    AppHandle, Emitter, EventTarget, LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder,
};
use tokio::sync::Mutex;

use crate::state;

pub const BUBBLE_WIDTH: f64 = 224.0;
pub const BUBBLE_MIN_HEIGHT: f64 = 20.0;
pub const BUBBLE_MAX_HEIGHT: f64 = 60.0;
const DEFAULT_HOOK_TTL_MS: u64 = 8_000;
const DEFAULT_USER_TTL_MS: u64 = 12_000;
const MAX_BUBBLE_TEXT_CHARS: usize = 2_000;
const CHAT_HISTORY_LIMIT: usize = 5;
const LABEL_PREFIX: &str = "bubble-";
const CHAT_WINDOW_PREFIX: &str = "chat-";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BubbleOwner {
    Local,
    Peer {
        peer_id: String,
        #[serde(default)]
        device_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BubbleSource {
    Hook,
    UserInput,
    PeerUserInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BubbleEvent {
    pub id: String,
    pub owner: BubbleOwner,
    pub source: BubbleSource,
    pub text: String,
    pub ttl_ms: u64,
    pub created_ms: u64,
    #[serde(default)]
    pub room_code: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BubbleInit {
    pub id: String,
    pub owner: BubbleOwner,
    pub source: BubbleSource,
    /// "hook" or "chat"
    pub mode: String,
    pub text: String,
    pub messages: Vec<String>,
    pub message_times: Vec<u64>,
    pub ttl_ms: u64,
    pub created_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BubbleConstraints {
    pub width: f64,
    pub min_height: f64,
    pub max_height: f64,
}

pub fn constraints() -> BubbleConstraints {
    BubbleConstraints {
        width: BUBBLE_WIDTH,
        min_height: BUBBLE_MIN_HEIGHT,
        max_height: BUBBLE_MAX_HEIGHT,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatOutboxItem {
    pub id: String,
    pub text: String,
    pub created_ms: u64,
    #[serde(default)]
    pub room_code: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChatOutboxFile {
    pub path: PathBuf,
    pub item: ChatOutboxItem,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatHistoryEntry {
    pub id: String,
    pub source: BubbleSource,
    pub owner: BubbleOwner,
    pub text: String,
    pub created_ms: u64,
    #[serde(default)]
    pub device_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ChatHistoryFile {
    room_code: String,
    updated_ms: u64,
    messages: Vec<ChatHistoryEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BubbleMode {
    Hook,
    Chat,
}

#[derive(Debug, Clone, Default)]
struct ChatBucket {
    recent: VecDeque<ChatLine>,
    dismissed: bool,
}

#[derive(Debug, Clone)]
struct ChatLine {
    text: String,
    created_ms: u64,
}

#[derive(Debug, Clone)]
struct OpenBubble {
    id: String,
    owner: BubbleOwner,
    source: BubbleSource,
    mode: BubbleMode,
    text: String,
    messages: Vec<ChatLine>,
    ttl_ms: u64,
    created_ms: u64,
    hovered: bool,
    height: f64,
}

impl OpenBubble {
    fn to_init(&self) -> BubbleInit {
        let messages: Vec<String> = self.messages.iter().map(|m| m.text.clone()).collect();
        let message_times: Vec<u64> = self.messages.iter().map(|m| m.created_ms).collect();
        BubbleInit {
            id: self.id.clone(),
            owner: self.owner.clone(),
            source: self.source.clone(),
            mode: match self.mode {
                BubbleMode::Hook => "hook".to_string(),
                BubbleMode::Chat => "chat".to_string(),
            },
            text: self.text.clone(),
            messages,
            message_times,
            ttl_ms: self.ttl_ms,
            created_ms: self.created_ms,
        }
    }
}

#[derive(Default)]
struct BubbleStore {
    open: HashMap<String, OpenBubble>,
    chat: HashMap<String, ChatBucket>,
}

pub struct BubbleWindowManager {
    store: Mutex<BubbleStore>,
}

impl BubbleWindowManager {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(BubbleStore::default()),
        }
    }

    pub async fn get_init(&self, bubble_id: &str) -> Option<BubbleInit> {
        let store = self.store.lock().await;
        store.open.get(bubble_id).map(OpenBubble::to_init)
    }

    pub async fn show(&self, app: &AppHandle, event: BubbleEvent) {
        if matches!(event.source, BubbleSource::Hook) {
            self.show_hook(app, event).await;
        } else {
            self.show_chat(app, event).await;
        }
    }

    pub async fn close(&self, app: &AppHandle, bubble_id: &str) {
        let removed = {
            let mut store = self.store.lock().await;
            store.open.remove(bubble_id)
        };
        if removed.is_none() {
            return;
        }
        if let Some(win) = app.get_webview_window(&bubble_window_label(bubble_id)) {
            let _ = win.close();
        }
    }

    pub async fn dismiss_chat(&self, app: &AppHandle, bubble_id: &str) {
        let (should_close, owner_key) = {
            let mut store = self.store.lock().await;
            let Some(open) = store.open.get(bubble_id) else {
                return;
            };
            if open.mode != BubbleMode::Chat {
                return;
            }
            let owner_key = open.owner.to_key();
            store.open.remove(bubble_id);
            if let Some(bucket) = store.chat.get_mut(&owner_key) {
                bucket.dismissed = true;
            }
            (true, owner_key)
        };
        if should_close {
            if let Some(win) = app.get_webview_window(&bubble_window_label(bubble_id)) {
                let _ = win.close();
            }
            let _ = owner_key;
            self.reposition_all(app).await;
        }
    }

    pub async fn set_hover(&self, bubble_id: &str, hovering: bool) {
        let mut store = self.store.lock().await;
        if let Some(open) = store.open.get_mut(bubble_id) {
            open.hovered = hovering;
        }
    }

    pub async fn expire_due(&self, app: &AppHandle) {
        let now = now_ms();
        let expired: Vec<String> = {
            let store = self.store.lock().await;
            store
                .open
                .values()
                .filter(|open| {
                    open.mode == BubbleMode::Hook
                        && !open.hovered
                        && open.created_ms.saturating_add(open.ttl_ms) <= now
                })
                .map(|open| open.id.clone())
                .collect()
        };
        for id in expired {
            self.close(app, &id).await;
        }
        self.reposition_all(app).await;
    }

    pub async fn reposition_all(&self, app: &AppHandle) {
        let mut entries: Vec<OpenBubble> = {
            let store = self.store.lock().await;
            store.open.values().cloned().collect()
        };
        if entries.is_empty() {
            return;
        }

        entries.sort_by(|a, b| {
            a.owner
                .to_key()
                .cmp(&b.owner.to_key())
                .then(mode_rank(a.mode).cmp(&mode_rank(b.mode)))
                .then(a.created_ms.cmp(&b.created_ms))
                .then(a.id.cmp(&b.id))
        });

        let mut anchor_by_owner: HashMap<String, AnchorRect> = HashMap::new();
        for entry in &entries {
            let owner_key = entry.owner.to_key();
            if anchor_by_owner.contains_key(&owner_key) {
                continue;
            }
            if let Some(anchor) = anchor_rect(app, &entry.owner) {
                anchor_by_owner.insert(owner_key, anchor);
            }
        }

        // Chat cross layout:
        // 0,2,4... stick close to the sprite; 1,3,5... one tier higher.
        let mut chat_tier_by_owner: HashMap<String, u8> = HashMap::new();
        let mut chat_owners: Vec<(String, f64)> = entries
            .iter()
            .filter(|e| e.mode == BubbleMode::Chat)
            .filter_map(|e| {
                let k = e.owner.to_key();
                anchor_by_owner.get(&k).map(|a| (k, a.x))
            })
            .collect();
        chat_owners.sort_by(|a, b| {
            let a_local = a.0 == "local";
            let b_local = b.0 == "local";
            b_local
                .cmp(&a_local)
                .then(a.1.total_cmp(&b.1))
                .then(a.0.cmp(&b.0))
        });
        chat_owners.dedup_by(|a, b| a.0 == b.0);
        for (idx, (owner_key, _)) in chat_owners.iter().enumerate() {
            let tier = if idx % 2 == 0 { 0 } else { 1 };
            chat_tier_by_owner.insert(owner_key.clone(), tier);
        }

        let mut per_owner_hook_offset: HashMap<String, f64> = HashMap::new();
        let mut placed: Vec<Rect> = Vec::new();

        for entry in entries {
            let owner_key = entry.owner.to_key();
            let Some(anchor) = anchor_by_owner.get(&owner_key).copied() else {
                continue;
            };
            let hook_offset = if entry.mode == BubbleMode::Hook {
                *per_owner_hook_offset.get(&owner_key).unwrap_or(&0.0)
            } else {
                0.0
            };
            let chat_tier = *chat_tier_by_owner.get(&owner_key).unwrap_or(&0);

            let preferred = preferred_position(&entry, &anchor, hook_offset, chat_tier);
            let resolved = if entry.mode == BubbleMode::Chat {
                preferred
            } else {
                resolve_collision(preferred, entry.mode, &anchor, &placed, entry.height)
            };
            if entry.mode == BubbleMode::Hook {
                per_owner_hook_offset.insert(owner_key, hook_offset + entry.height + 6.0);
            }
            placed.push(Rect {
                x: resolved.0,
                y: resolved.1,
                w: BUBBLE_WIDTH,
                h: entry.height,
            });

            let label = bubble_window_label(&entry.id);
            if let Some(win) = app.get_webview_window(&label) {
                let _ = win.set_position(LogicalPosition::new(resolved.0, resolved.1));
            }
        }
    }

    pub async fn close_all(&self, app: &AppHandle) {
        let labels: Vec<String> = {
            let mut store = self.store.lock().await;
            let labels: Vec<String> = store.open.keys().map(|id| bubble_window_label(id)).collect();
            store.open.clear();
            store.chat.clear();
            labels
        };
        for label in labels {
            if let Some(win) = app.get_webview_window(&label) {
                let _ = win.close();
            }
        }
    }

    pub async fn sync_peer_owners(
        &self,
        app: &AppHandle,
        active_peer_ids: &std::collections::HashSet<String>,
    ) {
        let stale_ids: Vec<String> = {
            let mut store = self.store.lock().await;
            let mut removed: Vec<String> = Vec::new();
            store.open.retain(|id, open| match &open.owner {
                BubbleOwner::Local => true,
                BubbleOwner::Peer { peer_id, .. } => {
                    let keep = active_peer_ids.contains(peer_id);
                    if !keep {
                        removed.push(id.clone());
                    }
                    keep
                }
            });
            // Chat buckets are keyed by device_id (the stable partner identity),
            // so we deliberately keep them across peer disconnect/reconnect:
            // history must persist when the same partner rejoins.
            removed
        };

        for id in stale_ids {
            if let Some(win) = app.get_webview_window(&bubble_window_label(&id)) {
                let _ = win.close();
            }
        }
        self.reposition_all(app).await;
    }

    async fn show_hook(&self, app: &AppHandle, event: BubbleEvent) {
        let id = event.id.clone();
        let should_open = {
            let mut store = self.store.lock().await;
            let open = OpenBubble {
                id: id.clone(),
                owner: event.owner,
                source: event.source,
                mode: BubbleMode::Hook,
                text: event.text.clone(),
                messages: vec![ChatLine {
                    text: event.text,
                    created_ms: event.created_ms,
                }],
                ttl_ms: event.ttl_ms,
                created_ms: event.created_ms,
                hovered: false,
                height: BUBBLE_MAX_HEIGHT,
            };
            store.open.insert(id.clone(), open).is_none()
        };

        if should_open {
            if let Err(e) = build_bubble_window(app, &id) {
                eprintln!("could not open hook bubble window for {id}: {e}");
                let mut store = self.store.lock().await;
                store.open.remove(&id);
                return;
            }
        } else {
            self.emit_update(app, &id).await;
        }
        self.reposition_all(app).await;
    }

    async fn show_chat(&self, app: &AppHandle, event: BubbleEvent) {
        let owner = event.owner.clone();
        let owner_key = owner.to_key();
        let id = chat_window_id(&owner);

        let (need_open, need_emit) = {
            let mut store = self.store.lock().await;
            let bucket = store.chat.entry(owner_key).or_default();
            bucket.recent.push_back(ChatLine {
                text: event.text.clone(),
                created_ms: event.created_ms,
            });
            while bucket.recent.len() > CHAT_HISTORY_LIMIT {
                bucket.recent.pop_front();
            }
            bucket.dismissed = false;

            let messages: Vec<ChatLine> = bucket.recent.iter().cloned().collect();
            let latest = messages
                .last()
                .map(|m| m.text.clone())
                .unwrap_or_default();

            let mut need_open = false;
            let mut need_emit = false;
            if let Some(open) = store.open.get_mut(&id) {
                open.owner = event.owner.clone();
                open.source = event.source;
                open.text = latest;
                open.messages = messages;
                open.created_ms = event.created_ms;
                open.hovered = false;
                need_emit = true;
            } else {
                need_open = true;
                store.open.insert(
                    id.clone(),
                    OpenBubble {
                        id: id.clone(),
                        owner,
                        source: event.source,
                        mode: BubbleMode::Chat,
                        text: latest,
                        messages,
                        ttl_ms: 0,
                        created_ms: event.created_ms,
                        hovered: false,
                        height: BUBBLE_MAX_HEIGHT,
                    },
                );
            }
            (need_open, need_emit)
        };

        if need_open {
            if let Err(e) = build_bubble_window(app, &id) {
                eprintln!("could not open chat bubble window for {id}: {e}");
                let mut store = self.store.lock().await;
                store.open.remove(&id);
                return;
            }
        } else if need_emit {
            self.emit_update(app, &id).await;
        }
        self.reposition_all(app).await;
    }

    async fn emit_update(&self, app: &AppHandle, id: &str) {
        let payload = {
            let store = self.store.lock().await;
            store.open.get(id).map(OpenBubble::to_init)
        };
        if let Some(next) = payload {
            let _ = app.emit_to(
                EventTarget::webview_window(bubble_window_label(id)),
                "bubble-update",
                next,
            );
        }
    }

    pub async fn set_height(&self, bubble_id: &str, height: f64) {
        let mut store = self.store.lock().await;
        if let Some(open) = store.open.get_mut(bubble_id) {
            open.height = height.clamp(BUBBLE_MIN_HEIGHT, BUBBLE_MAX_HEIGHT);
        }
    }
}

impl BubbleOwner {
    fn to_key(&self) -> String {
        match self {
            BubbleOwner::Local => "local".to_string(),
            BubbleOwner::Peer { peer_id, device_id } => {
                format!("peer:{}", stable_peer_key(peer_id, device_id.as_deref()))
            }
        }
    }
}

/// Pick the most stable identifier we have for this partner. `device_id` is
/// per-device and persists across reconnects; `peer_id` is per WebSocket
/// connection and rotates each time. We prefer device_id so the chat bucket
/// and bubble window survive disconnect/reconnect cycles.
fn stable_peer_key(peer_id: &str, device_id: Option<&str>) -> String {
    let dev = device_id.map(str::trim).filter(|s| !s.is_empty());
    match dev {
        Some(d) => sanitize_filename(d),
        None => sanitize_filename(peer_id),
    }
}

pub fn queue_hook_message(text: &str) -> io::Result<Option<BubbleEvent>> {
    let Some(clean) = normalize_text(text) else {
        return Ok(None);
    };
    let event = BubbleEvent {
        id: make_id("hook"),
        owner: BubbleOwner::Local,
        source: BubbleSource::Hook,
        text: clean,
        ttl_ms: DEFAULT_HOOK_TTL_MS,
        created_ms: now_ms(),
        room_code: None,
        device_id: None,
    };
    enqueue_bubble(event.clone())?;
    Ok(Some(event))
}

pub fn queue_local_user_message(text: &str, room_code: Option<String>) -> io::Result<Option<BubbleEvent>> {
    let Some(clean) = normalize_text(text) else {
        return Ok(None);
    };
    let event = BubbleEvent {
        id: make_id("user"),
        owner: BubbleOwner::Local,
        source: BubbleSource::UserInput,
        text: clean,
        ttl_ms: DEFAULT_USER_TTL_MS,
        created_ms: now_ms(),
        room_code,
        device_id: None,
    };
    enqueue_bubble(event.clone())?;
    Ok(Some(event))
}

pub fn queue_peer_user_message(
    peer_id: &str,
    text: &str,
    room_code: Option<String>,
    device_id: Option<String>,
) -> io::Result<Option<BubbleEvent>> {
    let peer = sanitize_peer_id(peer_id);
    if peer.is_empty() {
        return Ok(None);
    }
    let Some(clean) = normalize_text(text) else {
        return Ok(None);
    };
    let event = BubbleEvent {
        id: make_id("peer"),
        owner: BubbleOwner::Peer {
            peer_id: peer,
            device_id: device_id.clone(),
        },
        source: BubbleSource::PeerUserInput,
        text: clean,
        ttl_ms: DEFAULT_USER_TTL_MS,
        created_ms: now_ms(),
        room_code,
        device_id,
    };
    enqueue_bubble(event.clone())?;
    Ok(Some(event))
}

pub fn enqueue_chat_outbox(text: &str, room_code: Option<String>) -> io::Result<Option<ChatOutboxItem>> {
    let Some(clean) = normalize_text(text) else {
        return Ok(None);
    };
    let item = ChatOutboxItem {
        id: make_id("chat"),
        text: clean,
        created_ms: now_ms(),
        room_code,
    };
    write_spool_json(chat_outbox_dir(), &item.id, &item)?;
    Ok(Some(item))
}

pub fn drain_chat_outbox() -> io::Result<Vec<ChatOutboxFile>> {
    let mut out = Vec::new();
    for path in list_spool_files(chat_outbox_dir())? {
        let raw = match fs::read(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match serde_json::from_slice::<ChatOutboxItem>(&raw) {
            Ok(item) => out.push(ChatOutboxFile { path, item }),
            Err(_) => {
                let _ = fs::remove_file(&path);
            }
        }
    }
    Ok(out)
}

pub fn acknowledge_chat_outbox(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}

pub fn drain_bubble_spool() -> io::Result<Vec<BubbleEvent>> {
    let mut out = Vec::new();
    for path in list_spool_files(bubble_spool_dir())? {
        let raw = match fs::read(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match serde_json::from_slice::<BubbleEvent>(&raw) {
            Ok(event) => out.push(event),
            Err(_) => {
                eprintln!("warning: dropping invalid bubble spool file: {}", path.display());
            }
        }
        let _ = fs::remove_file(path);
    }
    Ok(out)
}

pub fn append_room_history(room_code: &str, entry: ChatHistoryEntry) -> io::Result<()> {
    let room = sanitize_room_code(room_code);
    if room.is_empty() {
        return Ok(());
    }
    let history_dir = chat_history_dir();
    let path = history_dir.join(format!("{room}.json"));
    let mut history = if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<ChatHistoryFile>(&t).ok())
            .unwrap_or(ChatHistoryFile {
                room_code: room.clone(),
                updated_ms: now_ms(),
                messages: Vec::new(),
            })
    } else {
        ChatHistoryFile {
            room_code: room.clone(),
            updated_ms: now_ms(),
            messages: Vec::new(),
        }
    };
    history.updated_ms = now_ms();
    history.messages.push(entry);
    fs::create_dir_all(history_dir)?;
    let json = serde_json::to_string_pretty(&history)?;
    fs::write(path, format!("{json}\n"))
}

fn enqueue_bubble(event: BubbleEvent) -> io::Result<()> {
    write_spool_json(bubble_spool_dir(), &event.id, &event)
}

fn bubble_spool_dir() -> PathBuf {
    state::app_dir().join("bubble-spool")
}

fn chat_outbox_dir() -> PathBuf {
    state::app_dir().join("chat-outbox")
}

fn chat_history_dir() -> PathBuf {
    state::app_dir().join("chat-history")
}

fn write_spool_json<T: Serialize>(dir: PathBuf, id: &str, value: &T) -> io::Result<()> {
    fs::create_dir_all(&dir)?;
    let safe = sanitize_filename(id);
    let final_path = dir.join(format!("{safe}.json"));
    let tmp_path = dir.join(format!("{safe}.tmp"));
    let raw = serde_json::to_vec(value)?;
    fs::write(&tmp_path, raw)?;
    fs::rename(tmp_path, final_path)
}

fn list_spool_files(dir: PathBuf) -> io::Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort();
    Ok(files)
}

fn build_bubble_window(app: &AppHandle, bubble_id: &str) -> tauri::Result<()> {
    let label = bubble_window_label(bubble_id);
    if app.get_webview_window(&label).is_some() {
        return Ok(());
    }
    let url = format!("bubble.html#{}", urlencoding(bubble_id));
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title("Eggs Bubble")
        .inner_size(BUBBLE_WIDTH, BUBBLE_MAX_HEIGHT)
        .position(20.0, 20.0)
        .transparent(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        .accept_first_mouse(true)
        .visible_on_all_workspaces(true);
    let window = builder.build()?;
    let _ = window.set_ignore_cursor_events(false);
    Ok(())
}

fn bubble_window_label(bubble_id: &str) -> String {
    format!("{LABEL_PREFIX}{}", sanitize_filename(bubble_id))
}

fn chat_window_id(owner: &BubbleOwner) -> String {
    match owner {
        BubbleOwner::Local => format!("{CHAT_WINDOW_PREFIX}local"),
        BubbleOwner::Peer { peer_id, device_id } => format!(
            "{CHAT_WINDOW_PREFIX}peer-{}",
            stable_peer_key(peer_id, device_id.as_deref())
        ),
    }
}

#[derive(Clone, Copy)]
struct AnchorRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    screen_w: f64,
    screen_h: f64,
}

#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    fn intersects(self, other: Rect) -> bool {
        self.x < other.x + other.w
            && self.x + self.w > other.x
            && self.y < other.y + other.h
            && self.y + self.h > other.y
    }
}

fn anchor_rect(app: &AppHandle, owner: &BubbleOwner) -> Option<AnchorRect> {
    let anchor_label = match owner {
        BubbleOwner::Local => "pet".to_string(),
        BubbleOwner::Peer { peer_id, .. } => crate::peers::peer_window_label(peer_id),
    };
    let anchor = app.get_webview_window(&anchor_label)?;
    let scale_factor = anchor.scale_factor().ok().unwrap_or(1.0);
    let pos = anchor.outer_position().ok()?;
    let size = anchor.outer_size().ok()?;
    let (screen_w, screen_h) = primary_monitor_size(app, scale_factor).unwrap_or((1440.0, 900.0));
    Some(AnchorRect {
        x: pos.x as f64 / scale_factor,
        y: pos.y as f64 / scale_factor,
        w: size.width as f64 / scale_factor,
        h: size.height as f64 / scale_factor,
        screen_w,
        screen_h,
    })
}

fn preferred_position(
    open: &OpenBubble,
    anchor: &AnchorRect,
    hook_offset: f64,
    chat_tier: u8,
) -> (f64, f64) {
    let x = (anchor.x + (anchor.w - BUBBLE_WIDTH) * 0.5)
        .clamp(8.0, (anchor.screen_w - BUBBLE_WIDTH - 8.0).max(8.0));
    let y = match open.mode {
        BubbleMode::Chat => {
            let lift = chat_tier as f64 * (open.height + 6.0);
            anchor.y - 8.0 - open.height - lift
        }
        BubbleMode::Hook => anchor.y + anchor.h + 8.0 + hook_offset,
    };
    (
        x,
        y.clamp(8.0, (anchor.screen_h - open.height - 8.0).max(8.0)),
    )
}

fn resolve_collision(
    preferred: (f64, f64),
    mode: BubbleMode,
    anchor: &AnchorRect,
    placed: &[Rect],
    height: f64,
) -> (f64, f64) {
    let step = height + 8.0;
    let mut candidate = Rect {
        x: preferred.0,
        y: preferred.1,
        w: BUBBLE_WIDTH,
        h: height,
    };

    if !placed.iter().any(|r| candidate.intersects(*r)) {
        return (candidate.x, candidate.y);
    }

    match mode {
        BubbleMode::Chat => {
            let mut y_up = preferred.1;
            while y_up > 8.0 {
                y_up = (y_up - step).max(8.0);
                candidate.y = y_up;
                if !placed.iter().any(|r| candidate.intersects(*r)) {
                    return (candidate.x, candidate.y);
                }
                if y_up <= 8.0 {
                    break;
                }
            }
            let mut y_down = preferred.1;
            let max_y = (anchor.screen_h - height - 8.0).max(8.0);
            while y_down < max_y {
                y_down = (y_down + step).min(max_y);
                candidate.y = y_down;
                if !placed.iter().any(|r| candidate.intersects(*r)) {
                    return (candidate.x, candidate.y);
                }
                if y_down >= max_y {
                    break;
                }
            }
        }
        BubbleMode::Hook => {
            let mut y_down = preferred.1;
            let max_y = (anchor.screen_h - height - 8.0).max(8.0);
            while y_down < max_y {
                y_down = (y_down + step).min(max_y);
                candidate.y = y_down;
                if !placed.iter().any(|r| candidate.intersects(*r)) {
                    return (candidate.x, candidate.y);
                }
                if y_down >= max_y {
                    break;
                }
            }
            let mut y_up = preferred.1;
            while y_up > 8.0 {
                y_up = (y_up - step).max(8.0);
                candidate.y = y_up;
                if !placed.iter().any(|r| candidate.intersects(*r)) {
                    return (candidate.x, candidate.y);
                }
                if y_up <= 8.0 {
                    break;
                }
            }
        }
    }

    (candidate.x, candidate.y)
}

fn mode_rank(mode: BubbleMode) -> u8 {
    match mode {
        BubbleMode::Chat => 0,
        BubbleMode::Hook => 1,
    }
}

fn primary_monitor_size(app: &AppHandle, scale_factor: f64) -> Option<(f64, f64)> {
    let mon = app.primary_monitor().ok().flatten()?;
    let size = mon.size();
    Some((
        size.width as f64 / scale_factor,
        size.height as f64 / scale_factor,
    ))
}

fn normalize_text(text: &str) -> Option<String> {
    let compact = text.trim();
    if compact.is_empty() {
        return None;
    }
    let mut out = String::new();
    for ch in compact.chars().take(MAX_BUBBLE_TEXT_CHARS) {
        out.push(ch);
    }
    Some(out)
}

fn sanitize_filename(input: &str) -> String {
    let safe: String = input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '-' | '_'))
        .collect();
    if safe.is_empty() {
        "bubble".to_string()
    } else {
        safe
    }
}

fn sanitize_peer_id(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '-' | '_'))
        .collect()
}

fn sanitize_room_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '-' | '_'))
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn make_id(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let seq = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{:x}-{}-{:x}", now_ms(), std::process::id(), seq)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_millis(0))
        .as_millis() as u64
}

fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}

pub type SharedBubbleWindowManager = Arc<BubbleWindowManager>;
