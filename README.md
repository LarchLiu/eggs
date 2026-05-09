# Eggs

Eggs is a desktop pet app with a Tauri 2 runtime (`desktop/`) and a standalone Go remote server (`server/`).

Current primary runtime is the `eggs` desktop binary built from `desktop/src-tauri`.  
The old Python runtime under `eggs/scripts/egg_desktop.py` is now legacy-compatible, not the main path.

## What The Desktop App Does

- Runs a transparent always-on-top pet window.
- Supports multiple pet assets (`pet.json` + spritesheet).
- Supports runtime state switching (`idle`, `running-right`, `running-left`, `waving`, `jumping`, `failed`, `waiting`, `running`, `review`).
- Supports multiplayer remote sessions (random pool or invite room).
- Shows peer windows and local/remote chat bubbles.
- Supports local hook bubble ingestion via `eggs hook <text>`.

## Quick Start

Build and run the desktop binary:

```bash
./desktop/dev fast
./desktop/src-tauri/target/release-fast/eggs
```

Or run through wrapper helpers:

```bash
./desktop/dev run
./desktop/dev run remote
```

Common helper targets:

```bash
./desktop/dev fast
./desktop/dev release
./desktop/dev check
./desktop/dev clean
./desktop/dev test
```

## CLI (Desktop Binary)

The same binary is both GUI and CLI:

```bash
eggs
eggs run
eggs start
eggs stop
eggs restart
eggs status
eggs list
eggs pet <source> <id>
eggs state <name>
eggs install <pet-dir>
eggs hook "<text>"
eggs message "<text>"
eggs remote help
eggs uninstall-cli
```

Notes:

- `eggs start` launches detached and writes PID to `~/.eggs/eggs.pid`.
- CLI mutations are file-driven (`state.json` / `remote.json`) and picked up by the running GUI via polling + single-instance forwarding.
- First GUI launch from packaged builds attempts best-effort CLI install into PATH (platform-specific).

## Remote Multiplayer

Remote state is stored in `~/.eggs/remote.json` and defaults to:

- `server_url`: `http://localhost:8787`
- `mode`: `random`
- `room_limit`: `5`

Key commands:

```bash
eggs remote
eggs remote random
eggs remote room <code> [limit]
eggs remote leave
eggs remote off
eggs remote server <url>
eggs remote status
eggs remote upload [pet_id]
```

Behavior highlights:

- `remote` and `remote on` keep the saved mode/room.
- `remote room <code> [limit]` persists invite room mode and cap.
- `remote random` switches mode without clearing saved room code.
- Pet switch in remote mode gates local change on successful upload to keep local/peer view consistent.
- `eggs pet <source> <id>` targets an exact pet source: `builtin`, `local`, or `remote`.
- Upload is source-aware (`pet_source + pet`) while peer downloads are content-addressed by `content_id` from server-provided asset URLs.
- The detailed upload/download protocol is documented in `desktop/README.md` under `Remote Asset Flow`.

## Codex Hook Integration

Project hook scripts are in:

- `.codex/hooks.json`
- `.codex/hooks/*.py`

These scripts emit local desktop bubbles by calling:

```bash
eggs hook "<text>"
```

So if hooks are enabled in your Codex environment, hook events can be visualized as pet bubbles.

## Data Directory

Runtime data is in `~/.eggs/` (or `EGGS_APP_DIR` override):

- `state.json`: current pet, pet source, state, scale, window position
- `client.json`: device identity
- `remote.json`: remote config
- `eggs.pid`: detached GUI pid
- `pets/<id>/`: installed pets
- `remote/`: cached peer sprite assets and blobs
- `bubble-spool/`: queued local bubble events

Pet lookup priority:

1. `EGGS_PETS_DIR` (if set, exclusive)
2. `~/.eggs/pets`
3. `$CODEX_HOME/pets` or `~/.codex/pets`
4. Remote cache under `~/.eggs/remote`

## Pet Asset Format

Each pet folder:

```text
<pet-id>/
  pet.json
  spritesheet.webp  # or png
```

`pet.json` fields used by desktop runtime:

- `id`
- `displayName`
- `description` (optional)
- `spritesheetPath`

## Remote Server

Go server lives in `server/`:

```bash
cd server
go build -o eggs-server .
./eggs-server -addr :8787 -data ~/.codex/eggs-server -base-url http://localhost:8787
```

It uses pure-Go SQLite (`modernc.org/sqlite`) and does not require system SQLite shared libraries on target hosts.

## Packaging

For app bundles:

```bash
cd desktop/src-tauri
cargo tauri build
```

Outputs are under `desktop/src-tauri/target/release/bundle/` (`.dmg`, `.msi`, `.deb`, `.AppImage`, etc, depending on platform/toolchain).

## Repository Layout

```text
desktop/     Tauri app (GUI + CLI binary)
server/      Go remote backend
eggs/        Skill assets, scripts, tools, compatibility wrappers
assets/      Source art and helper assets
scripts/     Release helpers
```

## Legacy Notes

- `eggs/scripts/egg_desktop.py` remains useful for compatibility and tooling, but feature development is centered on `desktop/src-tauri`.
- Existing legacy `state.json` fields like `sprite` are still accepted by the desktop runtime (`sprite` aliases to `pet`).
