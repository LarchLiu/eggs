// Anonymous device identity, persisted at ~/.eggs/client.json.
//
// Schema (matches the legacy egg_desktop.py file):
//   { "device_id": "<uuid hex>" }
//
// The device_id is generated lazily on first read and reused forever; the Go
// server uses it as the owner of uploaded sprites and as the websocket
// session identity.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ClientConfig {
    #[serde(default, rename = "device_id")]
    pub device_id: String,
}

fn client_path() -> std::path::PathBuf {
    crate::state::app_dir().join("client.json")
}

pub fn read_client_config() -> io::Result<ClientConfig> {
    let path = client_path();
    let mut cfg = if path.exists() {
        let text = fs::read_to_string(&path)?;
        serde_json::from_str::<ClientConfig>(&text).unwrap_or_default()
    } else {
        ClientConfig::default()
    };
    if cfg.device_id.trim().is_empty() {
        cfg.device_id = uuid::Uuid::new_v4().simple().to_string();
        write_client_config(&cfg)?;
    }
    Ok(cfg)
}

pub fn write_client_config(cfg: &ClientConfig) -> io::Result<()> {
    let dir = crate::state::app_dir();
    fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(cfg)?;
    fs::write(client_path(), format!("{json}\n"))
}
