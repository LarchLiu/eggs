package main

import (
	"bufio"
	"bytes"
	"database/sql"
	"encoding/json"
	"io"
	"mime/multipart"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func newTestServer(t *testing.T) (*server, *httptest.Server) {
	t.Helper()
	dir := t.TempDir()
	db, err := sql.Open("sqlite", filepath.Join(dir, "eggs.db"))
	if err != nil {
		t.Fatal(err)
	}
	db.SetMaxOpenConns(1)
	s := &server{
		db:               db,
		dataDir:          dir,
		assetsDir:        filepath.Join(dir, "assets"),
		publicByDefault:  true,
		heartbeatTimeout: defaultHeartbeatTimeout,
		hub:              &hub{rooms: map[string]map[string]*wsClient{}, byID: map[string]*wsClient{}},
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
	mux.HandleFunc("/api/v1/peers/", s.handlePeerSprite)
	mux.HandleFunc("/assets/", s.handleAsset)
	mux.HandleFunc("/ws", s.handleWebSocket)
	ts := httptest.NewServer(mux)
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

func TestRejectInvalidMetadata(t *testing.T) {
	_, ts := newTestServer(t)
	body, contentType := multipartUploadBody(t, map[string]string{
		"device_id":   "device-a",
		"sprite_name": "bad",
	}, map[string][]byte{
		"png":  []byte{0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'},
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
	spriteA := uploadTestSprite(t, ts.URL, "device-a", "dino")["id"].(string)
	spriteB := uploadTestSprite(t, ts.URL, "device-b", "goblin")["id"].(string)
	spriteC := uploadTestSprite(t, ts.URL, "device-c", "cat")["id"].(string)

	a := dialTestWS(t, ts.URL, "device-a", spriteA, "room", "ABC")
	defer a.Close()
	b := dialTestWS(t, ts.URL, "device-b", spriteB, "room", "ABC")
	defer b.Close()
	c := dialTestWS(t, ts.URL, "device-c", spriteC, "room", "XYZ")
	defer c.Close()

	joined := readJSONFrame(t, a)
	if joined["type"] != "peer_joined" || joined["sprite_id"] != spriteB {
		t.Fatalf("unexpected join message: %#v", joined)
	}
	existing := readJSONFrame(t, b)
	if existing["type"] != "peer_joined" || existing["sprite_id"] != spriteA {
		t.Fatalf("unexpected existing peer message: %#v", existing)
	}
	writeJSONFrame(t, a, map[string]any{"type": "state", "state": "walk"})
	msg := readJSONFrame(t, b)
	if msg["type"] != "peer_state" || msg["state"] != "walk" || msg["sprite_id"] != spriteA {
		t.Fatalf("unexpected state message: %#v", msg)
	}
	if got := readJSONFrameWithTimeout(t, c, 120*time.Millisecond); got != nil {
		t.Fatalf("isolated room received message: %#v", got)
	}
}

func TestWebSocketHeartbeatTimeoutRemovesPeer(t *testing.T) {
	s, ts := newTestServer(t)
	s.heartbeatTimeout = 180 * time.Millisecond
	spriteA := uploadTestSprite(t, ts.URL, "device-a", "dino")["id"].(string)
	spriteB := uploadTestSprite(t, ts.URL, "device-b", "goblin")["id"].(string)

	a := dialTestWS(t, ts.URL, "device-a", spriteA, "room", "ABC")
	defer a.Close()
	b := dialTestWS(t, ts.URL, "device-b", spriteB, "room", "ABC")
	defer b.Close()

	_ = readJSONFrame(t, a)
	_ = readJSONFrame(t, b)

	stop := make(chan struct{})
	defer close(stop)
	go func() {
		ticker := time.NewTicker(60 * time.Millisecond)
		defer ticker.Stop()
		for {
			select {
			case <-stop:
				return
			case <-ticker.C:
				_ = writeMaskedTextFrame(a, []byte(`{"type":"heartbeat"}`))
			}
		}
	}()

	deadline := time.Now().Add(1200 * time.Millisecond)
	for {
		left := readJSONFrameWithTimeout(t, a, time.Until(deadline))
		if left == nil {
			t.Fatal("expected peer_left after heartbeat timeout")
		}
		if left["type"] == "heartbeat" {
			continue
		}
		if left["type"] != "peer_left" {
			t.Fatalf("unexpected timeout message: %#v", left)
		}
		break
	}
}

func TestPeerSpriteEndpointOnlyWorksWhilePeerIsOnline(t *testing.T) {
	_, ts := newTestServer(t)
	spriteA := uploadTestSprite(t, ts.URL, "device-a", "dino")["id"].(string)
	spriteB := uploadTestSprite(t, ts.URL, "device-b", "goblin")["id"].(string)

	a := dialTestWS(t, ts.URL, "device-a", spriteA, "room", "ABC")
	defer a.Close()
	b := dialTestWS(t, ts.URL, "device-b", spriteB, "room", "ABC")

	joined := readJSONFrame(t, a)
	peerID, _ := joined["peer_id"].(string)
	if peerID == "" {
		t.Fatalf("missing peer_id in joined message: %#v", joined)
	}
	_ = readJSONFrame(t, b)

	resp, err := http.Get(ts.URL + "/api/v1/peers/" + peerID + "/sprite")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("online peer sprite status=%d", resp.StatusCode)
	}

	_ = b.Close()
	_ = readJSONFrameWithTimeout(t, a, time.Second)

	resp2, err := http.Get(ts.URL + "/api/v1/peers/" + peerID + "/sprite")
	if err != nil {
		t.Fatal(err)
	}
	defer resp2.Body.Close()
	if resp2.StatusCode != http.StatusNotFound {
		t.Fatalf("offline peer sprite status=%d, want 404", resp2.StatusCode)
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
		"png":  []byte{0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'},
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

func dialTestWS(t *testing.T, baseURL string, deviceID string, spriteID string, mode string, room string) net.Conn {
	t.Helper()
	u, err := url.Parse(baseURL)
	if err != nil {
		t.Fatal(err)
	}
	query := url.Values{}
	query.Set("device_id", deviceID)
	query.Set("sprite_id", spriteID)
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
	payload, err := readWebSocketFrame(bufio.NewReader(conn))
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
