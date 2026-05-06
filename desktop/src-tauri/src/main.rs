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
mod client;
mod peers;
mod pet;
mod pid;
mod remote;
mod remote_assets;
mod state;
mod upload;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager, RunEvent};

use peers::{PeerInit, PeerWindowManager, SharedPeerWindowManager};

#[tauri::command]
fn set_click_through(window: tauri::WebviewWindow, on: bool) -> Result<(), String> {
    window.set_ignore_cursor_events(on).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_pets() -> Result<Vec<pet::PetInfo>, String> {
    pet::list_installed_pets().map_err(|e| e.to_string())
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
            set_click_through,
            list_pets,
            load_pet,
            read_state,
            write_state,
            read_remote_config,
            write_remote_config,
            quit,
            get_peer_init,
        ])
        .setup(move |app| {
            // Publish our PID so `eggs stop` (and other tooling) can find us.
            // Stale PIDs from crashy exits are tolerated: stop falls back to
            // `kill -0` to detect liveness before sending SIGTERM.
            if let Err(e) = pid::write_self() {
                eprintln!("warning: could not write eggs.pid: {e}");
            }

            let win = app
                .get_webview_window("pet")
                .expect("pet window is configured in tauri.conf.json");

            // Default to click-through; the webview asks Rust to flip it off
            // while the user holds Cmd / Ctrl to drag.
            let _ = win.set_ignore_cursor_events(true);

            // Poll ~/.eggs/state.json and emit changes to the webview.
            let win_for_poller = win.clone();
            std::thread::spawn(move || {
                let mut last: Option<state::RuntimeState> = None;
                loop {
                    if let Ok(current) = state::read_state() {
                        if last.as_ref() != Some(&current) {
                            let _ = win_for_poller.emit("state-changed", &current);
                            last = Some(current);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            });

            // Peer window manager — owns transparent per-peer overlays. Shared
            // between the remote actor (which feeds it peer snapshots) and the
            // get_peer_init command (which the peer.html JS calls on load).
            let peer_windows: SharedPeerWindowManager = Arc::new(PeerWindowManager::new());
            app.manage(peer_windows.clone());

            // Spawn the remote-multiplayer actor (ws + reconnect + heartbeats).
            remote::start(app.handle().clone(), shutdown_for_setup.clone(), peer_windows);

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build eggs desktop");

    app.run(move |_app, event| {
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            shutdown_for_run.store(true, Ordering::Relaxed);
            pid::clear();
        }
    });
}
