// Pet manifest loading.
//
// Codex pet contract (see hatch-pet skill):
//   ${CODEX_HOME:-$HOME/.codex}/pets/<pet-id>/
//     pet.json
//     spritesheet.webp
//
// The manifest itself is intentionally tiny -- the atlas geometry (8x9 cells
// of 192x208) and per-state frame counts/durations are a hardcoded contract,
// not data, and live in the frontend's LAYOUT table.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PetManifest {
    pub id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "spritesheetPath")]
    pub spritesheet_path: String,

    /// Absolute path to the spritesheet, populated server-side so the webview
    /// can resolve it through the asset protocol.
    #[serde(rename = "spritesheetAbs", skip_deserializing)]
    pub spritesheet_abs: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PetInfo {
    pub id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "manifestPath")]
    pub manifest_path: PathBuf,
}

pub fn pets_dir() -> PathBuf {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        return PathBuf::from(home).join("pets");
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".codex")
        .join("pets")
}

pub fn list_installed_pets() -> io::Result<Vec<PetInfo>> {
    let dir = pets_dir();
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut pets = vec![];
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let manifest = entry.path().join("pet.json");
        if !manifest.exists() {
            continue;
        }
        match fs::read_to_string(&manifest)
            .ok()
            .and_then(|t| serde_json::from_str::<PetManifest>(&t).ok())
        {
            Some(m) => pets.push(PetInfo {
                id: m.id,
                display_name: m.display_name,
                manifest_path: manifest,
            }),
            None => continue,
        }
    }
    pets.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(pets)
}

pub fn load_pet(id: &str) -> io::Result<PetManifest> {
    let pet_dir = pets_dir().join(id);
    let manifest_path = pet_dir.join("pet.json");
    let text = fs::read_to_string(&manifest_path)?;
    let mut m: PetManifest = serde_json::from_str(&text)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    m.spritesheet_abs = Some(pet_dir.join(&m.spritesheet_path));
    Ok(m)
}

pub fn first_available_pet() -> Option<String> {
    list_installed_pets().ok()?.into_iter().next().map(|p| p.id)
}
