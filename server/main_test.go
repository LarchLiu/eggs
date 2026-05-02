package main

import (
	"bufio"
	"bytes"
	"container/list"
	"context"
	"database/sql"
	"encoding/json"
	"io"
	"mime/multipart"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

type localTestServer struct {
	URL    string
	server *http.Server
	ln     net.Listener
}

func (s *localTestServer) Close() {
	_ = s.server.Close()
	_ = s.ln.Close()
}

func newTestServer(t *testing.T) (*server, *localTestServer) {
	t.Helper()
	dir := t.TempDir()
	db, err := sql.Open("sqlite", filepath.Join(dir, "eggs.db"))
	if err != nil {
		t.Fatal(err)
	}
	configureDB(db)
	s := &server{
		db:              db,
		dataDir:         dir,
		assetsDir:       filepath.Join(dir, "assets"),
		publicByDefault: true,
		hub: &hub{
			rooms:         map[string]*roomState{},
			roomByClient:  map[string]string{},
			onlineSprites: map[string]*wsClient{},
		},
		assetCache:    &assetCache{byID: map[string]assetPaths{}},
		deviceCleanup: &deviceCleanupState{},
	}
	if err := os.MkdirAll(s.assetsDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := s.migrate(); err != nil {
		t.Fatal(err)
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/api/v1/sprites", s.handleSprites)
	mux.HandleFunc("/api/v1/sprites/", s.handleSprite)
	mux.HandleFunc("/assets/", s.handleAsset)
	mux.HandleFunc("/ws", s.handleWebSocket)
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Skipf("network listen unavailable in sandbox: %v", err)
		return nil, nil
	}
	httpServer := &http.Server{Handler: mux}
	go func() {
		_ = httpServer.Serve(listener)
	}()
	ts := &localTestServer{
		URL:    "http://" + listener.Addr().String(),
		server: httpServer,
		ln:     listener,
	}
	t.Cleanup(func() {
		ts.Close()
		_ = db.Close()
	})
	return s, ts
}

func TestUploadAndListSprite(t *testing.T) {
	_, ts := newTestServer(t)
	record := uploadTestSprite(t, ts.URL, "device-a", "dino")
	if record["id"] == "" {
		t.Fatalf("missing sprite id: %#v", record)
	}
	if record["png_hash"] == "" || record["json_hash"] == "" {
		t.Fatalf("missing asset hashes: %#v", record)
	}

	resp, err := http.Get(ts.URL + "/api/v1/sprites")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("list status=%d", resp.StatusCode)
	}
	var list struct {
		Sprites []map[string]any `json:"sprites"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&list); err != nil {
		t.Fatal(err)
	}
	if len(list.Sprites) != 1 {
		t.Fatalf("sprites=%d, want 1", len(list.Sprites))
	}
	if list.Sprites[0]["name"] != "dino" {
		t.Fatalf("name=%v", list.Sprites[0]["name"])
	}
	if list.Sprites[0]["png_hash"] == "" || list.Sprites[0]["json_hash"] == "" {
		t.Fatalf("list missing asset hashes: %#v", list.Sprites[0])
	}
}

func TestRandomListSpritesReturnsPublicRecords(t *testing.T) {
	_, ts := newTestServer(t)
	uploadTestSprite(t, ts.URL, "device-a", "dino")
	uploadTestSprite(t, ts.URL, "device-b", "goblin")
	uploadTestSprite(t, ts.URL, "device-c", "cat")

	resp, err := http.Get(ts.URL + "/api/v1/sprites?random=1&limit=2")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("random list status=%d", resp.StatusCode)
	}
	var list struct {
		Sprites []map[string]any `json:"sprites"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&list); err != nil {
		t.Fatal(err)
	}
	if len(list.Sprites) == 0 || len(list.Sprites) > 2 {
		t.Fatalf("unexpected random list size=%d", len(list.Sprites))
	}
	for _, sprite := range list.Sprites {
		if sprite["status"] != "public" {
			t.Fatalf("random list returned non-public sprite: %#v", sprite)
		}
		if sprite["id"] == "" {
			t.Fatalf("random list returned empty id: %#v", sprite)
		}
	}
}

func TestDuplicateUploadSameDeviceReturnsExistingRecord(t *testing.T) {
	s, ts := newTestServer(t)
	first := uploadTestSprite(t, ts.URL, "device-a", "dino")
	second, status := uploadTestSpriteWithStatus(t, ts.URL, "device-a", "dino")
	if status != http.StatusOK {
		t.Fatalf("duplicate upload status=%d, want 200", status)
	}

	if first["id"] != second["id"] {
		t.Fatalf("expected same sprite id, got first=%v second=%v", first["id"], second["id"])
	}

	var count int
	if err := s.db.QueryRow(`SELECT COUNT(*) FROM sprites`).Scan(&count); err != nil {
		t.Fatal(err)
	}
	if count != 1 {
		t.Fatalf("sprite rows=%d, want 1", count)
	}
}

func TestDuplicateUploadDifferentDevicesReuseFiles(t *testing.T) {
	s, ts := newTestServer(t)
	first := uploadTestSprite(t, ts.URL, "device-a", "dino")
	second := uploadTestSprite(t, ts.URL, "device-b", "dino")

	if first["id"] == second["id"] {
		t.Fatalf("different devices should keep distinct sprite records")
	}

	var rows int
	if err := s.db.QueryRow(`SELECT COUNT(*) FROM sprites`).Scan(&rows); err != nil {
		t.Fatal(err)
	}
	if rows != 2 {
		t.Fatalf("sprite rows=%d, want 2", rows)
	}

	var png1, png2, json1, json2 string
	if err := s.db.QueryRow(`SELECT png_path, json_path FROM sprites WHERE id=?`, first["id"]).Scan(&png1, &json1); err != nil {
		t.Fatal(err)
	}
	if err := s.db.QueryRow(`SELECT png_path, json_path FROM sprites WHERE id=?`, second["id"]).Scan(&png2, &json2); err != nil {
		t.Fatal(err)
	}
	if png1 != png2 {
		t.Fatalf("expected shared png path, got %s vs %s", png1, png2)
	}
	if json1 != json2 {
		t.Fatalf("expected shared json path, got %s vs %s", json1, json2)
	}

	blobCount := countFilesUnder(t, filepath.Join(s.assetsDir, "blobs"))
	if blobCount != 2 {
		t.Fatalf("blob files=%d, want 2", blobCount)
	}
}

func TestAssetRequestUsesCacheAfterWarmup(t *testing.T) {
	s, ts := newTestServer(t)
	record := uploadTestSprite(t, ts.URL, "device-a", "dino")
	id, _ := record["id"].(string)
	if id == "" {
		t.Fatalf("missing id in record: %#v", record)
	}

	resp, err := http.Get(ts.URL + "/assets/" + id + "/sprite.json")
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("warmup asset status=%d", resp.StatusCode)
	}

	if _, ok := s.assetCacheLookup(id); !ok {
		t.Fatalf("expected asset cache to contain %q after warmup", id)
	}

	if _, err := s.db.Exec(`DELETE FROM sprites WHERE id=?`, id); err != nil {
		t.Fatal(err)
	}

	resp, err = http.Get(ts.URL + "/assets/" + id + "/sprite.json")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("cached asset status=%d", resp.StatusCode)
	}
}

func TestMaybeCleanupDevicesDeletesExpiredRows(t *testing.T) {
	s, _ := newTestServer(t)
	stale := time.Now().UTC().Add(-(deviceRetention + 2*time.Hour)).Format(time.RFC3339)
	fresh := time.Now().UTC().Format(time.RFC3339)
	if _, err := s.db.Exec(`INSERT INTO devices(id, created_at, last_seen_at) VALUES(?, ?, ?)`, "stale-device", stale, stale); err != nil {
		t.Fatal(err)
	}
	if _, err := s.db.Exec(`INSERT INTO devices(id, created_at, last_seen_at) VALUES(?, ?, ?)`, "fresh-device", fresh, fresh); err != nil {
		t.Fatal(err)
	}

	s.maybeCleanupDevices(context.Background())

	var staleCount int
	if err := s.db.QueryRow(`SELECT COUNT(*) FROM devices WHERE id='stale-device'`).Scan(&staleCount); err != nil {
		t.Fatal(err)
	}
	if staleCount != 0 {
		t.Fatalf("stale device rows=%d, want 0", staleCount)
	}

	var freshCount int
	if err := s.db.QueryRow(`SELECT COUNT(*) FROM devices WHERE id='fresh-device'`).Scan(&freshCount); err != nil {
		t.Fatal(err)
	}
	if freshCount != 1 {
		t.Fatalf("fresh device rows=%d, want 1", freshCount)
	}
}

func TestRejectInvalidMetadata(t *testing.T) {
	_, ts := newTestServer(t)
	body, contentType := multipartUploadBody(t, map[string]string{
		"device_id":   "device-a",
		"sprite_name": "bad",
	}, map[string][]byte{
		"png":  {0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'},
		"json": []byte(`{"frameWidth":0}`),
	})
	resp, err := http.Post(ts.URL+"/api/v1/sprites", contentType, bytes.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusBadRequest {
		t.Fatalf("status=%d, want 400", resp.StatusCode)
	}
}

func TestWebSocketRoomBroadcastAndIsolation(t *testing.T) {
	_, ts := newTestServer(t)
	uploadTestSprite(t, ts.URL, "device-a", "dino")
	uploadTestSprite(t, ts.URL, "device-b", "goblin")
	uploadTestSprite(t, ts.URL, "device-c", "cat")

	a := dialTestWS(t, ts.URL, "device-a", "dino", "room", "ABC")
	defer a.Close()
	writeJSONFrame(t, a, map[string]any{"type": "state", "state": "walk"})
	b := dialTestWS(t, ts.URL, "device-b", "goblin", "room", "ABC")
	defer b.Close()
	c := dialTestWS(t, ts.URL, "device-c", "cat", "room", "XYZ")
	defer c.Close()

	joined := readJSONFrame(t, a)
	if joined["type"] != "peer_joined" {
		t.Fatalf("unexpected join message: %#v", joined)
	}
	joinedSprite, _ := joined["sprite"].(map[string]any)
	if joinedSprite["name"] != "goblin" {
		t.Fatalf("unexpected joined sprite: %#v", joined)
	}
	if joined["state"] != "hatched" {
		t.Fatalf("joined peer should include initial state: %#v", joined)
	}
	snapshot := readJSONFrame(t, b)
	if snapshot["type"] != "room_snapshot" {
		t.Fatalf("unexpected snapshot message: %#v", snapshot)
	}
	peers, _ := snapshot["peers"].([]any)
	if len(peers) != 1 {
		t.Fatalf("unexpected snapshot peers: %#v", snapshot)
	}
	existing, _ := peers[0].(map[string]any)
	existingSprite, _ := existing["sprite"].(map[string]any)
	if existingSprite["name"] != "dino" {
		t.Fatalf("unexpected existing sprite: %#v", snapshot)
	}
	if existing["state"] != "walk" {
		t.Fatalf("snapshot should include current state: %#v", snapshot)
	}
	msg := readJSONFrame(t, b)
	if msg["type"] != "peer_state" || msg["state"] != "walk" {
		t.Fatalf("unexpected state message: %#v", msg)
	}
	stateSprite, _ := msg["sprite"].(map[string]any)
	if stateSprite["name"] != "dino" {
		t.Fatalf("unexpected state sprite: %#v", msg)
	}
	if got := readJSONFrameWithTimeout(t, c, 120*time.Millisecond); got != nil {
		t.Fatalf("isolated room received message: %#v", got)
	}
}

func TestSpriteDetailEndpointRemainsAvailableOutsideRoomInteraction(t *testing.T) {
	_, ts := newTestServer(t)
	record := uploadTestSprite(t, ts.URL, "device-a", "dino")
	resp, err := http.Get(ts.URL + "/api/v1/sprites/" + record["id"].(string))
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("sprite detail status=%d", resp.StatusCode)
	}
	var detail map[string]any
	if err := json.NewDecoder(resp.Body).Decode(&detail); err != nil {
		t.Fatal(err)
	}
	if detail["name"] != "dino" {
		t.Fatalf("unexpected sprite detail: %#v", detail)
	}
}

func TestInviteRoomJoinDeliversExistingPeerSnapshot(t *testing.T) {
	h := &hub{
		rooms:         map[string]*roomState{},
		roomByClient:  map[string]string{},
		onlineSprites: map[string]*wsClient{},
	}
	first := &wsClient{id: "peer-a", mode: "room", spriteID: "sprite-a", roomCode: "LOAD", sendCh: make(chan []byte, 8), doneCh: make(chan struct{}), state: "hatched"}
	second := &wsClient{id: "peer-b", mode: "room", spriteID: "sprite-b", roomCode: "LOAD", sendCh: make(chan []byte, 8), doneCh: make(chan struct{}), state: "hatched"}
	firstDeliveries, err := h.join(first, "LOAD")
	if err != nil {
		t.Fatal(err)
	}
	if len(firstDeliveries) != 1 {
		t.Fatalf("first join deliveries=%d, want 1", len(firstDeliveries))
	}
	secondDeliveries, err := h.join(second, "LOAD")
	if err != nil {
		t.Fatal(err)
	}
	if len(secondDeliveries) != 2 {
		t.Fatalf("second join deliveries=%d, want 2", len(secondDeliveries))
	}
}

func TestRandomLobbyMatchesPairsOnly(t *testing.T) {
	h := &hub{
		rooms:         map[string]*roomState{},
		roomByClient:  map[string]string{},
		waitingRandom: list.New(),
		waitingByID:   map[string]*list.Element{},
		onlineSprites: map[string]*wsClient{},
	}
	a := &wsClient{id: "a", mode: "random", spriteID: "sprite-a", sendCh: make(chan []byte, 8), doneCh: make(chan struct{}), state: "hatched"}
	b := &wsClient{id: "b", mode: "random", spriteID: "sprite-b", sendCh: make(chan []byte, 8), doneCh: make(chan struct{}), state: "hatched"}
	c := &wsClient{id: "c", mode: "random", spriteID: "sprite-c", sendCh: make(chan []byte, 8), doneCh: make(chan struct{}), state: "hatched"}

	first, err := h.join(a, "RANDOM")
	if err != nil {
		t.Fatal(err)
	}
	if len(first) != 0 {
		t.Fatalf("first random join should wait, got %#v", first)
	}
	second, err := h.join(b, "RANDOM")
	if err != nil {
		t.Fatal(err)
	}
	if len(second) != 2 {
		t.Fatalf("second random join should create pair deliveries, got %d", len(second))
	}
	if a.roomCode == "" || a.roomCode != b.roomCode {
		t.Fatalf("random pair should share one room, got a=%q b=%q", a.roomCode, b.roomCode)
	}
	third, err := h.join(c, "RANDOM")
	if err != nil {
		t.Fatal(err)
	}
	if len(third) != 0 {
		t.Fatalf("third random join should wait for another peer, got %#v", third)
	}
	if c.roomCode != "" {
		t.Fatalf("waiting random peer should not be assigned a room yet")
	}
}

func TestRandomWaitingPeerRemovalSkipsDisconnectedClient(t *testing.T) {
	h := &hub{
		rooms:         map[string]*roomState{},
		roomByClient:  map[string]string{},
		waitingRandom: list.New(),
		waitingByID:   map[string]*list.Element{},
		onlineSprites: map[string]*wsClient{},
	}
	a := &wsClient{id: "a", mode: "random", spriteID: "sprite-a", sendCh: make(chan []byte, 8), doneCh: make(chan struct{}), state: "hatched"}
	b := &wsClient{id: "b", mode: "random", spriteID: "sprite-b", sendCh: make(chan []byte, 8), doneCh: make(chan struct{}), state: "hatched"}
	c := &wsClient{id: "c", mode: "random", spriteID: "sprite-c", sendCh: make(chan []byte, 8), doneCh: make(chan struct{}), state: "hatched"}

	if _, err := h.join(a, "RANDOM"); err != nil {
		t.Fatal(err)
	}
	if got := len(h.waitingByID); got != 1 {
		t.Fatalf("waiting peers=%d, want 1", got)
	}

	if deliveries := h.leave(a); len(deliveries) != 0 {
		t.Fatalf("waiting peer leave should not deliver room messages, got %#v", deliveries)
	}
	if _, ok := h.waitingByID[a.id]; ok {
		t.Fatalf("waiting peer %q should be removed from queue", a.id)
	}

	if _, err := h.join(b, "RANDOM"); err != nil {
		t.Fatal(err)
	}
	if got := len(h.waitingByID); got != 1 {
		t.Fatalf("waiting peers after rejoin=%d, want 1", got)
	}

	deliveries, err := h.join(c, "RANDOM")
	if err != nil {
		t.Fatal(err)
	}
	if len(deliveries) != 2 {
		t.Fatalf("join after removal should still pair remaining peer, got %d deliveries", len(deliveries))
	}
	if b.roomCode == "" || b.roomCode != c.roomCode {
		t.Fatalf("expected remaining peer to pair with new peer, got b=%q c=%q", b.roomCode, c.roomCode)
	}
}

func uploadTestSprite(t *testing.T, baseURL string, deviceID string, sprite string) map[string]any {
	t.Helper()
	record, status := uploadTestSpriteWithStatus(t, baseURL, deviceID, sprite)
	if status != http.StatusCreated {
		t.Fatalf("upload status=%d body=%#v", status, record)
	}
	return record
}

func uploadTestSpriteWithStatus(t *testing.T, baseURL string, deviceID string, sprite string) (map[string]any, int) {
	t.Helper()
	body, contentType := multipartUploadBody(t, map[string]string{
		"device_id":    deviceID,
		"sprite_name":  sprite,
		"display_name": sprite,
	}, map[string][]byte{
		"png":  {0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'},
		"json": []byte(`{"frameWidth":251,"frameHeight":251,"columns":1,"rows":1,"frameCount":1,"image":"` + sprite + `.png"}`),
	})
	resp, err := http.Post(baseURL+"/api/v1/sprites", contentType, bytes.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var record map[string]any
	if err := json.NewDecoder(resp.Body).Decode(&record); err != nil {
		data, _ := io.ReadAll(resp.Body)
		t.Fatalf("decode status=%d body=%s err=%v", resp.StatusCode, string(data), err)
	}
	return record, resp.StatusCode
}

func multipartUploadBody(t *testing.T, fields map[string]string, files map[string][]byte) ([]byte, string) {
	t.Helper()
	var body bytes.Buffer
	writer := multipart.NewWriter(&body)
	for name, value := range fields {
		if err := writer.WriteField(name, value); err != nil {
			t.Fatal(err)
		}
	}
	for name, data := range files {
		part, err := writer.CreateFormFile(name, name)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := part.Write(data); err != nil {
			t.Fatal(err)
		}
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	return body.Bytes(), writer.FormDataContentType()
}

func dialTestWS(t *testing.T, baseURL string, deviceID string, spriteName string, mode string, room string) net.Conn {
	t.Helper()
	u, err := url.Parse(baseURL)
	if err != nil {
		t.Fatal(err)
	}
	query := url.Values{}
	query.Set("device_id", deviceID)
	query.Set("sprite", spriteName)
	query.Set("mode", mode)
	query.Set("room", room)
	conn, err := net.Dial("tcp", u.Host)
	if err != nil {
		t.Fatal(err)
	}
	key := "dGhlIHNhbXBsZSBub25jZQ=="
	req := "GET /ws?" + query.Encode() + " HTTP/1.1\r\n" +
		"Host: " + u.Host + "\r\n" +
		"Upgrade: websocket\r\n" +
		"Connection: Upgrade\r\n" +
		"Sec-WebSocket-Key: " + key + "\r\n" +
		"Sec-WebSocket-Version: 13\r\n\r\n"
	if _, err := conn.Write([]byte(req)); err != nil {
		t.Fatal(err)
	}
	reader := bufio.NewReader(conn)
	status, err := reader.ReadString('\n')
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(status, "101") {
		t.Fatalf("websocket status: %s", status)
	}
	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			t.Fatal(err)
		}
		if line == "\r\n" {
			break
		}
	}
	return &bufferedConn{Conn: conn, reader: reader}
}

type bufferedConn struct {
	net.Conn
	reader *bufio.Reader
}

func (c *bufferedConn) Read(p []byte) (int, error) {
	return c.reader.Read(p)
}

func readJSONFrame(t *testing.T, conn net.Conn) map[string]any {
	t.Helper()
	msg := readJSONFrameWithTimeout(t, conn, time.Second)
	if msg == nil {
		t.Fatal("timed out waiting for websocket message")
	}
	return msg
}

func readJSONFrameWithTimeout(t *testing.T, conn net.Conn, timeout time.Duration) map[string]any {
	t.Helper()
	if err := conn.SetReadDeadline(time.Now().Add(timeout)); err != nil {
		t.Fatal(err)
	}
	_, payload, err := readWebSocketFrame(bufio.NewReader(conn))
	if err != nil {
		if ne, ok := err.(net.Error); ok && ne.Timeout() {
			_ = conn.SetReadDeadline(time.Time{})
			return nil
		}
		t.Fatal(err)
	}
	_ = conn.SetReadDeadline(time.Time{})
	var msg map[string]any
	if err := json.Unmarshal(payload, &msg); err != nil {
		t.Fatal(err)
	}
	return msg
}

func writeJSONFrame(t *testing.T, conn net.Conn, msg map[string]any) {
	t.Helper()
	data, err := json.Marshal(msg)
	if err != nil {
		t.Fatal(err)
	}
	if err := writeMaskedTextFrame(conn, data); err != nil {
		t.Fatal(err)
	}
}

func writeMaskedTextFrame(w io.Writer, payload []byte) error {
	header := []byte{0x81}
	if len(payload) < 126 {
		header = append(header, 0x80|byte(len(payload)))
	} else {
		header = append(header, 0x80|126, byte(len(payload)>>8), byte(len(payload)))
	}
	mask := []byte{1, 2, 3, 4}
	masked := make([]byte, len(payload))
	for i, b := range payload {
		masked[i] = b ^ mask[i%4]
	}
	_, err := w.Write(append(append(header, mask...), masked...))
	return err
}

func countFilesUnder(t *testing.T, root string) int {
	t.Helper()
	count := 0
	err := filepath.Walk(root, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if !info.IsDir() {
			count++
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	return count
}
