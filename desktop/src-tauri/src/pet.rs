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
use std::path::{Path, PathBuf};

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

    /// Absolute path to the source manifest (`pet.json` or cached
    /// `sprite.json`), used by upload.rs when a remote-cached sprite becomes
    /// the active local pet.
    #[serde(rename = "manifestAbs", skip_deserializing)]
    pub manifest_abs: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PetInfo {
    pub id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "manifestPath")]
    pub manifest_path: PathBuf,
    #[serde(default)]
    pub remote: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PetRootKind {
    Installed,
    RemoteCache,
}

#[derive(Debug, Clone)]
struct PetRoot {
    path: PathBuf,
    kind: PetRootKind,
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

pub fn pet_search_dirs() -> Vec<PathBuf> {
    pet_roots().into_iter().map(|root| root.path).collect()
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
    for root in pet_roots() {
        if !root.path.exists() {
            continue;
        }
        match root.kind {
            PetRootKind::Installed => {
                for entry in fs::read_dir(&root.path)? {
                    let entry = entry?;
                    if !entry.file_type()?.is_dir() {
                        continue;
                    }
                    let Some((m, manifest_path)) =
                        load_manifest_from_dir(&entry.path(), root.kind, None)
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
                        manifest_path,
                        remote: false,
                    });
                }
            }
            PetRootKind::RemoteCache => {
                for entry_id in crate::remote_assets::list_cached_sprite_ids()? {
                    let entry_path = root.path.join(&entry_id);
                    let Some((m, manifest_path)) =
                        load_manifest_from_dir(&entry_path, root.kind, Some(entry_id.as_str()))
                    else {
                        continue;
                    };
                    if !seen.insert(m.id.clone()) {
                        continue;
                    }
                    pets.push(PetInfo {
                        id: m.id,
                        display_name: m.display_name,
                        manifest_path,
                        remote: true,
                    });
                }
            }
        }
    }
    pets.sort_by(|a, b| a.remote.cmp(&b.remote).then_with(|| a.id.cmp(&b.id)));
    Ok(pets)
}

pub fn load_pet(id: &str) -> io::Result<PetManifest> {
    let mut last_err: Option<io::Error> = None;
    for root in pet_roots() {
        let pet_dir = root.path.join(id);
        if !pet_dir.exists() {
            continue;
        }
        let requested_id = if root.kind == PetRootKind::RemoteCache {
            Some(id)
        } else {
            None
        };
        match load_manifest_from_dir(&pet_dir, root.kind, requested_id) {
            Some((m, _)) => return Ok(m),
            None => {
                let manifest_path = manifest_path_for(&pet_dir, root.kind);
                if manifest_path.exists() {
                    last_err = Some(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid pet manifest: {}", manifest_path.display()),
                    ));
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("pet '{id}' not found in any of {:?}", pet_search_dirs()),
        )
    }))
}

pub fn first_available_pet() -> Option<String> {
    list_installed_pets().ok()?.into_iter().next().map(|p| p.id)
}

fn pet_roots() -> Vec<PetRoot> {
    let mut out: Vec<PetRoot> = pets_dirs()
        .into_iter()
        .map(|path| PetRoot {
            path,
            kind: PetRootKind::Installed,
        })
        .collect();
    out.push(PetRoot {
        path: crate::state::app_dir().join("remote"),
        kind: PetRootKind::RemoteCache,
    });
    out
}

fn manifest_path_for(dir: &Path, kind: PetRootKind) -> PathBuf {
    dir.join(match kind {
        PetRootKind::Installed => "pet.json",
        PetRootKind::RemoteCache => "sprite.json",
    })
}

fn load_manifest_from_dir(
    dir: &Path,
    kind: PetRootKind,
    requested_id: Option<&str>,
) -> Option<(PetManifest, PathBuf)> {
    let manifest_path = manifest_path_for(dir, kind);
    let text = fs::read_to_string(&manifest_path).ok()?;
    let mut manifest = serde_json::from_str::<PetManifest>(&text).ok()?;
    if let Some(id) = requested_id {
        manifest.id = id.to_string();
    }
    manifest.spritesheet_abs = resolve_spritesheet_path(dir, &manifest);
    manifest.manifest_abs = Some(manifest_path.clone());
    manifest.spritesheet_abs.as_ref()?;
    Some((manifest, manifest_path))
}

fn resolve_spritesheet_path(dir: &Path, manifest: &PetManifest) -> Option<PathBuf> {
    let manifest_path = dir.join(&manifest.spritesheet_path);
    if manifest_path.exists() {
        return Some(manifest_path);
    }
    [
        "sprite.webp",
        "sprite.png",
        "spritesheet.webp",
        "spritesheet.png",
    ]
    .into_iter()
    .map(|name| dir.join(name))
    .find(|path| path.exists())
}
