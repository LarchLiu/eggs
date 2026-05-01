#!/usr/bin/env python3
"""Spawn a small animated egg companion that roams around the desktop."""

from __future__ import annotations

import argparse
import json
import math
import os
import random
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path


APP_DIR = Path(os.environ.get("EGGS_APP_DIR", Path.home() / ".codex" / "eggs")).expanduser()
PID_FILE = APP_DIR / "egg.pid"
LOG_FILE = APP_DIR / "egg.log"
USER_SPRITESHEET = APP_DIR / "spritesheet.png"
USER_METADATA = APP_DIR / "spritesheet.json"
STATE_FILE = APP_DIR / "state.txt"
BUNDLED_SPRITESHEET = Path(__file__).resolve().parents[1] / "assets" / "spritesheet.png"
BUNDLED_METADATA = Path(__file__).resolve().parents[1] / "assets" / "spritesheet.json"
SWIFT_SOURCE = Path(__file__).resolve().with_name("EggDesktop.swift")
SWIFT_BINARY = APP_DIR / "EggDesktop"
DEFAULT_SPRITE_SIZE = 251
FRAME_MS = 33
ANIMATION_MS = 180
STATE_NAMES = [
    "unborn",
    "ready",
    "hatching",
    "hatched",
    "walk",
    "sleep",
    "eat",
    "drink",
    "play",
    "roar",
    "attack",
]
DEFAULT_STATE = "unborn"


def ensure_app_dir() -> None:
    APP_DIR.mkdir(parents=True, exist_ok=True)


def read_pid() -> int | None:
    try:
        return int(PID_FILE.read_text(encoding="utf-8").strip())
    except (FileNotFoundError, ValueError):
        return None


def process_alive(pid: int | None) -> bool:
    if not pid:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def process_command(pid: int) -> str | None:
    result = subprocess.run(
        ["ps", "-p", str(pid), "-o", "command="],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return None
    return result.stdout.strip() or None


def managed_process_alive(pid: int | None) -> bool:
    if not process_alive(pid):
        return False
    assert pid is not None
    command = process_command(pid)
    if command is None:
        return False

    script = str(Path(__file__).resolve())
    swift_binary = str(SWIFT_BINARY)
    return swift_binary in command or (script in command and " run" in command)


def write_pid(pid: int) -> None:
    ensure_app_dir()
    PID_FILE.write_text(f"{pid}\n", encoding="utf-8")


def normalize_state(value: str) -> str | None:
    state = value.strip().lower().replace("_", "-")
    aliases = {
        "0": "unborn",
        "egg": "unborn",
        "not-born": "unborn",
        "notborn": "unborn",
        "1": "ready",
        "waiting": "ready",
        "about-to-hatch": "ready",
        "2": "hatching",
        "birth": "hatching",
        "breaking": "hatching",
        "3": "hatched",
        "born": "hatched",
        "idle": "hatched",
        "4": "walk",
        "walking": "walk",
        "first-walk": "walk",
        "5": "sleep",
        "sleeping": "sleep",
        "睡觉": "sleep",
        "6": "eat",
        "eating": "eat",
        "chicken": "eat",
        "drumstick": "eat",
        "吃鸡腿": "eat",
        "吃": "eat",
        "7": "drink",
        "drinking": "drink",
        "water": "drink",
        "喝水": "drink",
        "喝": "drink",
        "8": "play",
        "playing": "play",
        "玩耍": "play",
        "玩": "play",
        "9": "roar",
        "roaring": "roar",
        "咆哮": "roar",
        "叫": "roar",
        "10": "attack",
        "attacking": "attack",
        "hit": "attack",
        "fight": "attack",
        "攻击": "attack",
        "打": "attack",
    }
    state = aliases.get(state, state)
    if state in STATE_NAMES:
        return state
    return None


def read_state() -> str:
    try:
        state = normalize_state(STATE_FILE.read_text(encoding="utf-8"))
    except FileNotFoundError:
        state = None
    return state or DEFAULT_STATE


def set_state(value: str) -> int:
    state = normalize_state(value)
    if state is None:
        print(f"unknown egg state '{value}'. choices: {', '.join(STATE_NAMES)}", file=sys.stderr)
        return 2
    ensure_app_dir()
    STATE_FILE.write_text(f"{state}\n", encoding="utf-8")
    print(f"egg state set to {state}")
    return 0


def clear_pid() -> None:
    try:
        PID_FILE.unlink()
    except FileNotFoundError:
        pass


def swift_available() -> bool:
    return sys.platform == "darwin" and shutil.which("swiftc") is not None


def ensure_swift_binary() -> Path | None:
    if not swift_available():
        return None
    ensure_app_dir()
    source_mtime = SWIFT_SOURCE.stat().st_mtime if SWIFT_SOURCE.exists() else 0
    binary_mtime = SWIFT_BINARY.stat().st_mtime if SWIFT_BINARY.exists() else 0
    if SWIFT_BINARY.exists() and binary_mtime >= source_mtime:
        return SWIFT_BINARY

    compile_cmd = [
        "swiftc",
        str(SWIFT_SOURCE),
        "-o",
        str(SWIFT_BINARY),
        "-framework",
        "Cocoa",
    ]
    result = subprocess.run(compile_cmd, capture_output=True, text=True)
    if result.returncode != 0:
        print(result.stderr.strip() or "failed to compile Swift egg runtime", file=sys.stderr)
        return None
    return SWIFT_BINARY


def runtime_command() -> list[str] | None:
    swift_binary = ensure_swift_binary()
    if swift_binary is not None:
        return [str(swift_binary), str(BUNDLED_SPRITESHEET), str(BUNDLED_METADATA)]
    return [sys.executable, str(Path(__file__).resolve()), "run"]


def start_background() -> int:
    ensure_app_dir()
    current = read_pid()
    if managed_process_alive(current):
        print(f"egg already running with pid {current}")
        return 0
    clear_pid()

    command = runtime_command()
    if command is None:
        print("no usable egg runtime found", file=sys.stderr)
        return 1

    with LOG_FILE.open("ab") as log:
        proc = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=log,
            start_new_session=True,
            close_fds=True,
        )
    write_pid(proc.pid)
    print(f"spawned egg with pid {proc.pid}")
    return 0


def stop_background() -> int:
    pid = read_pid()
    if not managed_process_alive(pid):
        clear_pid()
        print("egg is not running")
        return 0

    assert pid is not None
    os.kill(pid, signal.SIGTERM)
    deadline = time.time() + 3
    while time.time() < deadline:
        if not process_alive(pid):
            clear_pid()
            print("stopped egg")
            return 0
        time.sleep(0.1)

    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    clear_pid()
    print("stopped egg")
    return 0


def status() -> int:
    pid = read_pid()
    if managed_process_alive(pid):
        print(f"egg running with pid {pid}; state={read_state()}")
    else:
        clear_pid()
        print(f"egg is not running; state={read_state()}")
    return 0


def install_spritesheet(path: str) -> int:
    source = Path(path).expanduser().resolve()
    if not source.exists():
        print(f"spritesheet not found: {source}", file=sys.stderr)
        return 1
    if source.suffix.lower() != ".png":
        print("spritesheet must be a PNG file", file=sys.stderr)
        return 1
    ensure_app_dir()
    shutil.copy2(source, USER_SPRITESHEET)
    metadata = source.with_name("spritesheet.json")
    if metadata.exists():
        shutil.copy2(metadata, USER_METADATA)
    elif USER_METADATA.exists():
        USER_METADATA.unlink()
    print(f"installed spritesheet at {USER_SPRITESHEET}")
    return 0


def find_spritesheet() -> Path | None:
    if USER_SPRITESHEET.exists():
        return USER_SPRITESHEET
    if BUNDLED_SPRITESHEET.exists():
        return BUNDLED_SPRITESHEET
    return None


def find_metadata(spritesheet: Path) -> Path | None:
    if spritesheet == USER_SPRITESHEET and USER_METADATA.exists():
        return USER_METADATA
    if spritesheet == BUNDLED_SPRITESHEET and BUNDLED_METADATA.exists():
        return BUNDLED_METADATA
    metadata = spritesheet.with_name("spritesheet.json")
    return metadata if metadata.exists() else None


def load_sprite_metadata(spritesheet: Path, sheet_w: int, sheet_h: int) -> tuple[int, int]:
    metadata = find_metadata(spritesheet)
    if metadata is not None:
        try:
            data = json.loads(metadata.read_text(encoding="utf-8"))
            frame_w = int(data.get("frameWidth", 0))
            frame_h = int(data.get("frameHeight", 0))
            if frame_w > 0 and frame_h > 0:
                return frame_w, frame_h
        except (OSError, ValueError, TypeError) as exc:
            print(f"could not read spritesheet metadata {metadata}: {exc}", file=sys.stderr)
    return min(DEFAULT_SPRITE_SIZE, sheet_w), min(DEFAULT_SPRITE_SIZE, sheet_h)


def load_sprite_frames(tk) -> tuple[list, int, int]:
    spritesheet = find_spritesheet()
    if spritesheet is None:
        return [], DEFAULT_SPRITE_SIZE, DEFAULT_SPRITE_SIZE
    try:
        sheet = tk.PhotoImage(file=str(spritesheet))
        sheet_w = sheet.width()
        sheet_h = sheet.height()
        frame_w, frame_h = load_sprite_metadata(spritesheet, sheet_w, sheet_h)
        if sheet_w < frame_w or sheet_h < frame_h:
            print(f"spritesheet too small: {spritesheet}", file=sys.stderr)
            return [], DEFAULT_SPRITE_SIZE, DEFAULT_SPRITE_SIZE

        cols = sheet_w // frame_w
        rows = sheet_h // frame_h
        frames = []
        for row in range(rows):
            for col in range(cols):
                frame = tk.PhotoImage(width=frame_w, height=frame_h)
                x1 = col * frame_w
                y1 = row * frame_h
                frame.tk.call(frame, "copy", sheet, "-from", x1, y1, x1 + frame_w, y1 + frame_h, "-to", 0, 0)
                frames.append(frame)
        return frames, frame_w, frame_h
    except Exception as exc:
        print(f"could not load spritesheet {spritesheet}: {exc}", file=sys.stderr)
        return [], DEFAULT_SPRITE_SIZE, DEFAULT_SPRITE_SIZE


def frames_for_state(frames: list, state: str) -> list:
    if not frames:
        return []
    state_index = STATE_NAMES.index(state)
    frames_per_state = max(1, len(frames) // len(STATE_NAMES))
    start = state_index * frames_per_state
    end = min(start + frames_per_state, len(frames))
    return frames[start:end] or frames


class Egg:
    def __init__(self, root, canvas, frames, sprite_w: int, sprite_h: int):
        self.root = root
        self.canvas = canvas
        self.frames = frames
        self.sprite_w = sprite_w
        self.sprite_h = sprite_h
        self.screen_w = max(root.winfo_screenwidth(), self.sprite_w)
        self.screen_h = max(root.winfo_screenheight(), self.sprite_h)
        self.x = random.randint(0, max(0, self.screen_w - self.sprite_w))
        self.y = random.randint(80, max(80, self.screen_h - self.sprite_h - 80))
        self.vx = random.choice([-1, 1]) * random.uniform(0.6, 1.3)
        self.vy = random.uniform(-0.25, 0.25)
        self.phase = 0.0
        self.target_change_at = time.time() + random.uniform(4, 9)
        self.state = read_state()
        self.frame_index = 0
        self.state_check_at = 0.0
        self.next_frame_at = 0.0
        self.drag_offset_x = 0
        self.drag_offset_y = 0
        self.dragging = False

    def maybe_change_direction(self) -> None:
        now = time.time()
        if now < self.target_change_at:
            return
        self.target_change_at = now + random.uniform(4, 10)
        self.vx = random.choice([-1, 1]) * random.uniform(0.45, 1.4)
        self.vy = random.uniform(-0.35, 0.35)

    def update_position(self) -> None:
        if self.dragging:
            self.phase += 0.16
            return

        self.maybe_change_direction()
        self.x += self.vx
        self.y += self.vy + math.sin(self.phase * 0.65) * 0.12

        max_x = self.screen_w - self.sprite_w
        max_y = self.screen_h - self.sprite_h - 24
        if self.x <= 0 or self.x >= max_x:
            self.vx *= -1
            self.x = min(max(self.x, 0), max_x)
        if self.y <= 40 or self.y >= max_y:
            self.vy *= -1
            self.y = min(max(self.y, 40), max_y)

        self.root.geometry(f"{self.sprite_w}x{self.sprite_h}+{int(self.x)}+{int(self.y)}")
        self.phase += 0.16

    def draw(self) -> None:
        self.canvas.delete("all")
        now = time.time()
        if now >= self.state_check_at:
            next_state = read_state()
            if next_state != self.state:
                self.state = next_state
                self.frame_index = 0
            self.state_check_at = now + 0.2

        if self.frames:
            state_frames = frames_for_state(self.frames, self.state)
            frame = state_frames[self.frame_index % len(state_frames)]
            self.canvas.create_image(self.sprite_w / 2, self.sprite_h / 2, image=frame)
            if now >= self.next_frame_at:
                self.frame_index += 1
                self.next_frame_at = now + ANIMATION_MS / 1000
            return

        bob = math.sin(self.phase * 2) * 4
        self.canvas.create_oval(62, 194, 189, 210, fill="#2f2a1e", outline="")
        self.canvas.create_oval(74, 34 + bob, 177, 199 + bob, fill="#f3ecd2", outline="#222016", width=3)
        for x, y in [(111, 76), (91, 122), (146, 119), (125, 161)]:
            self.canvas.create_oval(x - 12, y - 10 + bob, x + 12, y + 10 + bob, fill="#89a957", outline="")

    def tick(self) -> None:
        self.update_position()
        self.draw()
        self.root.after(FRAME_MS, self.tick)

    def begin_drag(self, event) -> None:
        self.dragging = True
        self.drag_offset_x = event.x
        self.drag_offset_y = event.y
        self.vx = 0
        self.vy = 0

    def drag_to(self, event) -> None:
        self.x = event.x_root - self.drag_offset_x
        self.y = event.y_root - self.drag_offset_y
        self.root.geometry(f"{self.sprite_w}x{self.sprite_h}+{int(self.x)}+{int(self.y)}")

    def end_drag(self, _event) -> None:
        self.dragging = False
        self.vx = random.choice([-1, 1]) * random.uniform(0.45, 1.0)
        self.vy = random.uniform(-0.2, 0.2)
        self.target_change_at = time.time() + random.uniform(4, 10)


def run_gui() -> int:
    try:
        import tkinter as tk
    except Exception as exc:  # pragma: no cover - depends on local Python build
        print(f"Tkinter is required to display the desktop egg: {exc}", file=sys.stderr)
        return 1

    ensure_app_dir()
    write_pid(os.getpid())

    root = tk.Tk()
    root.title("Egg")
    root.overrideredirect(True)
    root.resizable(False, False)

    transparent = "#00ff01"
    root.configure(bg=transparent)
    try:
        root.wm_attributes("-topmost", True)
    except tk.TclError:
        pass
    try:
        root.wm_attributes("-transparentcolor", transparent)
    except tk.TclError:
        try:
            root.configure(bg="systemTransparent")
            root.wm_attributes("-transparent", True)
            transparent = "systemTransparent"
        except tk.TclError:
            pass

    frames, sprite_w, sprite_h = load_sprite_frames(tk)
    canvas = tk.Canvas(
        root,
        width=sprite_w,
        height=sprite_h,
        bg=transparent,
        highlightthickness=0,
        bd=0,
    )
    canvas.pack(fill="both", expand=True)

    def shutdown(*_args) -> None:
        clear_pid()
        try:
            root.destroy()
        except tk.TclError:
            pass

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)

    egg = Egg(root, canvas, frames, sprite_w, sprite_h)
    canvas.bind("<ButtonPress-1>", egg.begin_drag)
    canvas.bind("<B1-Motion>", egg.drag_to)
    canvas.bind("<ButtonRelease-1>", egg.end_drag)
    root.geometry(f"{sprite_w}x{sprite_h}+{int(egg.x)}+{int(egg.y)}")
    root.after(0, egg.tick)

    try:
        root.mainloop()
    finally:
        clear_pid()
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Manage the desktop egg companion.")
    parser.add_argument("command", choices=["start", "run", "stop", "restart", "status", "spritesheet", "state"])
    parser.add_argument("path", nargs="?", help="PNG spritesheet path or egg state.")
    args = parser.parse_args()

    if args.command == "start":
        return start_background()
    if args.command == "run":
        return run_gui()
    if args.command == "stop":
        return stop_background()
    if args.command == "restart":
        stop_background()
        return start_background()
    if args.command == "status":
        return status()
    if args.command == "state":
        if not args.path:
            print(f"current egg state: {read_state()}")
            print(f"choices: {', '.join(STATE_NAMES)}")
            return 0
        return set_state(args.path)
    if args.command == "spritesheet":
        if not args.path:
            print("usage: egg_desktop.py spritesheet /path/to/spritesheet.png", file=sys.stderr)
            return 2
        return install_spritesheet(args.path)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
