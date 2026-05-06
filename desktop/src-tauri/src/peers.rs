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
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::Mutex;

use crate::remote::PeerSnapshot;
use crate::remote_assets::{self, CachedAssets};

const PEER_WIDTH: f64 = 192.0;
const PEER_HEIGHT: f64 = 208.0;
const LABEL_PREFIX: &str = "peer-";
const DEFAULT_SCALE_MILLIS: u16 = 1000;

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
    /// Snapshot of the local pet's scale at window-open time. peer.js sizes
    /// its sprite cell to match; subsequent changes arrive via `peer-scale`.
    pub scale_millis: u16,
}

#[derive(Debug, Clone, Serialize)]
struct PeerStateEvent {
    pub peer_id: String,
    pub state: String,
}

pub struct PeerWindowManager {
    open: Mutex<HashMap<String, OpenPeer>>,
    /// Last-known local pet scale in millis (400/500/600/800/1000). New peer
    /// windows open at this size; `apply_scale` updates it and resyncs.
    scale_millis: AtomicU16,
}

impl PeerWindowManager {
    pub fn new() -> Self {
        Self {
            open: Mutex::new(HashMap::new()),
            scale_millis: AtomicU16::new(DEFAULT_SCALE_MILLIS),
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
            scale_millis: self.scale_millis.load(Ordering::Relaxed),
        })
    }

    /// Update the cached scale and push it to every open peer window:
    /// resize the window frame to match, reposition (the new size shifts the
    /// per-peer offset), and emit `peer-scale` so peer.js can rescale its
    /// sprite cell. Called from main.rs's state poller when state.json's
    /// scale_millis changes.
    pub async fn apply_scale(&self, app: &AppHandle, scale_millis: u16) {
        self.scale_millis.store(scale_millis, Ordering::Relaxed);
        let scale = scale_millis as f64 / 1000.0;
        let width = PEER_WIDTH * scale;
        let height = PEER_HEIGHT * scale;
        let entries: Vec<(String, usize)> = {
            let map = self.open.lock().await;
            let mut ids: Vec<&String> = map.keys().collect();
            ids.sort();
            ids.iter()
                .enumerate()
                .map(|(slot, peer_id)| (window_label(peer_id), slot))
                .collect()
        };
        for (label, slot) in entries {
            if let Some(win) = app.get_webview_window(&label) {
                let _ = win.set_size(LogicalSize::new(width, height));
                let (x, y) = position_for_peer(app, scale_millis, slot);
                let _ = win.set_position(LogicalPosition::new(x, y));
            }
        }
        let _ = app.emit("peer-scale", scale_millis);
    }

    /// Re-anchor every open peer window to the local pet's current outer
    /// position. Cheap on the steady state and during a drag (one
    /// `set_position` per peer per OS move event). Used the cached scale, so
    /// callers don't need to plumb it through.
    pub async fn reposition_all(&self, app: &AppHandle) {
        let scale_millis = self.scale_millis.load(Ordering::Relaxed);
        let entries: Vec<(String, usize)> = {
            let map = self.open.lock().await;
            let mut ids: Vec<&String> = map.keys().collect();
            ids.sort();
            ids.iter()
                .enumerate()
                .map(|(slot, peer_id)| (window_label(peer_id), slot))
                .collect()
        };
        for (label, slot) in entries {
            if let Some(win) = app.get_webview_window(&label) {
                let (x, y) = position_for_peer(app, scale_millis, slot);
                let _ = win.set_position(LogicalPosition::new(x, y));
            }
        }
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

        let topology_changed =
            !to_close.is_empty() || !to_open.is_empty() || !to_replace.is_empty();

        for peer_id in to_close {
            self.close_window(app, &peer_id).await;
        }
        for snap in to_replace {
            // Download the new atlas BEFORE touching the existing window so
            // the peer doesn't blink off-screen during a sprite swap. If the
            // download fails (server hiccup, peer's sprite still propagating
            // through the upload pipeline), keep the old window visible —
            // the next `peer_sprite_changed` (or a re-snapshot on reconnect)
            // will retry.
            let cached = match remote_assets::ensure_remote_assets(
                &snap.sprite_url,
                &snap.json_url,
                snap.config_url.as_deref(),
            )
            .await
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!(
                        "peer asset download failed for {} (keeping previous sprite): {}",
                        snap.peer_id, e
                    );
                    continue;
                }
            };
            // Download succeeded — close the old window and rebuild with the
            // already-cached assets. Atlas swap mid-animation is rare enough
            // (peer changes pet) that a tiny close/open flash is acceptable;
            // a true in-place atlas swap would need a peer.js protocol.
            self.close_window(app, &snap.peer_id).await;
            self.open_window_with_cached(app, snap, cached).await;
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

        // A peer joining or leaving shifts every other peer's slot index, so
        // re-anchor the whole set whenever the topology changed. (Newly
        // opened windows already opened at their final slot, but existing
        // siblings need to slide.)
        if topology_changed {
            self.reposition_all(app).await;
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
        self.open_window_with_cached(app, snap, cached).await;
    }

    /// Same as `open_window` but the caller already has the assets cached
    /// (used by `sync` so a peer mid-swap stays on-screen until the new
    /// atlas is fully downloaded).
    async fn open_window_with_cached(
        &self,
        app: &AppHandle,
        snap: PeerSnapshot,
        cached: CachedAssets,
    ) {
        // Insert into map BEFORE opening the window so get_peer_init() sees
        // it the moment the JS calls in. Compute the slot under the same
        // lock so the new window opens at its final position with no flash.
        let scale_millis = self.scale_millis.load(Ordering::Relaxed);
        let slot = {
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
            slot_for(&map, &snap.peer_id)
        };

        let (x, y) = position_for_peer(app, scale_millis, slot);
        if let Err(e) =
            build_peer_window(app, &snap.peer_id, &cached, scale_millis, x, y).await
        {
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
    scale_millis: u16,
    x: f64,
    y: f64,
) -> tauri::Result<()> {
    let label = window_label(peer_id);
    if app.get_webview_window(&label).is_some() {
        return Ok(());
    }
    let scale = scale_millis as f64 / 1000.0;
    let width = PEER_WIDTH * scale;
    let height = PEER_HEIGHT * scale;
    let url = format!("peer.html#{}", urlencoding(peer_id));
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title("Eggs Peer")
        .inner_size(width, height)
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

/// Always place peers immediately to the right of the local pet window
/// with bottoms aligned. For multiple peers, sort lexicographically by
/// `peer_id` and stack them right-ward (slot 0 = closest to local pet,
/// slot 1 = one cell further right, ...). The same `peer_id` always lands
/// in the same slot relative to its siblings, so reconnects don't
/// re-shuffle existing peers.
fn position_for_peer(app: &AppHandle, scale_millis: u16, slot_index: usize) -> (f64, f64) {
    let scale = scale_millis as f64 / 1000.0;
    let pet_w = PEER_WIDTH * scale;
    let pet_h = PEER_HEIGHT * scale;

    let pet_win = app.get_webview_window("pet");
    let scale_factor = pet_win
        .as_ref()
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or(1.0);
    let (anchor_x, anchor_y) = pet_win
        .as_ref()
        .and_then(|w| w.outer_position().ok())
        .map(|p| (p.x as f64 / scale_factor, p.y as f64 / scale_factor))
        .unwrap_or((200.0, 200.0));
    let (screen_w, screen_h) =
        primary_monitor_size(app, scale_factor).unwrap_or((1440.0, 900.0));

    const GAP: f64 = 16.0;
    let dx = (pet_w + GAP) * (slot_index as f64 + 1.0);
    // Same outer y as the local pet → bottoms align (both windows are
    // PEER_WIDTH x PEER_HEIGHT at the cached local scale).
    let x = (anchor_x + dx).clamp(0.0, (screen_w - pet_w).max(0.0));
    let y = anchor_y.clamp(40.0, (screen_h - pet_h - 24.0).max(40.0));
    (x, y)
}

/// Slot index of `peer_id` in the lexicographic ordering of currently
/// open peers (caller holds the `open` lock). Used to keep the same
/// physical position stable across reconnects: a peer's position only
/// shifts if a peer with a smaller id joins or leaves.
fn slot_for(map: &HashMap<String, OpenPeer>, peer_id: &str) -> usize {
    let mut ids: Vec<&String> = map.keys().collect();
    ids.sort();
    ids.iter().position(|id| **id == peer_id).unwrap_or(0)
}

fn primary_monitor_size(app: &AppHandle, scale_factor: f64) -> Option<(f64, f64)> {
    let mon = app.primary_monitor().ok().flatten()?;
    let size = mon.size();
    Some((
        size.width as f64 / scale_factor,
        size.height as f64 / scale_factor,
    ))
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
