#!/usr/bin/env python3
import json
import os
import pathlib
import shutil
import subprocess
import sys
import time
import uuid
from typing import Any


ROOT = pathlib.Path(__file__).resolve().parents[2]
LOCAL_DEBUG_EGGS = ROOT / "desktop" / "src-tauri" / "target" / "debug"
LOCAL_RELEASE_EGGS = ROOT / "desktop" / "src-tauri" / "target" / "release"


def read_payload() -> dict[str, Any]:
    try:
        raw = sys.stdin.read()
        if not raw.strip():
            return {}
        payload = json.loads(raw)
        if isinstance(payload, dict):
            return payload
    except Exception:
        pass
    return {}


def shorten(text: str, limit: int = 110) -> str:
    compact = " ".join((text or "").split())
    if len(compact) <= limit:
        return compact
    return compact[: max(0, limit - 1)].rstrip() + "…"


def send_hook(text: str) -> None:
    content = (text)
    if not content:
        return
    exe = resolve_eggs_exe()
    if exe:
        try:
            result = subprocess.run(
                [exe, "hook", content],
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            if result.returncode == 0:
                return
        except Exception:
            pass
    write_bubble_spool(content)


def resolve_eggs_exe() -> str | None:
    windows = os.name == "nt"
    binary_names = ["eggs.exe", "eggs"] if windows else ["eggs", "eggs.exe"]
    bin_dir = pathlib.Path(os.environ.get("EGGS_BIN_DIR", str(pathlib.Path.home() / ".eggs" / "bin")))
    candidates: list[pathlib.Path] = []
    for name in binary_names:
        candidates.append(LOCAL_DEBUG_EGGS / name)
        candidates.append(bin_dir / name)
        candidates.append(LOCAL_RELEASE_EGGS / name)
    for candidate in candidates:
        if candidate.exists() and candidate.is_file() and os_access_execute(candidate):
            return str(candidate)
    for name in binary_names:
        on_path = shutil.which(name)
        if on_path:
            return on_path
    return None


def os_access_execute(path: pathlib.Path) -> bool:
    if os.name == "nt":
        return path.exists() and path.is_file()
    try:
        return path.stat().st_mode & 0o111 != 0
    except Exception:
        return False


def write_bubble_spool(text: str) -> None:
    app_dir = pathlib.Path.home() / ".eggs"
    spool_dir = app_dir / "bubble-spool"
    event_id = f"hook-{int(time.time() * 1000):x}-{uuid.uuid4().hex[:8]}"
    payload = {
        "id": event_id,
        "owner": {"kind": "local"},
        "source": "hook",
        "text": text,
        "ttl_ms": 8000,
        "created_ms": int(time.time() * 1000),
        "room_code": None,
        "device_id": None,
    }
    try:
        spool_dir.mkdir(parents=True, exist_ok=True)
        tmp = spool_dir / f"{event_id}.tmp"
        final = spool_dir / f"{event_id}.json"
        tmp.write_text(json.dumps(payload), encoding="utf-8")
        tmp.replace(final)
    except Exception:
        pass


def done() -> None:
    sys.stdout.write('{"continue": true}\n')
