#!/usr/bin/env python3
"""Simple 1-to-1 matchmaking smoke test for the eggs remote server."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import socket
import ssl
import struct
import subprocess
import threading
import time
from dataclasses import dataclass, field
from typing import Any
from urllib import parse, request


def http_json(url: str, timeout: float = 10.0) -> dict[str, Any]:
    with request.urlopen(url, timeout=timeout) as response:
        data = response.read()
    parsed = json.loads(data.decode("utf-8"))
    return parsed if isinstance(parsed, dict) else {}


class SimpleWebSocket:
    def __init__(self, url: str, timeout: float = 10.0):
        parsed = parse.urlparse(url)
        port = parsed.port or (443 if parsed.scheme == "wss" else 80)
        self.sock = socket.create_connection((parsed.hostname, port), timeout=timeout)
        if parsed.scheme == "wss":
            self.sock = ssl.create_default_context().wrap_socket(self.sock, server_hostname=parsed.hostname)
        self.sock.settimeout(timeout)
        key = base64.b64encode(os.urandom(16)).decode("ascii")
        path = parsed.path or "/"
        if parsed.query:
            path += "?" + parsed.query
        req = (
            f"GET {path} HTTP/1.1\r\n"
            f"Host: {parsed.netloc}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n\r\n"
        )
        self.sock.sendall(req.encode("ascii"))
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

    def send_json(self, data: dict[str, Any]) -> None:
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

    def recv_json(self) -> dict[str, Any]:
        payload = self._recv_frame()
        data = json.loads(payload.decode("utf-8"))
        return data if isinstance(data, dict) else {}

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass

    def _recv_exact(self, count: int) -> bytes:
        chunks: list[bytes] = []
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
        while True:
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
            if opcode == 0x9:
                self._send_control(0xA, payload)
                continue
            if opcode == 0xA:
                continue
            if opcode != 0x1:
                return b"{}"
            return payload

    def _send_control(self, opcode: int, payload: bytes = b"") -> None:
        header = bytearray([0x80 | (opcode & 0x0F)])
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


def websocket_url(server_url: str, query: dict[str, str]) -> str:
    parsed = parse.urlparse(server_url)
    scheme = "wss" if parsed.scheme == "https" else "ws"
    prefix = parsed.path.rstrip("/")
    return parse.urlunparse((scheme, parsed.netloc, prefix + "/ws", "", parse.urlencode(query), ""))


@dataclass
class PeerStats:
    device_id: str
    sprite: str
    snapshots: int = 0
    joins: int = 0
    states: int = 0
    lefts: int = 0
    errors: list[str] = field(default_factory=list)


@dataclass
class ProcessSample:
    timestamp: float
    cpu_percent: float
    rss_kb: int


def receive_for(ws: SimpleWebSocket, seconds: float, stats: PeerStats) -> None:
    deadline = time.time() + seconds
    ws.sock.settimeout(0.2)
    while time.time() < deadline:
        try:
            msg = ws.recv_json()
        except TimeoutError:
            continue
        except socket.timeout:
            continue
        except Exception as exc:
            stats.errors.append(str(exc))
            return
        kind = str(msg.get("type", ""))
        if kind == "room_snapshot":
            stats.snapshots += 1
            peers = msg.get("peers", [])
            if isinstance(peers, list):
                stats.joins += len(peers)
        elif kind == "peer_joined":
            stats.joins += 1
        elif kind == "peer_state":
            stats.states += 1
        elif kind == "peer_left":
            stats.lefts += 1


def read_process_sample(pid: int) -> ProcessSample | None:
    result = subprocess.run(
        ["ps", "-p", str(pid), "-o", "%cpu=,rss="],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return None
    line = result.stdout.strip()
    if not line:
        return None
    parts = line.split()
    if len(parts) < 2:
        return None
    try:
        cpu_percent = float(parts[0])
        rss_kb = int(parts[1])
    except ValueError:
        return None
    return ProcessSample(timestamp=time.time(), cpu_percent=cpu_percent, rss_kb=rss_kb)


def sample_process_until(pid: int, interval: float, stop_event: threading.Event, output: list[ProcessSample]) -> None:
    while not stop_event.is_set():
        sample = read_process_sample(pid)
        if sample is not None:
            output.append(sample)
        stop_event.wait(interval)


def summarize_process_samples(pid: int, samples: list[ProcessSample]) -> dict[str, Any]:
    if not samples:
        return {"pid": pid, "sample_count": 0}
    cpu_values = [sample.cpu_percent for sample in samples]
    rss_values_mb = [sample.rss_kb / 1024.0 for sample in samples]
    return {
        "pid": pid,
        "sample_count": len(samples),
        "cpu_percent": {
            "min": round(min(cpu_values), 3),
            "avg": round(sum(cpu_values) / len(cpu_values), 3),
            "max": round(max(cpu_values), 3),
        },
        "rss_mb": {
            "min": round(min(rss_values_mb), 3),
            "avg": round(sum(rss_values_mb) / len(rss_values_mb), 3),
            "max": round(max(rss_values_mb), 3),
        },
    }


def load_peer_file(path: str) -> list[tuple[str, str]]:
    with open(path, "r", encoding="utf-8") as fh:
        data = json.load(fh)
    raw_peers = data.get("peers") if isinstance(data, dict) else data
    if not isinstance(raw_peers, list):
        raise SystemExit("peers file must contain a peers array")
    peers: list[tuple[str, str]] = []
    for item in raw_peers:
        if not isinstance(item, dict):
            continue
        device_id = str(item.get("device_id") or item.get("owner_device_id") or "").strip()
        sprite = str(item.get("sprite") or item.get("name") or "").strip()
        if device_id and sprite:
            peers.append((device_id, sprite))
    return list(dict.fromkeys(peers))


def load_public_peers(server_url: str, limit: int) -> list[tuple[str, str]]:
    sprites = http_json(server_url.rstrip("/") + f"/api/v1/sprites?limit={limit}").get("sprites", [])
    if not isinstance(sprites, list) or not sprites:
        raise SystemExit("no public sprites available on server")
    peers: list[tuple[str, str]] = []
    for item in sprites:
        device_id = str(item.get("owner_device_id") or "").strip()
        sprite = str(item.get("name") or "").strip()
        if device_id and sprite:
            peers.append((device_id, sprite))
    return list(dict.fromkeys(peers))


def display_path(path: str) -> str:
	if not path:
		return ""
	absolute = os.path.abspath(path)
	try:
		return os.path.relpath(absolute, os.getcwd())
	except ValueError:
		return path


def main() -> int:
    parser = argparse.ArgumentParser(description="Smoke test eggs 1-to-1 remote matching behavior.")
    parser.add_argument("--server", default="http://127.0.0.1:8787")
    parser.add_argument("--room", default="SMOKE")
    parser.add_argument("--pairs", type=int, default=12)
    parser.add_argument("--listen-seconds", type=float, default=2.0)
    parser.add_argument("--mode", choices=["room", "random"], default="room")
    parser.add_argument("--sprite-limit", type=int, default=0)
    parser.add_argument("--pid", type=int, default=0, help="optional server process id to sample with ps")
    parser.add_argument("--sample-interval", type=float, default=0.5, help="seconds between ps samples when --pid is set")
    parser.add_argument("--output", default="", help="optional path to write the JSON result")
    parser.add_argument("--peers-file", default="", help="optional JSON file with device_id + sprite pairs")
    args = parser.parse_args()

    needed = max(2, args.pairs * 2)
    sprite_limit = args.sprite_limit if args.sprite_limit > 0 else max(needed, 100)

    sockets: list[SimpleWebSocket] = []
    stats: list[PeerStats] = []
    process_samples: list[ProcessSample] = []
    sample_stop = threading.Event()
    sample_thread: threading.Thread | None = None
    try:
        unique_peers = load_peer_file(args.peers_file) if args.peers_file else load_public_peers(args.server, sprite_limit)
        if len(unique_peers) < needed:
            raise SystemExit(f"need at least {needed} unique public device_id+sprite pairs, found {len(unique_peers)}")

        for index in range(needed):
            device_id, sprite = unique_peers[index]
            room = args.room if args.mode == "room" else ""
            url = websocket_url(
                args.server,
                {
                    "device_id": device_id,
                    "sprite": sprite,
                    "mode": args.mode,
                    "room": room,
                },
            )
            sockets.append(SimpleWebSocket(url))
            stats.append(PeerStats(device_id=device_id, sprite=sprite))
        if args.pid > 0:
            sample_thread = threading.Thread(
                target=sample_process_until,
                args=(args.pid, max(0.05, args.sample_interval), sample_stop, process_samples),
                daemon=True,
            )
            sample_thread.start()
        threads = [
            threading.Thread(target=receive_for, args=(ws, args.listen_seconds, stat), daemon=True)
            for ws, stat in zip(sockets, stats)
        ]
        for thread in threads:
            thread.start()
        time.sleep(0.3)
        for index, ws in enumerate(sockets):
            ws.send_json({"type": "state", "state": f"walk-{index % 3}"})
        for thread in threads:
            thread.join()
    finally:
        sample_stop.set()
        if sample_thread is not None:
            sample_thread.join(timeout=1.0)
        for ws in sockets:
            ws.close()

    expected = 1
    min_joins = min(stat.joins for stat in stats) if stats else 0
    max_joins = max((stat.joins for stat in stats), default=0)
    min_states = min(stat.states for stat in stats) if stats else 0
    max_states = max((stat.states for stat in stats), default=0)
    output: dict[str, Any] = {
        "mode": args.mode,
        "room": args.room if args.mode == "room" else "",
        "pairs": args.pairs,
        "peers": needed,
        "peers_file": display_path(args.peers_file),
        "expected_per_peer": expected,
        "min_room_snapshot": min((stat.snapshots for stat in stats), default=0),
        "min_peer_joined": min_joins,
        "min_peer_state": min_states,
        "max_room_snapshot": max((stat.snapshots for stat in stats), default=0),
        "max_peer_joined": max_joins,
        "max_peer_state": max_states,
        "errors": [stat.errors for stat in stats if stat.errors],
    }
    if args.pid > 0:
        output["process"] = summarize_process_samples(args.pid, process_samples)
    if args.output:
        os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
        with open(args.output, "w", encoding="utf-8") as fh:
            json.dump(output, fh, ensure_ascii=False, indent=2)
            fh.write("\n")
    print(json.dumps(output, ensure_ascii=False, indent=2))
    if min_joins != expected or max_joins != expected:
        return 1
    if min_states > expected or max_states > expected:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
