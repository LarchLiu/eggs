// Cached download of remote peer sprite assets.
//
// Mirrors the Python reference (egg_desktop.py:cache_remote_sprite). The
// server publishes per-upload assets at /assets/<sprite_id>/{sprite.<ext>,
// sprite.json, config.json}; the trailing <sprite_id> is content-stable
// (a fresh upload gets a fresh id) so we can use it as the cache directory
// name. Multiple peers running the same sprite share one cache entry.
//
// Layout under ~/.codex/eggs/remote/:
//   <sprite_id>/
//     sprite.png  (or sprite.webp, depending on the original upload)
//     sprite.json
//     config.json (optional)

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

    let sprite_filename = filename_from_url(sprite_url).unwrap_or_else(|| "sprite.png".to_string());
    let json_filename = filename_from_url(json_url).unwrap_or_else(|| "sprite.json".to_string());
    let config_filename = config_url
        .and_then(filename_from_url)
        .unwrap_or_else(|| "config.json".to_string());

    let sprite_path = dir.join(&sprite_filename);
    let json_path = dir.join(&json_filename);
    let config_path = config_url.map(|_| dir.join(&config_filename));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()?;

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
    let bytes = client.get(url).send().await?.error_for_status()?.bytes().await?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension("download");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, target)?;
    Ok(())
}

/// Pull the second-to-last path segment, which the server uses as the
/// per-upload asset id (`/assets/<id>/sprite.png`).
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

/// Public wrapper used by the peer window manager to detect sprite swaps
/// without re-running the cache pipeline.
pub fn asset_id_hint(raw: &str) -> Option<String> {
    asset_id_from_url(raw)
}

fn filename_from_url(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw).ok()?;
    let last = parsed.path_segments()?.rfind(|s| !s.is_empty())?;
    let safe: String = last
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '-' | '_' | '.'))
        .collect();
    if safe.is_empty() {
        None
    } else {
        Some(safe)
    }
}
