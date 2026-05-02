# Eggs Remote Server

Standalone Go server for the remote sprite library and cross-user desktop companion interaction.

It is intentionally outside the `eggs/` skill directory. The skill can be installed by users without bundling or running a server; this service is deployed separately by whoever wants to host a shared sprite service with invite rooms and random matching.

## Build

```bash
cd server
go build -o eggs-server .
```

The server uses `modernc.org/sqlite`, a pure Go SQLite implementation. The built server does not require a target machine to have a SQLite dynamic library installed.

## Docker Compose

```bash
cd server
docker compose up -d --build
```

This uses:

- [Dockerfile](/Users/alex/Work/cloudgeek/eggs/server/Dockerfile)
- [docker-compose.yml](/Users/alex/Work/cloudgeek/eggs/server/docker-compose.yml)

Persistent data is stored in [data](</Users/alex/Work/cloudgeek/eggs/server/data>) on the host and mounted to `/data` in the container.

Useful environment overrides:

- `EGGS_SERVER_PORT=8787`
- `EGGS_BASE_URL=http://your-host:8787`
- `EGGS_PUBLIC_BY_DEFAULT=true`

Example:

```bash
cd server
EGGS_BASE_URL=https://eggs.example.com \
EGGS_SERVER_PORT=8787 \
docker compose up -d --build
```

## Run

```bash
./eggs-server \
  -addr :8787 \
  -data ~/.codex/eggs-server \
  -base-url http://localhost:8787
```

Flags:

- `-addr`: HTTP listen address.
- `-data`: directory for `eggs.db` and uploaded sprite assets.
- `-base-url`: public URL used in API asset links. If omitted, the server derives it from each request host.
- `-public-by-default`: publish uploads immediately. Set `false` to keep uploaded sprites pending.

## HTTP API

- `POST /api/v1/sprites`: multipart upload with `png`, `json`, optional `config`, and fields `device_id`, `sprite_name`, `display_name`.
- `GET /api/v1/sprites`: list public sprites. Use `?random=1&limit=10` for random results. The random list path uses an index-friendly random cursor strategy rather than `ORDER BY random()`.
- `GET /api/v1/sprites/{sprite_id}`: fetch one sprite metadata record for public listing or upload management.
- `GET /assets/{sprite_id}/sprite.png`: fetch uploaded PNG.
- `GET /assets/{sprite_id}/sprite.json`: fetch uploaded spritesheet metadata.
- `GET /assets/{sprite_id}/config.json`: fetch optional animation config.

## WebSocket

```text
GET /ws?device_id=<id>&sprite=<name>&mode=random|room&room=<code>
```

All live interaction is strictly 1-to-1. When `mode=random`, the server places the client into a waiting pool until it can pair with exactly one other online peer using a different sprite. After a match is found, the server creates a temporary private room for that pair. The same uploaded `sprite_id` cannot be online more than once anywhere on the server. Invite rooms are also capped at two peers.

Client messages:

- `{"type":"state","state":"walk"}`
- `{"type":"action","action":"roar"}`

Broadcast messages:

- `room_snapshot`
- `peer_joined`
- `peer_left`
- `peer_state`
- `peer_action`

Newly joined clients first receive a `room_snapshot` containing the currently online peers in that room, including each peer's current sprite metadata plus latest known state. Incremental updates then arrive as `peer_joined`, `peer_left`, `peer_state`, and `peer_action`.

Each peer broadcast includes `peer_id`, `device_id`, and an embedded `sprite` object with that peer's sprite metadata and asset URLs. Room interaction does not depend on `/api/v1/sprites/{sprite_id}` lookups.

The server is owner-authoritative: it stores metadata and forwards room messages, but it does not simulate movement or gameplay.

## Smoke Test

Use the simple smoke script to check 1-to-1 matching behavior:

```bash
cd server
python3 scripts/room_broadcast_smoke.py \
  --server http://127.0.0.1:8787 \
  --mode random \
  --pairs 12
```

The script opens many clients in pairs, sends one round of `state` broadcasts, and checks that each online peer is paired with exactly one other peer.

## Concurrency Notes

- Online peer presence is maintained in memory by the WebSocket hub.
- Each online peer keeps only its current sprite metadata in memory for the lifetime of that WebSocket session.
- Invite rooms, the random waiting pool, and temporary pair rooms are in-memory only; the database is used for uploaded sprite metadata, not live room/session state.
- When a peer disconnects, its in-memory presence and attached sprite metadata are removed together.
