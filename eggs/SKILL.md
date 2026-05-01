---
name: eggs
description: Spawn, stop, change state, or manage a standalone animated 2D desktop sprite companion. Use when the user invokes `/eggs`, asks for an animated desktop companion, wants a roaming sprite character, or asks to stop/status/restart/change the companion process.
---

# Eggs

## Quick Start

When the user asks to spawn the desktop companion, including `/eggs`, run the bundled runtime from this skill directory:

```bash
python3 scripts/egg_desktop.py start
```

When the user asks to stop it:

```bash
python3 scripts/egg_desktop.py stop
```

For status:

```bash
python3 scripts/egg_desktop.py status
```

For restart:

```bash
python3 scripts/egg_desktop.py restart
```

For state changes:

```bash
python3 scripts/egg_desktop.py state unborn
python3 scripts/egg_desktop.py state ready
python3 scripts/egg_desktop.py state hatching
python3 scripts/egg_desktop.py state hatched
python3 scripts/egg_desktop.py state walk
python3 scripts/egg_desktop.py state sleep
python3 scripts/egg_desktop.py state eat
python3 scripts/egg_desktop.py state drink
python3 scripts/egg_desktop.py state play
python3 scripts/egg_desktop.py state roar
python3 scripts/egg_desktop.py state attack
```

To install a replacement spritesheet:

```bash
python3 scripts/egg_desktop.py spritesheet /path/to/spritesheet.png
python3 scripts/egg_desktop.py restart
```

## Sprite Tools

Use the bundled Swift tools in `tools/` when asked to process, extract, validate, or merge desktop companion sprite sheets.

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

/tmp/merge_spritesheets <output-dir> <sheet-a.json> <sheet-b.json>
```

Validation helpers:

```bash
swiftc eggs/tools/check_sprite.swift -o /tmp/check_sprite
swiftc eggs/tools/bounds_sprite.swift -o /tmp/bounds_sprite
```

## Runtime Behavior

- Use only the bundled `scripts/egg_desktop.py`; it has no third-party Python dependencies.
- On macOS, the manager compiles and launches the bundled native Swift/Cocoa overlay at first run. This requires `python3` and the macOS Swift compiler, but no npm, Electron, PyPI packages, or external assets.
- On non-macOS, the manager falls back to its Python/Tk runtime. If Tkinter is unavailable, report that the local Python build cannot display the fallback GUI.
- The script launches a detached local GUI process and stores its PID/log under `~/.codex/eggs/`.
- Re-running `start` should not create duplicates; use `restart` when the user wants a fresh companion.
- The runtime first looks for a user-installed spritesheet at `~/.codex/eggs/spritesheet.png` with optional `~/.codex/eggs/spritesheet.json`, then the bundled skill assets at `assets/spritesheet.png` and `assets/spritesheet.json`, then falls back to a simple procedural placeholder drawing.
- Resolve bundled assets relative to this installed skill directory; never rely on the original repo path or any `/Users/...` absolute path.
- Do not hardcode the frame size. The animation runtime reads `frameWidth` and `frameHeight` from `spritesheet.json` to slice the PNG and size the desktop window. It only falls back to 251x251 if metadata is missing or invalid.
- The bundled spritesheet currently has 251x251 frames in a 5x11 regular grid.
- `assets/spritesheet.json` keeps `image` as `spritesheet.png`, relative to the JSON file's own directory. Generated sprite metadata should stay portable in the same way.
- Each row is a state: `unborn`, `ready`, `hatching`, `hatched`, `walk`, `sleep`, `eat`, `drink`, `play`, `roar`, `attack`.
- Chinese state requests are supported through aliases such as `睡觉`, `吃鸡腿`, `喝水`, `玩耍`, `咆哮`, and `攻击`.
- The `state` command writes `~/.codex/eggs/state.txt`; running windows poll it and switch animation rows without restarting.
- The desktop window can be repositioned by dragging it with the mouse.
- Sprite preparation tools are bundled under `tools/`; do not rely on old root-level compiled binaries.

## Notes For Codex

If the user types `/eggs` or asks to spawn the companion, do the start action immediately and briefly report whether it launched. If the user asks to change companion state, run the `state` command with the closest matching state name. Do not open or explain the script unless launch fails.
