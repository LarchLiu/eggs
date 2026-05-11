// Local IPC channel between the CLI and the running GUI.
//
// Replaces the older "CLI writes ~/.eggs/bubble-spool/<id>.json, GUI polls"
// path: now `eggs hook` / `eggs message` open a Unix socket (or Windows
// named pipe), write one line of JSON describing a BubbleEvent, and close.
// The GUI's accept loop hands the event straight to BubbleWindowManager,
// so display latency is bounded by the socket round-trip, not by the
// 200ms poll interval.
//
// Connection failures fall through silently — CLI callers already gate on
// `is_gui_running()` via the pid file, so a closed socket simply means the
// GUI shut down between the pid check and the connect.

use std::io;
use std::path::PathBuf;

use tauri::AppHandle;

use crate::bubbles::{BubbleEvent, SharedBubbleWindowManager};
use crate::state;

#[cfg(unix)]
pub fn endpoint_path() -> PathBuf {
    state::app_dir().join("eggs-ipc.sock")
}

#[cfg(windows)]
pub fn endpoint_path() -> PathBuf {
    // Named pipes live in their own kernel namespace, not the filesystem.
    // Keeping the name stable lets `eggs` CLI clients open it by path.
    PathBuf::from(r"\\.\pipe\eggs-ipc")
}

// ---------- server (GUI side) ------------------------------------------

pub fn start_server(app: AppHandle, bubble_windows: SharedBubbleWindowManager) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_server(app, bubble_windows).await {
            eprintln!("eggs ipc server stopped: {e}");
        }
    });
}

#[cfg(unix)]
async fn run_server(app: AppHandle, mgr: SharedBubbleWindowManager) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;

    let path = endpoint_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Stale sockets from a previous crash would make bind() fail with
    // EADDRINUSE — remove unconditionally; the worst case is a concurrent
    // GUI losing its socket, which is fine since single-instance ensures
    // only one GUI runs at a time anyway.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    // 0o600: only the owning user can connect. Anything broader would let
    // other accounts on the box queue arbitrary bubble text.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let app = app.clone();
                let mgr = mgr.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = handle_unix(stream, app, mgr).await {
                        eprintln!("ipc connection error: {e}");
                    }
                });
            }
            Err(e) => {
                eprintln!("ipc accept error: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

#[cfg(unix)]
async fn handle_unix(
    stream: tokio::net::UnixStream,
    app: AppHandle,
    mgr: SharedBubbleWindowManager,
) -> io::Result<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    dispatch_line(&line, &app, &mgr).await;
    Ok(())
}

#[cfg(windows)]
async fn run_server(app: AppHandle, mgr: SharedBubbleWindowManager) -> io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let pipe_name = endpoint_path().to_string_lossy().to_string();
    // Create the first server instance up front, then keep one "spare"
    // server pending for the next client — same idiom the tokio docs use.
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)?;

    loop {
        server.connect().await?;
        // Hand off the connected instance and immediately create the next
        // listener so concurrent clients aren't serialized.
        let connected = server;
        server = ServerOptions::new().create(&pipe_name)?;

        let app = app.clone();
        let mgr = mgr.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = handle_pipe(connected, app, mgr).await {
                eprintln!("ipc connection error: {e}");
            }
        });
    }
}

#[cfg(windows)]
async fn handle_pipe(
    stream: tokio::net::windows::named_pipe::NamedPipeServer,
    app: AppHandle,
    mgr: SharedBubbleWindowManager,
) -> io::Result<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    dispatch_line(&line, &app, &mgr).await;
    Ok(())
}

async fn dispatch_line(line: &str, app: &AppHandle, mgr: &SharedBubbleWindowManager) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    match serde_json::from_str::<BubbleEvent>(trimmed) {
        Ok(event) => mgr.show(app, event).await,
        Err(e) => eprintln!("ipc invalid bubble event: {e}"),
    }
}

// ---------- client (CLI side) ------------------------------------------

pub fn send_bubble_event(event: &BubbleEvent) -> io::Result<()> {
    let mut json = serde_json::to_string(event)?;
    json.push('\n');
    send_raw(json.as_bytes())
}

#[cfg(unix)]
fn send_raw(payload: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(endpoint_path())?;
    stream.write_all(payload)?;
    stream.flush()
}

#[cfg(windows)]
fn send_raw(payload: &[u8]) -> io::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    // Named pipe clients open the pipe path like a file. write_all delivers
    // the whole message in one shot — the server side reads until newline.
    let mut handle = OpenOptions::new()
        .read(true)
        .write(true)
        .open(endpoint_path())?;
    handle.write_all(payload)?;
    handle.flush()
}
