use std::sync::Mutex;

use tauri::menu::{CheckMenuItem, Menu, MenuId, MenuItem, Submenu};
use tauri::{AppHandle, LogicalSize, Manager, Size, WebviewWindow};

use crate::pet;
use crate::state::{self, RuntimeState};

const PET_PREFIX: &str = "pet:";
const STATE_PREFIX: &str = "state:";
const SCALE_PREFIX: &str = "scale:";
const QUIT_ID: &str = "quit";

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
    for info in pets {
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

    let menu = Menu::new(app).map_err(|e| e.to_string())?;
    menu.append(&pet_submenu).map_err(|e| e.to_string())?;
    menu.append(&state_submenu).map_err(|e| e.to_string())?;
    menu.append(&scale_submenu).map_err(|e| e.to_string())?;
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
        // Route through state::set_pet so the same upload-then-swap gate
        // that the CLI uses also applies to the right-click menu. If remote
        // is enabled and the upload fails, we leave state.json untouched
        // and log the error instead of silently dropping peers' view of us.
        if let Err(e) = state::set_pet(pet_id) {
            eprintln!("pet menu: failed to switch to '{pet_id}': {e}");
        }
        return;
    }

    if let Some(state_name) = id.strip_prefix(STATE_PREFIX) {
        let _ = update_state(|current| RuntimeState {
            pet: current.pet,
            state: state_name.to_string(),
            scale_millis: current.scale_millis,
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
            });
            if let Some(window) = app.get_webview_window("pet") {
                let scale = clamped as f64 / 1000.0;
                let size: Size = LogicalSize::new(192.0 * scale, 208.0 * scale).into();
                let _ = window.set_size(size);
            }
        }
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
