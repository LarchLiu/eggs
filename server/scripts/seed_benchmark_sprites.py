#!/usr/bin/env python3
"""Upload simple public sprites for remote server benchmarks."""

from __future__ import annotations

import argparse
import json
import os
import time
from urllib import request


PNG_1X1_HEADER = bytes([0x89, 80, 78, 71, 13, 10, 26, 10])


def multipart_body(fields: dict[str, str], files: dict[str, tuple[str, bytes, str]], boundary: str) -> bytes:
    parts: list[bytes] = []
    for name, value in fields.items():
        parts.append(
            f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{value}\r\n'.encode()
        )
    for name, (filename, content, content_type) in files.items():
        parts.append(
            f'--{boundary}\r\n'
            f'Content-Disposition: form-data; name="{name}"; filename="{filename}"\r\n'
            f"Content-Type: {content_type}\r\n\r\n".encode()
            + content
            + b"\r\n"
        )
    parts.append(f"--{boundary}--\r\n".encode())
    return b"".join(parts)


def upload_sprite(server_url: str, prefix: str, index: int) -> int:
    sprite = f"{prefix}{index:04d}"
    device = f"{prefix}-device-{index:04d}"
    metadata = json.dumps(
        {
            "frameWidth": 251,
            "frameHeight": 251,
            "columns": 1,
            "rows": 1,
            "frameCount": 1,
            "image": f"{sprite}.png",
        },
        separators=(",", ":"),
    ).encode("utf-8")
    boundary = "----eggsbenchboundary"
    body = multipart_body(
        {
            "device_id": device,
            "sprite_name": sprite,
            "display_name": sprite,
        },
        {
            "png": ("sprite.png", PNG_1X1_HEADER, "image/png"),
            "json": ("sprite.json", metadata, "application/json"),
        },
        boundary,
    )
    req = request.Request(
        server_url.rstrip("/") + "/api/v1/sprites",
        data=body,
        method="POST",
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"},
    )
    with request.urlopen(req, timeout=10) as resp:
        return resp.status


def display_path(path: str) -> str:
    if not path:
        return ""
    absolute = os.path.abspath(path)
    try:
        return os.path.relpath(absolute, os.getcwd())
    except ValueError:
        return path


def main() -> int:
    parser = argparse.ArgumentParser(description="Upload benchmark sprites for the eggs remote server.")
    parser.add_argument("--server", default="http://127.0.0.1:8787")
    parser.add_argument("--count", type=int, default=1000)
    parser.add_argument("--prefix", default="bench")
    parser.add_argument("--progress-every", type=int, default=250)
    parser.add_argument("--peers-output", default="", help="optional JSON file with uploaded device_id + sprite pairs")
    args = parser.parse_args()

    started = time.time()
    created = 0
    reused = 0
    peers: list[dict[str, str]] = []
    for index in range(args.count):
        status = upload_sprite(args.server, args.prefix, index)
        if status == 201:
            created += 1
        elif status == 200:
            reused += 1
        else:
            raise SystemExit(f"upload {index} returned HTTP {status}")
        peers.append(
            {
                "device_id": f"{args.prefix}-device-{index:04d}",
                "sprite": f"{args.prefix}{index:04d}",
            }
        )
        if args.progress_every > 0 and index > 0 and index % args.progress_every == 0:
            print(f"uploaded {index}", flush=True)

    output = {
        "server": args.server,
        "count": args.count,
        "prefix": args.prefix,
        "created": created,
        "reused": reused,
        "seconds": round(time.time() - started, 3),
    }
    if args.peers_output:
        with open(args.peers_output, "w", encoding="utf-8") as fh:
            json.dump({"server": args.server, "prefix": args.prefix, "peers": peers}, fh, ensure_ascii=False, indent=2)
            fh.write("\n")
        output["peers_output"] = display_path(args.peers_output)
    print(json.dumps(output, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
