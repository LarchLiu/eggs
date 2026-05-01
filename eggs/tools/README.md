# Sprite Tools

Small Swift tools for preparing and installing the sprite sheets used by the desktop companion skill.

These are source tools, not portable binaries. Compile them locally on macOS when needed.

## Build

```bash
mkdir -p .swift-module-cache
CLANG_MODULE_CACHE_PATH="$PWD/.swift-module-cache" \
swiftc -module-cache-path "$PWD/.swift-module-cache" \
  eggs/tools/extract_sprite.swift \
  -o /tmp/extract_sprite

CLANG_MODULE_CACHE_PATH="$PWD/.swift-module-cache" \
swiftc -module-cache-path "$PWD/.swift-module-cache" \
  eggs/tools/merge_spritesheets.swift \
  -o /tmp/merge_spritesheets
```

## Extract One Sheet

For bordered grid sheets:

```bash
/tmp/extract_sprite assets/input.png assets/sprites/output --prefix output
```

For borderless regular grid sheets:

```bash
/tmp/extract_sprite assets/input.png assets/sprites/output \
  --grid uniform \
  --columns 5 \
  --rows 6 \
  --prefix output
```

Useful options:

- `--frame-size 251`: force every output frame into a 251x251 canvas.
- `--name dino`: override output names to `dino.png` and `dino.json`.
- `--align preserve-cell`: default for animation; preserves source cell positioning.
- `--align center-content`: useful for icons, not usually for animation.

If `--name` is omitted, extraction writes `<input-name>_spritesheet.png` and `<input-name>_spritesheet.json`.

## Merge Extracted Sheets

After extracting multiple sheets into regular spritesheets with JSON metadata:

```bash
/tmp/merge_spritesheets assets/sprites/combined --name dino \
  assets/sprites/state_a/dino.json \
  assets/sprites/state_b/dino-extra.json
```

The merge tool stacks sources vertically, keeps the same column count, and centers smaller source frames into the maximum frame size.
It writes `<name>.png` and `<name>.json` into the output directory.
If `--name` is omitted, merge writes `<output-dir-name>_spritesheet.png` and `<output-dir-name>_spritesheet.json`.
Both extraction and merge also copy generated PNG/JSON files to `~/.codex/eggs/` by default.
The `eggs` runtime reads `<sprite>.json` for `frameWidth` and `frameHeight`, so keep the JSON next to the PNG when installing or copying a sheet manually.

## Inspection Helpers

```bash
swiftc eggs/tools/check_sprite.swift -o /tmp/check_sprite
swiftc eggs/tools/bounds_sprite.swift -o /tmp/bounds_sprite
```

`check_sprite` reports edge/background residue. `bounds_sprite` prints alpha bounding boxes.
