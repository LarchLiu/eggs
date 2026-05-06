// CLI sub-command dispatcher.
//
// Subcommands always mutate ~/.codex/eggs/state.json (or filesystem under
// ~/.codex/pets/) and exit with a status code. They never block waiting for
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
                println!(
                    "no pets installed at {}",
                    crate::pet::pets_dir().display()
                );
                println!("hint: use the hatch-pet skill or `eggs install <dir>`.");
                0
            }
            Ok(pets) => {
                for p in pets {
                    println!("{}\t{}", p.id, p.display_name);
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
        "stop" => run_stop_subcommand(),
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
    let dest = crate::pet::pets_dir().join(&manifest.id);
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
    eprintln!("  eggs                  launch the desktop pet (GUI)");
    eprintln!("  eggs start            same as `eggs`");
    eprintln!("  eggs state <name>     switch animation state");
    eprintln!("                        (idle, running-right, running-left,");
    eprintln!("                         waving, jumping, failed, waiting,");
    eprintln!("                         running, review)");
    eprintln!("  eggs pet <id>         switch active pet (folder name under");
    eprintln!("                        ~/.codex/pets/)");
    eprintln!("  eggs list             list installed pets");
    eprintln!("  eggs install <dir>    copy <dir>/{{pet.json,spritesheet.webp}}");
    eprintln!("                        into ~/.codex/pets/<id>/");
    eprintln!("  eggs status           show current pet + state");
    eprintln!("  eggs stop             SIGTERM the running GUI (SIGKILL after 3s)");
    eprintln!("  eggs remote ...       multiplayer (see `eggs remote help`)");
    eprintln!("  eggs help             show this help");
}

// ---------- stop subcommand --------------------------------------------

/// Mirrors egg_desktop.py:stop_background — read ~/.codex/eggs/eggs.pid,
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
                cfg.sprite = pet_id.clone();
            }) {
                Ok(cfg) => {
                    ensure_gui_running();
                    println!(
                        "remote random match pool enabled (server={}, sprite={})",
                        cfg.server_url, cfg.sprite
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
                cfg.sprite = pet_id.clone();
            }) {
                Ok(cfg) => {
                    ensure_gui_running();
                    println!(
                        "remote room enabled: {} (server={}, sprite={})",
                        cfg.room, cfg.server_url, cfg.sprite
                    );
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        "leave" => match crate::remote::update_remote_config(|cfg| {
            cfg.enabled = false;
            cfg.mode = "random".to_string();
            cfg.room.clear();
        }) {
            Ok(_) => {
                println!("left remote interaction");
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
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
            let room_display = if cfg.room.is_empty() { "-" } else { cfg.room.as_str() };
            let sprite_display = if cfg.sprite.is_empty() {
                "-"
            } else {
                cfg.sprite.as_str()
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

/// Spawn a detached `eggs` (no-arg) GUI process if one isn't already running.
/// Mirrors `egg_desktop.py`'s `apply_remote_runtime_change(ensure_running=True)`:
/// the user's `eggs remote` / `eggs remote room` should also bring up the pet
/// window when it isn't yet on screen. The single-instance plugin in any
/// already-running GUI catches the duplicate and the spawned child exits, so
/// we don't need our own PID file for dedup.
fn ensure_gui_running() {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("warning: could not resolve current_exe to start pet GUI: {e}");
            return;
        }
    };
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

    if let Err(e) = cmd.spawn() {
        eprintln!("warning: could not spawn pet GUI: {e}");
    }
}

/// Pick the pet id to upload: prefer remote.json::sprite, fall back to
/// state.json::pet so a fresh `eggs remote random` works without prior config.
fn resolve_pet_for_upload() -> Result<String, i32> {
    let remote = crate::remote::read_remote_config();
    if !remote.sprite.is_empty() {
        return Ok(remote.sprite);
    }
    match crate::state::read_state() {
        Ok(s) if !s.pet.is_empty() => Ok(s.pet),
        _ => {
            eprintln!(
                "no pet configured. set one with `eggs pet <id>` after installing one under ~/.codex/pets/"
            );
            Err(2)
        }
    }
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
                    println!(
                        "uploaded pet '{pet_id}' (sprite_id={})",
                        outcome.sprite_id
                    );
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
    eprintln!("  eggs remote leave           disable remote interaction");
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
    eprintln!("  ~/.codex/eggs/client.json   anonymous device_id (auto-generated)");
    eprintln!("  ~/.codex/eggs/remote.json   server_url, enabled, mode, room, sprite");
}
