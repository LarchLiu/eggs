package main

import (
	"bufio"
	"bytes"
	"container/list"
	"context"
	crand "crypto/rand"
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
	"runtime"
	"strconv"
	"strings"
	"sync"
	"time"

	_ "modernc.org/sqlite"
)

const (
	maxPNGBytes    = 8 << 20
	maxJSONBytes   = 1 << 20
	maxConfigBytes = 256 << 10
	maxFrameSize   = 1024
	maxFrameCount  = 512
	maxGridSide    = 64

	deviceRetention       = 30 * 24 * time.Hour
	deviceCleanupInterval = time.Hour
	wsPingInterval        = 15 * time.Second
	wsReadTimeout         = 45 * time.Second
	wsWriteTimeout        = 10 * time.Second
)

var errPeerSendQueueFull = errors.New("peer send queue full")

type server struct {
	db              *sql.DB
	dataDir         string
	assetsDir       string
	baseURL         string
	publicByDefault bool
	hub             *hub
	assetCache      *assetCache
	deviceCleanup   *deviceCleanupState
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
	PNGPath       string  `json:"-"`
	JSONPath      string  `json:"-"`
	ConfigPath    string  `json:"-"`
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
	id        string
	mode      string
	roomCode  string
	deviceID  string
	spriteID  string
	conn      net.Conn
	reader    *bufio.Reader
	sprite    spriteRecord
	sendCh    chan []byte
	doneCh    chan struct{}
	peerMu    sync.RWMutex
	peer      *wsClient
	stateMu   sync.RWMutex
	state     string
	closeMu   sync.Mutex
	closeInfo string
	closeOnce sync.Once
}

type hub struct {
	onlineMu      sync.Mutex
	matchMu       sync.Mutex
	rooms         map[string]*roomState
	roomByClient  map[string]string
	waitingRandom *list.List
	waitingByID   map[string]*list.Element
	onlineSprites map[string]*wsClient
}

type roomState struct {
	first  *wsClient
	second *wsClient
}

type delivery struct {
	target *wsClient
	msg    map[string]any
	raw    []byte
}

type assetPaths struct {
	png    string
	json   string
	config string
}

type assetCache struct {
	mu   sync.RWMutex
	byID map[string]assetPaths
}

type deviceCleanupState struct {
	mu         sync.Mutex
	lastRunUTC time.Time
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
	configureDB(db)
	s := &server{
		db:              db,
		dataDir:         *dataDir,
		assetsDir:       filepath.Join(*dataDir, "assets"),
		baseURL:         strings.TrimRight(*baseURL, "/"),
		publicByDefault: *publicByDefault,
		hub: &hub{
			rooms:         map[string]*roomState{},
			roomByClient:  map[string]string{},
			waitingRandom: list.New(),
			waitingByID:   map[string]*list.Element{},
			onlineSprites: map[string]*wsClient{},
		},
		assetCache:    &assetCache{byID: map[string]assetPaths{}},
		deviceCleanup: &deviceCleanupState{},
	}
	if err := s.migrate(); err != nil {
		log.Fatal(err)
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/api/v1/sprites", s.handleSprites)
	mux.HandleFunc("/api/v1/sprites/", s.handleSprite)
	mux.HandleFunc("/assets/", s.handleAsset)
	mux.HandleFunc("/ws", s.handleWebSocket)
	mux.HandleFunc("/healthz", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, http.StatusOK, map[string]string{"ok": "true"})
	})

	log.Printf("eggs server listening on %s, data=%s", *addr, *dataDir)
	log.Fatal(http.ListenAndServe(*addr, mux))
}

func configureDB(db *sql.DB) {
	maxConns := min(max(runtime.NumCPU(), 4), 16)
	db.SetMaxOpenConns(maxConns)
	db.SetMaxIdleConns(maxConns)
	db.SetConnMaxLifetime(0)
	for _, stmt := range []string{
		`PRAGMA journal_mode=WAL`,
		`PRAGMA synchronous=NORMAL`,
		`PRAGMA temp_store=MEMORY`,
		`PRAGMA foreign_keys=ON`,
		`PRAGMA busy_timeout=5000`,
	} {
		if _, err := db.Exec(stmt); err != nil {
			log.Printf("warning: failed to apply %q: %v", stmt, err)
		}
	}
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
		`CREATE INDEX IF NOT EXISTS idx_sprites_owner_name_created_at
			ON sprites(owner_device_id, name, created_at DESC)`,
		`CREATE INDEX IF NOT EXISTS idx_sprites_owner_name_hashes
			ON sprites(owner_device_id, name, png_hash, json_hash, config_hash, created_at DESC)`,
		`CREATE INDEX IF NOT EXISTS idx_sprites_status_created_at
			ON sprites(status, created_at DESC)`,
		`CREATE INDEX IF NOT EXISTS idx_sprites_status_id
			ON sprites(status, id)`,
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
	if err := upsertDevice(r.Context(), tx, deviceID, now); err != nil {
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
		s.maybeCleanupDevices(context.Background())
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
	s.assetCacheStore(id, assetPaths{png: pngPath, json: jsonPath, config: configPath})
	s.maybeCleanupDevices(context.Background())
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
	randomOrder := r.URL.Query().Get("random") == "1" || r.URL.Query().Get("random") == "true"
	var (
		records []spriteRecord
		err     error
	)
	if randomOrder {
		records, err = s.randomPublicSprites(r.Context(), limit, s.requestBaseURL(r))
	} else {
		records, err = s.latestPublicSprites(r.Context(), limit, s.requestBaseURL(r))
	}
	if err != nil {
		http.Error(w, "database unavailable", http.StatusInternalServerError)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"sprites": records})
}

func (s *server) latestPublicSprites(ctx context.Context, limit int, baseURL string) ([]spriteRecord, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT id, owner_device_id, name, display_name, status, png_path, json_path, config_path, png_hash, json_hash, config_hash, created_at
		FROM sprites
		WHERE status='public'
		ORDER BY created_at DESC
		LIMIT ?`, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return scanSprites(rows, baseURL)
}

func (s *server) randomPublicSprites(ctx context.Context, limit int, baseURL string) ([]spriteRecord, error) {
	cursor := randomID()
	records, err := s.publicSpritesFromCursor(ctx, cursor, limit, baseURL)
	if err != nil {
		return nil, err
	}
	if len(records) >= limit {
		return records, nil
	}
	more, err := s.publicSpritesBeforeCursor(ctx, cursor, limit-len(records), baseURL)
	if err != nil {
		return nil, err
	}
	return append(records, more...), nil
}

func (s *server) publicSpritesFromCursor(ctx context.Context, cursor string, limit int, baseURL string) ([]spriteRecord, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT id, owner_device_id, name, display_name, status, png_path, json_path, config_path, png_hash, json_hash, config_hash, created_at
		FROM sprites
		WHERE status='public' AND id >= ?
		ORDER BY id
		LIMIT ?`, cursor, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return scanSprites(rows, baseURL)
}

func (s *server) publicSpritesBeforeCursor(ctx context.Context, cursor string, limit int, baseURL string) ([]spriteRecord, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT id, owner_device_id, name, display_name, status, png_path, json_path, config_path, png_hash, json_hash, config_hash, created_at
		FROM sprites
		WHERE status='public' AND id < ?
		ORDER BY id
		LIMIT ?`, cursor, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return scanSprites(rows, baseURL)
}

func scanSprites(rows *sql.Rows, baseURL string) ([]spriteRecord, error) {
	var records []spriteRecord
	for rows.Next() {
		record, err := scanSprite(rows, baseURL)
		if err != nil {
			return nil, err
		}
		records = append(records, record)
	}
	return records, rows.Err()
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
	path := ""
	if paths, ok := s.assetCacheLookup(id); ok {
		path = paths.pathFor(name)
	} else {
		record, err := s.loadSpriteRecord(r.Context(), id)
		if errors.Is(err, sql.ErrNoRows) {
			http.NotFound(w, r)
			return
		}
		if err != nil {
			http.Error(w, "database unavailable", http.StatusInternalServerError)
			return
		}
		path = assetPaths{
			png:    record.PNGPath,
			json:   record.JSONPath,
			config: record.ConfigPath,
		}.pathFor(name)
	}
	if path == "" {
		http.NotFound(w, r)
		return
	}
	http.ServeFile(w, r, path)
}

func (s *server) handleWebSocket(w http.ResponseWriter, r *http.Request) {
	deviceID := safeID(r.URL.Query().Get("device_id"))
	spriteName := safeName(r.URL.Query().Get("sprite"))
	mode := r.URL.Query().Get("mode")
	roomCode := strings.ToUpper(safeName(r.URL.Query().Get("room")))
	if deviceID == "" || spriteName == "" {
		http.Error(w, "device_id and sprite are required", http.StatusBadRequest)
		return
	}
	if mode != "room" {
		mode = "random"
		roomCode = "RANDOM"
	} else if roomCode == "" {
		http.Error(w, "room code is required for room mode", http.StatusBadRequest)
		return
	}
	spriteRecord, err := s.loadSpriteByOwnerAndName(r.Context(), deviceID, spriteName)
	if errors.Is(err, sql.ErrNoRows) {
		http.Error(w, "unknown sprite for device", http.StatusBadRequest)
		return
	}
	if err != nil {
		http.Error(w, "database unavailable", http.StatusInternalServerError)
		return
	}
	conn, reader, err := acceptWebSocket(w, r)
	if err != nil {
		return
	}
	client := &wsClient{
		id:       randomID(),
		mode:     mode,
		roomCode: roomCode,
		deviceID: deviceID,
		spriteID: spriteRecord.ID,
		conn:     conn,
		reader:   reader,
		sprite:   spriteRecord.withBaseURL(s.requestBaseURL(r)),
		sendCh:   make(chan []byte, 64),
		doneCh:   make(chan struct{}),
		state:    "hatched",
	}

	deliveries, err := s.hub.join(client, roomCode)
	if err != nil {
		_ = conn.Close()
		http.Error(w, err.Error(), http.StatusConflict)
		return
	}
	go client.writeLoop()
	sendDeliveries(deliveries)

	defer func() {
		sendDeliveries(s.hub.leave(client))
		client.close()
		reason := client.closeReason()
		if reason == "" {
			reason = "closed"
		}
		log.Printf("ws closed mode=%s room=%s client=%s device=%s sprite=%s reason=%s", mode, roomCode, client.id, deviceID, spriteRecord.Name, reason)
	}()

	for {
		if err := client.conn.SetReadDeadline(time.Now().Add(wsReadTimeout)); err != nil {
			client.closeWithReason(err.Error())
			return
		}
		payload, err := readWebSocketTextMessage(client.conn, client.reader)
		if err != nil {
			if ne, ok := err.(net.Error); ok && ne.Timeout() {
				client.closeWithReason("read timeout")
			} else if errors.Is(err, io.EOF) {
				client.closeWithReason("peer closed")
			} else {
				client.closeWithReason(err.Error())
			}
			return
		}
		var msg map[string]any
		if err := json.Unmarshal(payload, &msg); err != nil {
			continue
		}
		t, _ := msg["type"].(string)
		outType := map[string]string{
			"state":  "peer_state",
			"action": "peer_action",
		}[t]
		if outType == "" {
			continue
		}
		client.applyIncomingMessage(t, msg)
		msg["type"] = outType
		msg["peer_id"] = client.id
		msg["device_id"] = client.deviceID
		client.appendPresence(msg)
		sendDeliveries(s.hub.broadcast(client, msg))
	}
}

func (s *server) spriteByID(ctx context.Context, id string, r *http.Request) (spriteRecord, error) {
	record, err := s.loadSpriteRecord(ctx, id)
	if err != nil {
		return record, err
	}
	return record.withBaseURL(s.requestBaseURL(r)), nil
}

func (s *server) loadSpriteRecord(ctx context.Context, id string) (spriteRecord, error) {
	row := s.db.QueryRowContext(ctx, `SELECT id, owner_device_id, name, display_name, status, png_path, json_path, config_path, png_hash, json_hash, config_hash, created_at FROM sprites WHERE id=?`, id)
	record, err := scanSprite(row, "")
	if err != nil {
		return record, err
	}
	s.assetCacheStore(record.ID, assetPaths{
		png:    record.PNGPath,
		json:   record.JSONPath,
		config: record.ConfigPath,
	})
	return record, nil
}

func (s *server) loadSpriteByOwnerAndName(ctx context.Context, deviceID string, spriteName string) (spriteRecord, error) {
	row := s.db.QueryRowContext(ctx, `SELECT id, owner_device_id, name, display_name, status, png_path, json_path, config_path, png_hash, json_hash, config_hash, created_at
		FROM sprites
		WHERE owner_device_id=? AND name=?
		ORDER BY created_at DESC
		LIMIT 1`, deviceID, spriteName)
	record, err := scanSprite(row, "")
	if err != nil {
		return record, err
	}
	s.assetCacheStore(record.ID, assetPaths{
		png:    record.PNGPath,
		json:   record.JSONPath,
		config: record.ConfigPath,
	})
	return record, nil
}

func scanSprite(scanner interface{ Scan(...any) error }, baseURL string) (spriteRecord, error) {
	var rec spriteRecord
	var pngPath, jsonPath string
	var configPath sql.NullString
	var configHash sql.NullString
	if err := scanner.Scan(&rec.ID, &rec.OwnerDeviceID, &rec.Name, &rec.DisplayName, &rec.Status, &pngPath, &jsonPath, &configPath, &rec.PNGHash, &rec.JSONHash, &configHash, &rec.CreatedAt); err != nil {
		return rec, err
	}
	rec.PNGPath = pngPath
	rec.JSONPath = jsonPath
	rec.PNGURL = baseURL + "/assets/" + rec.ID + "/sprite.png"
	rec.JSONURL = baseURL + "/assets/" + rec.ID + "/sprite.json"
	if configPath.Valid && configPath.String != "" {
		rec.ConfigPath = configPath.String
		u := baseURL + "/assets/" + rec.ID + "/config.json"
		rec.ConfigURL = &u
	}
	if configHash.Valid && configHash.String != "" {
		hash := configHash.String
		rec.ConfigHash = &hash
	}
	return rec, nil
}

func (r spriteRecord) withBaseURL(baseURL string) spriteRecord {
	out := r
	out.PNGURL = baseURL + "/assets/" + r.ID + "/sprite.png"
	out.JSONURL = baseURL + "/assets/" + r.ID + "/sprite.json"
	if r.ConfigHash != nil {
		u := baseURL + "/assets/" + r.ID + "/config.json"
		out.ConfigURL = &u
	} else {
		out.ConfigURL = nil
	}
	return out
}

func peerMessage(messageType string, client *wsClient) map[string]any {
	msg := map[string]any{
		"type":      messageType,
		"peer_id":   client.id,
		"device_id": client.deviceID,
		"sprite":    client.sprite,
	}
	client.appendPresence(msg)
	return msg
}

func roomSnapshotMessage(peers []*wsClient) map[string]any {
	items := make([]map[string]any, 0, len(peers))
	for _, peer := range peers {
		items = append(items, peerMessage("peer", peer))
	}
	return map[string]any{
		"type":  "room_snapshot",
		"peers": items,
	}
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

func (h *hub) join(c *wsClient, roomCode string) ([]delivery, error) {
	h.onlineMu.Lock()
	if existing := h.onlineSprites[c.spriteID]; existing != nil {
		h.onlineMu.Unlock()
		return nil, fmt.Errorf("sprite is already online")
	}
	h.onlineSprites[c.spriteID] = c
	h.matchMu.Lock()
	defer h.matchMu.Unlock()
	defer h.onlineMu.Unlock()
	if c.mode == "room" {
		deliveries, err := h.joinInviteRoomLocked(c, roomCode)
		if err != nil {
			delete(h.onlineSprites, c.spriteID)
			return nil, err
		}
		return deliveries, nil
	}
	return h.joinRandomLocked(c), nil
}

func (h *hub) joinInviteRoomLocked(c *wsClient, roomCode string) ([]delivery, error) {
	room := h.rooms[roomCode]
	if room == nil {
		room = &roomState{}
		h.rooms[roomCode] = room
	}
	if room.full() {
		return nil, fmt.Errorf("room is full")
	}
	existing := room.peers()
	room.add(c)
	if len(existing) == 1 {
		linkPeers(existing[0], c)
	}
	c.roomCode = roomCode
	h.roomByClient[c.id] = roomCode
	deliveries := []delivery{{target: c, msg: roomSnapshotMessage(existing)}}
	for _, peer := range existing {
		deliveries = append(deliveries, delivery{target: peer, msg: peerMessage("peer_joined", c)})
	}
	return deliveries, nil
}

func (h *hub) joinRandomLocked(c *wsClient) []delivery {
	if peer := h.dequeueWaitingRandomLocked(); peer != nil {
		roomCode := "random-" + randomID()
		room := &roomState{first: peer, second: c}
		h.rooms[roomCode] = room
		linkPeers(peer, c)
		peer.roomCode = roomCode
		c.roomCode = roomCode
		h.roomByClient[peer.id] = roomCode
		h.roomByClient[c.id] = roomCode
		return []delivery{
			{target: c, msg: roomSnapshotMessage([]*wsClient{peer})},
			{target: peer, msg: roomSnapshotMessage([]*wsClient{c})},
		}
	}
	h.enqueueWaitingRandomLocked(c)
	c.roomCode = ""
	delete(h.roomByClient, c.id)
	return nil
}

func (h *hub) leave(c *wsClient) []delivery {
	h.onlineMu.Lock()
	delete(h.onlineSprites, c.spriteID)
	h.matchMu.Lock()
	defer h.matchMu.Unlock()
	defer h.onlineMu.Unlock()
	h.removeWaitingLocked(c.id)
	roomCode := h.roomByClient[c.id]
	if roomCode == "" {
		delete(h.roomByClient, c.id)
		return nil
	}
	room := h.rooms[roomCode]
	if room == nil {
		delete(h.roomByClient, c.id)
		return nil
	}
	peer := room.remove(c.id)
	unlinkPeers(c, peer)
	delete(h.roomByClient, c.id)
	c.roomCode = ""
	deliveries := make([]delivery, 0, 2)
	if peer != nil {
		deliveries = append(deliveries, delivery{
			target: peer,
			msg:    map[string]any{"type": "peer_left", "peer_id": c.id},
		})
	}
	if room.empty() {
		delete(h.rooms, roomCode)
		return deliveries
	}
	if c.mode != "random" {
		return deliveries
	}
	survivor := room.onlyPeer()
	delete(h.rooms, roomCode)
	if survivor != nil {
		delete(h.roomByClient, survivor.id)
		survivor.roomCode = ""
		more := h.joinRandomLocked(survivor)
		deliveries = append(deliveries, more...)
	}
	return deliveries
}

func (h *hub) broadcast(sender *wsClient, msg map[string]any) []delivery {
	data, err := json.Marshal(msg)
	if err != nil {
		return nil
	}
	peer := sender.getPeer()
	if peer == nil {
		return nil
	}
	return []delivery{{
		target: peer,
		raw:    data,
	}}
}

func (h *hub) removeWaitingLocked(clientID string) {
	if h.waitingRandom == nil || h.waitingByID == nil {
		return
	}
	if elem := h.waitingByID[clientID]; elem != nil {
		h.waitingRandom.Remove(elem)
		delete(h.waitingByID, clientID)
	}
}

func (h *hub) enqueueWaitingRandomLocked(c *wsClient) {
	if h.waitingRandom == nil {
		h.waitingRandom = list.New()
	}
	if h.waitingByID == nil {
		h.waitingByID = map[string]*list.Element{}
	}
	if elem := h.waitingByID[c.id]; elem != nil {
		elem.Value = c
		return
	}
	h.waitingByID[c.id] = h.waitingRandom.PushBack(c)
}

func (h *hub) dequeueWaitingRandomLocked() *wsClient {
	if h.waitingRandom == nil || h.waitingByID == nil {
		return nil
	}
	for elem := h.waitingRandom.Front(); elem != nil; elem = h.waitingRandom.Front() {
		h.waitingRandom.Remove(elem)
		client, _ := elem.Value.(*wsClient)
		if client != nil {
			delete(h.waitingByID, client.id)
			return client
		}
	}
	return nil
}

func (r *roomState) full() bool {
	return r != nil && r.first != nil && r.second != nil
}

func (r *roomState) empty() bool {
	return r == nil || (r.first == nil && r.second == nil)
}

func (r *roomState) add(c *wsClient) {
	if r == nil || c == nil {
		return
	}
	if r.first == nil {
		r.first = c
		return
	}
	if r.second == nil {
		r.second = c
	}
}

func (r *roomState) peers() []*wsClient {
	if r == nil {
		return nil
	}
	peers := make([]*wsClient, 0, 2)
	if r.first != nil {
		peers = append(peers, r.first)
	}
	if r.second != nil {
		peers = append(peers, r.second)
	}
	return peers
}

func (r *roomState) peerOf(clientID string) *wsClient {
	if r == nil {
		return nil
	}
	if r.first != nil && r.first.id == clientID {
		return r.second
	}
	if r.second != nil && r.second.id == clientID {
		return r.first
	}
	return nil
}

func (r *roomState) remove(clientID string) *wsClient {
	if r == nil {
		return nil
	}
	if r.first != nil && r.first.id == clientID {
		r.first = nil
		return r.second
	}
	if r.second != nil && r.second.id == clientID {
		r.second = nil
		return r.first
	}
	return nil
}

func (r *roomState) onlyPeer() *wsClient {
	if r == nil {
		return nil
	}
	if r.first != nil && r.second == nil {
		return r.first
	}
	if r.second != nil && r.first == nil {
		return r.second
	}
	return nil
}

func (c *wsClient) enqueueJSON(v any) error {
	data, err := json.Marshal(v)
	if err != nil {
		return err
	}
	return c.enqueue(data)
}

func sendDeliveries(items []delivery) {
	for _, item := range items {
		if item.target == nil {
			continue
		}
		var err error
		if item.raw != nil {
			err = item.target.enqueue(item.raw)
		} else {
			err = item.target.enqueueJSON(item.msg)
		}
		if err != nil {
			go item.target.closeWithReason(err.Error())
		}
	}
}

func (c *wsClient) enqueue(data []byte) error {
	select {
	case <-c.doneCh:
		return errors.New("peer closed")
	default:
	}
	select {
	case c.sendCh <- data:
		return nil
	case <-c.doneCh:
		return errors.New("peer closed")
	default:
		return errPeerSendQueueFull
	}
}

func (c *wsClient) setPeer(peer *wsClient) {
	c.peerMu.Lock()
	defer c.peerMu.Unlock()
	c.peer = peer
}

func (c *wsClient) getPeer() *wsClient {
	c.peerMu.RLock()
	defer c.peerMu.RUnlock()
	return c.peer
}

func linkPeers(a *wsClient, b *wsClient) {
	if a != nil {
		a.setPeer(b)
	}
	if b != nil {
		b.setPeer(a)
	}
}

func unlinkPeers(a *wsClient, b *wsClient) {
	if a != nil {
		a.setPeer(nil)
	}
	if b != nil && b.getPeer() == a {
		b.setPeer(nil)
	}
}

func (c *wsClient) close() {
	c.closeOnce.Do(func() {
		close(c.doneCh)
		_ = c.conn.Close()
	})
}

func (c *wsClient) closeWithReason(reason string) {
	if strings.TrimSpace(reason) != "" {
		c.closeMu.Lock()
		if c.closeInfo == "" {
			c.closeInfo = reason
		}
		c.closeMu.Unlock()
	}
	c.close()
}

func (c *wsClient) closeReason() string {
	c.closeMu.Lock()
	defer c.closeMu.Unlock()
	return c.closeInfo
}

func (c *wsClient) appendPresence(msg map[string]any) {
	c.stateMu.RLock()
	defer c.stateMu.RUnlock()
	if c.state != "" {
		msg["state"] = c.state
	}
}

func (c *wsClient) applyIncomingMessage(messageType string, msg map[string]any) {
	c.stateMu.Lock()
	defer c.stateMu.Unlock()
	switch messageType {
	case "state", "action":
		if state := strings.TrimSpace(stringValue(msg["state"])); state != "" {
			c.state = state
		}
		if action := strings.TrimSpace(stringValue(msg["action"])); action != "" {
			c.state = action
		}
	}
}

func (c *wsClient) writeLoop() {
	ticker := time.NewTicker(wsPingInterval)
	defer ticker.Stop()
	for {
		select {
		case <-c.doneCh:
			return
		case <-ticker.C:
			if err := c.conn.SetWriteDeadline(time.Now().Add(wsWriteTimeout)); err != nil {
				c.closeWithReason(err.Error())
				return
			}
			if err := writeWebSocketControl(c.conn, 0x9, nil); err != nil {
				c.closeWithReason(err.Error())
				return
			}
		case payload, ok := <-c.sendCh:
			if !ok {
				return
			}
			if err := c.conn.SetWriteDeadline(time.Now().Add(wsWriteTimeout)); err != nil {
				c.closeWithReason(err.Error())
				return
			}
			if err := writeWebSocketText(c.conn, payload); err != nil {
				c.closeWithReason(err.Error())
				return
			}
		}
	}
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

func readWebSocketTextMessage(conn net.Conn, r *bufio.Reader) ([]byte, error) {
	for {
		opcode, payload, err := readWebSocketFrame(r)
		if err != nil {
			return nil, err
		}
		switch opcode {
		case 0x1:
			return payload, nil
		case 0x8:
			return nil, io.EOF
		case 0x9:
			if err := writeWebSocketControl(conn, 0xA, payload); err != nil {
				return nil, err
			}
		case 0xA:
			continue
		default:
			return nil, errors.New("unsupported websocket frame")
		}
	}
}

func readWebSocketFrame(r *bufio.Reader) (byte, []byte, error) {
	h0, err := r.ReadByte()
	if err != nil {
		return 0, nil, err
	}
	h1, err := r.ReadByte()
	if err != nil {
		return 0, nil, err
	}
	opcode := h0 & 0x0f
	masked := h1&0x80 != 0
	length := uint64(h1 & 0x7f)
	if length == 126 {
		b := make([]byte, 2)
		if _, err := io.ReadFull(r, b); err != nil {
			return 0, nil, err
		}
		length = uint64(b[0])<<8 | uint64(b[1])
	} else if length == 127 {
		b := make([]byte, 8)
		if _, err := io.ReadFull(r, b); err != nil {
			return 0, nil, err
		}
		length = 0
		for _, v := range b {
			length = length<<8 | uint64(v)
		}
	}
	if length > 1<<20 {
		return 0, nil, errors.New("websocket frame too large")
	}
	var mask [4]byte
	if masked {
		if _, err := io.ReadFull(r, mask[:]); err != nil {
			return 0, nil, err
		}
	}
	payload := make([]byte, length)
	if _, err := io.ReadFull(r, payload); err != nil {
		return 0, nil, err
	}
	if masked {
		for i := range payload {
			payload[i] ^= mask[i%4]
		}
	}
	return opcode, payload, nil
}

func writeWebSocketText(w io.Writer, payload []byte) error {
	return writeWebSocketControl(w, 0x1, payload)
}

func writeWebSocketControl(w io.Writer, opcode byte, payload []byte) error {
	header := []byte{0x80 | (opcode & 0x0f)}
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

func upsertDevice(ctx context.Context, execer interface {
	ExecContext(context.Context, string, ...any) (sql.Result, error)
}, deviceID string, now string) error {
	_, err := execer.ExecContext(ctx, `INSERT INTO devices(id, created_at, last_seen_at)
		VALUES(?, ?, ?)
		ON CONFLICT(id) DO UPDATE SET last_seen_at=excluded.last_seen_at`, deviceID, now, now)
	return err
}

func (s *server) maybeCleanupDevices(ctx context.Context) {
	if s.deviceCleanup == nil {
		return
	}
	now := time.Now().UTC()
	s.deviceCleanup.mu.Lock()
	if !s.deviceCleanup.lastRunUTC.IsZero() && now.Sub(s.deviceCleanup.lastRunUTC) < deviceCleanupInterval {
		s.deviceCleanup.mu.Unlock()
		return
	}
	s.deviceCleanup.lastRunUTC = now
	s.deviceCleanup.mu.Unlock()

	cutoff := now.Add(-deviceRetention).Format(time.RFC3339)
	if _, err := s.db.ExecContext(ctx, `DELETE FROM devices WHERE last_seen_at < ?`, cutoff); err != nil {
		log.Printf("warning: device cleanup failed: %v", err)
	}
}

func (s *server) assetCacheLookup(id string) (assetPaths, bool) {
	if s.assetCache == nil {
		return assetPaths{}, false
	}
	s.assetCache.mu.RLock()
	defer s.assetCache.mu.RUnlock()
	paths, ok := s.assetCache.byID[id]
	return paths, ok
}

func (s *server) assetCacheStore(id string, paths assetPaths) {
	if s.assetCache == nil || id == "" {
		return
	}
	s.assetCache.mu.Lock()
	defer s.assetCache.mu.Unlock()
	s.assetCache.byID[id] = paths
}

func (a assetPaths) pathFor(name string) string {
	switch name {
	case "sprite.png":
		return a.png
	case "sprite.json":
		return a.json
	case "config.json":
		return a.config
	default:
		return ""
	}
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
	if _, err := crand.Read(b[:]); err != nil {
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

func stringValue(value any) string {
	if s, ok := value.(string); ok {
		return s
	}
	return ""
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
