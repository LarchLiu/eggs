#!/usr/bin/env python3
"""Spawn a small animated sprite companion that roams around the desktop."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import math
import os
import queue
import random
import shutil
import signal
import socket
import subprocess
import sys
import struct
import ssl
import threading
import time
import uuid
from pathlib import Path
from urllib import parse, request


APP_DIR = Path(os.environ.get("EGGS_APP_DIR", Path.home() / ".codex" / "eggs")).expanduser()
PID_FILE = APP_DIR / "egg.pid"
LOG_FILE = APP_DIR / "egg.log"
STATE_FILE = APP_DIR / "state.json"
CONFIG_FILE = APP_DIR / "config.json"
CLIENT_FILE = APP_DIR / "client.json"
REMOTE_FILE = APP_DIR / "remote.json"
REMOTE_CACHE_DIR = APP_DIR / "remote"
BUNDLED_ASSETS_DIR = Path(__file__).resolve().parents[1] / "assets"
SWIFT_SOURCE = Path(__file__).resolve().with_name("EggDesktop.swift")
SWIFT_BINARY = APP_DIR / "EggDesktop"
DEFAULT_SPRITE_SIZE = 251
DEFAULT_SPRITE = "dino"
FRAME_MS = 33
ANIMATION_MS = 180
DEFAULT_ANIMATIONS = [
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
DEFAULT_SERVER_URL = "http://localhost:8787"


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


def normalized_key(value: str) -> str:
    return value.strip().lower().replace("_", "-")


def read_config() -> dict:
    try:
        data = json.loads(CONFIG_FILE.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError, OSError, TypeError):
        return {}
    return data if isinstance(data, dict) else {}


def configured_animations(sprite: str | None = None) -> dict[str, dict]:
    config = read_config()
    animations = config.get("animations")
    if not isinstance(animations, dict):
        return {}
    sprite_name = normalize_sprite(sprite) if sprite else DEFAULT_SPRITE
    sprite_animations = animations.get(sprite_name)
    if not isinstance(sprite_animations, dict):
        return {}
    result: dict[str, dict] = {}
    for name, spec in sprite_animations.items():
        if isinstance(spec, dict) and str(name).strip():
            result[str(name).strip()] = spec
    return result


def animation_names(sprite: str | None = None) -> list[str]:
    names = list(configured_animations(sprite).keys())
    return names or DEFAULT_ANIMATIONS


def default_state_for_sprite(sprite: str | None = None) -> str:
    names = animation_names(sprite)
    return names[0] if names else DEFAULT_STATE


def normalize_state(value: str, sprite: str | None = None) -> str | None:
    state = normalized_key(value)
    names = animation_names(sprite)
    for name in names:
        if normalized_key(name) == state:
            return name
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
    if names != DEFAULT_ANIMATIONS:
        for name in DEFAULT_ANIMATIONS:
            if normalized_key(name) == state:
                return name
    return None


def animation_spec(sprite: str, state: str) -> tuple[int, bool | int]:
    configured = configured_animations(sprite)
    state_key = normalized_key(state)
    for name, spec in configured.items():
        if normalized_key(name) != state_key:
            continue
        raw_row = spec.get("row", 0)
        try:
            row = int(raw_row)
        except (TypeError, ValueError):
            row = 0
        loop = spec.get("loop", True)
        if isinstance(loop, bool):
            return max(0, row), loop
        try:
            return max(0, row), max(1, int(loop))
        except (TypeError, ValueError):
            return max(0, row), True

    names = DEFAULT_ANIMATIONS
    row = next((index for index, name in enumerate(names) if normalized_key(name) == state_key), 0)
    return row, True


def normalize_sprite(value: str | None) -> str:
    if not value:
        return DEFAULT_SPRITE
    name = value.strip()
    if not name:
        return DEFAULT_SPRITE
    stem = Path(name).stem
    safe = "".join(ch for ch in stem if ch.isalnum() or ch in ("-", "_"))
    return safe or DEFAULT_SPRITE


def read_runtime_state() -> tuple[str, str]:
    sprite = DEFAULT_SPRITE
    state = default_state_for_sprite(sprite)
    try:
        data = json.loads(STATE_FILE.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return sprite, state
    except (json.JSONDecodeError, OSError, TypeError):
        return sprite, state

    if isinstance(data, dict):
        sprite = normalize_sprite(data.get("sprite"))
        state = normalize_state(str(data.get("state", "")), sprite) or default_state_for_sprite(sprite)
    return sprite, state


def write_runtime_state(sprite: str, state: str) -> None:
    ensure_app_dir()
    STATE_FILE.write_text(
        json.dumps({"sprite": normalize_sprite(sprite), "state": state}, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


def read_state() -> str:
    return read_runtime_state()[1]


def read_sprite() -> str:
    return read_runtime_state()[0]


def read_json_file(path: Path, fallback: dict | None = None) -> dict:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError, OSError, TypeError):
        return dict(fallback or {})
    return data if isinstance(data, dict) else dict(fallback or {})


def write_json_file(path: Path, data: dict) -> None:
    ensure_app_dir()
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def read_client_config() -> dict:
    config = read_json_file(CLIENT_FILE)
    device_id = str(config.get("device_id", "")).strip()
    if not device_id:
        device_id = uuid.uuid4().hex
        config["device_id"] = device_id
        write_json_file(CLIENT_FILE, config)
    return config


def default_remote_config() -> dict:
    return {
        "server_url": os.environ.get("EGGS_REMOTE_URL", DEFAULT_SERVER_URL),
        "enabled": False,
        "mode": "random",
        "room": "",
        "sprite_id": "",
    }


def read_remote_config() -> dict:
    config = default_remote_config()
    config.update(read_json_file(REMOTE_FILE))
    server_url = str(config.get("server_url") or DEFAULT_SERVER_URL).rstrip("/")
    if not server_url:
        server_url = DEFAULT_SERVER_URL
    config["server_url"] = server_url
    config["enabled"] = bool(config.get("enabled", False))
    config["mode"] = "room" if config.get("mode") == "room" else "random"
    config["room"] = str(config.get("room", "")).strip()
    config["sprite_id"] = str(config.get("sprite_id", "")).strip()
    return config


def write_remote_config(config: dict) -> None:
    merged = read_remote_config()
    merged.update(config)
    write_json_file(REMOTE_FILE, merged)


def set_state(value: str, sprite_name: str | None = None) -> int:
    current_sprite, _ = read_runtime_state()
    sprite = normalize_sprite(sprite_name) if sprite_name else current_sprite
    state = normalize_state(value, sprite)
    if state is None:
        print(f"unknown companion state '{value}'. choices: {', '.join(animation_names(sprite))}", file=sys.stderr)
        return 2
    write_runtime_state(sprite, state)
    print(f"companion state set to {state}; sprite={sprite}")
    return 0


def set_sprite(value: str) -> int:
    sprite = normalize_sprite(value)
    _, state = read_runtime_state()
    state = normalize_state(state, sprite) or default_state_for_sprite(sprite)
    write_runtime_state(sprite, state)
    print(f"companion sprite set to {sprite}; state={state}")
    return 0


def remote_enabled() -> bool:
    return read_remote_config().get("enabled", False)


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
        print(result.stderr.strip() or "failed to compile Swift companion runtime", file=sys.stderr)
        return None
    return SWIFT_BINARY


def runtime_command() -> list[str] | None:
    if remote_enabled():
        return [sys.executable, str(Path(__file__).resolve()), "run"]
    swift_binary = ensure_swift_binary()
    if swift_binary is not None:
        return [str(swift_binary), str(BUNDLED_ASSETS_DIR)]
    return [sys.executable, str(Path(__file__).resolve()), "run"]


def start_background() -> int:
    ensure_app_dir()
    current = read_pid()
    if managed_process_alive(current):
        print(f"companion already running with pid {current}")
        return 0
    clear_pid()

    command = runtime_command()
    if command is None:
        print("no usable companion runtime found", file=sys.stderr)
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
    print(f"spawned companion with pid {proc.pid}")
    return 0


def stop_background() -> int:
    pid = read_pid()
    if not managed_process_alive(pid):
        clear_pid()
        print("companion is not running")
        return 0

    assert pid is not None
    os.kill(pid, signal.SIGTERM)
    deadline = time.time() + 3
    while time.time() < deadline:
        if not process_alive(pid):
            clear_pid()
            print("stopped companion")
            return 0
        time.sleep(0.1)

    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    clear_pid()
    print("stopped companion")
    return 0


def status() -> int:
    pid = read_pid()
    sprite, state = read_runtime_state()
    if managed_process_alive(pid):
        print(f"companion running with pid {pid}; sprite={sprite}; state={state}")
    else:
        clear_pid()
        print(f"companion is not running; sprite={sprite}; state={state}")
    return 0


def install_spritesheet(path: str, sprite_name: str | None = None) -> int:
    source = Path(path).expanduser().resolve()
    if not source.exists():
        print(f"spritesheet not found: {source}", file=sys.stderr)
        return 1
    if source.suffix.lower() != ".png":
        print("spritesheet must be a PNG file", file=sys.stderr)
        return 1
    ensure_app_dir()
    sprite = normalize_sprite(sprite_name or source.stem)
    user_image = APP_DIR / f"{sprite}.png"
    user_metadata = APP_DIR / f"{sprite}.json"
    shutil.copy2(source, user_image)
    metadata = source.with_suffix(".json")
    if metadata.exists():
        shutil.copy2(metadata, user_metadata)
    elif user_metadata.exists():
        user_metadata.unlink()
    print(f"installed sprite {sprite} at {user_image}")
    return 0


def local_sprite_paths(sprite_name: str) -> tuple[Path | None, Path | None]:
    spritesheet = find_spritesheet(sprite_name)
    if spritesheet is None:
        return None, None
    return spritesheet, find_metadata(spritesheet)


def validate_remote_metadata(data: dict) -> bool:
    required = ("frameWidth", "frameHeight", "columns", "rows", "frameCount", "image")
    try:
        frame_w = int(data.get("frameWidth", 0))
        frame_h = int(data.get("frameHeight", 0))
        columns = int(data.get("columns", 0))
        rows = int(data.get("rows", 0))
        frame_count = int(data.get("frameCount", 0))
    except (TypeError, ValueError):
        return False
    return (
        all(key in data for key in required)
        and frame_w > 0
        and frame_h > 0
        and frame_w <= 1024
        and frame_h <= 1024
        and 0 < columns <= 64
        and 0 < rows <= 64
        and 0 < frame_count <= 512
        and frame_count <= columns * rows
        and isinstance(data.get("image"), str)
    )


def validate_remote_config(data: dict) -> bool:
    animations = data.get("animations")
    if not isinstance(animations, dict):
        return False
    for sprite_name, states in animations.items():
        if not normalize_sprite(str(sprite_name)):
            return False
        if not isinstance(states, dict):
            return False
        for state_name, spec in states.items():
            if not str(state_name).strip() or not isinstance(spec, dict):
                return False
            try:
                row = int(spec.get("row"))
            except (TypeError, ValueError):
                return False
            if row < 0:
                return False
            loop = spec.get("loop", True)
            if isinstance(loop, bool):
                continue
            try:
                if int(loop) < 1:
                    return False
            except (TypeError, ValueError):
                return False
    return True


def multipart_body(fields: dict[str, str], files: dict[str, tuple[str, bytes, str]]) -> tuple[bytes, str]:
    boundary = "----eggs-" + uuid.uuid4().hex
    chunks: list[bytes] = []
    for name, value in fields.items():
        chunks.extend(
            [
                f"--{boundary}\r\n".encode(),
                f'Content-Disposition: form-data; name="{name}"\r\n\r\n'.encode(),
                str(value).encode(),
                b"\r\n",
            ]
        )
    for name, (filename, data, content_type) in files.items():
        chunks.extend(
            [
                f"--{boundary}\r\n".encode(),
                f'Content-Disposition: form-data; name="{name}"; filename="{filename}"\r\n'.encode(),
                f"Content-Type: {content_type}\r\n\r\n".encode(),
                data,
                b"\r\n",
            ]
        )
    chunks.append(f"--{boundary}--\r\n".encode())
    return b"".join(chunks), f"multipart/form-data; boundary={boundary}"


def http_json(url: str, timeout: float = 10.0) -> dict:
    with request.urlopen(url, timeout=timeout) as response:
        data = response.read()
    parsed = json.loads(data.decode("utf-8"))
    return parsed if isinstance(parsed, dict) else {}


def remote_upload(sprite_name: str) -> int:
    sprite = normalize_sprite(sprite_name)
    image, metadata = local_sprite_paths(sprite)
    if image is None or metadata is None:
        print(f"missing local sprite assets for {sprite}; expected <sprite>.png and <sprite>.json", file=sys.stderr)
        return 1
    try:
        meta_data = json.loads(metadata.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"could not read sprite metadata: {exc}", file=sys.stderr)
        return 1
    if not validate_remote_metadata(meta_data):
        print("sprite metadata is not valid for remote upload", file=sys.stderr)
        return 1

    files = {
        "png": (f"{sprite}.png", image.read_bytes(), "image/png"),
        "json": (f"{sprite}.json", metadata.read_bytes(), "application/json"),
    }
    if CONFIG_FILE.exists():
        try:
            config_data = json.loads(CONFIG_FILE.read_text(encoding="utf-8"))
            if validate_remote_config(config_data):
                files["config"] = ("config.json", CONFIG_FILE.read_bytes(), "application/json")
        except (OSError, json.JSONDecodeError):
            pass

    remote = read_remote_config()
    client = read_client_config()
    body, content_type = multipart_body(
        {
            "device_id": client["device_id"],
            "sprite_name": sprite,
            "display_name": sprite,
        },
        files,
    )
    req = request.Request(
        remote["server_url"] + "/api/v1/sprites",
        data=body,
        headers={"Content-Type": content_type},
        method="POST",
    )
    try:
        with request.urlopen(req, timeout=20) as response:
            result = json.loads(response.read().decode("utf-8"))
    except Exception as exc:
        print(f"remote upload failed: {exc}", file=sys.stderr)
        return 1
    sprite_id = str(result.get("id", ""))
    if sprite_id:
        write_remote_config({"sprite_id": sprite_id})
    print(f"uploaded sprite {sprite}; sprite_id={sprite_id or 'unknown'}")
    return 0


def remote_command(action: str | None, value: str | None = None) -> int:
    if not action:
        remote = read_remote_config()
        print(
            f"remote enabled={remote['enabled']} server={remote['server_url']} "
            f"mode={remote['mode']} room={remote.get('room', '') or '-'} sprite_id={remote.get('sprite_id', '') or '-'}"
        )
        return 0
    if action == "on":
        write_remote_config({"enabled": True})
        print("remote interaction enabled")
        return 0
    if action == "off":
        write_remote_config({"enabled": False})
        print("remote interaction disabled")
        return 0
    if action == "server":
        if not value:
            print("usage: egg_desktop.py remote server <url>", file=sys.stderr)
            return 2
        write_remote_config({"server_url": value.rstrip("/")})
        print(f"remote server set to {value.rstrip('/')}")
        return 0
    if action == "upload":
        if not value:
            print("usage: egg_desktop.py remote upload <sprite>", file=sys.stderr)
            return 2
        return remote_upload(value)
    if action == "random":
        write_remote_config({"enabled": True, "mode": "random", "room": ""})
        print("remote random lobby enabled")
        return 0
    if action == "room":
        if not value:
            print("usage: egg_desktop.py remote room <code>", file=sys.stderr)
            return 2
        write_remote_config({"enabled": True, "mode": "room", "room": value.strip()})
        print(f"remote room enabled: {value.strip()}")
        return 0
    if action == "leave":
        write_remote_config({"enabled": False, "mode": "random", "room": ""})
        print("left remote interaction")
        return 0
    print(f"unknown remote action '{action}'", file=sys.stderr)
    return 2


def find_spritesheet(sprite_name: str) -> Path | None:
    sprite = normalize_sprite(sprite_name)
    user_spritesheet = APP_DIR / f"{sprite}.png"
    bundled_spritesheet = BUNDLED_ASSETS_DIR / f"{sprite}.png"
    if user_spritesheet.exists():
        return user_spritesheet
    if bundled_spritesheet.exists():
        return bundled_spritesheet
    return None


def find_metadata(spritesheet: Path) -> Path | None:
    metadata = spritesheet.with_suffix(".json")
    return metadata if metadata.exists() else None


def load_sprite_metadata(spritesheet: Path, sheet_w: int, sheet_h: int, metadata: Path | None = None) -> tuple[int, int]:
    metadata = metadata or find_metadata(spritesheet)
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


def load_sprite_frames_from_paths(tk, spritesheet: Path, metadata: Path | None = None) -> tuple[list, int, int, int, int]:
    try:
        sheet = tk.PhotoImage(file=str(spritesheet))
        sheet_w = sheet.width()
        sheet_h = sheet.height()
        frame_w, frame_h = load_sprite_metadata(spritesheet, sheet_w, sheet_h, metadata)
        if sheet_w < frame_w or sheet_h < frame_h:
            print(f"spritesheet too small: {spritesheet}", file=sys.stderr)
            return [], DEFAULT_SPRITE_SIZE, DEFAULT_SPRITE_SIZE, 1, 1

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
        return frames, frame_w, frame_h, cols, rows
    except Exception as exc:
        print(f"could not load spritesheet {spritesheet}: {exc}", file=sys.stderr)
        return [], DEFAULT_SPRITE_SIZE, DEFAULT_SPRITE_SIZE, 1, 1


def load_sprite_frames(tk, sprite_name: str) -> tuple[list, int, int, int, int]:
    spritesheet = find_spritesheet(sprite_name)
    if spritesheet is None:
        return [], DEFAULT_SPRITE_SIZE, DEFAULT_SPRITE_SIZE, 1, 1
    return load_sprite_frames_from_paths(tk, spritesheet, find_metadata(spritesheet))


def frames_for_state(frames: list, sprite: str, state: str, columns: int, rows: int) -> tuple[list, bool | int]:
    if not frames:
        return [], True
    row, loop = animation_spec(sprite, state)
    row = min(max(0, row), max(0, rows - 1))
    start = row * columns
    end = min(start + columns, len(frames))
    return (frames[start:end] or frames), loop


def animation_spec_from_config(config: dict, sprite: str, state: str) -> tuple[int, bool | int]:
    animations = config.get("animations")
    if isinstance(animations, dict):
        sprite_animations = animations.get(sprite)
        if isinstance(sprite_animations, dict):
            state_key = normalized_key(state)
            for name, spec in sprite_animations.items():
                if normalized_key(str(name)) != state_key or not isinstance(spec, dict):
                    continue
                try:
                    row = max(0, int(spec.get("row", 0)))
                except (TypeError, ValueError):
                    row = 0
                loop = spec.get("loop", True)
                if isinstance(loop, bool):
                    return row, loop
                try:
                    return row, max(1, int(loop))
                except (TypeError, ValueError):
                    return row, True
    return animation_spec(sprite, state)


def frames_for_state_with_config(
    frames: list,
    sprite: str,
    state: str,
    columns: int,
    rows: int,
    config: dict,
) -> tuple[list, bool | int]:
    if not frames:
        return [], True
    row, loop = animation_spec_from_config(config, sprite, state)
    row = min(max(0, row), max(0, rows - 1))
    start = row * columns
    end = min(start + columns, len(frames))
    return (frames[start:end] or frames), loop


class SimpleWebSocket:
    def __init__(self, url: str, timeout: float = 10.0):
        parsed = parse.urlparse(url)
        if parsed.scheme not in ("ws", "wss"):
            raise ValueError("websocket URL must use ws:// or wss://")
        self.sock = socket.create_connection((parsed.hostname, parsed.port or (443 if parsed.scheme == "wss" else 80)), timeout=timeout)
        if parsed.scheme == "wss":
            self.sock = ssl.create_default_context().wrap_socket(self.sock, server_hostname=parsed.hostname)
        self.sock.settimeout(timeout)
        key = base64.b64encode(os.urandom(16)).decode("ascii")
        path = parsed.path or "/"
        if parsed.query:
            path += "?" + parsed.query
        host = parsed.netloc
        request_text = (
            f"GET {path} HTTP/1.1\r\n"
            f"Host: {host}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n\r\n"
        )
        self.sock.sendall(request_text.encode("ascii"))
        response = b""
        while b"\r\n\r\n" not in response:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise ConnectionError("websocket handshake failed")
            response += chunk
        if b" 101 " not in response.split(b"\r\n", 1)[0]:
            raise ConnectionError(response.decode("utf-8", "replace").splitlines()[0])
        expected = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
        if expected not in response.decode("utf-8", "replace"):
            raise ConnectionError("websocket accept key mismatch")
        self.buffer = response.split(b"\r\n\r\n", 1)[1]
        self.sock.settimeout(None)
        self.lock = threading.Lock()

    def send_json(self, data: dict) -> None:
        payload = json.dumps(data, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        header = bytearray([0x81])
        length = len(payload)
        if length < 126:
            header.append(0x80 | length)
        elif length <= 0xFFFF:
            header.extend([0x80 | 126, (length >> 8) & 0xFF, length & 0xFF])
        else:
            header.append(0x80 | 127)
            header.extend(struct.pack(">Q", length))
        mask = os.urandom(4)
        masked = bytes(byte ^ mask[index % 4] for index, byte in enumerate(payload))
        with self.lock:
            self.sock.sendall(bytes(header) + mask + masked)

    def recv_json(self) -> dict:
        payload = self._recv_frame()
        data = json.loads(payload.decode("utf-8"))
        return data if isinstance(data, dict) else {}

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass

    def _recv_exact(self, count: int) -> bytes:
        chunks = []
        remaining = count
        if self.buffer:
            chunk = self.buffer[:remaining]
            self.buffer = self.buffer[len(chunk) :]
            chunks.append(chunk)
            remaining -= len(chunk)
        while remaining > 0:
            chunk = self.sock.recv(remaining)
            if not chunk:
                raise ConnectionError("websocket closed")
            chunks.append(chunk)
            remaining -= len(chunk)
        return b"".join(chunks)

    def _recv_frame(self) -> bytes:
        first, second = self._recv_exact(2)
        opcode = first & 0x0F
        masked = bool(second & 0x80)
        length = second & 0x7F
        if length == 126:
            length = struct.unpack(">H", self._recv_exact(2))[0]
        elif length == 127:
            length = struct.unpack(">Q", self._recv_exact(8))[0]
        mask = self._recv_exact(4) if masked else b""
        payload = self._recv_exact(length)
        if masked:
            payload = bytes(byte ^ mask[index % 4] for index, byte in enumerate(payload))
        if opcode == 0x8:
            raise ConnectionError("websocket closed")
        if opcode != 0x1:
            return b"{}"
        return payload


def websocket_url(server_url: str, query: dict[str, str]) -> str:
    parsed = parse.urlparse(server_url)
    scheme = "wss" if parsed.scheme == "https" else "ws"
    netloc = parsed.netloc
    prefix = parsed.path.rstrip("/")
    return parse.urlunparse((scheme, netloc, prefix + "/ws", "", parse.urlencode(query), ""))


def download_url(url: str, target: Path, timeout: float = 15.0) -> None:
    with request.urlopen(url, timeout=timeout) as response:
        data = response.read()
    target.write_bytes(data)


def cache_remote_sprite(server_url: str, sprite_id: str) -> tuple[Path, Path, Path | None, str] | None:
    sprite_id = safe_cache_name(sprite_id)
    if not sprite_id:
        return None
    cache_dir = REMOTE_CACHE_DIR / sprite_id
    image = cache_dir / "sprite.png"
    metadata = cache_dir / "sprite.json"
    config = cache_dir / "config.json"
    if image.exists() and metadata.exists():
        return image, metadata, config if config.exists() else None, sprite_id
    try:
        info = http_json(server_url.rstrip("/") + f"/api/v1/sprites/{sprite_id}")
        cache_dir.mkdir(parents=True, exist_ok=True)
        download_url(str(info["png_url"]), image)
        download_url(str(info["json_url"]), metadata)
        meta = json.loads(metadata.read_text(encoding="utf-8"))
        if not validate_remote_metadata(meta):
            raise ValueError("invalid downloaded sprite metadata")
        config_url = info.get("config_url")
        if config_url:
            download_url(str(config_url), config)
            config_data = json.loads(config.read_text(encoding="utf-8"))
            if not validate_remote_config(config_data):
                config.unlink(missing_ok=True)
        return image, metadata, config if config.exists() else None, str(info.get("name") or sprite_id)
    except Exception as exc:
        print(f"could not cache remote sprite {sprite_id}: {exc}", file=sys.stderr)
        return None


def safe_cache_name(value: str) -> str:
    return "".join(ch for ch in value.strip() if ch.isalnum() or ch in ("-", "_"))


class RemoteSession:
    def __init__(self, inbox: queue.Queue):
        self.inbox = inbox
        self.remote = read_remote_config()
        self.client = read_client_config()
        self.ws: SimpleWebSocket | None = None
        self.closed = False
        self.connected = False
        self.thread: threading.Thread | None = None

    def start(self) -> None:
        if not self.remote.get("enabled"):
            return
        sprite_id = str(self.remote.get("sprite_id") or "").strip()
        if not sprite_id:
            print("remote interaction is enabled, but no sprite_id is configured; run: egg_desktop.py remote upload <sprite>", file=sys.stderr)
            return
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def _run(self) -> None:
        query = {
            "device_id": self.client["device_id"],
            "mode": self.remote.get("mode", "random"),
            "room": self.remote.get("room", ""),
            "sprite_id": self.remote.get("sprite_id", ""),
        }
        url = websocket_url(self.remote["server_url"], query)
        try:
            self.ws = SimpleWebSocket(url)
            self.connected = True
            while not self.closed:
                self.inbox.put(self.ws.recv_json())
        except Exception as exc:
            if not self.closed:
                print(f"remote websocket stopped: {exc}", file=sys.stderr)
        finally:
            self.connected = False

    def send(self, data: dict) -> None:
        if self.ws is None or not self.connected:
            return
        try:
            self.ws.send_json(data)
        except Exception:
            self.connected = False

    def close(self) -> None:
        self.closed = True
        if self.ws is not None:
            self.ws.close()


class Egg:
    def __init__(
        self,
        tk,
        root,
        canvas,
        frames,
        sprite_w: int,
        sprite_h: int,
        columns: int,
        rows: int,
        sprite: str,
        remote_session: RemoteSession | None = None,
    ):
        self.tk = tk
        self.root = root
        self.canvas = canvas
        self.frames = frames
        self.sprite_w = sprite_w
        self.sprite_h = sprite_h
        self.columns = columns
        self.rows = rows
        self.screen_w = max(root.winfo_screenwidth(), self.sprite_w)
        self.screen_h = max(root.winfo_screenheight(), self.sprite_h)
        self.x = random.randint(0, max(0, self.screen_w - self.sprite_w))
        self.y = random.randint(80, max(80, self.screen_h - self.sprite_h - 80))
        self.vx = random.choice([-1, 1]) * random.uniform(0.6, 1.3)
        self.vy = random.uniform(-0.25, 0.25)
        self.phase = 0.0
        self.target_change_at = time.time() + random.uniform(4, 9)
        self.sprite = sprite
        self.state = read_state()
        self.frame_index = 0
        self.state_check_at = 0.0
        self.next_frame_at = 0.0
        self.drag_offset_x = 0
        self.drag_offset_y = 0
        self.dragging = False
        self.remote_session = remote_session
        self.next_remote_pose_at = 0.0
        self.last_remote_state = ""

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
            next_sprite, next_state = read_runtime_state()
            if next_sprite != self.sprite:
                self.sprite = next_sprite
                self.frames, self.sprite_w, self.sprite_h, self.columns, self.rows = load_sprite_frames(self.tk, self.sprite)
                self.canvas.configure(width=self.sprite_w, height=self.sprite_h)
                self.root.geometry(f"{self.sprite_w}x{self.sprite_h}+{int(self.x)}+{int(self.y)}")
                self.frame_index = 0
            if next_state != self.state:
                self.state = next_state
                self.frame_index = 0
                self.send_remote_state()
            self.state_check_at = now + 0.2

        if self.frames:
            state_frames, loop = frames_for_state(self.frames, self.sprite, self.state, self.columns, self.rows)
            if isinstance(loop, bool):
                frame_position = self.frame_index % len(state_frames) if loop else min(self.frame_index, len(state_frames) - 1)
                should_advance = loop or self.frame_index < len(state_frames) - 1
            else:
                max_frames = len(state_frames) * loop
                frame_position = self.frame_index % len(state_frames) if self.frame_index < max_frames else len(state_frames) - 1
                should_advance = self.frame_index < max_frames - 1
            frame = state_frames[frame_position]
            self.canvas.create_image(self.sprite_w / 2, self.sprite_h / 2, image=frame)
            if should_advance and now >= self.next_frame_at:
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
        self.send_remote_pose()
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

    def normalized_pose(self) -> dict:
        return {
            "x": min(max(self.x / max(1, self.screen_w - self.sprite_w), 0), 1),
            "y": min(max(self.y / max(1, self.screen_h - self.sprite_h), 0), 1),
            "facing": "right" if self.vx >= 0 else "left",
            "dragging": self.dragging,
        }

    def send_remote_pose(self) -> None:
        if self.remote_session is None:
            return
        now = time.time()
        if now < self.next_remote_pose_at:
            return
        self.next_remote_pose_at = now + 0.25
        self.remote_session.send({"type": "pose", **self.normalized_pose()})
        if self.state != self.last_remote_state:
            self.send_remote_state()

    def send_remote_state(self) -> None:
        if self.remote_session is None:
            return
        self.last_remote_state = self.state
        self.remote_session.send({"type": "state", "state": self.state, "sprite": self.sprite})


class RemoteActor:
    def __init__(self, tk, parent, server_url: str, peer_id: str, sprite_id: str, transparent: str):
        self.tk = tk
        self.peer_id = peer_id
        self.sprite_id = sprite_id
        self.sprite = sprite_id
        self.state = DEFAULT_STATE
        self.config: dict = {}
        self.frames: list = []
        self.sprite_w = DEFAULT_SPRITE_SIZE
        self.sprite_h = DEFAULT_SPRITE_SIZE
        self.columns = 1
        self.rows = 1
        self.frame_index = 0
        self.next_frame_at = 0.0
        self.target_x_norm = random.random()
        self.target_y_norm = random.random()
        self.x_norm = self.target_x_norm
        self.y_norm = self.target_y_norm
        self.screen_w = max(parent.winfo_screenwidth(), self.sprite_w)
        self.screen_h = max(parent.winfo_screenheight(), self.sprite_h)
        self.window = tk.Toplevel(parent)
        self.window.title(f"Egg Remote {peer_id}")
        self.window.overrideredirect(True)
        self.window.resizable(False, False)
        self.window.configure(bg=transparent)
        try:
            self.window.wm_attributes("-topmost", True)
            self.window.wm_attributes("-transparentcolor", transparent)
        except tk.TclError:
            pass
        self.canvas = tk.Canvas(self.window, width=self.sprite_w, height=self.sprite_h, bg=transparent, highlightthickness=0, bd=0)
        self.canvas.pack(fill="both", expand=True)
        self.load(server_url, sprite_id)

    def load(self, server_url: str, sprite_id: str) -> None:
        cached = cache_remote_sprite(server_url, sprite_id)
        if cached is None:
            return
        image, metadata, config_path, sprite = cached
        self.sprite = normalize_sprite(sprite)
        self.frames, self.sprite_w, self.sprite_h, self.columns, self.rows = load_sprite_frames_from_paths(self.tk, image, metadata)
        if config_path is not None:
            self.config = read_json_file(config_path)
        self.canvas.configure(width=self.sprite_w, height=self.sprite_h)
        self.window.geometry(f"{self.sprite_w}x{self.sprite_h}+{int(self.x)}+{int(self.y)}")

    @property
    def x(self) -> float:
        return self.x_norm * max(1, self.screen_w - self.sprite_w)

    @property
    def y(self) -> float:
        return self.y_norm * max(1, self.screen_h - self.sprite_h)

    def update_pose(self, msg: dict) -> None:
        try:
            self.target_x_norm = min(max(float(msg.get("x", self.target_x_norm)), 0), 1)
            self.target_y_norm = min(max(float(msg.get("y", self.target_y_norm)), 0), 1)
        except (TypeError, ValueError):
            pass

    def update_state(self, msg: dict) -> None:
        state = str(msg.get("state", "")).strip()
        if state and state != self.state:
            self.state = state
            self.frame_index = 0

    def tick(self) -> None:
        self.x_norm += (self.target_x_norm - self.x_norm) * 0.18
        self.y_norm += (self.target_y_norm - self.y_norm) * 0.18
        self.window.geometry(f"{self.sprite_w}x{self.sprite_h}+{int(self.x)}+{int(self.y)}")
        self.draw()

    def draw(self) -> None:
        self.canvas.delete("all")
        if not self.frames:
            self.canvas.create_oval(74, 34, 177, 199, fill="#d6ecff", outline="#25455f", width=3)
            return
        state_frames, loop = frames_for_state_with_config(
            self.frames,
            self.sprite,
            self.state,
            self.columns,
            self.rows,
            self.config,
        )
        if not state_frames:
            return
        now = time.time()
        if isinstance(loop, bool):
            frame_position = self.frame_index % len(state_frames) if loop else min(self.frame_index, len(state_frames) - 1)
            should_advance = loop or self.frame_index < len(state_frames) - 1
        else:
            max_frames = len(state_frames) * loop
            frame_position = self.frame_index % len(state_frames) if self.frame_index < max_frames else len(state_frames) - 1
            should_advance = self.frame_index < max_frames - 1
        self.canvas.create_image(self.sprite_w / 2, self.sprite_h / 2, image=state_frames[frame_position])
        if should_advance and now >= self.next_frame_at:
            self.frame_index += 1
            self.next_frame_at = now + ANIMATION_MS / 1000

    def destroy(self) -> None:
        try:
            self.window.destroy()
        except Exception:
            pass


def run_gui() -> int:
    try:
        import tkinter as tk
    except Exception as exc:  # pragma: no cover - depends on local Python build
        print(f"Tkinter is required to display the desktop companion: {exc}", file=sys.stderr)
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

    sprite, _ = read_runtime_state()
    frames, sprite_w, sprite_h, columns, rows = load_sprite_frames(tk, sprite)
    remote_inbox: queue.Queue = queue.Queue()
    remote_session: RemoteSession | None = RemoteSession(remote_inbox) if remote_enabled() else None
    if remote_session is not None:
        remote_session.start()
    remote_actors: dict[str, RemoteActor] = {}
    remote_signature = ""

    def active_remote_signature() -> str:
        remote = read_remote_config()
        if not remote.get("enabled"):
            return "off"
        return "|".join(
            [
                str(remote.get("server_url", "")),
                str(remote.get("mode", "")),
                str(remote.get("room", "")),
                str(remote.get("sprite_id", "")),
            ]
        )

    remote_signature = active_remote_signature()
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
        if remote_session is not None:
            remote_session.close()
        for actor in list(remote_actors.values()):
            actor.destroy()
        clear_pid()
        try:
            root.destroy()
        except tk.TclError:
            pass

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)

    egg = Egg(tk, root, canvas, frames, sprite_w, sprite_h, columns, rows, sprite, remote_session)
    canvas.bind("<ButtonPress-1>", egg.begin_drag)
    canvas.bind("<B1-Motion>", egg.drag_to)
    canvas.bind("<ButtonRelease-1>", egg.end_drag)
    root.geometry(f"{sprite_w}x{sprite_h}+{int(egg.x)}+{int(egg.y)}")

    def process_remote_events() -> None:
        nonlocal remote_session, remote_signature
        next_signature = active_remote_signature()
        if next_signature != remote_signature:
            remote_signature = next_signature
            if remote_session is not None:
                remote_session.close()
                remote_session = None
            for actor in list(remote_actors.values()):
                actor.destroy()
            remote_actors.clear()
            if next_signature != "off":
                remote_session = RemoteSession(remote_inbox)
                remote_session.start()
                egg.remote_session = remote_session
            else:
                egg.remote_session = None

        remote = read_remote_config()
        if not remote.get("enabled"):
            while True:
                try:
                    remote_inbox.get_nowait()
                except queue.Empty:
                    break
            root.after(FRAME_MS, process_remote_events)
            return
        server_url = remote["server_url"]
        while True:
            try:
                msg = remote_inbox.get_nowait()
            except queue.Empty:
                break
            peer_id = str(msg.get("peer_id", ""))
            if not peer_id:
                continue
            msg_type = msg.get("type")
            if msg_type == "peer_left":
                actor = remote_actors.pop(peer_id, None)
                if actor is not None:
                    actor.destroy()
                continue
            sprite_id = str(msg.get("sprite_id", ""))
            if msg_type == "peer_joined" and sprite_id:
                if peer_id not in remote_actors:
                    remote_actors[peer_id] = RemoteActor(tk, root, server_url, peer_id, sprite_id, transparent)
                continue
            actor = remote_actors.get(peer_id)
            if actor is None and sprite_id:
                actor = RemoteActor(tk, root, server_url, peer_id, sprite_id, transparent)
                remote_actors[peer_id] = actor
            if actor is None:
                continue
            if msg_type == "peer_pose":
                actor.update_pose(msg)
            elif msg_type == "peer_state":
                actor.update_state(msg)
            elif msg_type == "peer_action":
                actor.update_state({"state": msg.get("action", actor.state)})
        for actor in list(remote_actors.values()):
            actor.tick()
        root.after(FRAME_MS, process_remote_events)

    root.after(0, egg.tick)
    root.after(0, process_remote_events)

    try:
        root.mainloop()
    finally:
        if remote_session is not None:
            remote_session.close()
        clear_pid()
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Manage the desktop sprite companion.")
    parser.add_argument(
        "command",
        choices=["start", "run", "stop", "restart", "status", "spritesheet", "state", "sprite", "remote"],
    )
    parser.add_argument("path", nargs="?", help="PNG spritesheet path, sprite name, companion state, or remote action.")
    parser.add_argument("name", nargs="?", help="Optional sprite name, remote value, or room code.")
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
            sprite, state = read_runtime_state()
            print(f"current companion state: {state}; sprite={sprite}")
            print(f"choices: {', '.join(animation_names(sprite))}")
            return 0
        return set_state(args.path, args.name)
    if args.command == "sprite":
        if not args.path:
            print(f"current companion sprite: {read_sprite()}")
            return 0
        return set_sprite(args.path)
    if args.command == "spritesheet":
        if not args.path:
            print("usage: egg_desktop.py spritesheet /path/to/name.png [name]", file=sys.stderr)
            return 2
        return install_spritesheet(args.path, args.name)
    if args.command == "remote":
        return remote_command(args.path, args.name)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
