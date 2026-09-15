//! Agent-local request inspector + debugging dashboard.
//!
//! ngrok-style: the agent sees every request passing to/from the local
//! service, so it records an HTTP-level log (method, path, status, timing,
//! sizes, payloads) served on a localhost-only port (`--dashboard-addr`,
//! default 127.0.0.1:16345). This never leaves the machine — useful for local
//! debugging while the public tunnel stays opaque to the server.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use dashmap::DashMap;
use rust_embed::RustEmbed;
use serde::Serialize;

/// Keep the most recent this-many requests in memory.
pub const MAX_RECORDS: usize = 200;
/// Beyond this many header bytes we stop parsing, just keep counting bytes.
const MAX_HEAD: usize = 8 * 1024;
/// Payload preview cap stored per request.
const PAYLOAD_CAP: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct RequestRecord {
    pub id: u64,
    pub tunnel: String,
    pub method: String,
    pub path: String,
    pub status: Option<u16>,
    pub duration_ms: u64,
    pub req_bytes: u64,
    pub resp_bytes: u64,
    pub started_at: String,
    /// Raw request head+body (utf8-lossy, capped) for the detail view/replay.
    pub payload: String,
    /// Raw response head+body (utf8-lossy, capped).
    pub resp_payload: String,
}

/// Per-stream capture state while the stream is alive.
///
/// HTTP/1.1 keep-alive multiplexes several requests over one TCP stream; we
/// slice them by detecting successive request lines in the request direction
/// and finalize one [`RequestRecord`] per request (coarse byte attribution).
pub struct StreamCapture {
    /// Header parse buffer for the current request (request direction).
    req_head: Vec<u8>,
    /// Request bytes since stream start (detail/totals).
    req_bytes: u64,
    /// Bytes attributed to the current request segment.
    req_seg: u64,
    /// Header parse buffer for the current response.
    resp_head: Vec<u8>,
    /// Response bytes since stream start.
    resp_bytes: u64,
    /// Bytes attributed to the current response segment.
    resp_seg: u64,
    /// Current request (None until its request line is seen).
    method: Option<String>,
    path: Option<String>,
    /// Status of the current response (None until the status line is seen).
    status: Option<u16>,
    /// When the current request segment started.
    seg_started: Instant,
    /// Scan cursor into `req_head`.
    scan: usize,
    /// Current request header block is complete (`\r\n\r\n` seen).
    head_done: bool,
}

pub struct Inspector {
    /// Capture only when an http tunnel is active.
    enabled: AtomicBool,
    /// Tunnel display name, e.g. `http:mysite`.
    tunnel: Mutex<String>,
    streams: DashMap<u64, StreamCapture>,
    records: Mutex<VecDeque<RequestRecord>>,
    next_id: AtomicU64,
}

static INSPECTOR: OnceLock<Inspector> = OnceLock::new();

pub fn init() {
    let _ = INSPECTOR.set(Inspector {
        enabled: AtomicBool::new(false),
        tunnel: Mutex::new(String::new()),
        streams: DashMap::new(),
        records: Mutex::new(VecDeque::new()),
        next_id: AtomicU64::new(1),
    });
    let _ = INFO.set(Mutex::new(AgentInfo::default()));
}

fn inspector() -> &'static Inspector {
    INSPECTOR.get().expect("inspector not initialized")
}

/// Enable capture for the current http tunnel (`name` e.g. `http:mysite`).
/// TCP tunnels keep the inspector disabled (nothing structured to log).
pub fn enable(name: &str) {
    let i = inspector();
    i.enabled.store(true, Ordering::Relaxed);
    *i.tunnel.lock().unwrap() = name.to_string();
}

/// Bytes flowing server -> local (the request direction).
pub fn observe_request(stream_id: u64, data: &[u8]) {
    let i = inspector();
    if !i.enabled.load(Ordering::Relaxed) {
        return;
    }
    let mut c = i
        .streams
        .entry(stream_id)
        .or_insert_with(StreamCapture::new);
    let m = c.value_mut();
    m.req_bytes += data.len() as u64;
    m.req_seg += data.len() as u64;
    if m.req_head.len() >= MAX_HEAD {
        return; // too big to parse headers; keep counting bytes only
    }
    m.req_head.extend_from_slice(data);

    // Scan incremental request lines; a *new* request line after the first
    // means keep-alive pipelining/second request -> finalize the previous.
    loop {
        let line_start = m.scan;
        let Some(line_end) = find_line_end(&m.req_head, m.scan) else {
            break;
        };
        let line =
            std::str::from_utf8(&m.req_head[m.scan..line_end]).unwrap_or("").trim_end_matches('\r');
        m.scan = line_end + 1;
        let Some((method, path)) = is_request_line(line) else {
            continue;
        };

        if m.method.is_some() {
            push_record(i, m); // keep-alive: hand off the previous request
        }
        m.method = Some(method);
        m.path = Some(path);
        m.seg_started = Instant::now();
        // Re-credit the new request's bytes already buffered in req_head
        // (they were counted into the previous segment above).
        m.req_seg = m.req_head.len().saturating_sub(line_start) as u64;
        m.head_done = false;
        break;
    }

    // Mark the current request header complete once its terminator arrives.
    if m.method.is_some() && !m.head_done && has_header_terminator(&m.req_head) {
        m.head_done = true;
    }
}

/// Bytes flowing local -> server (the response direction).
pub fn observe_response(stream_id: u64, data: &[u8]) {
    let i = inspector();
    if !i.enabled.load(Ordering::Relaxed) {
        return;
    }
    let mut c = i
        .streams
        .entry(stream_id)
        .or_insert_with(StreamCapture::new);
    let m = c.value_mut();
    m.resp_bytes += data.len() as u64;
    m.resp_seg += data.len() as u64;
    if m.status.is_none() && m.resp_head.len() < MAX_HEAD {
        m.resp_head.extend_from_slice(data);
        if let Some(code) = parse_status_line(&m.resp_head) {
            m.status = Some(code);
        }
    }
}

/// Finalize a stream and record its pending request(s). Called when the
/// stream closes.
pub fn finish_stream(stream_id: u64) {
    let i = inspector();
    if !i.enabled.load(Ordering::Relaxed) {
        return;
    }
    let Some((_, mut c)) = i.streams.remove(&stream_id) else {
        return;
    };
    if c.method.is_some() {
        push_record(i, &mut c);
    }
}

impl StreamCapture {
    fn new() -> Self {
        StreamCapture {
            req_head: Vec::new(),
            req_bytes: 0,
            req_seg: 0,
            resp_head: Vec::new(),
            resp_bytes: 0,
            resp_seg: 0,
            method: None,
            path: None,
            status: None,
            seg_started: Instant::now(),
            scan: 0,
            head_done: false,
        }
    }
}

/// Build a record from the current segment and push it to the ring buffer,
/// then reset the segment state for the next request on this stream.
fn push_record(i: &Inspector, c: &mut StreamCapture) {
    let Some(method) = c.method.take() else {
        return;
    };
    let path = c.path.take().unwrap_or_default();
    let status = c.status.take();
    let id = i.next_id.fetch_add(1, Ordering::Relaxed);
    let record = RequestRecord {
        id,
        tunnel: i.tunnel.lock().unwrap().clone(),
        duration_ms: c.seg_started.elapsed().as_millis() as u64,
        method,
        path,
        status,
        req_bytes: c.req_seg,
        resp_bytes: c.resp_seg,
        started_at: format_timestamp(),
        payload: preview(&c.req_head),
        resp_payload: preview(&c.resp_head),
    };
    let mut recs = i.records.lock().unwrap();
    if recs.len() >= MAX_RECORDS {
        recs.pop_front();
    }
    recs.push_back(record);

    // reset segment state for the next request on this stream
    c.req_head.clear();
    c.resp_head.clear();
    c.req_seg = 0;
    c.resp_seg = 0;
    c.scan = 0;
    c.head_done = false;
    c.seg_started = Instant::now();
}

fn find_line_end(buf: &[u8], from: usize) -> Option<usize> {
    buf[from..].iter().position(|&b| b == b'\n').map(|p| from + p)
}

fn has_header_terminator(buf: &[u8]) -> bool {
    buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.windows(2).any(|w| w == b"\n\n")
}

/// Match `METHOD SP TARGET SP HTTP/x.y` (and NOT a response status line).
fn is_request_line(line: &str) -> Option<(String, String)> {
    let mut it = line.split_whitespace();
    let method = it.next()?;
    if method.is_empty() || method.len() > 8 || !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let target = it.next()?;
    if !target.starts_with('/') {
        return None;
    }
    let version = it.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    Some((method.to_string(), target.to_string()))
}

fn preview(buf: &[u8]) -> String {
    let cut = buf.len().min(PAYLOAD_CAP);
    String::from_utf8_lossy(&buf[..cut]).into_owned()
}


fn parse_status_line(buf: &[u8]) -> Option<u16> {
    let line = buf.splitn(2, |&b| b == b'\n').next()?;
    let line = std::str::from_utf8(line).ok()?.trim_end_matches('\r');
    let mut parts = line.split_whitespace();
    let _version = parts.next()?;
    parts.next()?.parse().ok()
}

fn format_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() % 86400;
    format!("{:02}:{:02}:{:02}.{:03}", secs / 3600, (secs % 3600) / 60, secs % 60, now.subsec_millis())
}

// ---------------------------------------------------------------------------
// Local dashboard HTTP server
// ---------------------------------------------------------------------------

/// Static facts about this agent run, updated on every (re)connect.
#[derive(Debug, Clone, Default)]
pub struct AgentInfo {
    pub mode: String,       // "http" | "tcp"
    pub local: String,      // e.g. "127.0.0.1:8000"
    pub remote: String,     // public entry, e.g. "mysite:8080" / "tcp://host:18008"
}
static INFO: OnceLock<Mutex<AgentInfo>> = OnceLock::new();

/// Overwrite the current tunnel facts. Called on every handshake success so a
/// reconnect (possibly with a new public URL) refreshes the dashboard instead
/// of leaving the first connection's value pinned forever.
pub fn set_info(info: AgentInfo) {
    match INFO.get() {
        Some(m) => *m.lock().unwrap() = info,
        None => {
            let _ = INFO.set(Mutex::new(info));
        }
    }
}
pub fn info() -> AgentInfo {
    INFO.get().expect("inspector not initialized").lock().unwrap().clone()
}

/// The built agent-ui (agent-ui/dist) embedded at compile time.
#[derive(RustEmbed)]
#[folder = "../../agent-ui/dist/"]
struct AgentAssets;

#[derive(Clone)]
struct UiState;

pub async fn serve_dashboard(addr: SocketAddr) -> anyhow::Result<()> {
    let app = axum::Router::new()
        .route("/api/tunnels", get(api_tunnels))
        .route("/api/requests", get(api_requests))
        .route("/api/requests/{id}", get(api_request_detail))
        .route("/api/requests/{id}/replay", post(api_replay))
        .route("/", get(index))
        .route("/{*path}", get(assets))
        .with_state(UiState);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "inspector dashboard listening (http://.../ )");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn api_tunnels() -> impl IntoResponse {
    #[derive(Serialize)]
    struct TunnelInfo {
        mode: String,
        local: String,
        remote: String,
    }
    let i = info();
    axum::Json(vec![TunnelInfo {
        mode: i.mode.clone(),
        local: i.local.clone(),
        remote: i.remote.clone(),
    }])
}

async fn api_requests() -> impl IntoResponse {
    let recs = inspector().records.lock().unwrap();
    let mut out: Vec<RequestRecord> = recs.iter().rev().cloned().collect();
    out.truncate(100);
    axum::Json(out)
}

async fn api_request_detail(UrlPath(id): UrlPath<u64>) -> impl IntoResponse {
    let recs = inspector().records.lock().unwrap();
    match recs.iter().find(|r| r.id == id) {
        Some(r) => axum::Json(r.clone()).into_response(),
        None => (StatusCode::NOT_FOUND, "request not found").into_response(),
    }
}

/// Replay: re-send the recorded request bytes straight to the local service
/// and return the fresh response. (Local debugging aid; does not re-enter the
/// tunnel.)
async fn api_replay(UrlPath(id): UrlPath<u64>) -> impl IntoResponse {
    let payload = {
        let recs = inspector().records.lock().unwrap();
        recs.iter().find(|r| r.id == id).map(|r| r.payload.clone())
    };
    let Some(payload) = payload else {
        return (StatusCode::NOT_FOUND, "request not found").into_response();
    };
    let local: SocketAddr = match info().local.parse() {
        Ok(a) => a,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "bad local address").into_response(),
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        send_local(local, payload.as_bytes()),
    )
    .await
    {
        Ok(Ok(body)) => ([(axum::http::header::CONTENT_TYPE, "text/plain")], body).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_GATEWAY, format!("local dial failed: {e}")).into_response(),
        Err(_) => (StatusCode::GATEWAY_TIMEOUT, "local service timed out").into_response(),
    }
}

async fn send_local(local: SocketAddr, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(local).await?;
    s.write_all(bytes).await?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).await?;
    Ok(out)
}

async fn index() -> impl IntoResponse {
    match AgentAssets::get("index.html") {
        Some(f) => Html(String::from_utf8_lossy(&f.data).into_owned()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "agent-ui not built — run `pnpm build` in agent-ui/",
        )
            .into_response(),
    }
}

async fn assets(State(_): State<UiState>, path: UrlPath<String>) -> impl IntoResponse {
    match AgentAssets::get(&path) {
        Some(f) => {
            let mime = mime_for(&path);
            ([(axum::http::header::CONTENT_TYPE, mime)], f.data).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "js" | "mjs" => "application/javascript",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}