package main

import (
	"bufio"
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha1"
	"crypto/sha256"
	"database/sql"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"mime/multipart"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"

	_ "modernc.org/sqlite"
)

const (
	maxPNGBytes             = 8 << 20
	maxJSONBytes            = 1 << 20
	maxConfigBytes          = 256 << 10
	maxFrameSize            = 1024
	maxFrameCount           = 512
	maxGridSide             = 64
	defaultHeartbeatTimeout = 12 * time.Second
)

type server struct {
	db               *sql.DB
	dataDir          string
	assetsDir        string
	baseURL          string
	publicByDefault  bool
	heartbeatTimeout time.Duration
	hub              *hub
}

type spriteRecord struct {
	ID            string  `json:"id"`
	OwnerDeviceID string  `json:"owner_device_id"`
	Name          string  `json:"name"`
	DisplayName   string  `json:"display_name"`
	Status        string  `json:"status"`
	PNGURL        string  `json:"png_url"`
	JSONURL       string  `json:"json_url"`
	ConfigURL     *string `json:"config_url,omitempty"`
	PNGHash       string  `json:"png_hash"`
	JSONHash      string  `json:"json_hash"`
	ConfigHash    *string `json:"config_hash,omitempty"`
	CreatedAt     string  `json:"created_at"`
}

type sheetMetadata struct {
	FrameWidth  int    `json:"frameWidth"`
	FrameHeight int    `json:"frameHeight"`
	Columns     int    `json:"columns"`
	Rows        int    `json:"rows"`
	FrameCount  int    `json:"frameCount"`
	Image       string `json:"image"`
}

type wsClient struct {
	id       string
	roomID   int64
	roomCode string
	deviceID string
	spriteID string
	conn     net.Conn
	reader   *bufio.Reader
	writerMu sync.Mutex
}

type hub struct {
	mu    sync.Mutex
	rooms map[string]map[string]*wsClient
	byID  map[string]*wsClient
}

func main() {
	addr := flag.String("addr", ":8787", "HTTP listen address")
	dataDir := flag.String("data", filepath.Join(homeDir(), ".codex", "eggs-server"), "server data directory")
	baseURL := flag.String("base-url", "", "public base URL; defaults to request host")
	publicByDefault := flag.Bool("public-by-default", true, "mark uploaded sprites public immediately")
	flag.Parse()

	if err := os.MkdirAll(filepath.Join(*dataDir, "assets"), 0o755); err != nil {
		log.Fatal(err)
	}
	db, err := sql.Open("sqlite", filepath.Join(*dataDir, "eggs.db"))
	if err != nil {
		log.Fatal(err)
	}
	db.SetMaxOpenConns(1)
	s := &server{
		db:               db,
		dataDir:          *dataDir,
		assetsDir:        filepath.Join(*dataDir, "assets"),
		baseURL:          strings.TrimRight(*baseURL, "/"),
		publicByDefault:  *publicByDefault,
		heartbeatTimeout: defaultHeartbeatTimeout,
		hub:              &hub{rooms: map[string]map[string]*wsClient{}, byID: map[string]*wsClient{}},
	}
	if err := s.migrate(); err != nil {
		log.Fatal(err)
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/api/v1/sprites", s.handleSprites)
	mux.HandleFunc("/api/v1/sprites/", s.handleSprite)
	mux.HandleFunc("/api/v1/peers/", s.handlePeerSprite)
	mux.HandleFunc("/assets/", s.handleAsset)
	mux.HandleFunc("/ws", s.handleWebSocket)
	mux.HandleFunc("/healthz", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, http.StatusOK, map[string]string{"ok": "true"})
	})

	log.Printf("eggs server listening on %s, data=%s", *addr, *dataDir)
	log.Fatal(http.ListenAndServe(*addr, mux))
}

func (s *server) migrate() error {
	stmts := []string{
		`CREATE TABLE IF NOT EXISTS devices (
			id TEXT PRIMARY KEY,
			created_at TEXT NOT NULL,
			last_seen_at TEXT NOT NULL
		)`,
		`CREATE TABLE IF NOT EXISTS sprites (
			id TEXT PRIMARY KEY,
			owner_device_id TEXT NOT NULL,
			name TEXT NOT NULL,
			display_name TEXT NOT NULL,
			status TEXT NOT NULL,
			png_path TEXT NOT NULL,
			json_path TEXT NOT NULL,
			config_path TEXT,
			png_hash TEXT NOT NULL DEFAULT '',
			json_hash TEXT NOT NULL DEFAULT '',
			config_hash TEXT,
			created_at TEXT NOT NULL
		)`,
		`CREATE TABLE IF NOT EXISTS rooms (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			code TEXT NOT NULL UNIQUE,
			mode TEXT NOT NULL,
			created_at TEXT NOT NULL
		)`,
		`CREATE TABLE IF NOT EXISTS sessions (
			id TEXT PRIMARY KEY,
			room_id INTEGER NOT NULL,
			device_id TEXT NOT NULL,
			sprite_id TEXT NOT NULL,
			last_seen_at TEXT NOT NULL
		)`,
	}
	for _, stmt := range stmts {
		if _, err := s.db.Exec(stmt); err != nil {
			return err
		}
	}
	for _, stmt := range []string{
		`ALTER TABLE sprites ADD COLUMN png_hash TEXT NOT NULL DEFAULT ''`,
		`ALTER TABLE sprites ADD COLUMN json_hash TEXT NOT NULL DEFAULT ''`,
		`ALTER TABLE sprites ADD COLUMN config_hash TEXT`,
	} {
		if _, err := s.db.Exec(stmt); err != nil && !strings.Contains(strings.ToLower(err.Error()), "duplicate column name") {
			return err
		}
	}
	return nil
}

func (s *server) handleSprites(w http.ResponseWriter, r *http.Request) {
	addCORS(w)
	if r.Method == http.MethodOptions {
		w.WriteHeader(http.StatusNoContent)
		return
	}
	switch r.Method {
	case http.MethodPost:
		s.uploadSprite(w, r)
	case http.MethodGet:
		s.listSprites(w, r)
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func (s *server) uploadSprite(w http.ResponseWriter, r *http.Request) {
	r.Body = http.MaxBytesReader(w, r.Body, maxPNGBytes+maxJSONBytes+maxConfigBytes+2<<20)
	if err := r.ParseMultipartForm(maxPNGBytes + maxJSONBytes + maxConfigBytes); err != nil {
		http.Error(w, "invalid multipart upload", http.StatusBadRequest)
		return
	}

	deviceID := safeID(r.FormValue("device_id"))
	spriteName := safeName(r.FormValue("sprite_name"))
	displayName := strings.TrimSpace(r.FormValue("display_name"))
	if deviceID == "" || spriteName == "" {
		http.Error(w, "device_id and sprite_name are required", http.StatusBadRequest)
		return
	}
	if displayName == "" {
		displayName = spriteName
	}

	png, err := readPart(r.MultipartForm, "png", maxPNGBytes)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	if !isPNG(png) {
		http.Error(w, "png must be a PNG file", http.StatusBadRequest)
		return
	}
	metaBytes, err := readPart(r.MultipartForm, "json", maxJSONBytes)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	var meta sheetMetadata
	if err := json.Unmarshal(metaBytes, &meta); err != nil {
		http.Error(w, "invalid sprite json", http.StatusBadRequest)
		return
	}
	if err := validateMetadata(meta); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}

	var configBytes []byte
	if hasPart(r.MultipartForm, "config") {
		configBytes, err = readPart(r.MultipartForm, "config", maxConfigBytes)
		if err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		if len(bytes.TrimSpace(configBytes)) > 0 {
			if err := validateConfig(configBytes); err != nil {
				http.Error(w, err.Error(), http.StatusBadRequest)
				return
			}
		}
	}

	pngHash := fileHash(png)
	jsonHash := fileHash(metaBytes)
	configHash := ""
	if len(bytes.TrimSpace(configBytes)) > 0 {
		configHash = fileHash(configBytes)
	}

	now := time.Now().UTC().Format(time.RFC3339)
	status := "pending"
	if s.publicByDefault {
		status = "public"
	}
	tx, err := s.db.BeginTx(r.Context(), nil)
	if err != nil {
		http.Error(w, "database unavailable", http.StatusInternalServerError)
		return
	}
	defer tx.Rollback()
	if _, err := tx.Exec(`INSERT INTO devices(id, created_at, last_seen_at)
		VALUES(?, ?, ?)
		ON CONFLICT(id) DO UPDATE SET last_seen_at=excluded.last_seen_at`, deviceID, now, now); err != nil {
		http.Error(w, "could not upsert device", http.StatusInternalServerError)
		return
	}
	existingID, err := existingSpriteID(tx, deviceID, spriteName, pngHash, jsonHash, configHash)
	if err != nil {
		http.Error(w, "could not query existing sprite", http.StatusInternalServerError)
		return
	}
	if existingID != "" {
		if err := tx.Commit(); err != nil {
			http.Error(w, "could not commit upload", http.StatusInternalServerError)
			return
		}
		record, _ := s.spriteByID(r.Context(), existingID, r)
		writeJSON(w, http.StatusOK, record)
		return
	}

	pngPath, err := ensureBlobFile(filepath.Join(s.assetsDir, "blobs", "png"), pngHash, ".png", png)
	if err != nil {
		http.Error(w, "could not store png", http.StatusInternalServerError)
		return
	}
	jsonPath, err := ensureBlobFile(filepath.Join(s.assetsDir, "blobs", "json"), jsonHash, ".json", metaBytes)
	if err != nil {
		http.Error(w, "could not store json", http.StatusInternalServerError)
		return
	}
	configPath := ""
	if configHash != "" {
		configPath, err = ensureBlobFile(filepath.Join(s.assetsDir, "blobs", "config"), configHash, ".json", configBytes)
		if err != nil {
			http.Error(w, "could not store config", http.StatusInternalServerError)
			return
		}
	}

	id := randomID()
	if _, err := tx.Exec(`INSERT INTO sprites(
			id, owner_device_id, name, display_name, status, png_path, json_path, config_path, png_hash, json_hash, config_hash, created_at
		) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		id, deviceID, spriteName, displayName, status, pngPath, jsonPath, nullable(configPath), pngHash, jsonHash, nullable(configHash), now,
	); err != nil {
		http.Error(w, "could not insert sprite", http.StatusInternalServerError)
		return
	}
	if err := tx.Commit(); err != nil {
		http.Error(w, "could not commit upload", http.StatusInternalServerError)
		return
	}
	record, _ := s.spriteByID(r.Context(), id, r)
	writeJSON(w, http.StatusCreated, record)
}

func (s *server) listSprites(w http.ResponseWriter, r *http.Request) {
	limit := 50
	if raw := r.URL.Query().Get("limit"); raw != "" {
		if n, err := strconv.Atoi(raw); err == nil {
			limit = min(max(n, 1), 100)
		}
	}
	order := "created_at DESC"
	if r.URL.Query().Get("random") == "1" || r.URL.Query().Get("random") == "true" {
		order = "random()"
	}
	rows, err := s.db.QueryContext(r.Context(), `SELECT id, owner_device_id, name, display_name, status, png_path, json_path, config_path, png_hash, json_hash, config_hash, created_at
		FROM sprites WHERE status='public' ORDER BY `+order+` LIMIT ?`, limit)
	if err != nil {
		http.Error(w, "database unavailable", http.StatusInternalServerError)
		return
	}
	defer rows.Close()
	var records []spriteRecord
	for rows.Next() {
		record, err := scanSprite(rows, s.requestBaseURL(r))
		if err != nil {
			http.Error(w, "could not read sprite", http.StatusInternalServerError)
			return
		}
		records = append(records, record)
	}
	writeJSON(w, http.StatusOK, map[string]any{"sprites": records})
}

func (s *server) handleSprite(w http.ResponseWriter, r *http.Request) {
	addCORS(w)
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	id := safeID(strings.TrimPrefix(r.URL.Path, "/api/v1/sprites/"))
	if id == "" {
		http.NotFound(w, r)
		return
	}
	record, err := s.spriteByID(r.Context(), id, r)
	if errors.Is(err, sql.ErrNoRows) {
		http.NotFound(w, r)
		return
	}
	if err != nil {
		http.Error(w, "database unavailable", http.StatusInternalServerError)
		return
	}
	writeJSON(w, http.StatusOK, record)
}

func (s *server) handleAsset(w http.ResponseWriter, r *http.Request) {
	addCORS(w)
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	parts := strings.Split(strings.TrimPrefix(r.URL.Path, "/assets/"), "/")
	if len(parts) != 2 {
		http.NotFound(w, r)
		return
	}
	id := safeID(parts[0])
	name := parts[1]
	if id == "" || (name != "sprite.png" && name != "sprite.json" && name != "config.json") {
		http.NotFound(w, r)
		return
	}
	path := filepath.Join(s.assetsDir, id, name)
	http.ServeFile(w, r, path)
}

func (s *server) handlePeerSprite(w http.ResponseWriter, r *http.Request) {
	addCORS(w)
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	path := strings.TrimPrefix(r.URL.Path, "/api/v1/peers/")
	if !strings.HasSuffix(path, "/sprite") {
		http.NotFound(w, r)
		return
	}
	peerID := safeID(strings.TrimSuffix(path, "/sprite"))
	peerID = strings.TrimSuffix(peerID, "/")
	if peerID == "" {
		http.NotFound(w, r)
		return
	}
	peer := s.hub.clientByID(peerID)
	if peer == nil {
		http.NotFound(w, r)
		return
	}
	record, err := s.spriteByID(r.Context(), peer.spriteID, r)
	if errors.Is(err, sql.ErrNoRows) {
		http.NotFound(w, r)
		return
	}
	if err != nil {
		http.Error(w, "database unavailable", http.StatusInternalServerError)
		return
	}
	writeJSON(w, http.StatusOK, record)
}

func (s *server) handleWebSocket(w http.ResponseWriter, r *http.Request) {
	deviceID := safeID(r.URL.Query().Get("device_id"))
	spriteID := safeID(r.URL.Query().Get("sprite_id"))
	mode := r.URL.Query().Get("mode")
	roomCode := strings.ToUpper(safeName(r.URL.Query().Get("room")))
	if deviceID == "" || spriteID == "" {
		http.Error(w, "device_id and sprite_id are required", http.StatusBadRequest)
		return
	}
	if mode != "room" {
		mode = "random"
		roomCode = "RANDOM"
	} else if roomCode == "" {
		http.Error(w, "room code is required for room mode", http.StatusBadRequest)
		return
	}
	roomID, err := s.ensureRoom(r.Context(), roomCode, mode)
	if err != nil {
		http.Error(w, "could not create room", http.StatusInternalServerError)
		return
	}
	conn, reader, err := acceptWebSocket(w, r)
	if err != nil {
		return
	}
	client := &wsClient{
		id:       randomID(),
		roomID:   roomID,
		roomCode: roomCode,
		deviceID: deviceID,
		spriteID: spriteID,
		conn:     conn,
		reader:   reader,
	}
	if s.heartbeatTimeout <= 0 {
		s.heartbeatTimeout = defaultHeartbeatTimeout
	}
	_ = conn.SetReadDeadline(time.Now().Add(s.heartbeatTimeout))
	now := time.Now().UTC().Format(time.RFC3339)
	_, _ = s.db.ExecContext(context.Background(), `INSERT INTO devices(id, created_at, last_seen_at)
		VALUES(?, ?, ?)
		ON CONFLICT(id) DO UPDATE SET last_seen_at=excluded.last_seen_at`, deviceID, now, now)
	_, _ = s.db.ExecContext(context.Background(), `INSERT INTO sessions(id, room_id, device_id, sprite_id, last_seen_at)
		VALUES(?, ?, ?, ?, ?)`, client.id, roomID, deviceID, spriteID, now)

	existing := s.hub.join(client)
	for _, peer := range existing {
		_ = client.writeJSON(map[string]any{"type": "peer_joined", "peer_id": peer.id, "device_id": peer.deviceID, "sprite_id": peer.spriteID})
	}
	s.hub.broadcast(client, map[string]any{"type": "peer_joined", "peer_id": client.id, "device_id": deviceID, "sprite_id": spriteID})
	log.Printf("ws joined room=%s client=%s device=%s sprite=%s", roomCode, client.id, deviceID, spriteID)

	defer func() {
		s.hub.leave(client)
		_, _ = s.db.ExecContext(context.Background(), `DELETE FROM sessions WHERE id=?`, client.id)
		_ = conn.Close()
		s.hub.broadcast(client, map[string]any{"type": "peer_left", "peer_id": client.id})
		log.Printf("ws left room=%s client=%s", roomCode, client.id)
	}()

	for {
		payload, err := readWebSocketFrame(client.reader)
		if err != nil {
			return
		}
		var msg map[string]any
		if err := json.Unmarshal(payload, &msg); err != nil {
			continue
		}
		_ = conn.SetReadDeadline(time.Now().Add(s.heartbeatTimeout))
		t, _ := msg["type"].(string)
		_, _ = s.db.ExecContext(context.Background(), `UPDATE sessions SET last_seen_at=? WHERE id=?`, time.Now().UTC().Format(time.RFC3339), client.id)
		if t == "heartbeat" {
			_ = client.writeJSON(map[string]any{"type": "heartbeat", "at": time.Now().UTC().Format(time.RFC3339)})
			continue
		}
		outType := map[string]string{
			"pose":   "peer_pose",
			"state":  "peer_state",
			"action": "peer_action",
		}[t]
		if outType == "" {
			continue
		}
		msg["type"] = outType
		msg["peer_id"] = client.id
		msg["device_id"] = client.deviceID
		msg["sprite_id"] = client.spriteID
		s.hub.broadcast(client, msg)
	}
}

func (s *server) ensureRoom(ctx context.Context, code string, mode string) (int64, error) {
	now := time.Now().UTC().Format(time.RFC3339)
	_, err := s.db.ExecContext(ctx, `INSERT INTO rooms(code, mode, created_at) VALUES(?, ?, ?)
		ON CONFLICT(code) DO NOTHING`, code, mode, now)
	if err != nil {
		return 0, err
	}
	var id int64
	err = s.db.QueryRowContext(ctx, `SELECT id FROM rooms WHERE code=?`, code).Scan(&id)
	return id, err
}

func (s *server) spriteByID(ctx context.Context, id string, r *http.Request) (spriteRecord, error) {
	row := s.db.QueryRowContext(ctx, `SELECT id, owner_device_id, name, display_name, status, png_path, json_path, config_path, png_hash, json_hash, config_hash, created_at FROM sprites WHERE id=?`, id)
	return scanSprite(row, s.requestBaseURL(r))
}

func scanSprite(scanner interface{ Scan(...any) error }, baseURL string) (spriteRecord, error) {
	var rec spriteRecord
	var pngPath, jsonPath string
	var configPath sql.NullString
	var configHash sql.NullString
	if err := scanner.Scan(&rec.ID, &rec.OwnerDeviceID, &rec.Name, &rec.DisplayName, &rec.Status, &pngPath, &jsonPath, &configPath, &rec.PNGHash, &rec.JSONHash, &configHash, &rec.CreatedAt); err != nil {
		return rec, err
	}
	rec.PNGURL = baseURL + "/assets/" + rec.ID + "/sprite.png"
	rec.JSONURL = baseURL + "/assets/" + rec.ID + "/sprite.json"
	if configPath.Valid && configPath.String != "" {
		u := baseURL + "/assets/" + rec.ID + "/config.json"
		rec.ConfigURL = &u
	}
	if configHash.Valid && configHash.String != "" {
		hash := configHash.String
		rec.ConfigHash = &hash
	}
	return rec, nil
}

func (s *server) requestBaseURL(r *http.Request) string {
	if s.baseURL != "" {
		return s.baseURL
	}
	scheme := "http"
	if r.TLS != nil {
		scheme = "https"
	}
	return scheme + "://" + r.Host
}

func (h *hub) join(c *wsClient) []*wsClient {
	h.mu.Lock()
	defer h.mu.Unlock()
	room := h.rooms[c.roomCode]
	if room == nil {
		room = map[string]*wsClient{}
		h.rooms[c.roomCode] = room
	}
	var existing []*wsClient
	for _, peer := range room {
		existing = append(existing, peer)
	}
	room[c.id] = c
	h.byID[c.id] = c
	return existing
}

func (h *hub) leave(c *wsClient) {
	h.mu.Lock()
	defer h.mu.Unlock()
	room := h.rooms[c.roomCode]
	if room == nil {
		return
	}
	delete(room, c.id)
	delete(h.byID, c.id)
	if len(room) == 0 {
		delete(h.rooms, c.roomCode)
	}
}

func (h *hub) broadcast(sender *wsClient, msg map[string]any) {
	h.mu.Lock()
	var peers []*wsClient
	for _, peer := range h.rooms[sender.roomCode] {
		if peer.id != sender.id {
			peers = append(peers, peer)
		}
	}
	h.mu.Unlock()
	for _, peer := range peers {
		_ = peer.writeJSON(msg)
	}
}

func (h *hub) clientByID(id string) *wsClient {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.byID[id]
}

func (c *wsClient) writeJSON(v any) error {
	data, err := json.Marshal(v)
	if err != nil {
		return err
	}
	c.writerMu.Lock()
	defer c.writerMu.Unlock()
	return writeWebSocketText(c.conn, data)
}

func acceptWebSocket(w http.ResponseWriter, r *http.Request) (net.Conn, *bufio.Reader, error) {
	if !strings.EqualFold(r.Header.Get("Upgrade"), "websocket") || !strings.Contains(strings.ToLower(r.Header.Get("Connection")), "upgrade") {
		http.Error(w, "websocket upgrade required", http.StatusBadRequest)
		return nil, nil, errors.New("missing websocket upgrade")
	}
	key := r.Header.Get("Sec-WebSocket-Key")
	if key == "" {
		http.Error(w, "missing websocket key", http.StatusBadRequest)
		return nil, nil, errors.New("missing websocket key")
	}
	hj, ok := w.(http.Hijacker)
	if !ok {
		http.Error(w, "websocket unavailable", http.StatusInternalServerError)
		return nil, nil, errors.New("hijack unavailable")
	}
	conn, rw, err := hj.Hijack()
	if err != nil {
		return nil, nil, err
	}
	accept := websocketAccept(key)
	_, err = fmt.Fprintf(rw, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: %s\r\n\r\n", accept)
	if err != nil {
		_ = conn.Close()
		return nil, nil, err
	}
	if err := rw.Flush(); err != nil {
		_ = conn.Close()
		return nil, nil, err
	}
	return conn, rw.Reader, nil
}

func websocketAccept(key string) string {
	sum := sha1.Sum([]byte(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))
	return base64.StdEncoding.EncodeToString(sum[:])
}

func readWebSocketFrame(r *bufio.Reader) ([]byte, error) {
	h0, err := r.ReadByte()
	if err != nil {
		return nil, err
	}
	h1, err := r.ReadByte()
	if err != nil {
		return nil, err
	}
	opcode := h0 & 0x0f
	masked := h1&0x80 != 0
	length := uint64(h1 & 0x7f)
	if length == 126 {
		b := make([]byte, 2)
		if _, err := io.ReadFull(r, b); err != nil {
			return nil, err
		}
		length = uint64(b[0])<<8 | uint64(b[1])
	} else if length == 127 {
		b := make([]byte, 8)
		if _, err := io.ReadFull(r, b); err != nil {
			return nil, err
		}
		length = 0
		for _, v := range b {
			length = length<<8 | uint64(v)
		}
	}
	if length > 1<<20 {
		return nil, errors.New("websocket frame too large")
	}
	var mask [4]byte
	if masked {
		if _, err := io.ReadFull(r, mask[:]); err != nil {
			return nil, err
		}
	}
	payload := make([]byte, length)
	if _, err := io.ReadFull(r, payload); err != nil {
		return nil, err
	}
	if masked {
		for i := range payload {
			payload[i] ^= mask[i%4]
		}
	}
	if opcode == 0x8 {
		return nil, io.EOF
	}
	if opcode != 0x1 {
		return nil, errors.New("unsupported websocket frame")
	}
	return payload, nil
}

func writeWebSocketText(w io.Writer, payload []byte) error {
	header := []byte{0x81}
	n := len(payload)
	if n < 126 {
		header = append(header, byte(n))
	} else if n <= 0xffff {
		header = append(header, 126, byte(n>>8), byte(n))
	} else {
		header = append(header, 127, 0, 0, 0, 0, byte(n>>24), byte(n>>16), byte(n>>8), byte(n))
	}
	if _, err := w.Write(header); err != nil {
		return err
	}
	_, err := w.Write(payload)
	return err
}

func existingSpriteID(tx *sql.Tx, deviceID string, spriteName string, pngHash string, jsonHash string, configHash string) (string, error) {
	var id string
	err := tx.QueryRow(
		`SELECT id FROM sprites
		WHERE owner_device_id = ?
		  AND name = ?
		  AND png_hash = ?
		  AND json_hash = ?
		  AND COALESCE(config_hash, '') = ?
		ORDER BY created_at DESC
		LIMIT 1`,
		deviceID, spriteName, pngHash, jsonHash, configHash,
	).Scan(&id)
	if errors.Is(err, sql.ErrNoRows) {
		return "", nil
	}
	return id, err
}

func readPart(form *multipart.Form, name string, limit int64) ([]byte, error) {
	files := form.File[name]
	if len(files) == 0 {
		return nil, fmt.Errorf("%s is required", name)
	}
	file, err := files[0].Open()
	if err != nil {
		return nil, fmt.Errorf("could not read %s", name)
	}
	defer file.Close()
	data, err := io.ReadAll(io.LimitReader(file, limit+1))
	if err != nil {
		return nil, fmt.Errorf("could not read %s", name)
	}
	if int64(len(data)) > limit {
		return nil, fmt.Errorf("%s is too large", name)
	}
	return data, nil
}

func hasPart(form *multipart.Form, name string) bool {
	return form != nil && len(form.File[name]) > 0
}

func validateMetadata(meta sheetMetadata) error {
	if meta.FrameWidth <= 0 || meta.FrameHeight <= 0 || meta.Columns <= 0 || meta.Rows <= 0 || meta.FrameCount <= 0 || strings.TrimSpace(meta.Image) == "" {
		return errors.New("sprite json must contain frameWidth, frameHeight, columns, rows, frameCount, and image")
	}
	if meta.FrameWidth > maxFrameSize || meta.FrameHeight > maxFrameSize {
		return fmt.Errorf("frame size must be <= %d", maxFrameSize)
	}
	if meta.Columns > maxGridSide || meta.Rows > maxGridSide {
		return fmt.Errorf("columns and rows must be <= %d", maxGridSide)
	}
	if meta.FrameCount > maxFrameCount {
		return fmt.Errorf("frameCount must be <= %d", maxFrameCount)
	}
	if meta.FrameCount > meta.Columns*meta.Rows {
		return errors.New("frameCount cannot exceed columns * rows")
	}
	return nil
}

func validateConfig(data []byte) error {
	var root map[string]any
	if err := json.Unmarshal(data, &root); err != nil {
		return errors.New("invalid config json")
	}
	animations, ok := root["animations"].(map[string]any)
	if !ok {
		return errors.New("config.json must contain animations")
	}
	for sprite, raw := range animations {
		if safeName(sprite) == "" {
			return errors.New("invalid animation sprite name")
		}
		states, ok := raw.(map[string]any)
		if !ok {
			return errors.New("animations.<sprite> must be an object")
		}
		for name, rawSpec := range states {
			if strings.TrimSpace(name) == "" {
				return errors.New("animation name cannot be empty")
			}
			spec, ok := rawSpec.(map[string]any)
			if !ok {
				return errors.New("animation spec must be an object")
			}
			row, ok := spec["row"].(float64)
			if !ok || row < 0 || row != float64(int(row)) {
				return errors.New("animation spec must contain integer row")
			}
			if loop, ok := spec["loop"]; ok {
				switch v := loop.(type) {
				case bool:
				case float64:
					if v < 1 || v != float64(int(v)) {
						return errors.New("loop number must be a positive integer")
					}
				default:
					return errors.New("loop must be bool or number")
				}
			}
		}
	}
	return nil
}

func isPNG(data []byte) bool {
	return len(data) >= 8 && bytes.Equal(data[:8], []byte{0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'})
}

func fileHash(data []byte) string {
	sum := sha256.Sum256(data)
	return hex.EncodeToString(sum[:])
}

func ensureBlobFile(baseDir string, hash string, suffix string, data []byte) (string, error) {
	if err := os.MkdirAll(baseDir, 0o755); err != nil {
		return "", err
	}
	path := filepath.Join(baseDir, hash+suffix)
	if _, err := os.Stat(path); err == nil {
		return path, nil
	} else if !errors.Is(err, os.ErrNotExist) {
		return "", err
	}
	if err := os.WriteFile(path, data, 0o644); err != nil {
		return "", err
	}
	return path, nil
}

var safeRe = regexp.MustCompile(`[^A-Za-z0-9_-]+`)

func safeID(value string) string {
	return safeName(value)
}

func safeName(value string) string {
	clean := safeRe.ReplaceAllString(strings.TrimSpace(value), "")
	return strings.Trim(clean, "-_")
}

func randomID() string {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		panic(err)
	}
	return hex.EncodeToString(b[:])
}

func nullable(value string) any {
	if value == "" {
		return nil
	}
	return value
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	addCORS(w)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func addCORS(w http.ResponseWriter) {
	w.Header().Set("Access-Control-Allow-Origin", "*")
	w.Header().Set("Access-Control-Allow-Methods", "GET,POST,OPTIONS")
	w.Header().Set("Access-Control-Allow-Headers", "Content-Type")
}

func homeDir() string {
	if home, err := os.UserHomeDir(); err == nil {
		return home
	}
	return "."
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

func max(a, b int) int {
	if a > b {
		return a
	}
	return b
}
