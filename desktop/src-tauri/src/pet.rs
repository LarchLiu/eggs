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
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const BUILTIN_STATES: [&str; 9] = [
    "idle",
    "running-right",
    "running-left",
    "waving",
    "jumping",
    "failed",
    "waiting",
    "running",
    "review",
];


#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnimationStateDef {
    pub state: String,
    pub row: u16,
    pub frames: u16,
    #[serde(default)]
    pub durations: Vec<u16>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PetManifest {
    pub id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "spritesheetPath")]
    pub spritesheet_path: String,
    /// Custom animation states appended after the built-in 9 states.
    #[serde(default)]
    pub custom: Vec<AnimationStateDef>,
    /// Hatch-process-only states (playable by state name, but intentionally
    /// hidden from the right-click State menu).
    #[serde(default)]
    pub hatch: Vec<AnimationStateDef>,

    /// Absolute path to the spritesheet, populated server-side so the webview
    /// can resolve it through the asset protocol.
    #[serde(rename = "spritesheetAbs", skip_deserializing)]
    pub spritesheet_abs: Option<PathBuf>,

    /// Absolute path to the source manifest (`pet.json` or cached
    /// `sprite.json`), used by upload.rs when a remote-cached sprite becomes
    /// the active local pet.
    #[serde(rename = "manifestAbs", skip_deserializing)]
    pub manifest_abs: Option<PathBuf>,

    /// Runtime-only source classification so the frontend can distinguish
    /// hatch-state keys for built-in / local / remote pets that share an id.
    #[serde(rename = "sourceKind", skip_deserializing)]
    pub source_kind: Option<String>,
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
    #[serde(default)]
    pub builtin: bool,
}

impl PetInfo {
    pub fn source_kind(&self) -> &'static str {
        if self.builtin {
            "builtin"
        } else if self.remote {
            "remote"
        } else {
            "local"
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PetRootKind {
    Installed,
    Builtin,
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
    out.extend(builtin_runtime_dirs());
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
            PetRootKind::Installed | PetRootKind::Builtin => {
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
                        builtin: root.kind == PetRootKind::Builtin,
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
                        builtin: false,
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

pub fn first_available_pet_info() -> Option<PetInfo> {
    list_installed_pets()
        .ok()?
        .into_iter()
        .min_by(|a, b| {
            pet_priority(a)
                .cmp(&pet_priority(b))
                .then_with(|| a.id.cmp(&b.id))
        })
}

pub fn preferred_source_for_pet(id: &str) -> Option<String> {
    list_installed_pets()
        .ok()?
        .into_iter()
        .find(|info| info.id == id)
        .map(|info| info.source_kind().to_string())
}

pub fn load_pet_exact(id: &str, source_kind: &str) -> io::Result<PetManifest> {
    let mut last_err: Option<io::Error> = None;
    for root in pet_roots() {
        if source_kind_for_root(root.kind) != source_kind {
            continue;
        }
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
            format!("pet '{id}' with source '{source_kind}' not found in any of {:?}", pet_search_dirs()),
        )
    }))
}

fn pet_priority(info: &PetInfo) -> u8 {
    if info.builtin {
        0
    } else if info.remote {
        2
    } else {
        1
    }
}

fn source_kind_for_root(kind: PetRootKind) -> &'static str {
    match kind {
        PetRootKind::Builtin => "builtin",
        PetRootKind::Installed => "local",
        PetRootKind::RemoteCache => "remote",
    }
}

pub fn sync_builtin_pets() -> io::Result<()> {
    let sync_dir = builtin_sync_dir();
    fs::create_dir_all(&sync_dir)?;

    let mut synced_ids = BTreeSet::new();
    for root in builtin_source_dirs() {
        if !root.exists() {
            continue;
        }
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some((manifest, manifest_path)) =
                load_manifest_from_dir(&entry.path(), PetRootKind::Installed, None)
            else {
                continue;
            };
            if !synced_ids.insert(manifest.id.clone()) {
                continue;
            }
            sync_builtin_pet(&manifest, &manifest_path)?;
        }
    }

    for entry in fs::read_dir(&sync_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        if synced_ids.contains(&id) {
            continue;
        }
        fs::remove_dir_all(entry.path())?;
    }

    Ok(())
}

fn pet_roots() -> Vec<PetRoot> {
    let builtin_paths: HashSet<PathBuf> = builtin_runtime_dirs().into_iter().collect();
    let mut out: Vec<PetRoot> = pets_dirs()
        .into_iter()
        .map(|path| PetRoot {
            kind: if builtin_paths.contains(&path) {
                PetRootKind::Builtin
            } else {
                PetRootKind::Installed
            },
            path,
        })
        .collect();
    out.push(PetRoot {
        path: crate::state::app_dir().join("remote"),
        kind: PetRootKind::RemoteCache,
    });
    out
}

fn builtin_runtime_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    push_unique_path(&mut out, builtin_sync_dir());
    out
}

fn builtin_source_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();

    if let Ok(dir) = std::env::var("EGGS_BUILTIN_PETS_DIR") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            out.push(path);
        }
        return out;
    }

    // Dev / local builds: resolve relative to this crate's source tree.
    // CARGO_MANIFEST_DIR points at desktop/src-tauri, so parent()/pets is
    // the repo's desktop/pets directory.
    push_unique_path(
        &mut out,
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("pets"),
    );

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            for candidate in [
                exe_dir.join("../../../pets"),
                exe_dir.join("../../../../pets"),
                exe_dir.join("pets"),
                exe_dir.join("../pets"),
                exe_dir.join("../../pets"),
            ] {
                push_unique_path(&mut out, candidate);
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        push_unique_path(&mut out, cwd.join("desktop").join("pets"));
        push_unique_path(&mut out, cwd.join("pets"));
    }

    out
}

fn builtin_sync_dir() -> PathBuf {
    crate::state::app_dir().join("builtin")
}

fn push_unique_path(out: &mut Vec<PathBuf>, path: PathBuf) {
    if out.iter().any(|existing| existing == &path) {
        return;
    }
    out.push(path);
}

fn manifest_path_for(dir: &Path, kind: PetRootKind) -> PathBuf {
    manifest_paths_for(dir, kind)
        .into_iter()
        .next()
        .unwrap_or_else(|| dir.join("pet.json"))
}

fn manifest_paths_for(dir: &Path, kind: PetRootKind) -> Vec<PathBuf> {
    match kind {
        PetRootKind::Installed | PetRootKind::Builtin => vec![dir.join("pet.json")],
        // New remote caches store `pet.json`, but older builds wrote
        // `sprite.json`. Accept both so cached peer sprites show up in the
        // context menu regardless of which version created them.
        PetRootKind::RemoteCache => vec![dir.join("pet.json"), dir.join("sprite.json")],
    }
}

fn load_manifest_from_dir(
    dir: &Path,
    kind: PetRootKind,
    requested_id: Option<&str>,
) -> Option<(PetManifest, PathBuf)> {
    for manifest_path in manifest_paths_for(dir, kind) {
        let Ok(text) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(mut manifest) = serde_json::from_str::<PetManifest>(&text) else {
            continue;
        };
        if !sanitize_manifest_states(&mut manifest) {
            continue;
        }
        if let Some(id) = requested_id {
            manifest.id = id.to_string();
        }
        let resolved = {
            let sheet_path = resolve_spritesheet_path(dir, &manifest)?;
            (sheet_path, manifest_path.clone())
        };
        manifest.spritesheet_abs = Some(resolved.0);
        manifest.manifest_abs = Some(resolved.1);
        manifest.source_kind = Some(source_kind_for_root(kind).to_string());
        manifest.spritesheet_abs.as_ref()?;
        return Some((manifest, manifest_path));
    }
    None
}

fn sync_builtin_pet(manifest: &PetManifest, manifest_path: &Path) -> io::Result<()> {
    let dest_dir = builtin_sync_dir().join(&manifest.id);
    fs::create_dir_all(&dest_dir)?;

    let sheet_src = manifest
        .spritesheet_abs
        .clone()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "builtin spritesheet missing"))?;
    let manifest_dest = dest_dir.join("pet.json");
    let sheet_name = sheet_src
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "builtin spritesheet missing filename"))?;
    let sheet_dest = dest_dir.join(sheet_name);

    copy_if_hash_diff(manifest_path, &manifest_dest)?;
    copy_if_hash_diff(&sheet_src, &sheet_dest)?;
    Ok(())
}

fn copy_if_hash_diff(src: &Path, dest: &Path) -> io::Result<()> {
    let should_copy = if !dest.exists() {
        true
    } else {
        file_sha256_hex(src)? != file_sha256_hex(dest)?
    };
    if should_copy {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dest)?;
    }
    Ok(())
}

fn file_sha256_hex(path: &Path) -> io::Result<String> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    use std::fmt::Write;
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    Ok(out)
}

fn sanitize_manifest_states(manifest: &mut PetManifest) -> bool {
    // `custom`: non-empty name, non-duplicate, and cannot override built-ins.
    let mut custom_seen = BTreeSet::new();
    for def in &manifest.custom {
        let name = def.state.trim();
        if name.is_empty() {
            return false;
        }
        if BUILTIN_STATES.contains(&name) {
            return false;
        }
        if !custom_seen.insert(name.to_string()) {
            return false;
        }
    }

    // `hatch`: optional and partial-friendly; keep only entries with
    // non-empty unique names so a malformed element won't break the whole pet.
    let mut hatch_seen = BTreeSet::new();
    manifest.hatch.retain(|def| {
        let name = def.state.trim();
        if name.is_empty() {
            return false;
        }
        hatch_seen.insert(name.to_string())
    });
    true
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

#[cfg(test)]
mod tests {
    use super::{load_manifest_from_dir, PetRootKind};
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn make_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eggs-pet-test-{name}-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_remote_fixture(dir: &PathBuf, manifest_name: &str) {
        fs::write(
            dir.join(manifest_name),
            r#"{
  "id": "manifest-id",
  "displayName": "Remote Buddy",
  "description": "",
  "spritesheetPath": "spritesheet.png"
}"#,
        )
        .unwrap();
        fs::write(dir.join("spritesheet.png"), b"png").unwrap();
    }

    #[test]
    fn remote_cache_accepts_pet_json() {
        let dir = make_temp_dir("pet-json");
        write_remote_fixture(&dir, "pet.json");

        let (manifest, manifest_path) =
            load_manifest_from_dir(&dir, PetRootKind::RemoteCache, Some("remote-cache-id"))
                .expect("remote cache should load from pet.json");

        assert_eq!(manifest.id, "remote-cache-id");
        assert_eq!(manifest.display_name, "Remote Buddy");
        assert_eq!(manifest_path, dir.join("pet.json"));
        assert_eq!(manifest.manifest_abs, Some(dir.join("pet.json")));
        assert_eq!(manifest.spritesheet_abs, Some(dir.join("spritesheet.png")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_cache_falls_back_to_legacy_sprite_json() {
        let dir = make_temp_dir("sprite-json");
        write_remote_fixture(&dir, "sprite.json");

        let (manifest, manifest_path) =
            load_manifest_from_dir(&dir, PetRootKind::RemoteCache, Some("remote-cache-id"))
                .expect("remote cache should load from sprite.json");

        assert_eq!(manifest.id, "remote-cache-id");
        assert_eq!(manifest_path, dir.join("sprite.json"));
        assert_eq!(manifest.manifest_abs, Some(dir.join("sprite.json")));

        let _ = fs::remove_dir_all(&dir);
    }
}
