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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RuntimeState {
    #[serde(default, alias = "sprite")]
    pub pet: String,
    #[serde(default = "default_state_name")]
    pub state: String,
}

fn default_state_name() -> String {
    "idle".to_string()
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
    let mut s: RuntimeState = serde_json::from_str(&text)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if s.pet.is_empty() {
        s.pet = default_state().pet;
    }
    if s.state.is_empty() {
        s.state = default_state().state;
    }
    Ok(s)
}

pub fn write_state(s: &RuntimeState) -> io::Result<()> {
    let dir = app_dir();
    fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(s)?;
    fs::write(state_path(), json)
}

pub fn set_state(name: &str) -> io::Result<()> {
    let mut s = read_state().unwrap_or_else(|_| default_state());
    s.state = name.to_string();
    write_state(&s)
}

pub fn set_pet(id: &str) -> io::Result<()> {
    // If remote is enabled, push the new pet's assets to the backend BEFORE
    // we touch state.json. The running GUI's state-poller (remote.rs) sends
    // a `{"type":"sprite", ...}` ws message as soon as it sees the pet field
    // change; the server silently drops that message if no sprite record
    // exists for (device, name), so peers would keep showing the old sprite.
    // Uploading first closes that race. Mirrors
    // egg_desktop.py:set_sprite → ensure_remote_sprite_uploaded.
    let remote = crate::remote::read_remote_config();
    if remote.enabled {
        match crate::client::read_client_config() {
            Ok(client) => {
                if let Err(e) = crate::upload::ensure_pet_uploaded_blocking(
                    &remote.server_url,
                    &client.device_id,
                    id,
                ) {
                    eprintln!("warning: remote sprite upload failed for '{id}': {e}");
                    eprintln!(
                        "warning: peers may keep the previous sprite until the upload succeeds"
                    );
                }
            }
            Err(e) => {
                eprintln!("warning: cannot read client.json for remote upload: {e}");
            }
        }
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
    }
}
