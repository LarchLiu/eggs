// Hash-skip pet upload to the Go server (server/main.go).
//
// Two-phase POST against /api/v1/sprites:
//
//   Phase 1 (hash-only): send device_id + sprite_name + display_name +
//   sprite_hash + json_hash as form fields, no file parts. The server either:
//     * 200 OK  -- (device_id, sprite_name, hashes) already exists, reuse row.
//     * 201 Created -- blobs are already on disk (uploaded by *anyone*); the
//       server just registers a fresh row for this device. ZERO bytes shipped.
//     * 404 + {"missing": ["sprite", "json"]} -- the blobs aren't there yet,
//       client must retry phase 2.
//     * 4xx -- validation / config / server error.
//
//   Phase 2 (full upload): same form plus `sprite` + `json` file parts. Server
//   verifies bytes match the claimed hashes, stores blobs, inserts row.
//
// Cross-device dedup: User B's first install of a pet User A already pushed
// completes in phase 1 with a single ~1 KB POST instead of an N MB upload.

use std::fs;
use std::time::Duration;

use serde::Deserialize;

use crate::pet;

const HTTP_TIMEOUT_SECS: u64 = 20;

// ---------- error type ----------

#[derive(Debug)]
pub enum UploadError {
    PetNotInstalled(String),
    Io(std::io::Error),
    Http(String),
    Server { status: u16, body: String },
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UploadError::PetNotInstalled(id) => write!(
                f,
                "pet '{id}' is not installed under ~/.eggs/pets/ or ~/.codex/pets/ (use `eggs install <dir>` or hatch-pet)"
            ),
            UploadError::Io(e) => write!(f, "io error: {e}"),
            UploadError::Http(e) => write!(f, "http error: {e}"),
            UploadError::Server { status, body } => {
                if body.is_empty() {
                    write!(f, "server returned HTTP {status}")
                } else {
                    write!(f, "server returned HTTP {status}: {body}")
                }
            }
        }
    }
}

impl UploadError {
    pub fn is_retryable(&self) -> bool {
        match self {
            UploadError::Http(_) => true,
            UploadError::Io(_) => false,
            UploadError::PetNotInstalled(_) => false,
            UploadError::Server { status, .. } => {
                !matches!(*status, 400 | 401 | 403 | 404 | 409 | 422)
            }
        }
    }
}

impl From<std::io::Error> for UploadError {
    fn from(e: std::io::Error) -> Self {
        UploadError::Io(e)
    }
}

impl From<reqwest::Error> for UploadError {
    fn from(e: reqwest::Error) -> Self {
        UploadError::Http(e.to_string())
    }
}

// ---------- response shapes ----------

#[derive(Deserialize, Debug)]
struct SpriteRecord {
    pub id: String,
}

#[derive(Deserialize, Debug, Default)]
struct MissingBlobsResponse {
    #[serde(default)]
    missing: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum UploadMode {
    /// Server already had a sprites row for (device, name, hashes); no work done.
    Reused,
    /// Server already had the blobs (uploaded by anyone); registered a row.
    HashRegistered,
    /// Bytes hit the wire.
    BytesUploaded,
}

#[derive(Debug, Clone)]
pub struct UploadOutcome {
    pub sprite_id: String,
    pub mode: UploadMode,
}

// ---------- hashing ----------

fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(data);
    let bytes = hasher.finalize();
    let mut s = String::with_capacity(bytes.len() * 2);
    use std::fmt::Write;
    for b in bytes {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

// ---------- public API ----------

pub async fn ensure_pet_uploaded_exact(
    server_url: &str,
    device_id: &str,
    pet_id: &str,
    source_kind: &str,
) -> Result<UploadOutcome, UploadError> {
    let manifest = pet::load_pet_exact(pet_id, source_kind)
        .map_err(|_| UploadError::PetNotInstalled(format!("{source_kind}:{pet_id}")))?;
    let sheet_path = manifest
        .spritesheet_abs
        .clone()
        .ok_or_else(|| UploadError::PetNotInstalled(format!("{source_kind}:{pet_id}")))?;
    let pet_dir = sheet_path
        .parent()
        .ok_or_else(|| UploadError::PetNotInstalled(format!("{source_kind}:{pet_id}")))?
        .to_path_buf();
    let manifest_path = manifest
        .manifest_abs
        .clone()
        .unwrap_or_else(|| pet_dir.join("pet.json"));
    let manifest_bytes = fs::read(&manifest_path)?;
    if !sheet_path.exists() {
        return Err(UploadError::PetNotInstalled(format!("{source_kind}:{pet_id}")));
    }
    let sheet_bytes = fs::read(&sheet_path)?;

    let sprite_hash = sha256_hex(&sheet_bytes);
    let json_hash = sha256_hex(&manifest_bytes);

    let display_name = if manifest.display_name.is_empty() {
        pet_id.to_string()
    } else {
        manifest.display_name.clone()
    };
    let sheet_filename = sheet_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .unwrap_or_else(|| "spritesheet.webp".to_string());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()?;
    let endpoint = format!("{}/api/v1/sprites", server_url.trim_end_matches('/'));

    let phase1 = phase_one_hash_only(
        &client,
        &endpoint,
        device_id,
        pet_id,
        &display_name,
        &sprite_hash,
        &json_hash,
    )
    .await?;
    if let Some(outcome) = phase1 {
        return Ok(outcome);
    }

    phase_two_full_upload(
        &client,
        &endpoint,
        device_id,
        pet_id,
        &display_name,
        &sprite_hash,
        &json_hash,
        sheet_bytes,
        manifest_bytes,
        &sheet_filename,
    )
    .await
}

async fn phase_one_hash_only(
    client: &reqwest::Client,
    endpoint: &str,
    device_id: &str,
    pet_id: &str,
    display_name: &str,
    sprite_hash: &str,
    json_hash: &str,
) -> Result<Option<UploadOutcome>, UploadError> {
    let form = reqwest::multipart::Form::new()
        .text("device_id", device_id.to_string())
        .text("sprite_name", pet_id.to_string())
        .text("display_name", display_name.to_string())
        .text("sprite_hash", sprite_hash.to_string())
        .text("json_hash", json_hash.to_string());

    let resp = client.post(endpoint).multipart(form).send().await?;
    let status = resp.status();
    match status.as_u16() {
        200 => {
            let record: SpriteRecord = resp.json().await?;
            Ok(Some(UploadOutcome {
                sprite_id: record.id,
                mode: UploadMode::Reused,
            }))
        }
        201 => {
            let record: SpriteRecord = resp.json().await?;
            Ok(Some(UploadOutcome {
                sprite_id: record.id,
                mode: UploadMode::HashRegistered,
            }))
        }
        404 => {
            // Server tells us which fields it could not resolve from existing
            // blobs. We always upload all of them in phase 2 anyway, so we
            // only check that the body actually advertises the missing list
            // (vs. some unrelated 404 from a misconfigured proxy).
            let body: MissingBlobsResponse = resp.json().await.unwrap_or_default();
            if body.missing.is_empty() {
                Err(UploadError::Server {
                    status: 404,
                    body: "missing-blobs response had empty list".to_string(),
                })
            } else {
                Ok(None)
            }
        }
        _ => {
            let body = resp.text().await.unwrap_or_default();
            Err(UploadError::Server {
                status: status.as_u16(),
                body: body.trim().to_string(),
            })
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn phase_two_full_upload(
    client: &reqwest::Client,
    endpoint: &str,
    device_id: &str,
    pet_id: &str,
    display_name: &str,
    sprite_hash: &str,
    json_hash: &str,
    sheet_bytes: Vec<u8>,
    manifest_bytes: Vec<u8>,
    sheet_filename: &str,
) -> Result<UploadOutcome, UploadError> {
    let sheet_mime = if sheet_filename.ends_with(".png") {
        "image/png"
    } else {
        "image/webp"
    };

    let sheet_part = reqwest::multipart::Part::bytes(sheet_bytes)
        .file_name(sheet_filename.to_string())
        .mime_str(sheet_mime)
        .map_err(UploadError::from)?;
    let manifest_part = reqwest::multipart::Part::bytes(manifest_bytes)
        .file_name("pet.json".to_string())
        .mime_str("application/json")
        .map_err(UploadError::from)?;

    let form = reqwest::multipart::Form::new()
        .text("device_id", device_id.to_string())
        .text("sprite_name", pet_id.to_string())
        .text("display_name", display_name.to_string())
        .text("sprite_hash", sprite_hash.to_string())
        .text("json_hash", json_hash.to_string())
        .part("sprite", sheet_part)
        .part("json", manifest_part);

    let resp = client.post(endpoint).multipart(form).send().await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(UploadError::Server {
            status: status.as_u16(),
            body: body.trim().to_string(),
        });
    }
    let record: SpriteRecord = resp.json().await?;
    Ok(UploadOutcome {
        sprite_id: record.id,
        mode: if status.as_u16() == 200 {
            UploadMode::Reused
        } else {
            UploadMode::BytesUploaded
        },
    })
}

/// Synchronous wrapper for CLI subcommands that run before the Tauri runtime
/// exists.
pub fn ensure_pet_uploaded_exact_blocking(
    server_url: &str,
    device_id: &str,
    pet_id: &str,
    source_kind: &str,
) -> Result<UploadOutcome, UploadError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| UploadError::Http(format!("tokio runtime init failed: {e}")))?;
    runtime.block_on(ensure_pet_uploaded_exact(
        server_url,
        device_id,
        pet_id,
        source_kind,
    ))
}
