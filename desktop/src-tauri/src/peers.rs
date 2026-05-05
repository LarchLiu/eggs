// Peer window manager.
//
// Mirrors egg_desktop.py's per-peer Tk Toplevel: each remote peer gets its own
// transparent, click-through, always-on-top WebviewWindow that animates the
// peer's atlas. The window URL is the same `peer.html`; the page identifies
// itself by its window label (`peer-<peer_id>`) and asks Rust for its initial
// state via `get_peer_init`. Subsequent state updates flow over the existing
// `remote-peers` event the actor already emits.
//
// Lifecycle:
//   * remote.rs::handle_incoming mutates the peer HashMap.
//   * After a change, run_actor calls `manager.sync(app, &peers).await`.
//   * sync() diffs against currently-open windows, downloads any new sprite
//     assets, opens new windows, closes stale ones, and emits `peer-state` for
//     state-only changes.
//
// Window labels are sanitized to alnum + `-_`, which is no-op for the UUIDs
// the Go server hands out today.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::Mutex;

use crate::remote::PeerSnapshot;
use crate::remote_assets::{self, CachedAssets};

const PEER_WIDTH: f64 = 192.0;
const PEER_HEIGHT: f64 = 208.0;
const LABEL_PREFIX: &str = "peer-";

#[derive(Debug, Clone)]
struct OpenPeer {
    asset_id: String,
    sprite_name: String,
    sprite_path: PathBuf,
    json_path: PathBuf,
    config_path: Option<PathBuf>,
    state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerInit {
    pub peer_id: String,
    pub sprite: String,
    pub state: String,
    /// Absolute filesystem paths; the webview turns these into asset:// URLs
    /// via convertFileSrc.
    pub sprite_path_abs: String,
    pub json_path_abs: String,
    pub config_path_abs: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct PeerStateEvent {
    pub peer_id: String,
    pub state: String,
}

pub struct PeerWindowManager {
    open: Mutex<HashMap<String, OpenPeer>>,
}

impl PeerWindowManager {
    pub fn new() -> Self {
        Self {
            open: Mutex::new(HashMap::new()),
        }
    }

    pub async fn get_init(&self, peer_id: &str) -> Option<PeerInit> {
        let map = self.open.lock().await;
        let entry = map.get(peer_id)?;
        Some(PeerInit {
            peer_id: peer_id.to_string(),
            sprite: entry.sprite_name.clone(),
            state: entry.state.clone(),
            sprite_path_abs: entry.sprite_path.to_string_lossy().to_string(),
            json_path_abs: entry.json_path.to_string_lossy().to_string(),
            config_path_abs: entry.config_path.as_ref().map(|p| p.to_string_lossy().to_string()),
        })
    }

    /// Reconcile open peer windows with the latest snapshot from the server.
    /// Cheap on the steady-state path (no diff -> no work).
    pub async fn sync(&self, app: &AppHandle, peers: &HashMap<String, PeerSnapshot>) {
        // 1. Collect plan under a brief lock.
        let mut to_open: Vec<PeerSnapshot> = Vec::new();
        let mut to_replace: Vec<PeerSnapshot> = Vec::new();
        let mut to_close: Vec<String> = Vec::new();
        let mut state_changes: Vec<(String, String)> = Vec::new();
        {
            let map = self.open.lock().await;
            let live: HashSet<&String> = peers.keys().collect();
            let open: HashSet<&String> = map.keys().collect();
            for peer_id in open.difference(&live) {
                to_close.push((*peer_id).clone());
            }
            for (peer_id, snap) in peers {
                let Some(entry) = map.get(peer_id) else {
                    if !snap.sprite_url.is_empty() && !snap.json_url.is_empty() {
                        to_open.push(snap.clone());
                    }
                    continue;
                };
                let new_asset = remote_assets::asset_id_hint(&snap.sprite_url);
                let asset_changed = new_asset
                    .as_deref()
                    .map(|id| id != entry.asset_id)
                    .unwrap_or(false);
                if asset_changed && !snap.sprite_url.is_empty() && !snap.json_url.is_empty() {
                    to_replace.push(snap.clone());
                } else if snap.state != entry.state {
                    state_changes.push((peer_id.clone(), snap.state.clone()));
                }
            }
        }

        for peer_id in to_close {
            self.close_window(app, &peer_id).await;
        }
        for snap in to_replace {
            // Replace = close then open. Atlas swap mid-animation is rare
            // enough (peer changes pet) that a flash is acceptable.
            self.close_window(app, &snap.peer_id).await;
            self.open_window(app, snap).await;
        }
        for snap in to_open {
            self.open_window(app, snap).await;
        }
        for (peer_id, state) in state_changes {
            {
                let mut map = self.open.lock().await;
                if let Some(entry) = map.get_mut(&peer_id) {
                    entry.state = state.clone();
                }
            }
            let label = window_label(&peer_id);
            let _ = app.emit_to(
                tauri::EventTarget::webview_window(label),
                "peer-state",
                PeerStateEvent { peer_id, state },
            );
        }
    }

    async fn open_window(&self, app: &AppHandle, snap: PeerSnapshot) {
        let cached = match remote_assets::ensure_remote_assets(
            &snap.sprite_url,
            &snap.json_url,
            snap.config_url.as_deref(),
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("peer asset download failed for {}: {}", snap.peer_id, e);
                return;
            }
        };

        // Insert into map BEFORE opening the window so get_peer_init() sees
        // it the moment the JS calls in.
        {
            let mut map = self.open.lock().await;
            map.insert(
                snap.peer_id.clone(),
                OpenPeer {
                    asset_id: cached.asset_id.clone(),
                    sprite_name: snap.sprite.clone(),
                    sprite_path: cached.sprite_path.clone(),
                    json_path: cached.json_path.clone(),
                    config_path: cached.config_path.clone(),
                    state: if snap.state.is_empty() {
                        "idle".to_string()
                    } else {
                        snap.state.clone()
                    },
                },
            );
        }

        if let Err(e) = build_peer_window(app, &snap.peer_id, &cached).await {
            eprintln!("could not open peer window for {}: {}", snap.peer_id, e);
            // Roll back the map entry on failure.
            let mut map = self.open.lock().await;
            map.remove(&snap.peer_id);
        }
    }

    async fn close_window(&self, app: &AppHandle, peer_id: &str) {
        {
            let mut map = self.open.lock().await;
            map.remove(peer_id);
        }
        let label = window_label(peer_id);
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.close();
        }
    }

    pub async fn close_all(&self, app: &AppHandle) {
        let labels: Vec<String> = {
            let mut map = self.open.lock().await;
            let labels: Vec<String> = map.keys().map(|id| window_label(id)).collect();
            map.clear();
            labels
        };
        for label in labels {
            if let Some(win) = app.get_webview_window(&label) {
                let _ = win.close();
            }
        }
    }
}

fn window_label(peer_id: &str) -> String {
    let safe: String = peer_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '-' | '_'))
        .collect();
    format!("{LABEL_PREFIX}{safe}")
}

async fn build_peer_window(
    app: &AppHandle,
    peer_id: &str,
    _cached: &CachedAssets,
) -> tauri::Result<()> {
    let label = window_label(peer_id);
    if app.get_webview_window(&label).is_some() {
        return Ok(());
    }
    let (x, y) = position_for_peer(app, peer_id);
    let url = format!("peer.html#{}", urlencoding(peer_id));
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title("Eggs Peer")
        .inner_size(PEER_WIDTH, PEER_HEIGHT)
        .position(x, y)
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
    let _ = window.set_ignore_cursor_events(true);
    Ok(())
}

/// Place each peer at a deterministic offset from the local pet window so
/// reconnects don't shuffle peers around. Falls back to current monitor's
/// origin when the local pet hasn't been measured yet.
fn position_for_peer(app: &AppHandle, peer_id: &str) -> (f64, f64) {
    let pet_pos = app
        .get_webview_window("pet")
        .and_then(|w| w.outer_position().ok());
    let (anchor_x, anchor_y) = match pet_pos {
        Some(p) => (p.x as f64, p.y as f64),
        None => (200.0, 200.0),
    };
    let (screen_w, screen_h) = primary_monitor_size(app).unwrap_or((1440.0, 900.0));
    let h = stable_hash(peer_id);
    let side = if h & 1 == 0 { PEER_WIDTH + 40.0 } else { -(PEER_WIDTH + 40.0) };
    let dx = ((h >> 1) as i64 % 121 - 60) as f64; // -60..60
    let dy = ((h >> 9) as i64 % 121 - 60) as f64;
    let x = (anchor_x + side + dx).clamp(0.0, (screen_w - PEER_WIDTH).max(0.0));
    let y = (anchor_y + dy).clamp(40.0, (screen_h - PEER_HEIGHT - 24.0).max(40.0));
    (x, y)
}

fn primary_monitor_size(app: &AppHandle) -> Option<(f64, f64)> {
    let mon = app.primary_monitor().ok().flatten()?;
    let size = mon.size();
    Some((size.width as f64, size.height as f64))
}

fn stable_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}

pub type SharedPeerWindowManager = Arc<PeerWindowManager>;
