#!/usr/bin/env python3
"""Build a 9-row Codex-style atlas from custom strip images."""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

CELL_WIDTH = 192
CELL_HEIGHT = 208
COLUMNS = 8
ROWS = 9
ATLAS_WIDTH = CELL_WIDTH * COLUMNS
ATLAS_HEIGHT = CELL_HEIGHT * ROWS
LABEL_HEIGHT = 22
IMAGE_SUFFIXES = {".png", ".webp", ".jpg", ".jpeg"}


def parse_hex_color(value: str) -> tuple[int, int, int]:
    if len(value) != 7 or not value.startswith("#"):
        raise SystemExit(f"invalid chroma key: {value}")
    return tuple(int(value[index : index + 2], 16) for index in (1, 3, 5))


def color_distance(left: tuple[int, int, int], right: tuple[int, int, int]) -> float:
    return math.sqrt(sum((left[index] - right[index]) ** 2 for index in range(3)))


def remove_chroma_background(
    image: Image.Image,
    chroma_key: tuple[int, int, int],
    threshold: float,
) -> Image.Image:
    rgba = image.convert("RGBA")
    pixels = rgba.load()
    for y in range(rgba.height):
        for x in range(rgba.width):
            red, green, blue, alpha = pixels[x, y]
            if alpha and color_distance((red, green, blue), chroma_key) <= threshold:
                pixels[x, y] = (red, green, blue, 0)
    return rgba


def connected_components(image: Image.Image) -> list[dict[str, object]]:
    alpha = image.getchannel("A")
    width, height = image.size
    data = alpha.tobytes()
    visited = bytearray(width * height)
    components: list[dict[str, object]] = []

    for start, alpha_value in enumerate(data):
        if alpha_value <= 16 or visited[start]:
            continue

        stack = [start]
        visited[start] = 1
        pixels: list[int] = []
        min_x = width
        min_y = height
        max_x = 0
        max_y = 0

        while stack:
            current = stack.pop()
            pixels.append(current)
            x = current % width
            y = current // width
            min_x = min(min_x, x)
            min_y = min(min_y, y)
            max_x = max(max_x, x)
            max_y = max(max_y, y)

            if x > 0:
                neighbor = current - 1
                if not visited[neighbor] and data[neighbor] > 16:
                    visited[neighbor] = 1
                    stack.append(neighbor)
            if x + 1 < width:
                neighbor = current + 1
                if not visited[neighbor] and data[neighbor] > 16:
                    visited[neighbor] = 1
                    stack.append(neighbor)
            if y > 0:
                neighbor = current - width
                if not visited[neighbor] and data[neighbor] > 16:
                    visited[neighbor] = 1
                    stack.append(neighbor)
            if y + 1 < height:
                neighbor = current + width
                if not visited[neighbor] and data[neighbor] > 16:
                    visited[neighbor] = 1
                    stack.append(neighbor)

        components.append(
            {
                "pixels": pixels,
                "area": len(pixels),
                "bbox": (min_x, min_y, max_x + 1, max_y + 1),
                "center_x": (min_x + max_x + 1) / 2,
            }
        )

    return components


def component_group_image(
    source: Image.Image,
    components: list[dict[str, object]],
    padding: int = 4,
) -> Image.Image:
    width, height = source.size
    min_x = max(0, min(component["bbox"][0] for component in components) - padding)
    min_y = max(0, min(component["bbox"][1] for component in components) - padding)
    max_x = min(width, max(component["bbox"][2] for component in components) + padding)
    max_y = min(height, max(component["bbox"][3] for component in components) + padding)

    output = Image.new("RGBA", (max_x - min_x, max_y - min_y), (0, 0, 0, 0))
    source_pixels = source.load()
    output_pixels = output.load()
    for component in components:
        for pixel_index in component["pixels"]:
            x = pixel_index % width
            y = pixel_index // width
            output_pixels[x - min_x, y - min_y] = source_pixels[x, y]
    return output


def fit_to_cell(image: Image.Image) -> Image.Image:
    bbox = image.getbbox()
    target = Image.new("RGBA", (CELL_WIDTH, CELL_HEIGHT), (0, 0, 0, 0))
    if bbox is None:
        return target

    sprite = image.crop(bbox)
    max_width = CELL_WIDTH - 10
    max_height = CELL_HEIGHT - 10
    scale = min(max_width / sprite.width, max_height / sprite.height, 1.0)
    if scale != 1.0:
        sprite = sprite.resize(
            (max(1, round(sprite.width * scale)), max(1, round(sprite.height * scale))),
            Image.Resampling.LANCZOS,
        )
    left = (CELL_WIDTH - sprite.width) // 2
    top = (CELL_HEIGHT - sprite.height) // 2
    target.alpha_composite(sprite, (left, top))
    return target


def extract_frames(strip_path: Path, chroma_key: tuple[int, int, int], threshold: float) -> list[Image.Image]:
    with Image.open(strip_path) as opened:
        strip = remove_chroma_background(opened, chroma_key, threshold)

    components = connected_components(strip)
    if not components:
        raise SystemExit(f"no visible sprite components found in {strip_path}")

    largest_area = max(component["area"] for component in components)
    seed_threshold = max(120, largest_area * 0.20)
    seeds = [component for component in components if component["area"] >= seed_threshold]
    if not seeds:
        raise SystemExit(f"could not identify frame seeds in {strip_path}")
    if len(seeds) > COLUMNS:
        raise SystemExit(f"{strip_path.name} appears to have {len(seeds)} frames; max supported is {COLUMNS}")

    seeds = sorted(seeds, key=lambda component: component["center_x"])
    seed_ids = {id(seed) for seed in seeds}
    groups: list[list[dict[str, object]]] = [[seed] for seed in seeds]
    noise_threshold = max(12, largest_area * 0.002)

    for component in components:
        if id(component) in seed_ids or component["area"] < noise_threshold:
            continue
        nearest_index = min(
            range(len(seeds)),
            key=lambda index: abs(seeds[index]["center_x"] - component["center_x"]),
        )
        groups[nearest_index].append(component)

    return [fit_to_cell(component_group_image(strip, group)) for group in groups]


def checker(size: tuple[int, int], square: int = 16) -> Image.Image:
    image = Image.new("RGB", size, "#ffffff")
    draw = ImageDraw.Draw(image)
    for y in range(0, size[1], square):
        for x in range(0, size[0], square):
            if (x // square + y // square) % 2:
                draw.rectangle((x, y, x + square - 1, y + square - 1), fill="#e8e8e8")
    return image


def make_contact_sheet(atlas: Image.Image, rows: list[dict[str, object]], output: Path) -> None:
    cell_w = CELL_WIDTH // 2
    cell_h = CELL_HEIGHT // 2
    width = COLUMNS * cell_w
    height = len(rows) * (cell_h + LABEL_HEIGHT)
    sheet = Image.new("RGB", (width, height), "#f7f7f7")
    draw = ImageDraw.Draw(sheet)
    font = ImageFont.load_default()

    for row_index, row in enumerate(rows):
        y = row_index * (cell_h + LABEL_HEIGHT)
        draw.rectangle((0, y, width, y + LABEL_HEIGHT - 1), fill="#111111")
        draw.text((6, y + 5), f"row {row_index}: {row['state']}", fill="#ffffff", font=font)
        draw.text((width - 92, y + 5), f"{row['frame_count']} frames", fill="#ffffff", font=font)
        for column in range(COLUMNS):
            crop = atlas.crop(
                (
                    column * CELL_WIDTH,
                    row_index * CELL_HEIGHT,
                    (column + 1) * CELL_WIDTH,
                    (row_index + 1) * CELL_HEIGHT,
                )
            )
            crop = crop.resize((cell_w, cell_h), Image.Resampling.LANCZOS)
            bg = checker((cell_w, cell_h))
            bg.paste(crop, (0, 0), crop)
            x = column * cell_w
            sheet.paste(bg, (x, y + LABEL_HEIGHT))
            outline = "#18a058" if column < row["frame_count"] else "#cc3344"
            draw.rectangle(
                (x, y + LABEL_HEIGHT, x + cell_w - 1, y + LABEL_HEIGHT + cell_h - 1),
                outline=outline,
            )
            draw.text((x + 4, y + LABEL_HEIGHT + 4), str(column), fill="#111111", font=font)

    output.parent.mkdir(parents=True, exist_ok=True)
    sheet.save(output)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input-dir", required=True)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--chroma-key", default="#FF00FF")
    parser.add_argument("--key-threshold", type=float, default=96.0)
    args = parser.parse_args()

    input_dir = Path(args.input_dir).expanduser().resolve()
    output_dir = Path(args.output_dir).expanduser().resolve()
    chroma_key = parse_hex_color(args.chroma_key)
    strips = sorted(
        path for path in input_dir.iterdir() if path.is_file() and path.suffix.lower() in IMAGE_SUFFIXES
    )
    if len(strips) != ROWS:
        raise SystemExit(f"expected exactly {ROWS} strip images in {input_dir}, found {len(strips)}")

    frames_root = output_dir / "frames"
    atlas = Image.new("RGBA", (ATLAS_WIDTH, ATLAS_HEIGHT), (0, 0, 0, 0))
    manifest_rows: list[dict[str, object]] = []

    for row_index, strip_path in enumerate(strips):
        state = strip_path.stem
        frames = extract_frames(strip_path, chroma_key, args.key_threshold)
        state_dir = frames_root / state
        state_dir.mkdir(parents=True, exist_ok=True)

        for column, frame in enumerate(frames):
            frame_path = state_dir / f"{column:02d}.png"
            frame.save(frame_path)
            atlas.alpha_composite(frame, (column * CELL_WIDTH, row_index * CELL_HEIGHT))

        manifest_rows.append(
            {
                "state": state,
                "row": row_index,
                "frame_count": len(frames),
                "source": str(strip_path),
                "frames_dir": str(state_dir),
            }
        )

    output_dir.mkdir(parents=True, exist_ok=True)
    atlas_png = output_dir / "spritesheet.png"
    atlas_webp = output_dir / "spritesheet.webp"
    atlas.save(atlas_png)
    atlas.save(atlas_webp, format="WEBP", lossless=True, quality=100, method=6)

    manifest = {
        "ok": True,
        "atlas": {
            "width": ATLAS_WIDTH,
            "height": ATLAS_HEIGHT,
            "columns": COLUMNS,
            "rows": ROWS,
            "cell_width": CELL_WIDTH,
            "cell_height": CELL_HEIGHT,
        },
        "rows": manifest_rows,
    }
    manifest_path = output_dir / "custom-frames-manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    contact_sheet = output_dir / "contact-sheet.png"
    make_contact_sheet(atlas, manifest_rows, contact_sheet)

    print(
        json.dumps(
            {
                "ok": True,
                "input_dir": str(input_dir),
                "output_dir": str(output_dir),
                "spritesheet_png": str(atlas_png),
                "spritesheet_webp": str(atlas_webp),
                "contact_sheet": str(contact_sheet),
                "manifest": str(manifest_path),
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
