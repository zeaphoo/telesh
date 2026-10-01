//! Agent-local request inspector + debugging dashboard.
//!
//! ngrok-style: the agent sees every request passing to/from the local
//! service, so it records an HTTP-level log (method, path, status, timing,
//! sizes, payloads) served on a localhost-only port (`--dashboard-addr`,
//! default 127.0.0.1:16345). This never leaves the machine — useful for local
//! debugging while the public tunnel stays opaque to the server.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use dashmap::DashMap;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Beyond this many header bytes we stop parsing, just keep counting bytes.
const MAX_HEAD: usize = 8 * 1024;
/// Payload preview cap stored per request.
const PAYLOAD_CAP: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestRecord {
    pub id: u64,
    /// 通道 key:`publish:<端口>` / `p2p:<端口>`。
    pub channel: String,
    /// 这条流当时拨向的本地地址(replay 用)。
    pub local: Option<SocketAddr>,
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

/// Per-stream routing info kept while the stream is alive: where to dial for a
/// replay, and whether this stream is being captured.
#[derive(Debug, Clone)]
struct StreamMeta {
    local: SocketAddr,
    /// Decided once at [`start_stream`] from the local port (HTTP? turned off?).
    capture: bool,
    /// `publish:<port>` / `p2p:<port>`;空串表示不抓包。
    channel: String,
}

pub struct Inspector {
    streams: DashMap<u64, StreamCapture>,
    /// 每条流的本地地址(replay 用)、通道与是否抓包。
    meta: DashMap<u64, StreamMeta>,
    /// 每个通道最近 `trace_log::PER_CHANNEL` 条记录(newest 在后)。
    logs: Mutex<HashMap<String, VecDeque<RequestRecord>>>,
    next_id: AtomicU64,
    /// 交给后台任务落盘(见 `trace_log::run`)。
    tx: mpsc::UnboundedSender<(String, RequestRecord)>,
}

static INSPECTOR: OnceLock<Inspector> = OnceLock::new();

/// Initialize the inspector: load persisted logs and start the writer.
///
/// `log_dir` is `<dataroot>/traces`; created on first write.
pub fn init(log_dir: std::path::PathBuf) {
    let rings = crate::trace_log::load(&log_dir);
    // Keep ids monotonic across restarts so newest-first ordering still works.
    let max_id = rings
        .values()
        .flat_map(|r| r.iter())
        .map(|r| r.id)
        .max()
        .unwrap_or(0);
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(crate::trace_log::run(log_dir, rx));
    let _ = INSPECTOR.set(Inspector {
        streams: DashMap::new(),
        meta: DashMap::new(),
        logs: Mutex::new(rings),
        next_id: AtomicU64::new(max_id + 1),
        tx,
    });
}

fn inspector() -> &'static Inspector {
    INSPECTOR.get().expect("inspector not initialized")
}

/// 只有 HTTP 才够格被追踪。publish 的流已由调用方限定为 `http:` 隧道;P2P 流
/// 还要看本地端口的探测结果是不是 HTTP。
fn is_http_port(port: u16) -> bool {
    crate::publish::shared()
        .ports()
        .iter()
        .any(|p| p.port == port && p.http_hint)
}

/// 这条流要不要抓:按 (section, 端口) 查 settings,默认开。
fn should_capture(section: crate::publish::Section, port: u16) -> bool {
    let shared = crate::publish::shared();
    match section {
        crate::publish::Section::Publish => shared.trace_enabled(section, port),
        crate::publish::Section::P2p => is_http_port(port) && shared.trace_enabled(section, port),
    }
}

/// Is the stream's own capture flag set? Decided in [`start_stream`].
fn capturing(i: &Inspector, stream_id: u64) -> bool {
    i.meta.get(&stream_id).map(|m| m.capture).unwrap_or(false)
}

/// Allocate a process-wide stream id for the inspector.
///
/// yamux and iroh do not expose a reusable `u64`, and both transports feed the
/// same per-stream maps, so ids must come from one shared counter.
pub fn next_stream_id() -> u64 {
    static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed)
}

/// Bytes flowing server -> local (the request direction).
/// 记下这条流的本地地址(replay 需要),并按 `(section, 端口)` 决定是否抓包。
/// `section` 为 `None` 表示不够格(比如 TCP 隧道),不抓。
pub fn start_stream(
    stream_id: u64,
    local: SocketAddr,
    section: Option<crate::publish::Section>,
) {
    let (channel, capture) = match section {
        Some(s) => (s.channel(local.port()), should_capture(s, local.port())),
        None => (String::new(), false),
    };
    inspector().meta.insert(
        stream_id,
        StreamMeta {
            local,
            capture,
            channel,
        },
    );
}

pub fn observe_request(stream_id: u64, data: &[u8]) {
    let i = inspector();
    if !capturing(i, stream_id) {
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
            // Keep-alive: the previous request's bytes are everything before this
            // new request line. Hand those off, then keep the tail (the new
            // request) so its head/body aren't discarded with the previous one.
            let tail = m.req_head.split_off(line_start);
            m.req_seg = m.req_head.len() as u64;
            push_record(i, stream_id, m);
            m.req_head = tail;
            m.scan = line_end + 1 - line_start;
        }
        m.method = Some(method);
        m.path = Some(path);
        m.seg_started = Instant::now();
        // Bytes already buffered for this request (the first request starts at
        // the stream head, later ones at their own request line).
        m.req_seg = m.req_head.len() as u64;
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
    if !capturing(i, stream_id) {
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
    // `streams` 只为抓包的流建立,所以这里不必再判 capture。
    // push_record 要读 per-stream 的 local,所以 meta 最后再删。
    if let Some((_, mut c)) = i.streams.remove(&stream_id) {
        if c.method.is_some() {
            push_record(i, stream_id, &mut c);
        }
    }
    i.meta.remove(&stream_id);
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
fn push_record(i: &Inspector, stream_id: u64, c: &mut StreamCapture) {
    let Some(method) = c.method.take() else {
        return;
    };
    let path = c.path.take().unwrap_or_default();
    let status = c.status.take();
    let id = i.next_id.fetch_add(1, Ordering::Relaxed);
    let (local, channel) = match i.meta.get(&stream_id) {
        Some(meta) => (Some(meta.local), meta.channel.clone()),
        None => (None, String::new()),
    };
    let record = RequestRecord {
        id,
        channel: channel.clone(),
        local,
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
    // In-memory ring (newest per channel) + hand it to the file writer.
    {
        let mut logs = i.logs.lock().unwrap();
        let ring = logs.entry(channel.clone()).or_default();
        ring.push_back(record.clone());
        while ring.len() > crate::trace_log::PER_CHANNEL {
            ring.pop_front();
        }
    }
    let _ = i.tx.send((channel, record));

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


/// The built agent-ui (agent-ui/dist) embedded at compile time.
#[derive(RustEmbed)]
#[folder = "../../agent-ui/dist/"]
struct AgentAssets;

#[derive(Clone)]
struct UiState;

pub async fn serve_dashboard(addr: SocketAddr) -> anyhow::Result<()> {
    let app = axum::Router::new()
        .route("/api/tunnels", get(api_tunnels))
        .route("/api/ports", get(api_ports))
        .route("/api/publish", get(api_publish).put(api_set_publish))
        .route("/api/account", get(api_account))
        .route("/api/p2p-policy", get(api_p2p_policy).put(api_set_p2p_policy))
        .route("/api/p2p/services", get(api_p2p_services))
        .route("/api/settings", get(api_settings).put(api_set_settings))
        .route("/api/channels", get(api_channels))
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

/// 本机正在监听的端口(启发式 `http_hint`,不是探测结果)。
async fn api_ports() -> impl IntoResponse {
    axum::Json(crate::publish::shared().ports())
}

/// 当前发布配置 + 每条运行状态。
async fn api_publish() -> impl IntoResponse {
    axum::Json(crate::publish::shared().status())
}

/// 本机 agent 的登录状态与账户信息。
async fn api_account() -> impl IntoResponse {
    axum::Json(crate::account::snapshot())
}

/// 当前 P2P 入站规则。
async fn api_p2p_policy() -> impl IntoResponse {
    axum::Json(crate::publish::shared().p2p_policy())
}

/// 端口设置(publish / p2p 分开)。
async fn api_settings() -> impl IntoResponse {
    axum::Json(crate::publish::shared().settings())
}

/// 覆盖式提交端口设置(立即生效并落盘)。
async fn api_set_settings(
    axum::Json(settings): axum::Json<crate::publish::Settings>,
) -> impl IntoResponse {
    crate::publish::shared().apply_settings(settings.clone());
    axum::Json(settings)
}

/// 某个账户当前提供的服务名(「连接远程端口」的选择列表)。
async fn api_p2p_services(
    axum::extract::Query(q): axum::extract::Query<ServicesQuery>,
) -> impl IntoResponse {
    let account = q.account.trim();
    let services = if account.is_empty() {
        Vec::new()
    } else {
        crate::directory::list(account).await
    };
    axum::Json(serde_json::json!({ "account": account, "services": services }))
}

#[derive(serde::Deserialize)]
struct ServicesQuery {
    #[serde(default)]
    account: String,
}

/// 覆盖式提交 P2P 入站规则(立即生效)。
async fn api_set_p2p_policy(
    axum::Json(policy): axum::Json<crate::publish::P2pPolicy>,
) -> impl IntoResponse {
    if let Some(path) = crate::publish::state_path() {
        if let Err(e) = crate::publish::save_policy(path, &policy) {
            tracing::warn!("cannot persist the P2P policy: {e:#}");
        }
    }
    crate::publish::shared().set_p2p_policy(policy.clone());
    axum::Json(policy)
}

/// 覆盖式提交期望状态(UI 的"保存并应用")。
async fn api_set_publish(
    axum::Json(entries): axum::Json<Vec<crate::publish::EntryStatus>>,
) -> impl IntoResponse {
    let wanted: Vec<crate::publish::Entry> = entries.into_iter().map(|e| e.entry).collect();
    let shared = crate::publish::shared();
    if let Some(path) = crate::publish::state_path() {
        if let Err(e) = crate::publish::save(path, &wanted) {
            tracing::warn!("cannot persist the publish configuration: {e:#}");
        }
    }
    shared.set_desired(wanted);
    axum::Json(shared.status())
}

async fn api_tunnels() -> impl IntoResponse {
    #[derive(Serialize)]
    struct TunnelInfo {
        mode: String,
        local: String,
        remote: String,
    }
    let rows: Vec<TunnelInfo> = crate::publish::shared()
        .status()
        .into_iter()
        .map(|s| TunnelInfo {
            mode: s.entry.kind().to_string(),
            local: format!("127.0.0.1:{}", s.entry.local_port()),
            remote: s.detail.clone().unwrap_or_default(),
        })
        .collect();
    axum::Json(rows)
}

#[derive(Deserialize)]
struct RequestsQuery {
    /// `publish:<port>` / `p2p:<port>`;不传 = 所有通道合并。
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// 通道列表(名字 + 当前条数),给 UI 做筛选。
#[derive(Serialize)]
struct ChannelInfo {
    channel: String,
    count: usize,
}

async fn api_channels() -> impl IntoResponse {
    let logs = inspector().logs.lock().unwrap();
    let mut out: Vec<ChannelInfo> = logs
        .iter()
        .map(|(channel, ring)| ChannelInfo {
            channel: channel.clone(),
            count: ring.len(),
        })
        .collect();
    out.sort_by(|a, b| a.channel.cmp(&b.channel));
    axum::Json(out)
}

async fn api_requests(
    axum::extract::Query(q): axum::extract::Query<RequestsQuery>,
) -> impl IntoResponse {
    let logs = inspector().logs.lock().unwrap();
    let mut out: Vec<RequestRecord> = match q.channel.as_deref() {
        Some(channel) => logs
            .get(channel)
            .map(|ring| ring.iter().rev().cloned().collect())
            .unwrap_or_default(),
        None => {
            let mut all: Vec<RequestRecord> =
                logs.values().flat_map(|ring| ring.iter().cloned()).collect();
            all.sort_by_key(|r| std::cmp::Reverse(r.id));
            all
        }
    };
    out.truncate(q.limit.unwrap_or(100));
    axum::Json(out)
}

async fn api_request_detail(UrlPath(id): UrlPath<u64>) -> impl IntoResponse {
    let logs = inspector().logs.lock().unwrap();
    for ring in logs.values() {
        if let Some(r) = ring.iter().find(|r| r.id == id) {
            return axum::Json(r.clone()).into_response();
        }
    }
    (StatusCode::NOT_FOUND, "request not found").into_response()
}

/// Replay: re-send the recorded request bytes straight to the local service
/// and return the fresh response. (Local debugging aid; does not re-enter the
/// tunnel.)
async fn api_replay(UrlPath(id): UrlPath<u64>) -> impl IntoResponse {
    let found = {
        let logs = inspector().logs.lock().unwrap();
        logs.values()
            .flat_map(|ring| ring.iter())
            .find(|r| r.id == id)
            .map(|r| (r.local, r.payload.clone()))
    };
    let Some((local, payload)) = found else {
        return (StatusCode::NOT_FOUND, "request not found").into_response();
    };
    let Some(local) = local else {
        return (StatusCode::GONE, "that stream is no longer active").into_response();
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
#[cfg(test)]
mod tests {
    use super::*;

    /// HTTP/1.1 keep-alive:检测到第二条请求行时,前一条要正确移交,且不能把
    /// 后一条的请求头一起清掉(否则新记录 payload 为空、req_bytes=0)。
    #[tokio::test]
    async fn keepalive_hands_off_without_losing_the_next_request() {
        let dir = std::env::temp_dir().join(format!("telesh-inspect-{}", rand::random::<u64>()));
        init(dir.clone());

        let addr: SocketAddr = "127.0.0.1:3000".parse().unwrap();
        start_stream(7, addr, Some(crate::publish::Section::Publish));

        let first = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";
        let second = b"GET /b HTTP/1.1\r\nHost: x\r\n\r\n";
        observe_request(7, first);
        observe_request(7, second); // scanning detects the second request line
        observe_response(7, b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        finish_stream(7);

        let logs = inspector().logs.lock().unwrap();
        let ring = logs.get("publish:3000").expect("channel");
        assert_eq!(ring.len(), 2);
        let (a, b) = (&ring[0], &ring[1]);

        assert_eq!((a.method.as_str(), a.path.as_str()), ("GET", "/a"));
        assert!(a.payload.starts_with("GET /a"));
        assert!(!a.payload.contains("GET /b"), "previous must not swallow the next");
        assert_eq!(a.req_bytes, first.len() as u64);

        assert_eq!((b.method.as_str(), b.path.as_str()), ("GET", "/b"));
        assert!(b.payload.starts_with("GET /b"), "new request head must be preserved");
        assert_eq!(b.req_bytes, second.len() as u64);

        std::fs::remove_dir_all(&dir).ok();
    }
}
