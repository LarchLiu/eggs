# Eggs Desktop (Tauri 2)

Rust + Tauri 2 port of [`eggs/scripts/egg_desktop.py`](../eggs/scripts/egg_desktop.py). Single self-contained binary that doubles as GUI (`eggs`) and CLI (`eggs <subcmd>`); when launched as a packaged app it also wires itself into the user's PATH so terminal subcommands work without manual setup.

## Build

`desktop/dev` (bash) and `desktop/dev.ps1` (PowerShell) wrap the common cargo invocations:

```bash
./desktop/dev fast        # release-fast profile (default; seconds-fast incremental)
./desktop/dev release     # full LTO release
./desktop/dev check       # cargo check, fastest
./desktop/dev clean
./desktop/dev run remote  # build + ./target/release-fast/eggs remote
./desktop/dev stop        # whatever binary is built → eggs stop
./desktop/dev restart remote room ABCD
./desktop/dev test        # cargo check + go test ./... in server/
```

For full release bundles (.dmg / .msi / .deb / .AppImage), use Tauri's bundler:

```bash
cd desktop/src-tauri && cargo tauri build
```

Cross-platform packaging (Mac → Win, Mac → Linux) is painful locally — use CI (`tauri-action` on GitHub Actions) for releases.

## Distribution

The compiled binary at `target/release/eggs` is **OS- and arch-specific**, not cross-platform; build per target.

Bundle outputs land under `target/release/bundle/`:

| Platform | Bundle artifact | Where the binary ends up after install |
|---|---|---|
| macOS | `bundle/dmg/Eggs_*_aarch64.dmg` (or `x86_64`) | `/Applications/Eggs.app/Contents/MacOS/eggs` |
| Windows | `bundle/nsis/Eggs_*_setup.exe` or `bundle/msi/*.msi` | `C:\Program Files\Eggs\eggs.exe` |
| Linux | `bundle/deb/*.deb`, `bundle/appimage/*.AppImage`, `bundle/rpm/*.rpm` | `/usr/bin/eggs`, AppImage FUSE mount, or `/usr/bin/eggs` |

### Auto CLI-install on first GUI launch

When the GUI starts from a packaged install location, [`cli_install::auto_install`](src-tauri/src/cli_install.rs) runs once during Tauri setup to make `eggs` invokable from a shell without manual PATH editing. **Best-effort, idempotent, silent on success — never blocks GUI launch.**

| Platform | Action |
|---|---|
| **macOS** (`.app/Contents/MacOS/`) | `ln -sf` to `/usr/local/bin/eggs`; falls back to `~/.local/bin/eggs` on permission denied. |
| **Windows** (anywhere outside `\target\`) | Append the install dir to user-level PATH via `reg add HKCU\Environment\Path`. New shells pick it up. |
| **Linux**, AppImage | `ln -sf $APPIMAGE` (the durable file, not the FUSE mount path) into `~/.local/bin/eggs`. |
| **Linux**, portable tarball | `ln -sf` to `~/.local/bin/eggs`. |
| **Linux**, `.deb` / `.rpm` | Skipped — package manager already wrote `/usr/bin/eggs`. |
| **Dev builds** (`target/...`, `cargo run`) | Skipped to avoid polluting system PATH with throw-away binaries. |

The hop is one-shot and idempotent: on subsequent launches, the symlink (or PATH entry) already exists and points at the current binary, so the function returns within microseconds. No admin / sudo required on any platform.

## Uninstall

The auto CLI-install in the previous section writes outside the app bundle, so a "drag to trash" / "MSI uninstall" doesn't fully clean up. The easy path:

```bash
eggs stop            # 1. shut down the GUI (uses ~/.eggs/eggs.pid)
eggs uninstall-cli   # 2. remove the symlink / PATH entry written on first launch
                     #    (does not touch the app bundle or ~/.eggs/)
```

Then drag `Eggs.app` to trash (macOS) / run the MSI/NSIS uninstaller (Windows) / `apt remove` or `rm -f Eggs.AppImage` (Linux). Optionally `rm -rf ~/.eggs/` if you don't plan to reinstall.

### What `eggs uninstall-cli` actually does

Symmetric to [`cli_install::auto_install`](src-tauri/src/cli_install.rs); only removes things it can identify as ours.

| Platform | Action | Conservative check |
|---|---|---|
| macOS | `rm` `/usr/local/bin/eggs` and `~/.local/bin/eggs` | only when the symlink target points at our `current_exe`, an `.app/Contents/MacOS/eggs`, or is dangling |
| Windows | `reg add HKCU\Environment\Path` with our dir filtered out | exact (case-insensitive) match against `current_exe.parent()` |
| Linux | same as macOS, plus | accepts `$APPIMAGE` matches and `*.AppImage` targets |

If you ever symlinked `/usr/local/bin/eggs` yourself to point at some other binary, `eggs uninstall-cli` will **leave it alone** — so it's safe to run more than once or after partial cleanup.

### Manual fallback

If the binary is already gone (you uninstalled the app first) and `eggs uninstall-cli` isn't reachable any more, do it by hand:

```bash
# macOS / Linux
rm -f /usr/local/bin/eggs ~/.local/bin/eggs
```

```powershell
# Windows (PowerShell, user scope, no admin)
$current = [Environment]::GetEnvironmentVariable('Path', 'User')
$cleaned = ($current -split ';') | Where-Object { $_ -and $_ -ne 'C:\Program Files\Eggs' }
[Environment]::SetEnvironmentVariable('Path', ($cleaned -join ';'), 'User')
```

User data lives at `~/.eggs/` (macOS / Linux) or `C:\Users\<name>\.eggs\` (Windows) — wipe that folder if you're certain you won't reinstall.

### Verify

```bash
which eggs               # should print nothing (or "eggs not found")
ls -la /usr/local/bin/eggs ~/.local/bin/eggs 2>/dev/null
```

A leftover dangling symlink isn't dangerous (just `ENOENT` when invoked). If you reinstall later, the next GUI launch silently rewrites the symlink / PATH entry — no manual re-bootstrap needed.

## CLI

`eggs help` prints the canonical list. Headlines:

- `eggs` / `eggs run` — launch foreground GUI (blocks current shell).
- `eggs start` / `eggs stop` — fork detached GUI / SIGTERM the running one (matches `egg_desktop.py:start_background` / `stop_background`).
- `eggs pet <source> <id>` — switch active pet; in remote mode also pushes that exact source-group sprite to the server before broadcasting to peers.
- `eggs list` shows user-installed pets and any built-in pets that a given build ships with.
- `eggs remote` — enable remote using the saved `remote.json` mode/room; when `mode=room` but `room` is empty it falls back to random matchmaking.
- `eggs remote room <code> [limit]` — save room mode with an invite code and optional room cap (default `5`), upload the sprite if needed, and ensure the GUI is running. The first client to create a room locks its limit; later joiners cannot change it.
- `eggs remote random` — switch to random mode without clearing any saved room code from `remote.json`.
- `eggs remote leave` — leave current room/pair while keeping remote enabled.
- `eggs remote off` — disable remote without changing saved mode/room.
- `eggs install <pet-dir>` — copy a pet folder into `~/.eggs/pets/`.
- `eggs status` / `eggs list`.

When a build ships built-in pets, they are synced into `~/.eggs/builtin/<id>/` using SHA-256 content comparison, and the runtime loads built-in assets from that synced directory. When `state.json` is first created (or has an empty `pet` field), the default pet is chosen by priority: `Built-in` first, then `Local`, then `Remote`; ties within the same group fall back to lexical `id` order. The active pet in `state.json` is stored as `pet` plus `pet_source`, and CLI switches use `eggs pet <source> <id>` (`builtin`, `local`, or `remote`).

GUI-side runtime mutation (state, pet, scale, …) is forwarded to the running GUI by `tauri-plugin-single-instance`, so CLI subcommands don't need to talk to a separate sidecar process.

## Remote Asset Flow

Remote sync is split into two separate paths:

1. Upload local pet assets to the server so peers can reference them.
2. Download peer asset URLs from the server into a local content-addressed cache.

### Upload path

The active local pet is resolved from `state.json` as `pet_source + pet`, not just by `id`. That means `eggs pet builtin kebo-a` and `eggs pet local kebo-a` can upload different asset directories even when they share the same pet id.

Upload uses `POST /api/v1/sprites` in two phases:

1. Hash-only probe:
   - form fields: `device_id`, `sprite_name`, `display_name`, `sprite_hash`, `json_hash`
   - no file parts yet
2. Full upload only when needed:
   - same fields as above
   - multipart file parts: `sprite` and `json`

Server responses:

- `200 OK`: this `(device_id, sprite_name, hashes)` row already exists
- `201 Created`: blobs already existed server-side; server registers a fresh row for this device
- `404 {"missing":[...]}`: client must retry with bytes

The server stores the uploaded metadata as a sprite record with fields including:

- `id`
- `content_id`
- `owner_device_id`
- `name`
- `display_name`
- `sprite_url`
- `json_url`
- `config_url`
- `sprite_hash`
- `json_hash`

`content_id` is derived from the uploaded hashes and becomes the stable asset identifier used by peers.

### Download path

After both peers connect to `/ws`, the server does not push binary data through WebSocket. Instead it broadcasts peer presence messages such as:

- `room_snapshot`
- `peer_joined`
- `peer_state`
- `peer_sprite_changed`

Each message includes:

- `device_id`
- optional `state`
- `sprite` object

The client consumes these sprite fields:

- `sprite.name`
- `sprite.sprite_url`
- `sprite.json_url`
- `sprite.config_url`

The desktop extracts `<content_id>` from URLs like:

- `/assets/<content_id>/sprite.webp`
- `/assets/<content_id>/sprite.json`

and caches the peer assets under:

- `~/.eggs/remote/<content_id>/pet.json`
- `~/.eggs/remote/<content_id>/spritesheet.webp` (or `.png`)
- `~/.eggs/remote/<content_id>/config.json`

If those files already exist locally, download is skipped. So peer asset reuse is keyed by `content_id`, not by pet name.

## Data directory

All runtime data lives in a single per-user directory:

- macOS / Linux: `~/.eggs/`
- Windows: `C:\Users\<name>\.eggs\`

Override with the `EGGS_APP_DIR` environment variable (handy for tests).

| File | Purpose |
|---|---|
| `state.json` | Current `pet`, `pet_source`, animation `state`, `scale_millis` |
| `client.json` | Anonymous device id (auto-generated UUID v4) |
| `remote.json` | Server URL, enabled flag, saved mode (random / room), optional saved room code, saved `room_limit`, reconnect `session_nonce` |
| `eggs.pid` | Running GUI's PID, used by `eggs stop` |
| `pets/<id>/` | Installed pet manifests + spritesheets |
| `remote/blobs/`, `remote/<content_id>/` | Cached remote peer assets |

Pet folder layout (matching the Codex hatch-pet contract):

```
~/.eggs/pets/<pet_id>/
    pet.json                # display name, frame metadata, atlas filename
    spritesheet.webp        # or .png; 192×208 cells, row count inferred from image height
```

## Source layout

```
desktop/
├── dev                          # bash build wrapper
├── dev.ps1                      # PowerShell wrapper (Windows)
├── src/                         # webview frontend (HTML/JS/CSS, no bundler)
│   ├── index.html  pet.html
│   ├── pet.js      peer.js
│   └── style.css
└── src-tauri/
    ├── Cargo.toml               # release / release-fast profiles
    ├── tauri.conf.json
    ├── icons/  capabilities/
    └── src/
        ├── main.rs              # GUI entry + CLI fast-path dispatch
        ├── cli.rs               # subcommand handlers
        ├── cli_install.rs       # first-launch PATH wire-up (this README §)
        ├── state.rs   pid.rs    # runtime state, GUI PID file
        ├── pet.rs     pet_menu.rs
        ├── client.rs  upload.rs # device identity, hash-skip sprite upload
        ├── remote.rs  remote_assets.rs  # ws actor + cached peer assets
        └── peers.rs             # per-peer transparent overlay windows
```
