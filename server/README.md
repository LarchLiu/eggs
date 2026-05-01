# Eggs Remote Server

Standalone Go server for the remote sprite library and cross-user desktop companion interaction.

It is intentionally outside the `eggs/` skill directory. The skill can be installed by users without bundling or running a server; this service is deployed separately by whoever wants to host a shared sprite lobby.

## Build

```bash
cd server
go build -o eggs-server .
```

The server uses `modernc.org/sqlite`, a pure Go SQLite implementation. The built server does not require a target machine to have a SQLite dynamic library installed.

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
- `GET /api/v1/sprites`: list public sprites. Use `?random=1&limit=10` for random results.
- `GET /api/v1/sprites/{sprite_id}`: fetch one sprite metadata record.
- `GET /assets/{sprite_id}/sprite.png`: fetch uploaded PNG.
- `GET /assets/{sprite_id}/sprite.json`: fetch uploaded spritesheet metadata.
- `GET /assets/{sprite_id}/config.json`: fetch optional animation config.

## WebSocket

```text
GET /ws?device_id=<id>&mode=random|room&room=<code>&sprite_id=<id>
```

Client messages:

- `{"type":"pose","x":0.4,"y":0.7,"facing":"left"}`
- `{"type":"state","state":"walk"}`
- `{"type":"action","action":"roar"}`
- `{"type":"heartbeat"}`

Broadcast messages:

- `peer_joined`
- `peer_left`
- `peer_pose`
- `peer_state`
- `peer_action`

The server is owner-authoritative: it stores metadata and forwards room messages, but it does not simulate movement or gameplay.
