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
		db:              db,
		dataDir:         dir,
		assetsDir:       filepath.Join(dir, "assets"),
		publicByDefault: true,
		hub:             &hub{rooms: map[string]map[string]*wsClient{}},
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

func uploadTestSprite(t *testing.T, baseURL string, deviceID string, sprite string) map[string]any {
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
	if resp.StatusCode != http.StatusCreated {
		data, _ := io.ReadAll(resp.Body)
		t.Fatalf("upload status=%d body=%s", resp.StatusCode, string(data))
	}
	var record map[string]any
	if err := json.NewDecoder(resp.Body).Decode(&record); err != nil {
		t.Fatal(err)
	}
	return record
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
