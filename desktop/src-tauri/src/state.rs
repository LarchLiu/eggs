// Runtime state, persisted at ~/.eggs/state.json so external CLIs can
// drive the running pet without IPC.
//
// Schema (forward compatible with the legacy egg_desktop.py file):
//   { "pet": "noir-webling", "state": "idle" }
//
// Reads also accept the legacy field name `sprite` so an existing state.json
// from the Python/Swift runtime keeps working until the user runs a new
// `eggs state` or `eggs pet` command.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RuntimeState {
    #[serde(default, alias = "sprite")]
    pub pet: String,
    #[serde(default = "default_state_name")]
    pub state: String,
    #[serde(default = "default_scale_millis")]
    pub scale_millis: u16,
    #[serde(default)]
    pub window_x: Option<i32>,
    #[serde(default)]
    pub window_y: Option<i32>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HatchState {
    #[serde(default)]
    pub completed: Vec<String>,
}

fn default_state_name() -> String {
    "idle".to_string()
}

fn default_scale_millis() -> u16 {
    1000
}

/// Per-user app data directory. Defaults to `~/.eggs` on every platform
/// (dotfile-style under `dirs::home_dir`, which is `$HOME` on unix and
/// `%USERPROFILE%` on Windows). Override via `EGGS_APP_DIR`.
pub fn app_dir() -> PathBuf {
    if let Ok(d) = std::env::var("EGGS_APP_DIR") {
        return PathBuf::from(d);
    }
    dirs::home_dir().unwrap_or_default().join(".eggs")
}

pub fn state_path() -> PathBuf {
    app_dir().join("state.json")
}

pub fn hatch_state_path() -> PathBuf {
    app_dir().join("hatch-state.json")
}

pub fn read_state() -> io::Result<RuntimeState> {
    let path = state_path();
    if !path.exists() {
        // First launch: persist a default state.json so the file is visible to
        // external tooling (CLI subcommands, Python egg_desktop.py) and the
        // pet poller's "did this change?" check has a stable starting point.
        let s = default_state();
        let _ = write_state(&s);
        return Ok(s);
    }
    let text = fs::read_to_string(&path)?;
    let mut s: RuntimeState =
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if s.pet.is_empty() {
        s.pet = default_state().pet;
    }
    if s.state.is_empty() {
        s.state = default_state().state;
    }
    if !matches!(s.scale_millis, 400 | 500 | 600 | 800 | 1000) {
        s.scale_millis = default_scale_millis();
    }
    Ok(s)
}

pub fn write_state(s: &RuntimeState) -> io::Result<()> {
    let dir = app_dir();
    fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(s)?;
    fs::write(state_path(), json)
}

pub fn read_hatch_state() -> io::Result<HatchState> {
    let path = hatch_state_path();
    if !path.exists() {
        return Ok(HatchState::default());
    }
    let text = fs::read_to_string(path)?;
    let mut state: HatchState = serde_json::from_str(&text)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    state.completed = state
        .completed
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
    Ok(state)
}

pub fn mark_pet_hatched(pet_id: &str) -> io::Result<()> {
    let pet_id = pet_id.trim();
    if pet_id.is_empty() {
        return Ok(());
    }
    let mut state = read_hatch_state().unwrap_or_default();
    let mut set: BTreeSet<String> = state.completed.into_iter().collect();
    set.insert(pet_id.to_string());
    state.completed = set.into_iter().collect();
    write_hatch_state(&state)
}

fn write_hatch_state(state: &HatchState) -> io::Result<()> {
    let dir = app_dir();
    fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(state)?;
    fs::write(hatch_state_path(), format!("{json}\n"))
}

pub fn set_state(name: &str) -> io::Result<()> {
    let mut s = read_state().unwrap_or_else(|_| default_state());
    s.state = name.to_string();
    write_state(&s)
}

pub fn set_window_position(x: i32, y: i32) -> io::Result<()> {
    let mut s = read_state().unwrap_or_else(|_| default_state());
    s.window_x = Some(x);
    s.window_y = Some(y);
    write_state(&s)
}

pub fn set_pet(id: &str) -> io::Result<()> {
    // If remote is enabled, push the new pet's assets to the backend BEFORE
    // we touch state.json — and refuse the swap if upload fails. The running
    // GUI's state-poller (remote.rs) sends a `{"type":"sprite",...}` ws
    // message as soon as it sees the pet field change; the server silently
    // drops that message if no sprite record exists for (device, name), so
    // peers would keep showing the old sprite. By gating the local swap on
    // upload success we keep local + peer views in lockstep: either both
    // move to the new pet, or neither does.
    //
    // This blocking variant is for the CLI subcommand process (`eggs pet
    // <id>`) — blocking the user's terminal during upload is fine. GUI
    // callers (right-click menu) must use `set_pet_async` instead, since
    // `block_on` from the Tauri main event loop would freeze every window.
    let remote = crate::remote::read_remote_config();
    if remote.enabled {
        let client = crate::client::read_client_config().map_err(|e| {
            io::Error::other(format!("cannot read client.json for remote upload: {e}"))
        })?;
        crate::upload::ensure_pet_uploaded_blocking(&remote.server_url, &client.device_id, id)
            .map_err(|e| {
                io::Error::other(format!("remote sprite upload failed for '{id}': {e}"))
            })?;
    }

    let mut s = read_state().unwrap_or_else(|_| default_state());
    s.pet = id.to_string();
    write_state(&s)
}

/// Async counterpart of [`set_pet`] for callers that already live on a
/// tokio runtime (the Tauri main event loop, the remote actor, etc.).
/// Same upload-before-swap gate as the sync version, but the HTTP upload
/// runs on the runtime instead of `block_on`-ing the calling thread.
pub async fn set_pet_async(id: &str) -> io::Result<()> {
    let remote = crate::remote::read_remote_config();
    if remote.enabled {
        let client = crate::client::read_client_config().map_err(|e| {
            io::Error::other(format!("cannot read client.json for remote upload: {e}"))
        })?;
        crate::upload::ensure_pet_uploaded(&remote.server_url, &client.device_id, id)
            .await
            .map_err(|e| {
                io::Error::other(format!("remote sprite upload failed for '{id}': {e}"))
            })?;
    }

    let mut s = read_state().unwrap_or_else(|_| default_state());
    s.pet = id.to_string();
    write_state(&s)
}

fn default_state() -> RuntimeState {
    let pet = crate::pet::first_available_pet().unwrap_or_else(|| "noir-webling".to_string());
    RuntimeState {
        pet,
        state: default_state_name(),
        scale_millis: default_scale_millis(),
        window_x: None,
        window_y: None,
    }
}
