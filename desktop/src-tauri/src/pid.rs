// PID file for the running GUI, persisted at `<app_dir>/eggs.pid`
// (see `state::app_dir()` — defaults to `~/.eggs/`, overridable via
// `EGGS_APP_DIR`).
//
// Mirrors egg_desktop.py's read_pid / write_pid / managed_process_alive trio
// so `eggs stop` can find and SIGTERM the GUI process. The file is written
// during Tauri setup and intentionally NOT cleaned up on graceful exit:
// stale PIDs are detected by `is_alive`, matching the Python behavior and
// avoiding races with crashy exits.

use std::fs;
use std::io;

pub fn pid_path() -> std::path::PathBuf {
    crate::state::app_dir().join("eggs.pid")
}

pub fn write_self() -> io::Result<()> {
    let dir = crate::state::app_dir();
    fs::create_dir_all(&dir)?;
    fs::write(pid_path(), std::process::id().to_string())
}

pub fn read() -> Option<u32> {
    let text = fs::read_to_string(pid_path()).ok()?;
    text.trim().parse::<u32>().ok()
}

pub fn clear() {
    let _ = fs::remove_file(pid_path());
}

/// Returns true iff a process with the given PID is currently running.
/// Uses `kill -0` semantics: signal 0 doesn't deliver anything, just probes
/// for existence + permission. EPERM still means alive, ESRCH means gone.
#[cfg(unix)]
pub fn is_alive(pid: u32) -> bool {
    use std::process::{Command, Stdio};
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(windows)]
pub fn is_alive(pid: u32) -> bool {
    use std::process::{Command, Stdio};
    // tasklist /FI "PID eq <pid>" /NH prints exactly one line per match.
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .any(|l| l.contains(&pid.to_string())),
        Err(_) => false,
    }
}

/// Best-effort SIGTERM, then SIGKILL after the timeout. Returns true if the
/// process was reaped within the timeout (or wasn't alive to begin with).
#[cfg(unix)]
pub fn terminate(pid: u32, timeout: std::time::Duration) -> bool {
    use std::process::{Command, Stdio};
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if !is_alive(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let _ = Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    !is_alive(pid)
}

#[cfg(windows)]
pub fn terminate(pid: u32, _timeout: std::time::Duration) -> bool {
    use std::process::{Command, Stdio};
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    !is_alive(pid)
}
