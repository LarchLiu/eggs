// Pet manifest loading.
//
// Codex pet contract (see hatch-pet skill):
//   <pets-root>/<pet-id>/
//     pet.json
//     spritesheet.webp
//
// `pets_dirs()` returns every place the desktop will look for a pet, in
// priority order:
//   1. $EGGS_PETS_DIR (when set, this is the only entry — explicit override).
//   2. ~/.eggs/pets         (current default; primary location for `eggs install`)
//   3. $CODEX_HOME/pets or ~/.codex/pets (legacy; lets users keep pets they
//      installed via the Python desktop without copying them over.)
//
// The manifest itself is intentionally tiny -- the atlas geometry (8x9 cells
// of 192x208) and per-state frame counts/durations are a hardcoded contract,
// not data, and live in the frontend's LAYOUT table.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
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

/// Every directory the desktop will scan when looking up an installed pet,
/// in priority order. The first existing manifest wins for a given id.
pub fn pets_dirs() -> Vec<PathBuf> {
    if let Ok(p) = std::env::var("EGGS_PETS_DIR") {
        return vec![PathBuf::from(p)];
    }
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        out.push(home.join(".eggs").join("pets"));
    }
    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        out.push(PathBuf::from(codex_home).join("pets"));
    } else if let Some(home) = dirs::home_dir() {
        out.push(home.join(".codex").join("pets"));
    }
    out
}

/// The directory `eggs install` writes into. Always the highest-priority
/// entry from `pets_dirs()` (so a fresh install lands where the desktop
/// will look for it first).
pub fn primary_pets_dir() -> PathBuf {
    pets_dirs()
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("pets"))
}

pub fn list_installed_pets() -> io::Result<Vec<PetInfo>> {
    let mut pets: Vec<PetInfo> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for dir in pets_dirs() {
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let manifest = entry.path().join("pet.json");
            if !manifest.exists() {
                continue;
            }
            let Some(m) = fs::read_to_string(&manifest)
                .ok()
                .and_then(|t| serde_json::from_str::<PetManifest>(&t).ok())
            else {
                continue;
            };
            if !seen.insert(m.id.clone()) {
                // First occurrence wins (higher-priority dir already had it).
                continue;
            }
            pets.push(PetInfo {
                id: m.id,
                display_name: m.display_name,
                manifest_path: manifest,
            });
        }
    }
    pets.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(pets)
}

pub fn load_pet(id: &str) -> io::Result<PetManifest> {
    let mut last_err: Option<io::Error> = None;
    for dir in pets_dirs() {
        let pet_dir = dir.join(id);
        let manifest_path = pet_dir.join("pet.json");
        if !manifest_path.exists() {
            continue;
        }
        match fs::read_to_string(&manifest_path) {
            Ok(text) => match serde_json::from_str::<PetManifest>(&text) {
                Ok(mut m) => {
                    m.spritesheet_abs = Some(pet_dir.join(&m.spritesheet_path));
                    return Ok(m);
                }
                Err(e) => {
                    last_err = Some(io::Error::new(io::ErrorKind::InvalidData, e));
                }
            },
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("pet '{id}' not found in any of {:?}", pets_dirs()),
        )
    }))
}

pub fn first_available_pet() -> Option<String> {
    list_installed_pets().ok()?.into_iter().next().map(|p| p.id)
}
