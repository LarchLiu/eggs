use std::sync::Mutex;

use tauri::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, LogicalSize, Manager, Size, WebviewWindow};

use crate::pet;
use crate::remote;
use crate::state::{self, RuntimeState};

const PET_PREFIX: &str = "pet:";
const PET_HEADER_LOCAL_ID: &str = "pet-header:local";
const PET_HEADER_REMOTE_ID: &str = "pet-header:remote";
const STATE_PREFIX: &str = "state:";
const SCALE_PREFIX: &str = "scale:";
const QUIT_ID: &str = "quit";

const REMOTE_TOGGLE_ID: &str = "remote:toggle";
const REMOTE_MODE_RANDOM_ID: &str = "remote:mode:random";
const REMOTE_MODE_ROOM_ID: &str = "remote:mode:room";
const REMOTE_UPLOAD_ID: &str = "remote:upload";
const REMOTE_LEAVE_ID: &str = "remote:leave";

pub const PET_STATES: [&str; 9] = [
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

pub const SCALE_PRESETS: [u16; 5] = [400, 500, 600, 800, 1000];

pub struct PetMenuStore {
    active_menu: Mutex<Option<Menu<tauri::Wry>>>,
}

impl PetMenuStore {
    pub fn new() -> Self {
        Self {
            active_menu: Mutex::new(None),
        }
    }

    pub fn set(&self, menu: Menu<tauri::Wry>) {
        if let Ok(mut slot) = self.active_menu.lock() {
            *slot = Some(menu);
        }
    }
}

pub fn list_states() -> Vec<String> {
    PET_STATES.iter().map(|s| (*s).to_string()).collect()
}

pub fn show_context_menu(app: &AppHandle, window: &WebviewWindow) -> Result<(), String> {
    let current = state::read_state().map_err(|e| e.to_string())?;
    let pets = pet::list_installed_pets().map_err(|e| e.to_string())?;

    let pet_submenu = Submenu::new(app, "Pet", true).map_err(|e| e.to_string())?;
    let (local_pets, remote_pets): (Vec<_>, Vec<_>) =
        pets.into_iter().partition(|info| !info.remote);
    let has_local_pets = !local_pets.is_empty();
    let has_remote_pets = !remote_pets.is_empty();
    if has_local_pets {
        let header = MenuItem::with_id(
            app,
            PET_HEADER_LOCAL_ID,
            "Local",
            false,
            Option::<&str>::None,
        )
        .map_err(|e| e.to_string())?;
        pet_submenu.append(&header).map_err(|e| e.to_string())?;
    }
    for info in local_pets {
        let item = CheckMenuItem::with_id(
            app,
            format!("{PET_PREFIX}{}", info.id),
            info.display_name,
            true,
            info.id == current.pet,
            Option::<&str>::None,
        )
        .map_err(|e| e.to_string())?;
        pet_submenu.append(&item).map_err(|e| e.to_string())?;
    }
    if has_local_pets && has_remote_pets {
        pet_submenu
            .append(&PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    if has_remote_pets {
        let header = MenuItem::with_id(
            app,
            PET_HEADER_REMOTE_ID,
            "Remote",
            false,
            Option::<&str>::None,
        )
        .map_err(|e| e.to_string())?;
        pet_submenu.append(&header).map_err(|e| e.to_string())?;
    }
    for info in remote_pets {
        let item = CheckMenuItem::with_id(
            app,
            format!("{PET_PREFIX}{}", info.id),
            info.display_name,
            true,
            info.id == current.pet,
            Option::<&str>::None,
        )
        .map_err(|e| e.to_string())?;
        pet_submenu.append(&item).map_err(|e| e.to_string())?;
    }

    let state_submenu = Submenu::new(app, "State", true).map_err(|e| e.to_string())?;
    for state_name in PET_STATES {
        let item = CheckMenuItem::with_id(
            app,
            format!("{STATE_PREFIX}{state_name}"),
            state_name,
            true,
            state_name == current.state,
            Option::<&str>::None,
        )
        .map_err(|e| e.to_string())?;
        state_submenu.append(&item).map_err(|e| e.to_string())?;
    }

    let scale_submenu = Submenu::new(app, "Size", true).map_err(|e| e.to_string())?;
    for scale_millis in SCALE_PRESETS {
        let label = format!("{:.1}x", scale_millis as f64 / 1000.0);
        let item = CheckMenuItem::with_id(
            app,
            format!("{SCALE_PREFIX}{scale_millis}"),
            label,
            true,
            scale_millis == current.scale_millis,
            Option::<&str>::None,
        )
        .map_err(|e| e.to_string())?;
        scale_submenu.append(&item).map_err(|e| e.to_string())?;
    }

    let quit_item = MenuItem::with_id(app, QUIT_ID, "Quit", true, Option::<&str>::None)
        .map_err(|e| e.to_string())?;

    let remote_submenu = build_remote_submenu(app)?;

    let menu = Menu::new(app).map_err(|e| e.to_string())?;
    menu.append(&pet_submenu).map_err(|e| e.to_string())?;
    menu.append(&state_submenu).map_err(|e| e.to_string())?;
    menu.append(&scale_submenu).map_err(|e| e.to_string())?;
    menu.append(&remote_submenu).map_err(|e| e.to_string())?;
    menu.append(&quit_item).map_err(|e| e.to_string())?;

    let store = app.state::<PetMenuStore>();
    store.set(menu.clone());

    window.popup_menu(&menu).map_err(|e| e.to_string())
}

pub fn handle_menu_event(app: &AppHandle, id: &MenuId) {
    let id = id.0.as_str();
    if id == QUIT_ID {
        app.exit(0);
        return;
    }

    if let Some(pet_id) = id.strip_prefix(PET_PREFIX) {
        // Spawn the upload-then-swap on the tokio runtime so the GUI's main
        // event loop stays responsive while ensure_pet_uploaded runs (HTTP
        // upload of a fresh sprite atlas takes a few seconds on slow links;
        // doing it on the menu callback's thread froze every window for the
        // duration). state.json is only written after the upload succeeds,
        // so the local UI keeps showing the previous pet until the swap is
        // confirmed by the server.
        let id_owned = pet_id.to_string();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = state::set_pet_async(&id_owned).await {
                eprintln!("pet menu: failed to switch to '{id_owned}': {e}");
            }
        });
        return;
    }

    if let Some(state_name) = id.strip_prefix(STATE_PREFIX) {
        let _ = update_state(|current| RuntimeState {
            pet: current.pet,
            state: state_name.to_string(),
            scale_millis: current.scale_millis,
            window_x: current.window_x,
            window_y: current.window_y,
        });
        return;
    }

    if let Some(scale_millis) = id.strip_prefix(SCALE_PREFIX) {
        if let Ok(scale_millis) = scale_millis.parse::<u16>() {
            let clamped = match scale_millis {
                400 | 500 | 600 | 800 | 1000 => scale_millis,
                _ => return,
            };
            let _ = update_state(|current| RuntimeState {
                pet: current.pet,
                state: current.state,
                scale_millis: clamped,
                window_x: current.window_x,
                window_y: current.window_y,
            });
            if let Some(window) = app.get_webview_window("pet") {
                let scale = clamped as f64 / 1000.0;
                let size: Size = LogicalSize::new(192.0 * scale, 208.0 * scale).into();
                let _ = window.set_size(size);
            }
        }
        return;
    }

    match id {
        REMOTE_TOGGLE_ID => {
            update_remote(|cfg| cfg.enabled = !cfg.enabled);
        }
        REMOTE_MODE_RANDOM_ID => {
            // "Random Match" both enables remote and switches mode in one
            // click. Preserve any saved room code so the user can switch back
            // to room mode later without retyping it.
            update_remote(|cfg| {
                cfg.enabled = true;
                cfg.mode = "random".to_string();
            });
        }
        REMOTE_MODE_ROOM_ID => {
            // Re-engage the saved room code from remote.json. New codes
            // still need `eggs remote room <code>` (no menu input).
            update_remote(|cfg| {
                cfg.enabled = true;
                cfg.mode = "room".to_string();
            });
        }
        REMOTE_UPLOAD_ID => {
            spawn_remote_upload();
        }
        REMOTE_LEAVE_ID => {
            if let Err(e) = remote::leave_room() {
                eprintln!("remote menu: failed to leave room: {e}");
            }
        }
        _ => {}
    }
}

fn update_state<F>(f: F) -> Result<(), String>
where
    F: FnOnce(RuntimeState) -> RuntimeState,
{
    let current = state::read_state().map_err(|e| e.to_string())?;
    let next = f(current);
    state::write_state(&next).map_err(|e| e.to_string())
}

/// Build the right-click "Remote" submenu off the live `remote.json` config.
/// Text-input actions (set room code, change server URL) stay CLI-only since
/// the native menu primitives don't have an input field; the menu still
/// covers the full toggle / mode-switch / upload / leave surface.
fn build_remote_submenu(app: &AppHandle) -> Result<Submenu<tauri::Wry>, String> {
    let cfg = remote::read_remote_config();
    let submenu = Submenu::new(app, "Remote", true).map_err(|e| e.to_string())?;

    let toggle = CheckMenuItem::with_id(
        app,
        REMOTE_TOGGLE_ID,
        "Enabled",
        true,
        cfg.enabled,
        Option::<&str>::None,
    )
    .map_err(|e| e.to_string())?;
    submenu.append(&toggle).map_err(|e| e.to_string())?;

    submenu
        .append(&PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;

    // Mode submenu — a "Random" radio plus an optional "Room: <code>" radio
    // when remote.json carries a saved room code. New room codes still need
    // `eggs remote room <code>` from the CLI.
    let mode_submenu = Submenu::new(app, "Mode", true).map_err(|e| e.to_string())?;
    let mode_random = CheckMenuItem::with_id(
        app,
        REMOTE_MODE_RANDOM_ID,
        "Random Match",
        true,
        cfg.enabled && cfg.mode == "random",
        Option::<&str>::None,
    )
    .map_err(|e| e.to_string())?;
    mode_submenu
        .append(&mode_random)
        .map_err(|e| e.to_string())?;
    if !cfg.room.is_empty() {
        let mode_room = CheckMenuItem::with_id(
            app,
            REMOTE_MODE_ROOM_ID,
            format!("Room: {}", cfg.room),
            true,
            cfg.enabled && cfg.mode == "room",
            Option::<&str>::None,
        )
        .map_err(|e| e.to_string())?;
        mode_submenu.append(&mode_room).map_err(|e| e.to_string())?;
    }
    submenu.append(&mode_submenu).map_err(|e| e.to_string())?;

    submenu
        .append(&PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;

    let upload = MenuItem::with_id(
        app,
        REMOTE_UPLOAD_ID,
        "Sync Sprite",
        true,
        Option::<&str>::None,
    )
    .map_err(|e| e.to_string())?;
    submenu.append(&upload).map_err(|e| e.to_string())?;

    let leave = MenuItem::with_id(
        app,
        REMOTE_LEAVE_ID,
        "Leave Room",
        cfg.enabled && remote::can_leave_room(),
        Option::<&str>::None,
    )
    .map_err(|e| e.to_string())?;
    submenu.append(&leave).map_err(|e| e.to_string())?;

    Ok(submenu)
}

/// Apply a synchronous mutation to remote.json. The remote actor's poller
/// picks up the change on the next tick (≤ POLL_INTERVAL_MS), so the WS
/// connect / disconnect / mode swap happens off the menu thread.
fn update_remote<F: FnOnce(&mut remote::RemoteConfig)>(f: F) {
    if let Err(e) = remote::update_remote_config(f) {
        eprintln!("remote menu: failed to update remote.json: {e}");
    }
}

/// Run the manual sprite re-upload on the tokio runtime so the menu thread
/// stays responsive — same reason as the pet-swap path. Mirrors the CLI's
/// `eggs remote upload`.
fn spawn_remote_upload() {
    tauri::async_runtime::spawn(async move {
        let cfg = remote::read_remote_config();
        let pet_id = match state::read_state() {
            Ok(s) if !s.pet.is_empty() => s.pet,
            Ok(_) => {
                eprintln!("remote menu: no active pet to upload");
                return;
            }
            Err(e) => {
                eprintln!("remote menu: state.json read failed: {e}");
                return;
            }
        };
        let device_id = match crate::client::read_client_config() {
            Ok(c) => c.device_id,
            Err(e) => {
                eprintln!("remote menu: client.json read failed: {e}");
                return;
            }
        };
        match crate::upload::ensure_pet_uploaded(&cfg.server_url, &device_id, &pet_id).await {
            Ok(_) => eprintln!("remote menu: re-uploaded sprite '{pet_id}'"),
            Err(e) => eprintln!("remote menu: sprite upload failed for '{pet_id}': {e}"),
        }
    });
}
