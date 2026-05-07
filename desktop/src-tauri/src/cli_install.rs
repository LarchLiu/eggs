// First-launch CLI shim.
//
// When the user installs Eggs.app via .dmg / NSIS / etc., the binary lives at
// `/Applications/Eggs.app/Contents/MacOS/eggs` (macOS) or
// `C:\Program Files\Eggs\eggs.exe` (Windows) — neither of which is in PATH.
// This module is called once during Tauri setup; on subsequent launches it
// is a cheap no-op (already-installed symlink / already-on-PATH).
//
// Behavior is best-effort and silent: failures log to stderr but never
// propagate, because GUI launch must not depend on filesystem-writing
// privileges.
//
// Skipped silently when current_exe doesn't look like an installed location
// (e.g. `target/release/eggs` from `cargo build`), so dev iteration doesn't
// pollute /usr/local/bin or the user's PATH.

use std::env;
use std::path::Path;

pub fn auto_install() {
    let exe = match env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };

    if !is_installed_location(&exe) {
        return;
    }

    #[cfg(target_os = "macos")]
    macos::install(&exe);

    #[cfg(target_os = "windows")]
    windows::install(&exe);

    #[cfg(target_os = "linux")]
    linux::install(&exe);
}

/// Reverse of [`auto_install`] — explicit, user-driven, verbose. Removes
/// only the symlink / PATH entry we recognize as our own (target points at
/// a current or stale Eggs binary). Touches neither the app bundle nor
/// `~/.eggs/`. Returns a list of human-readable lines describing what was
/// removed; empty when nothing was found.
pub fn run_uninstall() -> Vec<String> {
    let mut removed: Vec<String> = Vec::new();
    let exe = match env::current_exe() {
        Ok(p) => p,
        Err(_) => return removed,
    };

    #[cfg(target_os = "macos")]
    macos::uninstall(&exe, &mut removed);

    #[cfg(target_os = "windows")]
    windows::uninstall(&exe, &mut removed);

    #[cfg(target_os = "linux")]
    linux::uninstall(&exe, &mut removed);

    let _ = exe;
    removed
}

#[cfg(target_os = "macos")]
fn is_installed_location(exe: &Path) -> bool {
    // Only auto-install when running from a real .app bundle; ad-hoc copies
    // and `cargo build` artefacts shouldn't try to claim /usr/local/bin/eggs.
    exe.to_string_lossy().contains(".app/Contents/MacOS/")
}

#[cfg(target_os = "windows")]
fn is_installed_location(exe: &Path) -> bool {
    let s = exe.to_string_lossy().to_lowercase();
    // Skip cargo dev builds; otherwise trust the path.
    !s.contains("\\target\\")
}

#[cfg(target_os = "linux")]
fn is_installed_location(exe: &Path) -> bool {
    // Skip cargo dev builds. The .deb / .rpm /usr/bin path, the AppImage
    // FUSE mount under /tmp/.mount_*/, and a manual tarball under $HOME all
    // pass; the linux::install module decides what (if anything) to do.
    !exe.to_string_lossy().contains("/target/")
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn is_installed_location(_exe: &Path) -> bool {
    false
}

// ---------- macOS: symlink into a user-visible bin dir ----------

#[cfg(target_os = "macos")]
mod macos {
    use std::fs;
    use std::path::{Path, PathBuf};

    pub fn install(exe: &Path) {
        let home = match dirs::home_dir() {
            Some(h) => h,
            None => return,
        };
        // Priority order: /usr/local/bin is in the system default PATH
        // (Homebrew, /etc/paths). ~/.local/bin is the standard user-writable
        // fallback for users without admin / without Homebrew.
        let candidates = [
            PathBuf::from("/usr/local/bin/eggs"),
            home.join(".local/bin/eggs"),
        ];
        for target in &candidates {
            if try_symlink(exe, target) {
                return;
            }
        }
        eprintln!(
            "cli_install: no writable bin location for `eggs` symlink; \
             invoke /Applications/Eggs.app/Contents/MacOS/eggs directly"
        );
    }

    pub fn uninstall(exe: &Path, removed: &mut Vec<String>) {
        let candidates = candidate_targets();
        for target in &candidates {
            if try_unlink_ours(target, exe, removed) {
                continue;
            }
        }
    }

    fn candidate_targets() -> Vec<PathBuf> {
        let mut v = vec![PathBuf::from("/usr/local/bin/eggs")];
        if let Some(home) = dirs::home_dir() {
            v.push(home.join(".local/bin/eggs"));
        }
        v
    }

    /// Returns true if the symlink ends up correct (created or already
    /// pointing at us). False on permission error so the caller can try the
    /// next candidate.
    fn try_symlink(exe: &Path, target: &Path) -> bool {
        if let Ok(existing) = fs::read_link(target) {
            if existing == exe {
                return true;
            }
        }
        if let Some(parent) = target.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::remove_file(target);
        match std::os::unix::fs::symlink(exe, target) {
            Ok(()) => {
                eprintln!("cli_install: {} -> {}", target.display(), exe.display());
                true
            }
            Err(_) => false,
        }
    }

    /// Remove `target` only when it is a symlink we recognise as ours
    /// (current_exe match, looks-like-Eggs.app target, or dangling). A
    /// regular file or a symlink to some unrelated `eggs` binary is left
    /// alone. Returns true when something was removed.
    fn try_unlink_ours(target: &Path, current_exe: &Path, removed: &mut Vec<String>) -> bool {
        let link_target = match fs::read_link(target) {
            Ok(t) => t,
            Err(_) => return false, // not a symlink, or doesn't exist
        };
        if !looks_like_ours(&link_target, current_exe) {
            return false;
        }
        if fs::remove_file(target).is_ok() {
            removed.push(format!(
                "removed symlink {} (was -> {})",
                target.display(),
                link_target.display()
            ));
            true
        } else {
            false
        }
    }

    fn looks_like_ours(link_target: &Path, current_exe: &Path) -> bool {
        if link_target == current_exe {
            return true;
        }
        let s = link_target.to_string_lossy();
        if s.contains(".app/Contents/MacOS/eggs") {
            return true;
        }
        // Dangling symlink: target gone, almost certainly an old Eggs install
        // we set up before. Cleaning these up is the whole point.
        !link_target.exists()
    }
}

// ---------- Windows: append install dir to user PATH ----------

#[cfg(target_os = "windows")]
mod windows {
    use std::path::Path;
    use std::process::Command;

    /// Append the install dir to the user-level (HKCU\Environment\Path)
    /// PATH via the built-in `reg.exe`. New shells (cmd / PowerShell / WSL)
    /// pick it up; existing shells must restart. No admin required.
    /// We avoid a winreg crate dep — `reg.exe` ships with every Windows
    /// since NT.
    pub fn install(exe: &Path) {
        let Some(dir) = exe.parent() else { return };
        let dir = dir.to_string_lossy().into_owned();

        let current = read_user_path().unwrap_or_default();
        if current.split(';').any(|p| p.eq_ignore_ascii_case(&dir)) {
            return; // already on PATH
        }
        let new = if current.is_empty() {
            dir.clone()
        } else {
            format!("{};{}", current.trim_end_matches(';'), dir)
        };

        let status = Command::new("reg")
            .args([
                "add",
                "HKCU\\Environment",
                "/v",
                "Path",
                "/t",
                "REG_EXPAND_SZ",
                "/d",
                &new,
                "/f",
            ])
            .status();
        match status {
            Ok(s) if s.success() => {
                eprintln!("cli_install: added {dir} to user PATH (open a new shell to use `eggs`)");
            }
            _ => eprintln!("cli_install: failed to update user PATH"),
        }
    }

    pub fn uninstall(exe: &Path, removed: &mut Vec<String>) {
        let Some(dir) = exe.parent() else { return };
        let dir = dir.to_string_lossy().into_owned();

        let current = match read_user_path() {
            Some(p) => p,
            None => return,
        };
        let entries: Vec<&str> = current.split(';').filter(|p| !p.is_empty()).collect();
        let kept: Vec<&str> = entries
            .iter()
            .copied()
            .filter(|p| !p.eq_ignore_ascii_case(&dir))
            .collect();
        if kept.len() == entries.len() {
            return; // dir wasn't on user PATH; nothing to do
        }
        let new = kept.join(";");
        let status = Command::new("reg")
            .args([
                "add",
                "HKCU\\Environment",
                "/v",
                "Path",
                "/t",
                "REG_EXPAND_SZ",
                "/d",
                &new,
                "/f",
            ])
            .status();
        if matches!(status, Ok(s) if s.success()) {
            removed.push(format!(
                "removed {dir} from user PATH (open a new shell to refresh)"
            ));
        }
    }

    fn read_user_path() -> Option<String> {
        let output = Command::new("reg")
            .args(["query", "HKCU\\Environment", "/v", "Path"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Lines look like: "    Path    REG_EXPAND_SZ    <value>"
        for line in stdout.lines() {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("Path") {
                continue;
            }
            let after_name = trimmed.trim_start_matches("Path").trim_start();
            // Drop the type token (REG_SZ / REG_EXPAND_SZ / ...) and return
            // the remainder verbatim — values can contain spaces and `;`.
            if let Some((_type_token, value)) = after_name.split_once(char::is_whitespace) {
                return Some(value.trim().to_string());
            }
        }
        None
    }
}

// ---------- Linux: AppImage / portable tarball symlink ----------

#[cfg(target_os = "linux")]
mod linux {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// `.deb` / `.rpm` installs already drop `/usr/bin/eggs` (or
    /// `/usr/local/bin/eggs`), which is on PATH for every distro — nothing
    /// for us to do. The interesting cases are:
    ///
    ///   * **AppImage**: `current_exe()` lives in a transient `/tmp/.mount_*/`
    ///     FUSE mount that disappears when the AppImage exits. The durable
    ///     artifact is the `.AppImage` file itself, exposed via `$APPIMAGE`.
    ///   * **Portable tarball**: user extracted somewhere under `$HOME`.
    ///     Symlink `~/.local/bin/eggs` at the actual binary so future shells
    ///     pick it up.
    pub fn install(exe: &Path) {
        let exe_str = exe.to_string_lossy();
        if exe_str.starts_with("/usr/bin/") || exe_str.starts_with("/usr/local/bin/") {
            // Already in a system bin dir; package manager owns this entry.
            return;
        }

        let target_exe = std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .unwrap_or_else(|| exe.to_path_buf());

        let Some(home) = dirs::home_dir() else { return };
        // Try $HOME first (no sudo, ubiquitous on Linux), then /usr/local/bin
        // for the rare user who has it owned by their account.
        let candidates = [
            home.join(".local/bin/eggs"),
            PathBuf::from("/usr/local/bin/eggs"),
        ];
        for target in &candidates {
            if try_symlink(&target_exe, target) {
                return;
            }
        }
    }

    pub fn uninstall(exe: &Path, removed: &mut Vec<String>) {
        let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
        let candidates = candidate_targets();
        for target in &candidates {
            try_unlink_ours(target, exe, appimage.as_deref(), removed);
        }
    }

    fn candidate_targets() -> Vec<PathBuf> {
        let mut v = Vec::new();
        if let Some(home) = dirs::home_dir() {
            v.push(home.join(".local/bin/eggs"));
        }
        v.push(PathBuf::from("/usr/local/bin/eggs"));
        v
    }

    fn try_symlink(exe: &Path, target: &Path) -> bool {
        if let Ok(existing) = fs::read_link(target) {
            if existing == exe {
                return true;
            }
        }
        if let Some(parent) = target.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::remove_file(target);
        match std::os::unix::fs::symlink(exe, target) {
            Ok(()) => {
                eprintln!("cli_install: {} -> {}", target.display(), exe.display());
                true
            }
            Err(_) => false,
        }
    }

    /// Mirror of macos::try_unlink_ours. The "looks like ours" check accepts
    /// either current_exe or `$APPIMAGE` so AppImage installs clean up too.
    fn try_unlink_ours(
        target: &Path,
        current_exe: &Path,
        appimage: Option<&Path>,
        removed: &mut Vec<String>,
    ) {
        let link_target = match fs::read_link(target) {
            Ok(t) => t,
            Err(_) => return,
        };
        if !looks_like_ours(&link_target, current_exe, appimage) {
            return;
        }
        if fs::remove_file(target).is_ok() {
            removed.push(format!(
                "removed symlink {} (was -> {})",
                target.display(),
                link_target.display()
            ));
        }
    }

    fn looks_like_ours(link_target: &Path, current_exe: &Path, appimage: Option<&Path>) -> bool {
        if link_target == current_exe {
            return true;
        }
        if let Some(p) = appimage {
            if link_target == p {
                return true;
            }
        }
        // Heuristic: a symlink named `eggs` whose target ends in `eggs` and
        // either is dangling or contains an obvious AppImage / portable
        // marker. Conservative — refuses to nuke a user's own `eggs` symlink
        // pointing at some unrelated binary.
        if !link_target.exists() {
            return true;
        }
        let s = link_target.to_string_lossy();
        s.ends_with(".AppImage")
    }
}
