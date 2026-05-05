// Runtime state, persisted at ~/.codex/eggs/state.json so external CLIs can
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

pub fn app_dir() -> PathBuf {
    if let Ok(d) = std::env::var("EGGS_APP_DIR") {
        return PathBuf::from(d);
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".codex")
        .join("eggs")
}

pub fn state_path() -> PathBuf {
    app_dir().join("state.json")
}

pub fn read_state() -> io::Result<RuntimeState> {
    let path = state_path();
    if !path.exists() {
        return Ok(default_state());
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
    let mut s = read_state().unwrap_or_else(|_| default_state());
    s.pet = id.to_string();
    write_state(&s)?;
    // Keep remote.json::sprite in sync so the next ws (re)connect announces
    // the right pet to peers, mirroring egg_desktop.py:sync_remote_sprite.
    let _ = crate::remote::update_remote_config(|c| c.sprite = id.to_string());
    Ok(())
}

fn default_state() -> RuntimeState {
    let pet = crate::pet::first_available_pet().unwrap_or_else(|| "noir-webling".to_string());
    RuntimeState {
        pet,
        state: default_state_name(),
    }
}
