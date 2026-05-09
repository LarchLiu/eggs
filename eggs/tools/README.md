# Sprite Tools

Small Swift tools for preparing and installing the sprite sheets used by the desktop companion skill.

These are source tools, not portable binaries. Compile them locally on macOS when needed.

## Build

```bash
./eggs/tools/build_tools.sh
```

This compiles the main tools into `~/.eggs/bin/`:

- `~/.eggs/bin/extract_sprite`
- `~/.eggs/bin/merge_spritesheets`
- `~/.eggs/bin/resize_crop_frames`

Helper tools are optional:

```bash
./eggs/tools/build_tools.sh --with-helpers
```

That also installs:

- `~/.eggs/bin/bounds_sprite`
- `~/.eggs/bin/check_sprite`

Optional:

```bash
./eggs/tools/build_tools.sh --dest /tmp/eggs-bin --with-helpers
```

## Extract One Sheet

For bordered grid sheets:

```bash
~/.eggs/bin/extract_sprite assets/input.png assets/sprites/output --prefix output
```

For borderless regular grid sheets:

```bash
~/.eggs/bin/extract_sprite assets/input.png assets/sprites/output \
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
Extracted frame files are named as `<prefix>_<row>x<column>.png`, for example `dino_00x04.png`.

## Merge Extracted Sheets

After extracting multiple sheets into regular spritesheets with JSON metadata:

```bash
~/.eggs/bin/merge_spritesheets assets/sprites/combined --name dino \
  assets/sprites/state_a/dino.json \
  assets/sprites/state_b/dino-extra.json
```

The merge tool stacks sources vertically, keeps the same column count, and centers smaller source frames into the maximum frame size.
It writes `<name>.png` and `<name>.json` into the output directory.
If `--name` is omitted, merge writes `<output-dir-name>_spritesheet.png` and `<output-dir-name>_spritesheet.json`.
Both extraction and merge also copy generated PNG/JSON files to `~/.codex/eggs/` by default.
The `eggs` runtime reads `<sprite>.json` for `frameWidth` and `frameHeight`, so keep the JSON next to the PNG when installing or copying a sheet manually.

## Resize Extracted Frames

For a `frames/` directory already cut by `extract_sprite`, generate a resized/cropped frame set and repack it into a transparent spritesheet:

```bash
~/.eggs/bin/resize_crop_frames assets/sprites/egg_hatch/frames 160x120 --out dino
```

Useful options:

- `--x 10`: pack the output spritesheet using 10 columns per row. Default: 8.
- `--out dino`: write output into a sibling `dino/` directory. Default: `frames-<width>x<height>`.
- `--format webp`: export the combined spritesheet as `spritesheet.webp` instead of `spritesheet.png`.
- `64`: shorthand for a square target, equivalent to `64x64`.

The tool writes processed frames into the output directory, generates `pet.json`, and writes the combined spritesheet as `spritesheet.png` or `spritesheet.webp`.
It also installs the generated pet package into `~/.eggs/pets/<out>/`.
The generated `pet.json` is intentionally minimal and does not include per-frame dimensions; the desktop runtime treats atlas layout as a fixed contract.
Each source frame is scaled using its longest edge first, then center-cropped into the target size. Empty cells in the last row stay transparent.
When the source frames come from a smaller original column count such as `5`, using `--x 8` keeps each source row intact and pads columns `6-8` with transparency on every row.

## Inspection Helpers

```bash
~/.eggs/bin/check_sprite path/to/spritesheet.png
~/.eggs/bin/bounds_sprite path/to/spritesheet.png
```

`check_sprite` reports edge/background residue. `bounds_sprite` prints alpha bounding boxes.
