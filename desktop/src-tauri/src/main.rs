// Eggs desktop pet — Tauri 2 entry point.
//
// One binary, two roles:
//   * `eggs`            -> launch the transparent overlay (GUI mode).
//   * `eggs <subcmd>`   -> mutate ~/.eggs/state.json (CLI mode) and exit.
//
// When a CLI invocation lands while the GUI is already running, the
// single-instance plugin forwards argv to the running process; the running
// process applies the change to state.json, and its file poller picks it up
// and notifies the webview via the "state-changed" event.
//
// Dev:    cargo install tauri-cli --version "^2"
//         cd desktop/src-tauri && cargo tauri dev
// Build:  cd desktop/src-tauri && cargo tauri build

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cli;
mod cli_install;
mod client;
mod peers;
mod pet;
mod pet_menu;
mod pid;
mod remote;
mod remote_assets;
mod state;
mod upload;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, LogicalPosition, LogicalSize, Manager, RunEvent};

use peers::{PeerInit, PeerWindowManager, SharedPeerWindowManager};

#[tauri::command]
fn list_pets() -> Result<Vec<pet::PetInfo>, String> {
    pet::list_installed_pets().map_err(|e| e.to_string())
}

#[tauri::command]
fn list_states() -> Vec<String> {
    pet_menu::list_states()
}

#[tauri::command]
fn load_pet(id: String) -> Result<pet::PetManifest, String> {
    pet::load_pet(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn read_state() -> Result<state::RuntimeState, String> {
    state::read_state().map_err(|e| e.to_string())
}

#[tauri::command]
fn write_state(state: state::RuntimeState) -> Result<(), String> {
    state::write_state(&state).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_scale(window: tauri::WebviewWindow, scale_millis: u16) -> Result<(), String> {
    let clamped = match scale_millis {
        400 | 500 | 600 | 800 | 1000 => scale_millis,
        _ => 1000,
    };
    let scale = (clamped as f64) / 1000.0;
    let width = 192.0 * scale;
    let height = 208.0 * scale;
    window
        .set_size(LogicalSize::new(width, height))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn read_remote_config() -> remote::RemoteConfig {
    remote::read_remote_config()
}

#[tauri::command]
fn write_remote_config(config: remote::RemoteConfig) -> Result<(), String> {
    remote::write_remote_config(&config).map_err(|e| e.to_string())
}

#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn show_context_menu(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    pet_menu::show_context_menu(&app, &window)
}

#[tauri::command]
async fn get_peer_init(
    state: tauri::State<'_, SharedPeerWindowManager>,
    peer_id: String,
) -> Result<PeerInit, String> {
    state
        .get_init(&peer_id)
        .await
        .ok_or_else(|| format!("unknown peer: {peer_id}"))
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();

    // CLI fast-path: any subcommand (including `start`, which forks a detached
    // GUI and exits) goes through cli::run_subcommand. Only a bare `eggs` (no
    // args) or the explicit `eggs run` foreground form falls through to the
    // tauri::Builder below.
    if let Some(sub) = argv.get(1).map(String::as_str) {
        if sub != "run" && !sub.is_empty() {
            std::process::exit(cli::run_subcommand(&argv));
        }
    }

    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let shutdown_for_setup = shutdown_flag.clone();
    let shutdown_for_run = shutdown_flag.clone();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // Second invocation of `eggs <subcmd>` arrives here. The CLI
            // sub-command is just a state.json / remote.json mutation; the
            // GUI's pollers will re-emit "state-changed" / "remote-status"
            // automatically.
            cli::run_in_running_instance(app, argv);
        }))
        .invoke_handler(tauri::generate_handler![
            list_pets,
            list_states,
            load_pet,
            read_state,
            write_state,
            set_scale,
            read_remote_config,
            write_remote_config,
            quit,
            show_context_menu,
            get_peer_init,
        ])
        .setup(move |app| {
            // Best-effort: if launched from a packaged install (.app on macOS,
            // Program Files on Windows), wire `eggs` into a shell-visible
            // location so the user can run subcommands from the terminal
            // without manually managing PATH. Idempotent and silent on
            // success; never blocks GUI launch.
            cli_install::auto_install();

            // Publish our PID so `eggs stop` (and other tooling) can find us.
            // Stale PIDs from crashy exits are tolerated: stop falls back to
            // `kill -0` to detect liveness before sending SIGTERM.
            if let Err(e) = pid::write_self() {
                eprintln!("warning: could not write eggs.pid: {e}");
            }

            let win = app
                .get_webview_window("pet")
                .expect("pet window is configured in tauri.conf.json");

            if let Ok(current) = state::read_state() {
                let clamped = match current.scale_millis {
                    400 | 500 | 600 | 800 | 1000 => current.scale_millis,
                    _ => 1000,
                };
                let scale = (clamped as f64) / 1000.0;
                let width = 192.0 * scale;
                let height = 208.0 * scale;
                let _ = win.set_size(LogicalSize::new(width, height));
                if let Some((x, y)) = initial_pet_position(app, width, height) {
                    let _ = win.set_position(LogicalPosition::new(x, y));
                }
            }

            app.manage(pet_menu::PetMenuStore::new());

            // Peer window manager — owns transparent per-peer overlays. Shared
            // between the remote actor (which feeds it peer snapshots), the
            // state poller (scale changes), the pet-window move listener
            // (drag → reposition), and the get_peer_init command.
            let peer_windows: SharedPeerWindowManager = Arc::new(PeerWindowManager::new());
            app.manage(peer_windows.clone());
            // Seed the manager's cached scale from disk so the first peer that
            // arrives is built at the right size, even before any user-driven
            // scale change.
            if let Ok(current) = state::read_state() {
                let app_handle = win.app_handle().clone();
                let mgr = peer_windows.clone();
                let scale_ms = current.scale_millis;
                tauri::async_runtime::spawn(async move {
                    mgr.apply_scale(&app_handle, scale_ms).await;
                });
            }

            // Poll ~/.eggs/state.json and emit changes to the webview. When
            // scale_millis changes, also push the new scale to peer windows
            // (resize + reposition + emit `peer-scale` for peer.js).
            let win_for_poller = win.clone();
            let app_handle_for_poller = win.app_handle().clone();
            let peer_windows_for_poller = peer_windows.clone();
            std::thread::spawn(move || {
                let mut last: Option<state::RuntimeState> = None;
                loop {
                    if let Ok(current) = state::read_state() {
                        if last.as_ref() != Some(&current) {
                            let scale_changed = last
                                .as_ref()
                                .map(|l| l.scale_millis != current.scale_millis)
                                .unwrap_or(true);
                            let _ = win_for_poller.emit("state-changed", &current);
                            if scale_changed {
                                let app = app_handle_for_poller.clone();
                                let mgr = peer_windows_for_poller.clone();
                                let scale_ms = current.scale_millis;
                                tauri::async_runtime::spawn(async move {
                                    mgr.apply_scale(&app, scale_ms).await;
                                });
                            }
                            last = Some(current);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            });

            // Drag tracking: when the local pet window moves, anchor every
            // open peer window to the new position. `Moved` fires per OS
            // notification (~60Hz on macOS during a drag); each spawn is a
            // fire-and-forget set_position per peer.
            let app_handle_for_move = win.app_handle().clone();
            let peer_windows_for_move = peer_windows.clone();
            win.on_window_event(move |event| {
                if let tauri::WindowEvent::Moved(_) = event {
                    let app = app_handle_for_move.clone();
                    let mgr = peer_windows_for_move.clone();
                    tauri::async_runtime::spawn(async move {
                        mgr.reposition_all(&app).await;
                    });
                }
            });

            // Spawn the remote-multiplayer actor (ws + reconnect + heartbeats).
            remote::start(app.handle().clone(), shutdown_for_setup.clone(), peer_windows);

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build eggs desktop");

    app.run(move |_app, event| {
        if let RunEvent::MenuEvent(menu_event) = &event {
            pet_menu::handle_menu_event(_app, menu_event.id());
        }
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            shutdown_for_run.store(true, Ordering::Relaxed);
            pid::clear();
        }
    });
}

fn initial_pet_position(app: &tauri::App, _window_width: f64, _window_height: f64) -> Option<(f64, f64)> {
    // Anchor at the top-left of the primary work area: peer windows are
    // stacked to the right with bottoms aligned (see peers::position_for_peer),
    // so a top-left anchor leaves the entire screen width available for peers
    // and keeps everything on-screen at any scale factor.
    let monitor = app.primary_monitor().ok().flatten()?;
    let work_area = monitor.work_area();
    let margin = 20.0;
    let x = work_area.position.x as f64 + margin;
    let y = work_area.position.y as f64 + margin;
    Some((x, y))
}
