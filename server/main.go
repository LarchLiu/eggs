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
	maxPNGBytes            = 8 << 20
	maxJSONBytes           = 1 << 20
	maxConfigBytes         = 256 << 10
	maxFrameSize           = 1024
	maxFrameCount          = 512
	maxGridSide            = 64
	defaultInviteRoomLimit = 5
	minInviteRoomLimit     = 2
	maxInviteRoomLimit     = 100

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
	ContentID     string  `json:"content_id,omitempty"`
	OwnerDeviceID string  `json:"owner_device_id"`
	Name          string  `json:"name"`
	DisplayName   string  `json:"display_name"`
	Status        string  `json:"status"`
	SpriteURL     string  `json:"sprite_url"`
	JSONURL       string  `json:"json_url"`
	ConfigURL     *string `json:"config_url,omitempty"`
	SpriteHash    string  `json:"sprite_hash"`
	JSONHash      string  `json:"json_hash"`
	ConfigHash    *string `json:"config_hash,omitempty"`
	CreatedAt     string  `json:"created_at"`
	SpritePath    string  `json:"-"`
	JSONPath      string  `json:"-"`
	ConfigPath    string  `json:"-"`
}

type sheetMetadata struct {
	// Legacy spritesheet fields (older clients still upload these).
	FrameWidth  int    `json:"frameWidth"`
	FrameHeight int    `json:"frameHeight"`
	Columns     int    `json:"columns"`
	Rows        int    `json:"rows"`
	FrameCount  int    `json:"frameCount"`
	Image       string `json:"image"`

	// Codex pet manifest fields. When SpritesheetPath is non-empty the
	// upload is treated as a pet package and the legacy fields are optional.
	ID              string `json:"id,omitempty"`
	DisplayName     string `json:"displayName,omitempty"`
	Description     string `json:"description,omitempty"`
	SpritesheetPath string `json:"spritesheetPath,omitempty"`
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
	limit   int
	members []*wsClient
}

type delivery struct {
	target *wsClient
	msg    map[string]any
	raw    []byte
}

type assetPaths struct {
	sprite string
	json   string
	config string
}

type assetCache struct {
	mu          sync.RWMutex
	byContentID map[string]assetPaths
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
		assetCache:    &assetCache{byContentID: map[string]assetPaths{}},
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
			content_id TEXT NOT NULL DEFAULT '',
			owner_device_id TEXT NOT NULL,
			name TEXT NOT NULL,
			display_name TEXT NOT NULL,
			status TEXT NOT NULL,
			sprite_path TEXT NOT NULL,
			json_path TEXT NOT NULL,
			config_path TEXT,
			sprite_hash TEXT NOT NULL DEFAULT '',
			json_hash TEXT NOT NULL DEFAULT '',
			config_hash TEXT,
			created_at TEXT NOT NULL
		)`,
		`CREATE INDEX IF NOT EXISTS idx_sprites_owner_name_created_at
			ON sprites(owner_device_id, name, created_at DESC)`,
		`CREATE INDEX IF NOT EXISTS idx_sprites_owner_name_hashes
			ON sprites(owner_device_id, name, sprite_hash, json_hash, config_hash, created_at DESC)`,
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
	// Rename pre-rename columns from old deployments. Idempotent: skipped
	// when the new name already exists OR the old name doesn't.
	for _, rename := range [][2]string{
		{"png_path", "sprite_path"},
		{"png_hash", "sprite_hash"},
	} {
		oldCol, newCol := rename[0], rename[1]
		hasOld, hasNew, err := columnsExist(s.db, "sprites", oldCol, newCol)
		if err != nil {
			return err
		}
		if hasOld && !hasNew {
			if _, err := s.db.Exec(fmt.Sprintf(`ALTER TABLE sprites RENAME COLUMN %s TO %s`, oldCol, newCol)); err != nil {
				return err
			}
		}
	}
	for _, stmt := range []string{
		`ALTER TABLE sprites ADD COLUMN content_id TEXT NOT NULL DEFAULT ''`,
		`ALTER TABLE sprites ADD COLUMN sprite_hash TEXT NOT NULL DEFAULT ''`,
		`ALTER TABLE sprites ADD COLUMN json_hash TEXT NOT NULL DEFAULT ''`,
		`ALTER TABLE sprites ADD COLUMN config_hash TEXT`,
	} {
		if _, err := s.db.Exec(stmt); err != nil && !strings.Contains(strings.ToLower(err.Error()), "duplicate column name") {
			return err
		}
	}
	if err := backfillContentIDs(s.db); err != nil {
		return err
	}
	if _, err := s.db.Exec(`CREATE INDEX IF NOT EXISTS idx_sprites_content_id_created_at
		ON sprites(content_id, created_at DESC)`); err != nil {
		return err
	}
	return nil
}

// columnsExist returns (hasA, hasB) for the given column names on the given
// table, using PRAGMA table_info.
func columnsExist(db *sql.DB, table, colA, colB string) (bool, bool, error) {
	rows, err := db.Query(fmt.Sprintf(`PRAGMA table_info(%s)`, table))
	if err != nil {
		return false, false, err
	}
	defer rows.Close()
	hasA, hasB := false, false
	for rows.Next() {
		var (
			cid       int
			name      string
			ctype     string
			notnull   int
			dfltValue sql.NullString
			pk        int
		)
		if err := rows.Scan(&cid, &name, &ctype, &notnull, &dfltValue, &pk); err != nil {
			return false, false, err
		}
		switch name {
		case colA:
			hasA = true
		case colB:
			hasB = true
		}
	}
	return hasA, hasB, rows.Err()
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

	spriteHashHint, err := normalizeHashHint(r.FormValue("sprite_hash"))
	if err != nil {
		http.Error(w, "sprite_hash: "+err.Error(), http.StatusBadRequest)
		return
	}
	jsonHashHint, err := normalizeHashHint(r.FormValue("json_hash"))
	if err != nil {
		http.Error(w, "json_hash: "+err.Error(), http.StatusBadRequest)
		return
	}
	configHashHint, err := normalizeHashHint(r.FormValue("config_hash"))
	if err != nil {
		http.Error(w, "config_hash: "+err.Error(), http.StatusBadRequest)
		return
	}

	spriteMat, err := s.resolveMaterial(r.MultipartForm, "sprite", spriteHashHint, "sprite", []string{".png", ".webp"}, maxPNGBytes, true)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	jsonMat, err := s.resolveMaterial(r.MultipartForm, "json", jsonHashHint, "json", []string{".json"}, maxJSONBytes, true)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	configMat, err := s.resolveMaterial(r.MultipartForm, "config", configHashHint, "config", []string{".json"}, maxConfigBytes, false)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}

	missing := make([]string, 0, 3)
	if spriteMat.missingBlob {
		missing = append(missing, "sprite")
	}
	if jsonMat.missingBlob {
		missing = append(missing, "json")
	}
	if configMat.missingBlob {
		missing = append(missing, "config")
	}
	if len(missing) > 0 {
		writeJSON(w, http.StatusNotFound, map[string]any{"missing": missing})
		return
	}

	// Validate uploaded bytes only when freshly received. Bytes coming from an
	// existing blob were validated when first uploaded; re-running validation
	// would force pet manifests to be re-readable from disk for no real gain.
	if spriteMat.bytes != nil && !isAcceptedSpritesheet(spriteMat.bytes) {
		http.Error(w, "spritesheet must be PNG or WebP", http.StatusBadRequest)
		return
	}
	if jsonMat.bytes != nil {
		var meta sheetMetadata
		if err := json.Unmarshal(jsonMat.bytes, &meta); err != nil {
			http.Error(w, "invalid sprite json", http.StatusBadRequest)
			return
		}
		if err := validateMetadata(meta); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
	}
	if configMat.bytes != nil && len(bytes.TrimSpace(configMat.bytes)) > 0 {
		if err := validateConfig(configMat.bytes); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
	}

	spriteHash := spriteMat.hash
	jsonHash := jsonMat.hash
	configHash := configMat.hash
	if configMat.bytes != nil && len(bytes.TrimSpace(configMat.bytes)) == 0 {
		configHash = ""
	}
	contentID := contentIDFromHashes(spriteHash, jsonHash, configHash)

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
	existingID, err := existingSpriteID(tx, deviceID, spriteName, spriteHash, jsonHash, configHash)
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

	spritePath := spriteMat.blobPath
	if spriteMat.bytes != nil {
		spritePath, err = ensureBlobFile(filepath.Join(s.assetsDir, "blobs", "sprite"), spriteHash, spritesheetSuffix(spriteMat.bytes), spriteMat.bytes)
		if err != nil {
			http.Error(w, "could not store png", http.StatusInternalServerError)
			return
		}
	}
	jsonPath := jsonMat.blobPath
	if jsonMat.bytes != nil {
		jsonPath, err = ensureBlobFile(filepath.Join(s.assetsDir, "blobs", "json"), jsonHash, ".json", jsonMat.bytes)
		if err != nil {
			http.Error(w, "could not store json", http.StatusInternalServerError)
			return
		}
	}
	configPath := configMat.blobPath
	if configMat.bytes != nil && len(bytes.TrimSpace(configMat.bytes)) > 0 {
		configPath, err = ensureBlobFile(filepath.Join(s.assetsDir, "blobs", "config"), configHash, ".json", configMat.bytes)
		if err != nil {
			http.Error(w, "could not store config", http.StatusInternalServerError)
			return
		}
	}

	id := randomID()
	if _, err := tx.Exec(`INSERT INTO sprites(
			id, content_id, owner_device_id, name, display_name, status, sprite_path, json_path, config_path, sprite_hash, json_hash, config_hash, created_at
		) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		id, contentID, deviceID, spriteName, displayName, status, spritePath, jsonPath, nullable(configPath), spriteHash, jsonHash, nullable(configHash), now,
	); err != nil {
		http.Error(w, "could not insert sprite", http.StatusInternalServerError)
		return
	}
	if err := tx.Commit(); err != nil {
		http.Error(w, "could not commit upload", http.StatusInternalServerError)
		return
	}
	s.assetCacheStore(contentID, assetPaths{sprite: spritePath, json: jsonPath, config: configPath})
	s.maybeCleanupDevices(context.Background())
	record, _ := s.spriteByID(r.Context(), id, r)
	writeJSON(w, http.StatusCreated, record)
}

func (s *server) listSprites(w http.ResponseWriter, r *http.Request) {
	deviceID := safeID(r.URL.Query().Get("device_id"))
	spriteName := safeName(r.URL.Query().Get("sprite_name"))
	if deviceID != "" && spriteName != "" {
		record, err := s.loadSpriteByOwnerAndName(r.Context(), deviceID, spriteName)
		if errors.Is(err, sql.ErrNoRows) {
			writeJSON(w, http.StatusOK, map[string]any{"sprite": nil})
			return
		}
		if err != nil {
			http.Error(w, "database unavailable", http.StatusInternalServerError)
			return
		}
		writeJSON(w, http.StatusOK, map[string]any{"sprite": record.withBaseURL(s.requestBaseURL(r))})
		return
	}
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
	rows, err := s.db.QueryContext(ctx, `SELECT id, content_id, owner_device_id, name, display_name, status, sprite_path, json_path, config_path, sprite_hash, json_hash, config_hash, created_at
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
	rows, err := s.db.QueryContext(ctx, `SELECT id, content_id, owner_device_id, name, display_name, status, sprite_path, json_path, config_path, sprite_hash, json_hash, config_hash, created_at
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
	rows, err := s.db.QueryContext(ctx, `SELECT id, content_id, owner_device_id, name, display_name, status, sprite_path, json_path, config_path, sprite_hash, json_hash, config_hash, created_at
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
	if id == "" || !isAcceptedAssetName(name) {
		http.NotFound(w, r)
		return
	}
	path := ""
	if paths, ok := s.assetCacheLookup(id); ok {
		path = paths.pathFor(name)
	} else {
		record, err := s.loadSpriteByContentID(r.Context(), id)
		if errors.Is(err, sql.ErrNoRows) {
			http.NotFound(w, r)
			return
		}
		if err != nil {
			http.Error(w, "database unavailable", http.StatusInternalServerError)
			return
		}
		path = assetPaths{
			sprite: record.SpritePath,
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
	roomLimit := defaultInviteRoomLimit
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
	} else {
		parsedLimit, err := parseInviteRoomLimit(r.URL.Query().Get("room_limit"))
		if err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		roomLimit = parsedLimit
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

	deliveries, err := s.hub.join(client, roomCode, roomLimit)
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
		if t == "sprite" {
			nextSprite := safeName(stringValue(msg["sprite"]))
			if nextSprite == "" {
				continue
			}
			record, err := s.loadSpriteByOwnerAndName(r.Context(), client.deviceID, nextSprite)
			if err != nil {
				continue
			}
			record = record.withBaseURL(s.requestBaseURL(r))
			if err := s.hub.updateClientSprite(client, record); err != nil {
				continue
			}
			state := strings.TrimSpace(stringValue(msg["state"]))
			if state != "" {
				client.applyIncomingMessage("state", map[string]any{"state": state})
			}
			peerMsg := peerMessage("peer_sprite_changed", client)
			sendDeliveries(s.hub.broadcast(client, peerMsg))
			continue
		}
		outType := map[string]string{
			"state":  "peer_state",
			"action": "peer_action",
			"chat":   "peer_chat",
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
	row := s.db.QueryRowContext(ctx, `SELECT id, content_id, owner_device_id, name, display_name, status, sprite_path, json_path, config_path, sprite_hash, json_hash, config_hash, created_at FROM sprites WHERE id=?`, id)
	record, err := scanSprite(row, "")
	if err != nil {
		return record, err
	}
	s.assetCacheStore(record.ContentID, assetPaths{
		sprite: record.SpritePath,
		json:   record.JSONPath,
		config: record.ConfigPath,
	})
	return record, nil
}

func (s *server) loadSpriteByContentID(ctx context.Context, contentID string) (spriteRecord, error) {
	row := s.db.QueryRowContext(ctx, `SELECT id, content_id, owner_device_id, name, display_name, status, sprite_path, json_path, config_path, sprite_hash, json_hash, config_hash, created_at
		FROM sprites
		WHERE content_id=?
		ORDER BY created_at DESC
		LIMIT 1`, contentID)
	record, err := scanSprite(row, "")
	if err != nil {
		return record, err
	}
	s.assetCacheStore(record.ContentID, assetPaths{
		sprite: record.SpritePath,
		json:   record.JSONPath,
		config: record.ConfigPath,
	})
	return record, nil
}

func (s *server) loadSpriteByOwnerAndName(ctx context.Context, deviceID string, spriteName string) (spriteRecord, error) {
	row := s.db.QueryRowContext(ctx, `SELECT id, content_id, owner_device_id, name, display_name, status, sprite_path, json_path, config_path, sprite_hash, json_hash, config_hash, created_at
		FROM sprites
		WHERE owner_device_id=? AND name=?
		ORDER BY created_at DESC
		LIMIT 1`, deviceID, spriteName)
	record, err := scanSprite(row, "")
	if err != nil {
		return record, err
	}
	s.assetCacheStore(record.ContentID, assetPaths{
		sprite: record.SpritePath,
		json:   record.JSONPath,
		config: record.ConfigPath,
	})
	return record, nil
}

func scanSprite(scanner interface{ Scan(...any) error }, baseURL string) (spriteRecord, error) {
	var rec spriteRecord
	var spritePath, jsonPath string
	var configPath sql.NullString
	var configHash sql.NullString
	if err := scanner.Scan(&rec.ID, &rec.ContentID, &rec.OwnerDeviceID, &rec.Name, &rec.DisplayName, &rec.Status, &spritePath, &jsonPath, &configPath, &rec.SpriteHash, &rec.JSONHash, &configHash, &rec.CreatedAt); err != nil {
		return rec, err
	}
	rec.SpritePath = spritePath
	rec.JSONPath = jsonPath
	rec.SpriteURL = baseURL + "/assets/" + rec.assetID() + "/" + spriteAssetName(spritePath)
	rec.JSONURL = baseURL + "/assets/" + rec.assetID() + "/sprite.json"
	if configPath.Valid && configPath.String != "" {
		rec.ConfigPath = configPath.String
		u := baseURL + "/assets/" + rec.assetID() + "/config.json"
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
	out.SpriteURL = baseURL + "/assets/" + r.assetID() + "/" + spriteAssetName(r.SpritePath)
	out.JSONURL = baseURL + "/assets/" + r.assetID() + "/sprite.json"
	if r.ConfigHash != nil {
		u := baseURL + "/assets/" + r.assetID() + "/config.json"
		out.ConfigURL = &u
	} else {
		out.ConfigURL = nil
	}
	return out
}

func peerMessage(messageType string, client *wsClient) map[string]any {
	sprite, state := client.presenceSnapshot()
	msg := map[string]any{
		"type":      messageType,
		"peer_id":   client.id,
		"device_id": client.deviceID,
		"sprite":    sprite,
	}
	if state != "" {
		msg["state"] = state
	}
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

func (h *hub) join(c *wsClient, roomCode string, roomLimit int) ([]delivery, error) {
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
		deliveries, err := h.joinInviteRoomLocked(c, roomCode, roomLimit)
		if err != nil {
			delete(h.onlineSprites, c.spriteID)
			return nil, err
		}
		return deliveries, nil
	}
	return h.joinRandomLocked(c), nil
}

func (h *hub) joinInviteRoomLocked(c *wsClient, roomCode string, roomLimit int) ([]delivery, error) {
	room := h.rooms[roomCode]
	if room == nil {
		room = &roomState{limit: normalizeInviteRoomLimit(roomLimit)}
		h.rooms[roomCode] = room
	}
	if room.full() {
		return nil, fmt.Errorf("room is full")
	}
	existing := room.peers()
	room.add(c)
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
		room := &roomState{
			limit:   minInviteRoomLimit,
			members: []*wsClient{peer, c},
		}
		h.rooms[roomCode] = room
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
	others := room.remove(c.id)
	delete(h.roomByClient, c.id)
	c.roomCode = ""
	deliveries := make([]delivery, 0, 2)
	for _, peer := range others {
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
	h.matchMu.Lock()
	defer h.matchMu.Unlock()
	roomCode := h.roomByClient[sender.id]
	if roomCode == "" {
		return nil
	}
	room := h.rooms[roomCode]
	if room == nil {
		return nil
	}
	peers := room.peers()
	deliveries := make([]delivery, 0, len(peers))
	for _, peer := range peers {
		if peer == nil || peer.id == sender.id {
			continue
		}
		deliveries = append(deliveries, delivery{
			target: peer,
			raw:    data,
		})
	}
	return deliveries
}

func (h *hub) updateClientSprite(c *wsClient, sprite spriteRecord) error {
	h.onlineMu.Lock()
	defer h.onlineMu.Unlock()
	if existing := h.onlineSprites[sprite.ID]; existing != nil && existing != c {
		return fmt.Errorf("sprite is already online")
	}
	delete(h.onlineSprites, c.spriteID)
	c.setSprite(sprite)
	h.onlineSprites[c.spriteID] = c
	return nil
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
	return r != nil && len(r.members) >= r.effectiveLimit()
}

func (r *roomState) empty() bool {
	return r == nil || len(r.members) == 0
}

func (r *roomState) add(c *wsClient) {
	if r == nil || c == nil {
		return
	}
	for _, peer := range r.members {
		if peer != nil && peer.id == c.id {
			return
		}
	}
	r.members = append(r.members, c)
}

func (r *roomState) peers() []*wsClient {
	if r == nil {
		return nil
	}
	peers := make([]*wsClient, 0, len(r.members))
	peers = append(peers, r.members...)
	return peers
}

func (r *roomState) remove(clientID string) []*wsClient {
	if r == nil {
		return nil
	}
	for i, peer := range r.members {
		if peer == nil || peer.id != clientID {
			continue
		}
		r.members = append(r.members[:i], r.members[i+1:]...)
		return r.peers()
	}
	return nil
}

func (r *roomState) onlyPeer() *wsClient {
	if r == nil {
		return nil
	}
	if len(r.members) == 1 {
		return r.members[0]
	}
	return nil
}

func (r *roomState) effectiveLimit() int {
	if r == nil {
		return defaultInviteRoomLimit
	}
	return normalizeInviteRoomLimit(r.limit)
}

func normalizeInviteRoomLimit(limit int) int {
	if limit <= 0 {
		return defaultInviteRoomLimit
	}
	if limit < minInviteRoomLimit {
		return minInviteRoomLimit
	}
	if limit > maxInviteRoomLimit {
		return maxInviteRoomLimit
	}
	return limit
}

func parseInviteRoomLimit(raw string) (int, error) {
	if strings.TrimSpace(raw) == "" {
		return defaultInviteRoomLimit, nil
	}
	n, err := strconv.Atoi(strings.TrimSpace(raw))
	if err != nil {
		return 0, fmt.Errorf("room_limit must be an integer between %d and %d", minInviteRoomLimit, maxInviteRoomLimit)
	}
	if n < minInviteRoomLimit || n > maxInviteRoomLimit {
		return 0, fmt.Errorf("room_limit must be between %d and %d", minInviteRoomLimit, maxInviteRoomLimit)
	}
	return n, nil
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

func (c *wsClient) presenceSnapshot() (spriteRecord, string) {
	c.stateMu.RLock()
	defer c.stateMu.RUnlock()
	return c.sprite, c.state
}

func (c *wsClient) setSprite(sprite spriteRecord) {
	c.stateMu.Lock()
	defer c.stateMu.Unlock()
	c.sprite = sprite
	c.spriteID = sprite.ID
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
	switch length {
	case 126:
		b := make([]byte, 2)
		if _, err := io.ReadFull(r, b); err != nil {
			return 0, nil, err
		}
		length = uint64(b[0])<<8 | uint64(b[1])
	case 127:
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

func existingSpriteID(tx *sql.Tx, deviceID string, spriteName string, spriteHash string, jsonHash string, configHash string) (string, error) {
	var id string
	err := tx.QueryRow(
		`SELECT id FROM sprites
		WHERE owner_device_id = ?
		  AND name = ?
		  AND sprite_hash = ?
		  AND json_hash = ?
		  AND COALESCE(config_hash, '') = ?
		ORDER BY created_at DESC
		LIMIT 1`,
		deviceID, spriteName, spriteHash, jsonHash, configHash,
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
	paths, ok := s.assetCache.byContentID[id]
	return paths, ok
}

func (s *server) assetCacheStore(contentID string, paths assetPaths) {
	if s.assetCache == nil || contentID == "" {
		return
	}
	s.assetCache.mu.Lock()
	defer s.assetCache.mu.Unlock()
	s.assetCache.byContentID[contentID] = paths
}

func (a assetPaths) pathFor(name string) string {
	switch name {
	case "sprite.png", "sprite.webp":
		return a.sprite
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

// material captures one upload field (png / json / config) resolved either
// from uploaded bytes or from an existing on-disk blob looked up by hash.
type material struct {
	bytes       []byte
	hash        string
	blobPath    string
	missingBlob bool // true iff the client asked for hash-only mode but the blob is gone
}

// resolveMaterial returns the bytes-or-path resolution for a single multipart
// field. If the file part is present, returns the uploaded bytes plus the
// computed hash (cross-checked against claimedHash if provided). Otherwise
// the field is in hash-only mode: we look for a blob at
// `${assetsDir}/blobs/<blobSubdir>/<claimedHash><suffix>` for each suffix in
// `suffixes`. If found, returns the blob path with bytes=nil. If neither file
// part nor a usable hash is provided for a required field, returns a 400-style
// error. If the field is hash-only but the blob is missing, returns a material
// with missingBlob=true so the caller can build a 404 response.
func (s *server) resolveMaterial(form *multipart.Form, field, claimedHash, blobSubdir string, suffixes []string, sizeLimit int64, required bool) (material, error) {
	if hasPart(form, field) {
		data, err := readPart(form, field, sizeLimit)
		if err != nil {
			return material{}, err
		}
		actual := fileHash(data)
		if claimedHash != "" && !strings.EqualFold(claimedHash, actual) {
			return material{}, fmt.Errorf("%s_hash does not match uploaded %s contents", field, field)
		}
		return material{bytes: data, hash: actual}, nil
	}
	if claimedHash == "" {
		if required {
			return material{}, fmt.Errorf("%s or %s_hash is required", field, field)
		}
		return material{}, nil
	}
	blobDir := filepath.Join(s.assetsDir, "blobs", blobSubdir)
	for _, suffix := range suffixes {
		candidate := filepath.Join(blobDir, claimedHash+suffix)
		if _, err := os.Stat(candidate); err == nil {
			return material{hash: claimedHash, blobPath: candidate}, nil
		} else if !errors.Is(err, os.ErrNotExist) {
			return material{}, err
		}
	}
	return material{hash: claimedHash, missingBlob: true}, nil
}

var sha256HexRe = regexp.MustCompile(`^[a-fA-F0-9]{64}$`)

// normalizeHashHint trims and lowercases a client-supplied SHA-256 hex string,
// rejecting anything that does not have exactly 64 hex digits. An empty input
// is allowed (means "no hint").
func normalizeHashHint(raw string) (string, error) {
	hint := strings.TrimSpace(raw)
	if hint == "" {
		return "", nil
	}
	if !sha256HexRe.MatchString(hint) {
		return "", errors.New("must be a 64-character hex SHA-256 digest")
	}
	return strings.ToLower(hint), nil
}

func validateMetadata(meta sheetMetadata) error {
	if strings.TrimSpace(meta.SpritesheetPath) != "" || strings.TrimSpace(meta.ID) != "" {
		return validatePetMetadata(meta)
	}
	return validateLegacySheetMetadata(meta)
}

func validatePetMetadata(meta sheetMetadata) error {
	id := strings.TrimSpace(meta.ID)
	if id == "" {
		return errors.New("pet manifest must contain id")
	}
	if safeName(id) == "" {
		return errors.New("pet id must be alphanumeric, underscore, or hyphen")
	}
	if strings.TrimSpace(meta.SpritesheetPath) == "" {
		return errors.New("pet manifest must contain spritesheetPath")
	}
	// Atlas geometry (8x9 cells of 192x208) is a hardcoded contract on the
	// client; the server intentionally does not enforce it so future contract
	// revisions don't require a coordinated server release.
	return nil
}

func validateLegacySheetMetadata(meta sheetMetadata) error {
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

func isWebP(data []byte) bool {
	return len(data) >= 12 &&
		bytes.Equal(data[:4], []byte("RIFF")) &&
		bytes.Equal(data[8:12], []byte("WEBP"))
}

func isAcceptedSpritesheet(data []byte) bool {
	return isPNG(data) || isWebP(data)
}

// spritesheetSuffix picks the on-disk filename suffix for a stored blob. The
// suffix is just a hint for human inspection of ${assetsDir}/blobs/sprite/;
// the public asset URL is /assets/<content_id>/sprite.{png,webp} matching the
// actual stored format, and Content-Type is sniffed by http.ServeFile from the
// bytes.
func spritesheetSuffix(data []byte) string {
	if isWebP(data) {
		return ".webp"
	}
	return ".png"
}

// spriteAssetName converts a stored blob path back to its public asset
// filename. Blobs land at .../<hash>.png or .../<hash>.webp, so the URL
// extension follows whatever was actually persisted.
func spriteAssetName(blobPath string) string {
	switch strings.ToLower(filepath.Ext(blobPath)) {
	case ".webp":
		return "sprite.webp"
	default:
		return "sprite.png"
	}
}

// isAcceptedAssetName allow-lists the filename component of
// /assets/<content_id>/<name>. Both PNG and WebP variants are valid for the
// sprite payload so peers can download whatever was originally uploaded.
func isAcceptedAssetName(name string) bool {
	switch name {
	case "sprite.png", "sprite.webp", "sprite.json", "config.json":
		return true
	default:
		return false
	}
}

func fileHash(data []byte) string {
	sum := sha256.Sum256(data)
	return hex.EncodeToString(sum[:])
}

func contentIDFromHashes(spriteHash, jsonHash, configHash string) string {
	sum := sha256.Sum256([]byte("eggs-content-v1\n" + spriteHash + "\n" + jsonHash + "\n" + configHash))
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

func (r spriteRecord) assetID() string {
	return r.ContentID
}

func backfillContentIDs(db *sql.DB) error {
	rows, err := db.Query(`SELECT id, sprite_hash, json_hash, COALESCE(config_hash, ''), sprite_path, json_path, COALESCE(config_path, '') FROM sprites WHERE content_id = ''`)
	if err != nil {
		return err
	}
	defer rows.Close()
	type rowUpdate struct {
		id        string
		contentID string
	}
	var updates []rowUpdate
	for rows.Next() {
		var id, spriteHash, jsonHash, configHash string
		var spritePath, jsonPath, configPath string
		if err := rows.Scan(&id, &spriteHash, &jsonHash, &configHash, &spritePath, &jsonPath, &configPath); err != nil {
			return err
		}
		if spriteHash == "" && spritePath != "" {
			data, err := os.ReadFile(spritePath)
			if err != nil {
				return err
			}
			spriteHash = fileHash(data)
		}
		if jsonHash == "" && jsonPath != "" {
			data, err := os.ReadFile(jsonPath)
			if err != nil {
				return err
			}
			jsonHash = fileHash(data)
		}
		if configHash == "" && configPath != "" {
			data, err := os.ReadFile(configPath)
			if err != nil {
				return err
			}
			configHash = fileHash(data)
		}
		if spriteHash == "" || jsonHash == "" {
			continue
		}
		updates = append(updates, rowUpdate{
			id:        id,
			contentID: contentIDFromHashes(spriteHash, jsonHash, configHash),
		})
	}
	if err := rows.Err(); err != nil {
		return err
	}
	for _, update := range updates {
		if _, err := db.Exec(`UPDATE sprites SET content_id=? WHERE id=?`, update.contentID, update.id); err != nil {
			return err
		}
	}
	return nil
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
