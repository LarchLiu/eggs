// Cached download of remote peer sprite assets.
//
// Mirrors the Python reference (egg_desktop.py:cache_remote_sprite). The
// server publishes per-upload assets at /assets/<content_id>/{sprite.<ext>,
// sprite.json, config.json}; the trailing <content_id> is content-stable, so
// we can use it as the cache directory name. Multiple peers running the same
// sprite share one cache entry.
//
// Layout under ~/.eggs/remote/:
//   <content_id>/
//     pet.json
//     spritesheet.png  (or .webp, depending on the original upload)
//     config.json (optional)
//
// Older builds wrote `sprite.json` + `sprite.png|webp`; we still recognize
// and promote those files in place so existing caches keep working.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::state;

const HTTP_TIMEOUT_SECS: u64 = 15;

#[derive(Debug, Clone)]
pub struct CachedAssets {
    pub asset_id: String,
    pub sprite_path: PathBuf,
    pub json_path: PathBuf,
    pub config_path: Option<PathBuf>,
}

#[derive(Debug)]
pub enum AssetError {
    InvalidUrl(String),
    Io(std::io::Error),
    Http(String),
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetError::InvalidUrl(u) => write!(f, "invalid asset url: {u}"),
            AssetError::Io(e) => write!(f, "io error: {e}"),
            AssetError::Http(e) => write!(f, "http error: {e}"),
        }
    }
}

impl From<std::io::Error> for AssetError {
    fn from(e: std::io::Error) -> Self {
        AssetError::Io(e)
    }
}

impl From<reqwest::Error> for AssetError {
    fn from(e: reqwest::Error) -> Self {
        AssetError::Http(e.to_string())
    }
}

pub fn cache_root() -> PathBuf {
    state::app_dir().join("remote")
}

/// List cached remote sprite ids under ~/.eggs/remote/<content_id>/.
/// Only returns directories that look like a cached sprite bundle
/// (must include a manifest plus a sprite image file).
pub fn list_cached_sprite_ids() -> std::io::Result<Vec<String>> {
    let root = cache_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        if id.is_empty() || id == "blobs" {
            continue;
        }
        let dir = entry.path();
        let has_json = dir.join("pet.json").exists() || dir.join("sprite.json").exists();
        let has_sprite = dir.join("spritesheet.png").exists()
            || dir.join("spritesheet.webp").exists()
            || dir.join("sprite.png").exists()
            || dir.join("sprite.webp").exists();
        if has_json && has_sprite {
            out.push(id);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Download (and cache) the three assets for a peer. Skips bytes for any
/// file that already exists on disk — server-published asset paths are
/// content-stable, so a hit is a definitive hit.
pub async fn ensure_remote_assets(
    sprite_url: &str,
    json_url: &str,
    config_url: Option<&str>,
) -> Result<CachedAssets, AssetError> {
    let asset_id = asset_id_from_url(sprite_url)
        .or_else(|| asset_id_from_url(json_url))
        .ok_or_else(|| AssetError::InvalidUrl(sprite_url.to_string()))?;

    let dir = cache_root().join(&asset_id);
    std::fs::create_dir_all(&dir)?;

    let sprite_ext = sprite_extension_from_url(sprite_url).unwrap_or_else(|| {
        if dir.join("spritesheet.webp").exists() || dir.join("sprite.webp").exists() {
            "webp".to_string()
        } else {
            "png".to_string()
        }
    });
    let sprite_path = dir.join(format!("spritesheet.{sprite_ext}"));
    let legacy_sprite_path = dir.join(format!("sprite.{sprite_ext}"));
    let json_path = dir.join("pet.json");
    let legacy_json_path = dir.join("sprite.json");
    let config_path = config_url.map(|_| dir.join("config.json"));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()?;

    promote_legacy_file(&legacy_sprite_path, &sprite_path)?;
    promote_legacy_file(&legacy_json_path, &json_path)?;
    download_if_missing(&client, sprite_url, &sprite_path).await?;
    download_if_missing(&client, json_url, &json_path).await?;
    if let (Some(url), Some(path)) = (config_url, config_path.as_ref()) {
        download_if_missing(&client, url, path).await?;
    }

    Ok(CachedAssets {
        asset_id,
        sprite_path,
        json_path,
        config_path,
    })
}

async fn download_if_missing(
    client: &reqwest::Client,
    url: &str,
    target: &Path,
) -> Result<(), AssetError> {
    if target.exists() {
        return Ok(());
    }
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension("download");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, target)?;
    Ok(())
}

/// Pull the second-to-last path segment, which the server uses as the
/// content-addressed asset id (`/assets/<content_id>/sprite.png`).
fn asset_id_from_url(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw).ok()?;
    let mut segs: Vec<&str> = parsed.path_segments()?.filter(|s| !s.is_empty()).collect();
    segs.pop()?; // filename
    let id = segs.pop()?;
    let safe: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if safe.is_empty() {
        None
    } else {
        Some(safe)
    }
}

fn sprite_extension_from_url(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw).ok()?;
    let last = parsed.path_segments()?.rfind(|s| !s.is_empty())?;
    let ext = Path::new(last)
        .extension()
        .and_then(|ext| ext.to_str())?
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "png" | "webp") {
        Some(ext)
    } else {
        None
    }
}

fn promote_legacy_file(legacy: &Path, target: &Path) -> Result<(), AssetError> {
    if target.exists() || !legacy.exists() {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(legacy, target)?;
    Ok(())
}

/// Public wrapper used by the peer window manager to detect sprite swaps
/// without re-running the cache pipeline.
pub fn asset_id_hint(raw: &str) -> Option<String> {
    asset_id_from_url(raw)
}
