//! Edge ingress: accept public TCP connections, extract the routing key
//! (HTTP Host header for http tunnels; port for raw-TCP tunnels), and pump
//! bytes over the control plane.
//!
//! Both entry points share [`open_stream`], which allocates a stream, tells
//! the owning agent to dial its local service (`Init`), and drives the
//! bidirectional data loop.

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, warn};



use crate::config::config;
use crate::metrics;
use crate::registry::AgentConn;
use crate::state::registry;

const MAX_PEEK_BYTES: usize = 4096;

/// Bind the HTTP ingress and accept connections forever.
pub async fn run_ingress() -> anyhow::Result<()> {
    let addr = config().ingress_addr;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "ingress listening");
    loop {
        let (socket, peer) = listener.accept().await?;
        let peer_ip = peer.ip().to_string();
        tokio::spawn(async move {
            if let Err(e) = handle_socket(socket).await {
                debug!(peer = %peer_ip, "ingress connection error: {e}");
            }
        });
    }
}

/// HTTP-routed connection: peek the Host header to find the subdomain holder,
/// then hand off to the shared stream pump.
async fn handle_socket(mut socket: TcpStream) -> anyhow::Result<()> {
    // Peek the Host header without consuming bytes (we forward the full
    // stream, Host header included, to the agent).
    let Some((host, _xff)) = peek_http_host(&mut socket).await else {
        return Ok(());
    };
    let Some(sub_domain) = host_prefix(&host) else {
        warn!(%host, "host not under public domain");
        write_error(&mut socket, "400", "Invalid Hostname").await?;
        return Ok(());
    };

    let client = match registry().find_by_host(&sub_domain) {
        Some(c) => c,
        None => {
            warn!(%host, "no agent for host");
            write_error(&mut socket, "404", "Tunnel Not Found").await?;
            return Ok(());
        }
    };
    metrics::inc_requests();
    debug!(%host, sub_domain, agent_id = %client.agent_id, "routing connection");
    open_stream(client, socket, format!("http:{sub_domain}")).await;
    Ok(())
}

/// Allocate a path for `socket` and pump bytes to the owning agent.
///
/// Order: direct iroh data plane (opportunistic) → agent-initiated data plane →
/// refuse the public connection. `traffic_key` names the tunnel in metrics and
/// also decides whether an error response makes sense (HTTP tunnels only).
pub async fn open_stream(client: AgentConn, mut socket: TcpStream, traffic_key: String) {
    let http = traffic_key.starts_with("http:");

    // 1) iroh direct path.
    if let Some(endpoint) = crate::state::server_endpoint() {
        if let Some(addr) = registry().iroh_addr(&client.agent_id) {
            match crate::gateway::dial(endpoint, addr).await {
                Ok((conn, mut send, mut recv)) => {
                    if let Err(e) =
                        crate::gateway::write_handshake(&mut send, &config().secret, "").await
                    {
                        debug!("iroh prelude failed, falling back to the data plane: {e:#}");
                        drop(conn);
                    } else {
                        match crate::gateway::read_ack(&mut recv).await {
                            Ok(crate::gateway::Ack::Ok) => {
                                debug!(agent_id = %client.agent_id, key = %traffic_key, "routing over iroh");
                                metrics::inc_conns();
                                let (up, down) = crate::gateway::pump(socket, send, recv).await;
                                metrics::add_traffic(&traffic_key, up, down);
                                metrics::dec_conns();
                                drop(conn);
                                return;
                            }
                            Ok(crate::gateway::Ack::Refused) => {
                                // Definitive: the agent could not serve it.
                                debug!(agent_id = %client.agent_id, "agent refused iroh stream");
                                drop(conn);
                                return;
                            }
                            Err(e) => {
                                debug!("iroh ack lost, falling back to the data plane: {e:#}");
                                drop(conn);
                            }
                        }
                    }
                }
                Err(e) => debug!("iroh dial failed, falling back to the data plane: {e:#}"),
            }
        }
    }

    // 2) Agent-initiated data plane.
    if let Some(handle) = registry().data_handle(&client.agent_id) {
        match handle.open_stream().await {
            Ok(stream) => {
                debug!(agent_id = %client.agent_id, key = %traffic_key, "routing over the data plane");
                metrics::inc_conns();
                let (up, down) = pump_data(socket, stream, http).await;
                metrics::add_traffic(&traffic_key, up, down);
                metrics::dec_conns();
                return;
            }
            Err(e) => warn!(agent_id = %client.agent_id, "cannot open a data stream: {e:#}"),
        }
    }

    // 3) No path to the agent.
    warn!(agent_id = %client.agent_id, key = %traffic_key, "no data path for this agent");
    if http {
        let _ = write_error(&mut socket, "503", "Tunnel data path unavailable").await;
    }
}

/// Pump a public TCP connection over a data-plane yamux stream.
///
/// The stream is a plain byte pipe. yamux turns a stream reset into a clean EOF,
/// so the agent's refusal is *not* observable as an error: the rule is "the agent
/// closed without sending a single byte back", which for an HTTP tunnel can only
/// mean it could not dial the local service. That maps to 502.
///
/// Whichever direction finishes first ends the pump: the agent side finishing
/// means the response is complete (we stop reading the request body), the public
/// side finishing means the client half-closed (we FIN the agent stream and let
/// it drain its response). Either way the agent stream is closed with an explicit
/// `shutdown` (FIN) — dropping it would send RST and can lose buffered data.
async fn pump_data(socket: TcpStream, stream: yamux::Stream, http: bool) -> (u64, u64) {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use tokio_util::compat::FuturesAsyncReadCompatExt;

    let (mut pub_r, mut pub_w) = socket.into_split();
    let (mut s_r, mut s_w) = tokio::io::split(stream.compat());
    let wrote_back = Arc::new(AtomicBool::new(false));
    let up_bytes = Arc::new(AtomicU64::new(0));

    let down_res: io::Result<u64>;
    {
        let counter = up_bytes.clone();
        let up = async {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match pub_r.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if s_w.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                        counter.fetch_add(n as u64, Ordering::Relaxed);
                    }
                    Err(_) => break,
                }
            }
        };
        let flag = wrote_back.clone();
        let down = async {
            let mut buf = vec![0u8; 16 * 1024];
            let mut total = 0u64;
            loop {
                match s_r.read(&mut buf).await {
                    Ok(0) => return Ok(total),
                    Ok(n) => {
                        flag.store(true, Ordering::Relaxed);
                        pub_w.write_all(&buf[..n]).await?;
                        total += n as u64;
                    }
                    Err(e) => return Err(e),
                }
            }
        };
        tokio::pin!(up, down);
        tokio::select! {
            _ = &mut up => {
                // 公网侧结束:agent 流已写完,等它把响应发完。
                down_res = down.await;
            }
            d = &mut down => {
                // agent 侧结束:响应发完了,或者本地拨号失败后直接关闭。
                down_res = d;
            }
        }
    }
    // 两个泵都已停,借用结束:显式半关闭而不是 RST。
    let _ = s_w.shutdown().await;

    let down_bytes = down_res.as_ref().copied().unwrap_or(0);
    // 没有任何回包 = 拒绝(合法 HTTP 响应至少有一个状态行)。
    if !wrote_back.load(Ordering::Relaxed) && http {
        let _ = pub_w
            .write_all(
                b"HTTP/1.1 502
Content-Length: 26

Tunnel refused: local connection failed",
            )
            .await;
    }
    (up_bytes.load(Ordering::Relaxed), down_bytes)
}

/// Peek the first HTTP request and return `(host, x-forwarded-for)`.
async fn peek_http_host(socket: &mut TcpStream) -> Option<(String, String)> {
    let mut buf = vec![0u8; MAX_PEEK_BYTES];
    let n = socket.peek(&mut buf).await.ok()?;
    if n == 0 {
        return None;
    }
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Request::new(&mut headers);
    req.parse(&buf[..n]).ok()?;

    let host = req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("host"))
        .and_then(|h| std::str::from_utf8(h.value).ok())
        .map(|s| s.to_string())?;

    let xff = req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("x-forwarded-for"))
        .and_then(|h| std::str::from_utf8(h.value).ok())
        .unwrap_or("")
        .to_string();

    Some((host, xff))
}

/// Extract `<sub>` from `<sub>.<public-host>[:port]`. With no public host
/// configured, the *entire* host is treated as the subdomain.
fn host_prefix(host: &str) -> Option<String> {
    // Strip an explicit :port suffix.
    let host = host.rsplit_once(':').map_or(host, |(h, _p)| h);

    let cfg = config();
    if cfg.public_host.is_empty() {
        return Some(host.to_string());
    }
    let base = cfg.public_host.as_str();
    let suffix = format!(".{base}");
    host.strip_suffix(&suffix).map(|pre| pre.to_string())
}

async fn write_error(socket: &mut TcpStream, code: &str, text: &str) -> io::Result<()> {
    let body = format!("Error: {text}");
    let resp = format!(
        "HTTP/1.1 {code}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(resp.as_bytes()).await
}