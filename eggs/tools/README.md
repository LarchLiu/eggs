# Egg Sprite Tools

Small Swift tools for preparing the sprite sheets used by the `eggs` skill.

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
- `--align preserve-cell`: default for animation; preserves source cell positioning.
- `--align center-content`: useful for icons, not usually for animation.

## Merge Extracted Sheets

After extracting multiple sheets into regular spritesheets with JSON metadata:

```bash
/tmp/merge_spritesheets assets/sprites/combined \
  assets/sprites/state_a/spritesheet.json \
  assets/sprites/state_b/spritesheet.json
```

The merge tool stacks sources vertically, keeps the same column count, and centers smaller source frames into the maximum frame size.
It writes `spritesheet.png` and `spritesheet.json` into the output directory.
The `eggs` runtime reads `spritesheet.json` for `frameWidth` and `frameHeight`, so keep the JSON next to the PNG when installing or copying a sheet.

## Inspection Helpers

```bash
swiftc eggs/tools/check_sprite.swift -o /tmp/check_sprite
swiftc eggs/tools/bounds_sprite.swift -o /tmp/bounds_sprite
```

`check_sprite` reports edge/background residue. `bounds_sprite` prints alpha bounding boxes.
