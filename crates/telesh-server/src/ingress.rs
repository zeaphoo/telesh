//! Edge ingress: accept public TCP connections, extract the routing key
//! (HTTP Host header for http tunnels; port for raw-TCP tunnels), and pump
//! bytes over the control plane.
//!
//! Both entry points share [`open_stream`], which allocates a stream, tells
//! the owning agent to dial its local service (`Init`), and drives the
//! bidirectional data loop.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use telesh_protocol::{Frame, StreamPacket};

use crate::config::config;
use crate::metrics;
use crate::registry::AgentConn;
use crate::state::{active_streams, next_stream_id, registry};

const MAX_PEEK_BYTES: usize = 4096;
/// How long to wait for the agent to accept a stream after Init.
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);

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
/// Prefers the direct iroh data plane when the agent announced one, falling
/// back to the WebSocket relay on any failure. Used by both HTTP-routed and
/// raw-TCP tunnels; `traffic_key` names the tunnel in metrics.
pub async fn open_stream(client: AgentConn, socket: TcpStream, traffic_key: String) {
    if let Some(endpoint) = crate::state::server_endpoint() {
        if let Some(addr) = registry().iroh_addr(&client.agent_id) {
            match crate::gateway::dial(endpoint, addr).await {
                Ok((conn, mut send, mut recv)) => {
                    // The agent authenticates the edge, attaches the local
                    // service, then acks; only then does payload flow.
                    if let Err(e) =
                        crate::gateway::write_handshake(&mut send, &config().secret, "").await
                    {
                        debug!("iroh prelude failed, falling back to relay: {e:#}");
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
                                debug!("iroh ack lost, falling back to relay: {e:#}");
                                drop(conn);
                            }
                        }
                    }
                }
                Err(e) => debug!("iroh dial failed, falling back to relay: {e:#}"),
            }
        }
    }
    open_stream_relay(client, socket, traffic_key).await;
}

/// Relay path: signal the agent over the control connection and multiplex the
/// stream over the same WebSocket.
async fn open_stream_relay(client: AgentConn, mut socket: TcpStream, traffic_key: String) {
    metrics::inc_conns();
    let stream_id = next_stream_id();
    let (tx, mut rx) = mpsc::unbounded_channel::<StreamPacket>();
    active_streams().insert(stream_id, tx.clone());
    if client
        .tx
        .send(Frame::stream(StreamPacket::Init { stream_id }))
        .is_err()
    {
        active_streams().remove(&stream_id);
        warn!(agent_id = %client.agent_id, "agent control channel closed");
        return;
    }

    let (mut read_half, mut write_half) = socket.split();
    let agent_tx = client.tx.clone();
    let accept_deadline = tokio::time::Instant::now() + ACCEPT_TIMEOUT;

    // Bidirectional pump, driven by a single select loop:
    //  - public socket -> agent, as Data frames
    //  - agent (Data/End/Refused) -> public socket
    let mut buf = vec![0u8; 8192];
    let mut accepted = false;
    loop {
        if !accepted && tokio::time::Instant::now() >= accept_deadline {
            debug!(stream_id, "agent did not accept stream in time, closing");
            break;
        }

        tokio::select! {
            biased;
            // Public socket closed / error / EOF: stop.
            read_res = read_half.read(&mut buf) => {
                match read_res {
                    Ok(0) => break,
                    Ok(n) => {
                        metrics::add_traffic(&traffic_key, n as u64, 0);
                        if agent_tx
                            .send(Frame::stream(StreamPacket::Data {
                                stream_id,
                                data: buf[..n].to_vec(),
                            }))
                            .is_err()
                        {
                            debug!(stream_id, "agent control channel closed mid-stream");
                            break;
                        }
                    }
                    Err(e) => {
                        debug!(stream_id, "ingress read error: {e}");
                        break;
                    }
                }
            }
            // Agent traffic for this stream.
            packet = rx.recv() => {
                match packet {
                    Some(StreamPacket::Data { data, .. }) => {
                        accepted = true;
                        metrics::add_traffic(&traffic_key, 0, data.len() as u64);
                        if write_half.write_all(&data).await.is_err() {
                            debug!(stream_id, "ingress write error");
                            break;
                        }
                    }
                    Some(StreamPacket::Refused { .. }) => {
                        let _ = write_half
                            .write_all(b"HTTP/1.1 502\r\nContent-Length: 26\r\n\r\nTunnel refused: local connection failed")
                            .await;
                        break;
                    }
                    // Agent closed the stream.
                    Some(StreamPacket::End { .. }) | None => break,
                    Some(StreamPacket::Init { .. }) => {}
                }
            }
        }
    }

    // Tell the agent the stream is over (idempotent on its side).
    let _ = agent_tx.send(Frame::stream(StreamPacket::End { stream_id }));
    active_streams().remove(&stream_id);
    metrics::dec_conns();
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