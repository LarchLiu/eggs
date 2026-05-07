// CLI sub-command dispatcher.
//
// Subcommands always mutate ~/.eggs/state.json (or filesystem under
// ~/.eggs/pets/) and exit with a status code. They never block waiting for
// the GUI -- the GUI polls state.json and reacts asynchronously.

use std::path::PathBuf;

pub fn run_subcommand(argv: &[String]) -> i32 {
    let cmd = argv.get(1).map(String::as_str).unwrap_or("");
    match cmd {
        "state" => match argv.get(2) {
            Some(name) => match crate::state::set_state(name) {
                Ok(()) => {
                    println!("state -> {name}");
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            },
            None => {
                eprintln!("usage: eggs state <name>");
                eprintln!("  e.g. eggs state idle | running-right | waving");
                2
            }
        },
        "pet" => match argv.get(2) {
            Some(id) => match crate::state::set_pet(id) {
                Ok(()) => {
                    println!("pet -> {id}");
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            },
            None => {
                eprintln!("usage: eggs pet <id>");
                2
            }
        },
        "list" => match crate::pet::list_installed_pets() {
            Ok(pets) if pets.is_empty() => {
                let dirs = crate::pet::pet_search_dirs();
                let display = if dirs.is_empty() {
                    "(no pets directories configured)".to_string()
                } else {
                    dirs.iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                println!("no pets installed (searched: {display})");
                println!("hint: use the hatch-pet skill or `eggs install <dir>`.");
                0
            }
            Ok(pets) => {
                let local: Vec<_> = pets.iter().filter(|p| !p.remote).collect();
                let remote: Vec<_> = pets.iter().filter(|p| p.remote).collect();
                let has_local = !local.is_empty();
                if !local.is_empty() {
                    println!("Local");
                    for p in &local {
                        println!("{}\t{}", p.id, p.display_name);
                    }
                }
                if !remote.is_empty() {
                    if has_local {
                        println!();
                    }
                    println!("Remote");
                    for p in &remote {
                        println!("{}\t{}", p.id, p.display_name);
                    }
                }
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
        "install" => match argv.get(2) {
            Some(src) => match install_pet(src) {
                Ok(dest) => {
                    println!("installed pet -> {}", dest.display());
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            },
            None => {
                eprintln!("usage: eggs install <pet-dir>");
                eprintln!("  expects <pet-dir>/pet.json + <pet-dir>/<spritesheet>");
                2
            }
        },
        "status" => match crate::state::read_state() {
            Ok(s) => {
                println!("pet={} state={}", s.pet, s.state);
                println!("file={}", crate::state::state_path().display());
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
        "remote" => run_remote_subcommand(argv),
        "start" => run_start_subcommand(),
        "stop" => run_stop_subcommand(),
        "restart" => run_restart_subcommand(),
        "uninstall-cli" => run_uninstall_cli_subcommand(),
        "help" | "-h" | "--help" => {
            print_help();
            0
        }
        other => {
            eprintln!("unknown subcommand: {other}");
            print_help();
            2
        }
    }
}

/// Called by the single-instance plugin when a second invocation lands while
/// the GUI is running. State changes are picked up automatically by the
/// state.json poller, so we just dispatch and discard the exit code.
pub fn run_in_running_instance(_app: &tauri::AppHandle, argv: Vec<String>) {
    let _ = run_subcommand(&argv);
}

fn install_pet(src: &str) -> std::io::Result<PathBuf> {
    let src_path = PathBuf::from(src);
    if !src_path.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{src} is not a directory"),
        ));
    }
    let manifest_text = std::fs::read_to_string(src_path.join("pet.json"))?;
    let manifest: crate::pet::PetManifest = serde_json::from_str(&manifest_text)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let dest = crate::pet::primary_pets_dir().join(&manifest.id);
    std::fs::create_dir_all(&dest)?;
    std::fs::copy(src_path.join("pet.json"), dest.join("pet.json"))?;
    let sheet_src = src_path.join(&manifest.spritesheet_path);
    let sheet_dst = dest.join(&manifest.spritesheet_path);
    std::fs::copy(&sheet_src, &sheet_dst)?;
    Ok(dest)
}

fn print_help() {
    eprintln!("eggs — animated desktop pet (Codex pet contract)");
    eprintln!();
    eprintln!("usage:");
    eprintln!("  eggs                  launch the desktop pet inline (foreground)");
    eprintln!("  eggs run              same as `eggs` (foreground GUI)");
    eprintln!("  eggs start            fork a detached background GUI and exit,");
    eprintln!("                        printing its pid (alias of egg_desktop.py:start)");
    eprintln!("  eggs stop             SIGTERM the running GUI (SIGKILL after 3s)");
    eprintln!("  eggs restart          stop + start (matches egg_desktop.py)");
    eprintln!("  eggs uninstall-cli    remove the symlink / PATH entry created on first");
    eprintln!("                        GUI launch (does not touch ~/.eggs/ or the app)");
    eprintln!("  eggs state <name>     switch animation state");
    eprintln!("                        (idle, running-right, running-left,");
    eprintln!("                         waving, jumping, failed, waiting,");
    eprintln!("                         running, review)");
    eprintln!("  eggs pet <id>         switch active pet (folder name under");
    eprintln!("                        ~/.eggs/pets/ or ~/.codex/pets/)");
    eprintln!("  eggs list             list installed pets");
    eprintln!("  eggs install <dir>    copy <dir>/{{pet.json,spritesheet.webp}}");
    eprintln!("                        into ~/.eggs/pets/<id>/");
    eprintln!("  eggs status           show current pet + state");
    eprintln!("  eggs remote ...       multiplayer (see `eggs remote help`)");
    eprintln!("  eggs help             show this help");
}

// ---------- stop subcommand --------------------------------------------

/// Mirrors egg_desktop.py:start_background — fork a detached no-arg `eggs`
/// child that drops into the GUI branch of `main`, print the PID, exit.
/// Returns 0 if a GUI was already running so scripts can call `eggs start`
/// idempotently.
fn run_start_subcommand() -> i32 {
    if let Some(pid) = crate::pid::read() {
        if crate::pid::is_alive(pid) {
            println!("eggs is already running (pid {pid})");
            return 0;
        }
        crate::pid::clear();
    }
    match spawn_detached_self() {
        Ok(child_pid) => {
            println!("spawned eggs (pid {child_pid})");
            0
        }
        Err(e) => {
            eprintln!("error: could not spawn eggs: {e}");
            1
        }
    }
}

/// Mirrors egg_desktop.py:stop_background — read `<app_dir>/eggs.pid`,
/// SIGTERM the GUI, escalate to SIGKILL if it doesn't exit within 3s, and
/// clear the PID file. Returns 0 even if no GUI was running, matching the
/// Python "is not running" path.
fn run_stop_subcommand() -> i32 {
    let pid = match crate::pid::read() {
        Some(p) => p,
        None => {
            println!("eggs is not running");
            return 0;
        }
    };
    if !crate::pid::is_alive(pid) {
        crate::pid::clear();
        println!("eggs is not running (cleared stale pid {pid})");
        return 0;
    }
    let stopped = crate::pid::terminate(pid, std::time::Duration::from_secs(3));
    crate::pid::clear();
    if stopped {
        println!("stopped eggs (pid {pid})");
        0
    } else {
        eprintln!("warning: pid {pid} still alive after SIGKILL");
        1
    }
}

// ---------- uninstall-cli subcommand -----------------------------------

/// Stop-then-start. Matches egg_desktop.py:restart so SKILL.md keeps a
/// single name across the Python/Rust transition. Best-effort stop: if the
/// previous GUI wasn't running, just go straight to start.
fn run_restart_subcommand() -> i32 {
    let _ = run_stop_subcommand();
    // Brief breather so the OS recycles window handles / single-instance lock
    // before the new GUI tries to claim them. 250ms is empirically enough on
    // macOS / Linux; harmless on Windows.
    std::thread::sleep(std::time::Duration::from_millis(250));
    run_start_subcommand()
}

/// Reverses the auto-CLI-install that runs on first GUI launch. Removes only
/// our own symlink / PATH entry; leaves `~/.eggs/` and the app bundle alone.
/// Always returns 0 — "nothing to remove" is success, not an error.
fn run_uninstall_cli_subcommand() -> i32 {
    let removed = crate::cli_install::run_uninstall();
    if removed.is_empty() {
        println!("nothing to uninstall (no CLI residue found)");
    } else {
        for line in removed {
            println!("{line}");
        }
    }
    0
}

// ---------- remote subcommand ------------------------------------------

fn run_remote_subcommand(argv: &[String]) -> i32 {
    let action = argv.get(2).map(String::as_str).unwrap_or("");
    let value = argv.get(3).map(String::as_str);
    match action {
        "" | "random" => {
            let pet_id = match resolve_pet_for_upload() {
                Ok(id) => id,
                Err(code) => return code,
            };
            if let Err(code) = run_pet_upload(&pet_id, /*quiet=*/ true) {
                return code;
            }
            match crate::remote::update_remote_config(|cfg| {
                cfg.enabled = true;
                cfg.mode = "random".to_string();
                cfg.room.clear();
            }) {
                Ok(cfg) => {
                    ensure_gui_running();
                    println!(
                        "remote random match pool enabled (server={}, sprite={})",
                        cfg.server_url, pet_id
                    );
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        "room" => {
            let Some(code_value) = value else {
                eprintln!("usage: eggs remote room <code>");
                return 2;
            };
            let code = code_value.trim();
            if code.is_empty() {
                eprintln!("room code cannot be empty");
                return 2;
            }
            let pet_id = match resolve_pet_for_upload() {
                Ok(id) => id,
                Err(code) => return code,
            };
            if let Err(code) = run_pet_upload(&pet_id, /*quiet=*/ true) {
                return code;
            }
            match crate::remote::update_remote_config(|cfg| {
                cfg.enabled = true;
                cfg.mode = "room".to_string();
                cfg.room = code.to_string();
            }) {
                Ok(cfg) => {
                    ensure_gui_running();
                    println!(
                        "remote room enabled: {} (server={}, sprite={})",
                        cfg.room, cfg.server_url, pet_id
                    );
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        "leave" => {
            let current = crate::remote::read_remote_config();
            if !current.enabled {
                println!("remote is disabled; no room to leave");
                return 0;
            }
            match crate::remote::leave_room() {
                Ok(_) => {
                    println!("left room (remote remains enabled)");
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        "on" => match crate::remote::update_remote_config(|cfg| cfg.enabled = true) {
            Ok(cfg) => {
                println!(
                    "remote interaction enabled (mode={}, server={})",
                    cfg.mode, cfg.server_url
                );
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
        "off" => match crate::remote::update_remote_config(|cfg| cfg.enabled = false) {
            Ok(_) => {
                println!("remote interaction disabled");
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
        "server" => {
            let Some(url) = value else {
                eprintln!("usage: eggs remote server <url>");
                return 2;
            };
            let trimmed = url.trim_end_matches('/').to_string();
            match crate::remote::update_remote_config(|cfg| cfg.server_url = trimmed.clone()) {
                Ok(cfg) => {
                    println!("remote server set to {}", cfg.server_url);
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        "status" => {
            let cfg = crate::remote::read_remote_config();
            let room_display = if cfg.room.is_empty() {
                "-"
            } else {
                cfg.room.as_str()
            };
            // Sprite isn't stored in remote.json anymore; surface state.json::pet
            // here so `eggs remote status` still tells the user which pet would be
            // announced on connect.
            let sprite = crate::state::read_state()
                .map(|s| s.pet)
                .unwrap_or_default();
            let sprite_display = if sprite.is_empty() {
                "-"
            } else {
                sprite.as_str()
            };
            println!(
                "remote enabled={} server={} mode={} room={} sprite={}",
                cfg.enabled, cfg.server_url, cfg.mode, room_display, sprite_display
            );
            0
        }
        "upload" => {
            let pet_id = match value.map(str::to_string) {
                Some(id) if !id.is_empty() => id,
                _ => match resolve_pet_for_upload() {
                    Ok(id) => id,
                    Err(code) => return code,
                },
            };
            match run_pet_upload(&pet_id, /*quiet=*/ false) {
                Ok(()) => 0,
                Err(code) => code,
            }
        }
        "help" | "-h" | "--help" => {
            print_remote_help();
            0
        }
        other => {
            eprintln!("unknown remote action '{other}'");
            print_remote_help();
            2
        }
    }
}

/// Fork a detached, no-arg `eggs` child that lands in the GUI branch of
/// `main`. Returns the child PID so callers can decide whether to print
/// `spawned …` (eggs start) or keep quiet (eggs remote auto-launch).
fn spawn_detached_self() -> std::io::Result<u32> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // New process group so the child survives the calling shell's SIGHUP.
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }

    let child = cmd.spawn()?;
    Ok(child.id())
}

/// Spawn a detached GUI if one isn't already running.
/// Mirrors `egg_desktop.py`'s `apply_remote_runtime_change(ensure_running=True)`:
/// `eggs remote` / `eggs remote room` should bring up the pet window when it
/// isn't on screen yet. The single-instance plugin in any already-running GUI
/// catches duplicates, so the PID-file check is just a fast-path that avoids
/// the spawn-and-immediate-exit churn.
fn ensure_gui_running() {
    if let Some(pid) = crate::pid::read() {
        if crate::pid::is_alive(pid) {
            return;
        }
        crate::pid::clear();
    }
    if let Err(e) = spawn_detached_self() {
        eprintln!("warning: could not spawn pet GUI: {e}");
    }
}

/// Pick the pet id to upload from state.json. state.json is the single source
/// of truth for the active pet — what the GUI is rendering is what we want
/// peers to see. remote.json no longer carries a sprite field.
fn resolve_pet_for_upload() -> Result<String, i32> {
    if let Ok(s) = crate::state::read_state() {
        if !s.pet.is_empty() {
            return Ok(s.pet);
        }
    }
    eprintln!(
        "no pet configured. set one with `eggs pet <id>` after installing one under ~/.eggs/pets/"
    );
    Err(2)
}

fn run_pet_upload(pet_id: &str, quiet: bool) -> Result<(), i32> {
    let remote = crate::remote::read_remote_config();
    let device_id = match crate::client::read_client_config() {
        Ok(c) => c.device_id,
        Err(e) => {
            eprintln!("error: cannot read client.json: {e}");
            return Err(1);
        }
    };
    if !quiet {
        println!(
            "uploading pet '{pet_id}' to {} (device_id={device_id})...",
            remote.server_url
        );
    }
    match crate::upload::ensure_pet_uploaded_blocking(&remote.server_url, &device_id, pet_id) {
        Ok(outcome) => {
            match outcome.mode {
                crate::upload::UploadMode::Reused => {
                    if !quiet {
                        println!(
                            "pet '{pet_id}' already registered for this device (sprite_id={})",
                            outcome.sprite_id
                        );
                    }
                }
                crate::upload::UploadMode::HashRegistered => {
                    println!(
                        "pet '{pet_id}' bytes already on server; registered new row (sprite_id={}) -- no upload needed",
                        outcome.sprite_id
                    );
                }
                crate::upload::UploadMode::BytesUploaded => {
                    println!("uploaded pet '{pet_id}' (sprite_id={})", outcome.sprite_id);
                }
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("upload failed: {e}");
            Err(1)
        }
    }
}

fn print_remote_help() {
    eprintln!("eggs remote — multiplayer with the Go server (server/main.go)");
    eprintln!();
    eprintln!("usage:");
    eprintln!("  eggs remote                 enable random match pool (alias of `random`)");
    eprintln!("  eggs remote random          enable random match pool");
    eprintln!("  eggs remote room <code>     enable room mode with invite code");
    eprintln!("  eggs remote leave           leave current room/pair, keep remote enabled");
    eprintln!("  eggs remote on              enable using current mode/room");
    eprintln!("  eggs remote off             disable without changing mode/room");
    eprintln!("  eggs remote server <url>    set base http(s) URL of the Go server");
    eprintln!("  eggs remote status          print remote.json snapshot");
    eprintln!("  eggs remote upload [<id>]   force re-upload of <id> (default: current pet)");
    eprintln!();
    eprintln!("notes:");
    eprintln!("  * `random` and `room` auto-upload the current pet first; the server");
    eprintln!("    skips body upload when (sprite_hash, json_hash) already match (~1 RTT).");
    eprintln!("  * `random` and `room` also bring up the pet GUI if it isn't running");
    eprintln!("    (detached child process; deduped by the single-instance plugin).");
    eprintln!();
    eprintln!("config files (legacy compatible with egg_desktop.py):");
    eprintln!("  ~/.eggs/state.json    pet + animation state (source of truth for the active pet)");
    eprintln!("  ~/.eggs/client.json   anonymous device_id (auto-generated)");
    eprintln!("  ~/.eggs/remote.json   server_url, enabled, mode, room");
}
