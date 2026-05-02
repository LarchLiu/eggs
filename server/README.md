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
- `GET /api/v1/sprites?device_id=<id>&sprite_name=<name>`: fetch the latest sprite metadata record for one owner/name pair, or `{"sprite":null}` when that owner has not uploaded the named sprite.
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
- `{"type":"sprite","sprite":"dino"}` updates the current live peer sprite after the client has confirmed the server already has matching assets.

Broadcast messages:

- `room_snapshot`
- `peer_joined`
- `peer_left`
- `peer_state`
- `peer_action`
- `peer_sprite_changed`

Newly joined clients first receive a `room_snapshot` containing the currently online peers in that room, including each peer's current sprite metadata plus latest known state. Incremental updates then arrive as `peer_joined`, `peer_left`, `peer_state`, `peer_action`, and `peer_sprite_changed`.

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

Important: the script connects using the real `owner_device_id + sprite name` pairs returned by `GET /api/v1/sprites`. It is not enough to have many sprite names; the server must already contain enough uploaded public sprites for the target peer count.

Seed benchmark sprites first when you are testing an empty local server:

```bash
cd server
python3 scripts/seed_benchmark_sprites.py \
  --server http://127.0.0.1:8787 \
  --count 1000 \
  --prefix bench \
  --peers-output ./benchmarks/bench-1000peers.json
```

### Save Results To A File

Use `--output` to keep each run as a JSON artifact for later comparison:

```bash
cd server
python3 scripts/room_broadcast_smoke.py \
  --server http://127.0.0.1:8787 \
  --mode random \
  --pairs 500 \
  --listen-seconds 10 \
  --peers-file ./benchmarks/bench-1000peers.json \
  --output ./benchmarks/random-1000peers.json
```

### Sample CPU And Memory

If you also want CPU and memory numbers, pass the server process id with `--pid`. The script samples `%CPU` and `RSS` via `ps` during the test and adds min/avg/max values to the JSON output.

Example:

```bash
cd server
SERVER_PID="$(pgrep -f './eggs-server|-addr :8787' | head -n 1)"
python3 scripts/room_broadcast_smoke.py \
  --server http://127.0.0.1:8787 \
  --mode random \
  --pairs 500 \
  --listen-seconds 10 \
  --peers-file ./benchmarks/bench-1000peers.json \
  --pid "$SERVER_PID" \
  --sample-interval 0.5 \
  --output ./benchmarks/random-1000peers-with-metrics.json
```

The output JSON includes:

- `min_room_snapshot`, `max_room_snapshot`
- `min_peer_joined`, `max_peer_joined`
- `min_peer_state`, `max_peer_state`
- `errors`
- `process.cpu_percent.min|avg|max` when `--pid` is set
- `process.rss_mb.min|avg|max` when `--pid` is set

### Recommended Comparison Workflow

When you change the matchmaking or WebSocket code:

1. Run the same `--pairs`, `--listen-seconds`, and `--sample-interval` values as a previous benchmark.
2. Save the new result to a fresh file under `server/benchmarks/` or another timestamped folder.
3. Compare:
   - connection correctness: `min_peer_joined`, `max_peer_joined`, `min_peer_state`, `max_peer_state`
   - resource usage: `process.cpu_percent.max`, `process.rss_mb.max`
   - regressions: any non-empty `errors`

For a quick local stress target, `--pairs 500` means `1000` concurrent peers.

The repository currently includes one functional baseline sample at [benchmarks/random-1000peers-short-success.json](benchmarks/random-1000peers-short-success.json). It captures a successful short `1000`-peer run without CPU/RSS metrics. If a local run returns `HTTP 502`, verify that the Go server is actually listening with `lsof -nP -iTCP:8787 -sTCP:LISTEN` and that `/healthz` returns `200`; in this workspace the earlier `502` came from a detached temporary server that never started, so the request hit the local proxy layer instead of the Go server.

## Concurrency Notes

- Online peer presence is maintained in memory by the WebSocket hub.
- Each online peer keeps only its current sprite metadata in memory for the lifetime of that WebSocket session.
- Invite rooms, the random waiting pool, and temporary pair rooms are in-memory only; the database is used for uploaded sprite metadata, not live room/session state.
- When a peer disconnects, its in-memory presence and attached sprite metadata are removed together.
