---
name: eggs
description: Spawn, stop, change state, or manage a standalone animated 2D desktop sprite companion. Use when the user invokes `/eggs`, asks for an animated desktop companion, wants a roaming sprite character, or asks to stop/status/restart/change the companion process.
---

# Eggs

## Quick Start

The skill ships with a tiny launcher (`./eggs` on macOS/Linux, `eggs.cmd` on Windows) that downloads the right pre-built binary on first use and caches it at `~/.eggs/bin/eggs`. No Python, npm, or compiler required at runtime.

When the user asks to spawn the desktop companion, including `/eggs`, run the launcher from this skill directory:

```bash
./eggs start            # macOS / Linux
.\eggs.cmd start        # Windows (PowerShell or cmd)
```

When the user asks to stop it:

```bash
./eggs stop
```

For status:

```bash
./eggs status
```

For restart (stop + start):

```bash
./eggs restart
```

Remote interaction is opt-in. To connect this skill to a separately deployed remote sprite server:

```bash
./eggs remote server http://localhost:8787
./eggs remote upload dino
./eggs remote
./eggs remote status
```

`remote` enables remote using the saved `~/.eggs/remote.json` config and also brings the GUI up if it isn't running. If the saved config is `mode=room` with a non-empty room code, it rejoins that room; otherwise it falls back to random matchmaking. After a random match is found, the server creates a temporary private room for that pair.

For invite rooms:

```bash
./eggs remote room ABC123
```

To leave the current room/pair while keeping remote enabled:

```bash
./eggs remote leave
```

To disable remote interaction entirely:

```bash
./eggs remote off
```

For state changes:

```bash
./eggs state idle
./eggs state running-right
./eggs state running-left
./eggs state waving
./eggs state jumping
./eggs state failed
./eggs state waiting
./eggs state running
./eggs state review
```

To switch active pet (folder name under `~/.eggs/pets/` or the legacy `~/.codex/pets/`):

```bash
./eggs pet noir-webling
```

To install a pet folder (must contain `pet.json` plus a spritesheet) into `~/.eggs/pets/`:

```bash
./eggs install /path/to/pet-dir
```

To uninstall the CLI shim that the GUI's first launch placed in `/usr/local/bin/` or the user PATH:

```bash
./eggs uninstall-cli
```

## Sprite Tools

Use the bundled Swift tools in `tools/` when asked to process, extract, validate, or merge desktop companion sprite sheets. When tools are run with `--name <sprite>`, they write `<sprite>.png` and `<sprite>.json` to the requested output directory and also install copies to `~/.eggs/<sprite>.png` and `~/.eggs/<sprite>.json`. If extraction is run without `--name`, use the input image stem and write `<input-name>_spritesheet.png/json`.

Build tools into a temporary location instead of committing platform-specific binaries:

```bash
mkdir -p .swift-module-cache
CLANG_MODULE_CACHE_PATH="$PWD/.swift-module-cache" \
swiftc -module-cache-path "$PWD/.swift-module-cache" \
  eggs/tools/extract_sprite.swift \
  -o /tmp/extract_sprite
```

Extract a bordered grid:

```bash
/tmp/extract_sprite <input.png> <output-dir> --prefix <name>
```

Extract a borderless regular grid:

```bash
/tmp/extract_sprite <input.png> <output-dir> \
  --grid uniform \
  --columns <n> \
  --rows <n> \
  --prefix <name>
```

Force multiple source sheets into a common frame canvas:

```bash
/tmp/extract_sprite <input.png> <output-dir> --frame-size 251 --prefix <name>
```

Merge extracted sheets vertically:

```bash
CLANG_MODULE_CACHE_PATH="$PWD/.swift-module-cache" \
swiftc -module-cache-path "$PWD/.swift-module-cache" \
  eggs/tools/merge_spritesheets.swift \
  -o /tmp/merge_spritesheets

/tmp/merge_spritesheets <output-dir> [--name <sprite>] <sheet-a.json> <sheet-b.json>
```

Validation helpers:

```bash
swiftc eggs/tools/check_sprite.swift -o /tmp/check_sprite
swiftc eggs/tools/bounds_sprite.swift -o /tmp/bounds_sprite
```

## Runtime Behavior

- The skill has no Python or compiler dependency at runtime. The launcher (`./eggs` / `eggs.cmd`) is POSIX shell / cmd; it downloads a single self-contained Tauri binary on first use.
- Override the download source via `EGGS_RELEASE_URL` (defaults to the project's GitHub Releases) and the cache directory via `EGGS_BIN_DIR` (defaults to `~/.eggs/bin`).
- Cached binary is re-checked against the release's `SHA256SUMS` periodically (default every 600 s; override with `EGGS_VERIFY_INTERVAL` in seconds, or `EGGS_SKIP_VERIFY=1` to always trust the cache for offline / CI use). The launcher records the server's expected hash at download time (`~/.eggs/bin/eggs.sha256`) and on subsequent launches just compares the stored line to a freshly fetched `SHA256SUMS` — no local sha tool required. Mismatch / missing record triggers a re-download; an inconclusive check (no network, asset missing from sums) falls back to the cache without bumping the verify marker.
- The binary doubles as GUI and CLI: `eggs` (no args) launches the foreground transparent overlay, `eggs <subcmd>` mutates `~/.eggs/state.json` / `remote.json` and exits. The single-instance plugin forwards `eggs <subcmd>` invocations to a running GUI; the GUI's pollers re-emit `state-changed` / `remote-status` automatically.
- `eggs start` forks a detached background GUI and exits with its PID; `eggs stop` SIGTERMs the running GUI (SIGKILL after 3s); `eggs restart` is stop + start.
- Re-running `start` is idempotent — if a GUI is already running, it prints `eggs is already running (pid N)` instead of duplicating.
- Runtime data lives at `~/.eggs/` (Windows: `C:\Users\<n>\.eggs\`). Override via `EGGS_APP_DIR`. State, remote config, device id, PID file, cached peer assets all share that one folder.
- Pets live at `~/.eggs/pets/<id>/` (each with `pet.json` + a spritesheet). The runtime also still reads `~/.codex/pets/<id>/` for backward compatibility with the legacy Python skill.
- The transparent always-on-top window is 192x208 (8x9 atlas with 192x208 cells per the Codex pet contract); the user can rescale via the right-click context menu (0.4x / 0.5x / 0.6x / 0.8x / 1.0x). Peer windows on screen mirror the local scale and follow the local pet during drag.
- Remote interaction is opt-in. Settings live in `~/.eggs/remote.json` (`server_url`, `enabled`, `mode`, `room`, `session_nonce`), anonymous device identity in `~/.eggs/client.json`, and downloaded peer assets cache to `~/.eggs/remote/<content_id>/` with shared blob files under `~/.eggs/remote/blobs/`.
- `remote` / `remote on` preserve the saved `mode` and `room` in `remote.json`; `remote random` switches only the mode and keeps any saved room code for later reuse.
- `mode=room` only stays in room mode when `room` is non-empty; an empty room code automatically falls back to `random`.
- `remote leave` bumps `session_nonce` to force a reconnect cycle (leave room/pair) without flipping `enabled` to `false`.
- Hash-skip upload: when remote is enabled and the user switches pet, the client first POSTs `(sprite_hash, json_hash)` only; the server replies 200 if the row already exists, 201 if the blobs already exist server-side (a fresh row gets registered for this device, zero bytes shipped), or 404 listing missing blobs (client retries phase 2 with bytes).
- The remote Go server is not part of the installed skill; it lives at the repository root under `server/` and should be deployed separately.
- The launcher itself (`./eggs` / `eggs.cmd`) is committed to the skill; the binary it caches is not. The launcher reuses any `eggs` already on `$PATH` (e.g. when the user installed the standalone Eggs.app GUI) before falling back to download.

## Notes For Agents

If the user types `/eggs` or asks to spawn the companion, run `./eggs start` immediately (use `eggs.cmd` on Windows) and briefly report whether it launched. If the user asks to change companion state, run the `state` command with the closest matching state name. Do not open or explain the launcher unless launch fails.

The first `./eggs` invocation downloads ~10 MB and caches it; report progress to the user only if the download takes more than a few seconds.

`./eggs start` and `./eggs restart` must be launched from an unsandboxed shell. On macOS the GUI has to attach to WindowServer (and on Linux to the X / Wayland session); if the parent shell blocks those desktop / IPC services, the process still spawns and a PID file gets written — `./eggs status` will report it as running — but the transparent window never reaches the screen. In Claude Code that means setting `dangerouslyDisableSandbox: true` on the Bash tool call that runs `start` / `restart`; in other harnesses, use the equivalent unsandboxed mode. Pure state-mutation commands (`state`, `pet`, `remote`, `install`, `stop`, `status`) are safe to run sandboxed since they only read or write JSON under `~/.eggs/`.
