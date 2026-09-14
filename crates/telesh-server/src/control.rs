//! WebSocket control plane: agent handshake, frame dispatch, keepalive.

use std::net::SocketAddr;
use std::time::Duration;

use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc::{self, UnboundedSender};
use tracing::{debug, error, info, warn};

use telesh_protocol::{
    self as proto, decode, encode, ClientHello, Frame, ServerHello, StreamPacket, TcpRegisterResp,
    PROTOCOL_VERSION,
};

use crate::config::config;
use crate::registry::ConnectedClient;
use crate::state::{active_streams, registry};

const PING_INTERVAL: Duration = Duration::from_secs(30);
const PING_TIMEOUT: Duration = Duration::from_secs(90);

/// axum router mounted at `/tunnel`.
pub fn router() -> axum::Router {
    axum::Router::new()
        .route("/tunnel", axum::routing::get(tunnel_ws))
        .route("/healthz", axum::routing::get(|| async { "ok" }))
}

async fn tunnel_ws(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_connection(addr, socket))
}

/// One agent: handshake -> register -> pump frames until disconnect.
async fn handle_connection(addr: SocketAddr, mut socket: WebSocket) {
    let reg = registry();

    // ---- Handshake ----
    let Some(hello) = recv_hello(&mut socket).await else {
        return;
    };
    if !authenticate(&mut socket, &hello).await {
        return;
    }
    let (sub_domain, assigned) = match resolve_subdomain(&mut socket, &hello).await {
        Some(r) => r,
        None => return,
    };

    // A single channel carries every frame bound for this agent.
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    if !reg.insert(&hello.agent_id, &sub_domain, tx.clone()) {
        warn!(agent_id = %hello.agent_id, sub_domain, "subdomain already in use");
        let _ = send(&mut socket, &Frame::ServerHello(ServerHello::SubDomainInUse)).await;
        return;
    }
    debug!(agent_id = %hello.agent_id, sub_domain, "agent registered");

    let hostname = public_hostname(&sub_domain);
    if let Err(e) = send(
        &mut socket,
        &Frame::ServerHello(ServerHello::Success {
            sub_domain: sub_domain.clone(),
            hostname,
            agent_id: hello.agent_id.clone(),
        }),
    )
    .await
    {
        debug!("failed to send server hello: {e}");
        reg.remove(&hello.agent_id);
        return;
    }

    // ---- Pump loops ----
    let (mut sink, mut stream) = socket.split();

    // Sink task: drains the agent's outbound queue into the WebSocket.
    let agent_id = hello.agent_id.clone();
    let reg_clone = reg;
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let wire = match encode(&frame) {
                Ok(w) => w,
                Err(e) => {
                    error!(?e, "frame encode failed");
                    continue;
                }
            };
            if sink.send(Message::Binary(Bytes::from(wire))).await.is_err() {
                break;
            }
        }
        reg_clone.remove(&agent_id);
    });

    // Keepalive pings: keep NAT/proxy idle timeouts from killing the channel.
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(PING_INTERVAL).await;
                if tx.send(Frame::Ping).is_err() {
                    return;
                }
            }
        });
    }

    // Reader loop. (Application-level Ping/Pong frames are handled inside
    // `dispatch`/the tx queue; ws-level Pings are not sent by our client.)
    loop {
        let next = tokio::time::timeout(PING_TIMEOUT, stream.next()).await;
        match next {
            Err(_elapsed) => {
                warn!(agent_id = %hello.agent_id, "control connection idle, closing");
                break;
            }
            Ok(Some(Ok(Message::Binary(data)))) => {
                if let Err(e) = dispatch(&tx, &hello.agent_id, &data).await {
                    debug!("frame dispatch error: {e}");
                }
            }
            Ok(Some(Ok(Message::Close(_)))) => {
                debug!(agent_id = %hello.agent_id, "agent closed connection");
                break;
            }
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Text(_)))) => {}
            Ok(Some(Err(e))) => {
                debug!(agent_id = %hello.agent_id, "websocket error: {e}");
                break;
            }
            Ok(None) => break,
        }
    }

    let _ = tx.send(Frame::Ping); // nudge the sink task to notice the closed ws
    reg.remove(&hello.agent_id);
    crate::tcp::unregister_all(&hello.agent_id);
    info!(agent_id = %hello.agent_id, sub_domain, %addr, "agent disconnected");

    let _ = assigned;
}

/// Read the first message; must be a `Hello`.
async fn recv_hello(socket: &mut WebSocket) -> Option<ClientHello> {
    match tokio::time::timeout(Duration::from_secs(10), socket.next()).await {
        Ok(Some(Ok(Message::Binary(data)))) => match decode(&data) {
            Ok(Frame::Hello(h)) => Some(h),
            other => {
                warn!("expected Hello, got {:?}", other.map(|_| "non-hello frame"));
                None
            }
        },
        _ => None,
    }
}

/// Validate the protocol version and (if configured) the auth secret.
async fn authenticate(socket: &mut WebSocket, hello: &ClientHello) -> bool {
    if hello.protocol_version != PROTOCOL_VERSION {
        let _ = send(
            socket,
            &Frame::ServerHello(ServerHello::Error(format!(
                "unsupported protocol version {}, expected {}",
                hello.protocol_version, PROTOCOL_VERSION
            ))),
        )
        .await;
        return false;
    }

    let cfg = config();
    let ok = if cfg.secret.is_empty() {
        true // secret is optional when the server has none configured
    } else {
        hello.auth_key.as_deref().map(|k| {
            // constant-time comparison
            k.as_bytes().iter().zip(cfg.secret.as_bytes()).all(|(a, b)| a == b)
                && k.len() == cfg.secret.len()
        }) == Some(true)
    };
    if !ok {
        warn!(agent_id = %hello.agent_id, "auth failed");
        let _ = send(socket, &Frame::ServerHello(ServerHello::AuthFailed)).await;
    }
    ok
}

/// Resolve the subdomain: requested (validated) or randomly assigned.
/// Returns `(sub_domain, assigned_randomly)`.
async fn resolve_subdomain(
    socket: &mut WebSocket,
    hello: &ClientHello,
) -> Option<(String, bool)> {
    match hello.sub_domain.clone() {
        Some(s) if proto::validate_subdomain(&s) => Some((s, false)),
        Some(_) => {
            let _ = send(socket, &Frame::ServerHello(ServerHello::InvalidSubDomain)).await;
            None
        }
        None => Some((proto::random_subdomain(), true)),
    }
}

fn public_hostname(sub_domain: &str) -> String {
    let cfg = config();
    if cfg.public_host.is_empty() {
        format!("{}:{}", sub_domain, cfg.ingress_addr.port())
    } else {
        format!("{}.{}", sub_domain, cfg.public_host)
    }
}

/// Handle one data-plane frame from an agent.
async fn dispatch(
    tx: &UnboundedSender<Frame>,
    agent_id: &str,
    data: &[u8],
) -> anyhow::Result<()> {
    let frame = decode(data)?;
    match frame {
        Frame::Stream(packet) => match packet {
            StreamPacket::Data { stream_id, data } => {
                if let Some(s) = active_streams().get(&stream_id) {
                    s.send(StreamPacket::Data { stream_id, data })?;
                } else {
                    debug!(stream_id, "data for unknown/closed stream");
                }
            }
            StreamPacket::End { stream_id } => {
                if let Some(s) = active_streams().get(&stream_id) {
                    s.send(StreamPacket::End { stream_id })?;
                }
            }
            StreamPacket::Refused { stream_id } => {
                if let Some(s) = active_streams().get(&stream_id) {
                    s.send(StreamPacket::Refused { stream_id })?;
                }
            }
            StreamPacket::Init { .. } => debug!("agent sent unexpected Init, ignoring"),
        },
        // Raw-TCP tunnel registration: allocate a public port and reply.
        Frame::TcpRegister(req) => {
            let client = ConnectedClient {
                agent_id: agent_id.to_string(),
                sub_domain: String::new(),
                tx: tx.clone(),
                connected_at: std::time::Instant::now(),
            };
            let resp = match crate::tcp::register(client, req.requested_port).await {
                Ok(port) => TcpRegisterResp::Success { port },
                Err(crate::tcp::TcpError::InUse) => TcpRegisterResp::PortInUse,
                Err(crate::tcp::TcpError::OutOfRange) => TcpRegisterResp::InvalidPort,
                Err(e) => TcpRegisterResp::Error(e.to_string()),
            };
            let _ = tx.send(Frame::TcpRegisterResp(resp));
        }
        Frame::TcpRegisterResp(_) => debug!("unexpected TcpRegisterResp from agent"),
        Frame::Pong => { /* keepalive ack */ }
        Frame::Hello(_) | Frame::ServerHello(_) | Frame::Ping => {
            debug!("unexpected frame from agent, ignoring");
        }
    }
    Ok(())
}

async fn send(socket: &mut WebSocket, frame: &Frame) -> anyhow::Result<()> {
    let wire = encode(frame)?;
    socket.send(Message::Binary(Bytes::from(wire))).await?;
    Ok(())
}