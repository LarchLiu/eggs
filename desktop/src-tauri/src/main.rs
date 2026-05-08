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
mod bubbles;
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
use tauri::{Emitter, LogicalSize, Manager, PhysicalPosition, RunEvent, WebviewWindow};

use bubbles::{BubbleInit, BubbleWindowManager, SharedBubbleWindowManager};
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
fn read_hatched_pets() -> Result<Vec<String>, String> {
    state::read_hatch_state()
        .map(|s| s.completed)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn mark_pet_hatched(pet_id: String) -> Result<(), String> {
    state::mark_pet_hatched(&pet_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn queue_hatch_finish_state(state: String) {
    remote::queue_hatch_finish_state(&state);
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

fn disable_remote_before_exit() {
    // Ensure next launch starts with remote mode disabled, so users must opt in
    // explicitly from the menu / CLI.
    if let Err(e) = remote::update_remote_config(|cfg| {
        cfg.enabled = false;
        cfg.session_nonce = 0;
    }) {
        eprintln!("warning: could not disable remote mode during shutdown: {e}");
    }
}

#[tauri::command]
fn show_context_menu(app: tauri::AppHandle, window: tauri::WebviewWindow) -> Result<(), String> {
    pet_menu::show_context_menu(&app, &window)
}

#[tauri::command]
async fn get_peer_init(
    state: tauri::State<'_, SharedPeerWindowManager>,
    device_id: String,
) -> Result<PeerInit, String> {
    state
        .get_init(&device_id)
        .await
        .ok_or_else(|| format!("unknown peer: {device_id}"))
}

#[tauri::command]
async fn get_bubble_init(
    state: tauri::State<'_, SharedBubbleWindowManager>,
    bubble_id: String,
) -> Result<BubbleInit, String> {
    state
        .get_init(&bubble_id)
        .await
        .ok_or_else(|| format!("unknown bubble: {bubble_id}"))
}

#[tauri::command]
async fn bubble_hover(
    state: tauri::State<'_, SharedBubbleWindowManager>,
    bubble_id: String,
    hovering: bool,
) -> Result<(), String> {
    state.set_hover(&bubble_id, hovering).await;
    Ok(())
}

#[tauri::command]
async fn bubble_dismiss(
    state: tauri::State<'_, SharedBubbleWindowManager>,
    app: tauri::AppHandle,
    bubble_id: String,
) -> Result<(), String> {
    state.dismiss_chat(&app, &bubble_id).await;
    Ok(())
}

#[tauri::command]
fn bubble_constraints() -> bubbles::BubbleConstraints {
    bubbles::constraints()
}

#[tauri::command]
async fn bubble_resize(
    state: tauri::State<'_, SharedBubbleWindowManager>,
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    bubble_id: String,
    height: f64,
) -> Result<(), String> {
    let (min, max) = state.allowed_height_range(&bubble_id).await;
    let clamped = height.clamp(min, max);
    window
        .set_size(LogicalSize::new(bubbles::BUBBLE_WIDTH, clamped))
        .map_err(|e| e.to_string())?;
    state.set_height(&bubble_id, clamped).await;
    state.reposition_all(&app).await;
    Ok(())
}

#[tauri::command]
async fn open_local_input(
    state: tauri::State<'_, SharedBubbleWindowManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    state.open_local_input(&app).await;
    Ok(())
}

#[tauri::command]
async fn cancel_local_input(
    state: tauri::State<'_, SharedBubbleWindowManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    state.cancel_local_input(&app).await;
    Ok(())
}

#[tauri::command]
async fn send_local_input(
    state: tauri::State<'_, SharedBubbleWindowManager>,
    app: tauri::AppHandle,
    text: String,
) -> Result<(), String> {
    let remote = remote::read_remote_config();
    let room_code = if remote.mode == "room" && !remote.room.trim().is_empty() {
        Some(remote.room.trim().to_string())
    } else {
        None
    };

    let Some(event) = bubbles::build_local_user_event(&text, room_code.clone()) else {
        // Empty after trim — just dismiss the input.
        state.cancel_local_input(&app).await;
        return Ok(());
    };

    state.close_local_input(&app).await;
    state.show(&app, event.clone()).await;

    if remote.enabled {
        if let Err(e) = bubbles::enqueue_chat_outbox(&event.text, room_code.clone()) {
            eprintln!("warning: could not enqueue chat outbox: {e}");
        }
    }

    if let Some(room) = room_code {
        let history = bubbles::ChatHistoryEntry {
            id: event.id.clone(),
            source: bubbles::BubbleSource::UserInput,
            owner: bubbles::HistoryOwner::Local,
            text: event.text.clone(),
            created_ms: event.created_ms,
            device_id: None,
        };
        if let Err(e) = bubbles::append_room_history(&room, history) {
            eprintln!("warning: could not append room history: {e}");
        }
    }

    Ok(())
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
            read_hatched_pets,
            mark_pet_hatched,
            queue_hatch_finish_state,
            set_scale,
            read_remote_config,
            write_remote_config,
            quit,
            show_context_menu,
            get_peer_init,
            get_bubble_init,
            bubble_hover,
            bubble_dismiss,
            bubble_constraints,
            bubble_resize,
            open_local_input,
            cancel_local_input,
            send_local_input,
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
                if let Some((x, y)) = saved_or_initial_pet_position(app, &current, width, height) {
                    let _ = win.set_position(PhysicalPosition::new(x, y));
                }
            }

            app.manage(pet_menu::PetMenuStore::new());

            // Peer window manager — owns transparent per-peer overlays. Shared
            // between the remote actor (which feeds it peer snapshots), the
            // state poller (scale changes), the pet-window move listener
            // (drag → reposition), and the get_peer_init command.
            let peer_windows: SharedPeerWindowManager = Arc::new(PeerWindowManager::new());
            app.manage(peer_windows.clone());
            let bubble_windows: SharedBubbleWindowManager = Arc::new(BubbleWindowManager::new());
            app.manage(bubble_windows.clone());
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
            let bubble_windows_for_poller = bubble_windows.clone();
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
                                let bubbles = bubble_windows_for_poller.clone();
                                let scale_ms = current.scale_millis;
                                tauri::async_runtime::spawn(async move {
                                    mgr.apply_scale(&app, scale_ms).await;
                                    bubbles.reposition_all(&app).await;
                                });
                            }
                            last = Some(current);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            });

            // Poll one-shot bubble spool files, open bubble overlays, and keep
            // them anchored / expired. This decouples CLI producers (`eggs hook`,
            // `eggs message`) from GUI lifecycle and supports concurrent bubbles.
            let app_handle_for_bubbles = win.app_handle().clone();
            let bubble_windows_for_spool = bubble_windows.clone();
            std::thread::spawn(move || loop {
                match bubbles::drain_bubble_spool() {
                    Ok(items) => {
                        for evt in items {
                            let app = app_handle_for_bubbles.clone();
                            let mgr = bubble_windows_for_spool.clone();
                            tauri::async_runtime::spawn(async move {
                                mgr.show(&app, evt).await;
                            });
                        }
                    }
                    Err(e) => eprintln!("bubble spool read failed: {e}"),
                }
                let app = app_handle_for_bubbles.clone();
                let mgr = bubble_windows_for_spool.clone();
                tauri::async_runtime::spawn(async move {
                    mgr.expire_due(&app).await;
                });
                std::thread::sleep(Duration::from_millis(200));
            });

            // Drag tracking: when the local pet window moves, anchor every
            // open peer window to the new position. `Moved` fires per OS
            // notification (~60Hz on macOS during a drag); each spawn is a
            // fire-and-forget set_position per peer.
            let app_handle_for_move = win.app_handle().clone();
            let peer_windows_for_move = peer_windows.clone();
            let bubble_windows_for_move = bubble_windows.clone();
            win.on_window_event(move |event| {
                if let tauri::WindowEvent::Moved(_) = event {
                    let app = app_handle_for_move.clone();
                    let mgr = peer_windows_for_move.clone();
                    let bubbles = bubble_windows_for_move.clone();
                    tauri::async_runtime::spawn(async move {
                        mgr.reposition_all(&app).await;
                        bubbles.reposition_all(&app).await;
                    });
                }
            });

            // Spawn the remote-multiplayer actor (ws + reconnect + heartbeats).
            remote::start(
                app.handle().clone(),
                shutdown_for_setup.clone(),
                peer_windows,
                bubble_windows.clone(),
            );

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build eggs desktop");

    app.run(move |_app, event| {
        if let RunEvent::MenuEvent(menu_event) = &event {
            pet_menu::handle_menu_event(_app, menu_event.id());
        }
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            if let Some(mgr) = _app.try_state::<SharedBubbleWindowManager>() {
                let app_handle = _app.clone();
                let mgr = mgr.inner().clone();
                tauri::async_runtime::spawn(async move {
                    mgr.close_all(&app_handle).await;
                });
            }
            if let Some(window) = _app.get_webview_window("pet") {
                persist_window_position(&window);
            }
            disable_remote_before_exit();
            shutdown_for_run.store(true, Ordering::Relaxed);
            pid::clear();
        }
    });
}

fn saved_or_initial_pet_position(
    app: &tauri::App,
    current: &state::RuntimeState,
    window_width: f64,
    window_height: f64,
) -> Option<(i32, i32)> {
    match (current.window_x, current.window_y) {
        (Some(x), Some(y)) => Some((x, y)),
        _ => initial_pet_position(app, window_width, window_height),
    }
}

fn persist_window_position(window: &WebviewWindow) {
    if let Ok(position) = window.outer_position() {
        let _ = state::set_window_position(position.x, position.y);
    }
}

fn initial_pet_position(
    app: &tauri::App,
    _window_width: f64,
    _window_height: f64,
) -> Option<(i32, i32)> {
    // Anchor at the top-left of the primary work area: peer windows are
    // stacked to the right with bottoms aligned (see peers::position_for_peer),
    // so a top-left anchor leaves the entire screen width available for peers
    // and keeps everything on-screen at any scale factor.
    //
    // work_area returns physical pixels; we keep physical throughout so the
    // saved/restored round-trip stays correct on Retina (scale_factor != 1).
    let monitor = app.primary_monitor().ok().flatten()?;
    let work_area = monitor.work_area();
    let margin = 20;
    Some((work_area.position.x + margin, work_area.position.y + margin))
}
