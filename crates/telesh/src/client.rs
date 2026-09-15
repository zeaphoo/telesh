//! Agent control client: connects to the server, registers the subdomain,
//! and multiplexes streams over one WebSocket.

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use telesh_protocol::{
    decode, encode, validate_subdomain, ClientHello, Frame, ServerHello, StreamPacket, TcpRegister,
    TcpRegisterResp, PROTOCOL_VERSION,
};

use crate::local;

// The agent's WebSocket connection (split): a sink task owns the write half
// and drains this queue, so every component (main loop + local sessions) can
// send frames.
static OUTBOUND: OnceLock<mpsc::UnboundedSender<Frame>> = OnceLock::new();
static SUBDOMAIN: OnceLock<std::sync::Mutex<Option<String>>> = OnceLock::new();

/// Cell holding the subdomain the server assigned us, so reconnects within
/// this process re-request it and keep the public URL stable. Lazily created
/// because [`OnceLock::set`] would otherwise never run before the first read.
fn subdomain_cell() -> &'static std::sync::Mutex<Option<String>> {
    SUBDOMAIN.get_or_init(|| std::sync::Mutex::new(None))
}

/// Per-run raw-TCP tunnel parameters.
#[derive(Debug, Clone)]
pub struct TcpOptions {
    /// Preferred remote public port; `None` asks the server to pick one.
    pub requested_port: Option<u16>,
}

/// Connection parameters for one run.
#[derive(Debug, Clone)]
pub struct AgentOptions {
    pub control_addr: String, // host:port of the ws control server
    pub local: SocketAddr,
    pub subdomain: Option<String>,
    pub secret: Option<String>,
    pub agent_id: String, // persisted identity, stable across reconnects
    /// When set, the assigned subdomain is persisted immediately on
    /// handshake success (so URLs survive restarts / SIGKILL).
    pub persist_file: Option<std::path::PathBuf>,
    /// Some(_) requests a raw-TCP tunnel instead of a host-routed one.
    pub tcp: Option<TcpOptions>,
}

/// Connect, handshake, then serve frames until the connection drops.
/// Returns the subdomain assigned by the server (if any).
pub async fn run_once(opts: &AgentOptions) -> Result<Option<String>> {
    let url = format!("ws://{}/tunnel", opts.control_addr);
    info!(%url, agent_id = %opts.agent_id, "connecting to control server");
    let (ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .with_context(|| format!("failed to connect to control server at {url}"))?;

    let (mut sink, mut stream) = ws.split();

    // Framing: every outbound Frame -> one binary WS message.
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    let _ = OUTBOUND.set(tx.clone());
    let sink_task = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let wire = match encode(&frame) {
                Ok(w) => w,
                Err(e) => {
                    error!(?e, "frame encode failed");
                    continue;
                }
            };
            if sink.send(WsMessage::Binary(wire)).await.is_err() {
                break;
            }
        }
        debug!("outbound sink task ended");
    });

    // ---- Handshake (sent through the shared outbound queue) ----
    let subdomain = resolve_subdomain(opts);
    let hello = ClientHello {
        protocol_version: PROTOCOL_VERSION,
        agent_id: opts.agent_id.clone(),
        sub_domain: subdomain.clone(),
        auth_key: opts.secret.clone(),
    };
    tx.send(Frame::Hello(hello)).expect("sink task alive");

    let server_hello = match wait_server_hello(&mut stream).await? {
        Some(h) => h,
        None => anyhow::bail!("control connection closed during handshake"),
    };
    let assigned_subdomain = match &server_hello {
        ServerHello::Success { sub_domain, hostname, .. } => {
            info!(hostname, "tunnel established — public URL: http://{hostname}");
            // http tunnel: tell the local inspector what to log + where.
            crate::inspect::set_info(crate::inspect::AgentInfo {
                mode: "http".into(),
                local: opts.local.to_string(),
                remote: format!("http://{hostname}"),
            });
            if opts.tcp.is_none() {
                crate::inspect::enable(&format!("http:{sub_domain}"));
            }
            // Persist the assignment immediately so a later kill/restart
            // re-registers the same subdomain.
            if let Some(path) = &opts.persist_file {
                if let Err(e) = crate::state::save(
                    path,
                    &crate::state::AgentState {
                        agent_id: opts.agent_id.clone(),
                        subdomain: Some(sub_domain.clone()),
                    },
                ) {
                    warn!(path = %path.display(), "cannot persist state: {e:#}");
                }
            }
            Some(sub_domain.clone())
        }
        ServerHello::SubDomainInUse => anyhow::bail!("subdomain already in use on server"),
        ServerHello::InvalidSubDomain => anyhow::bail!("invalid subdomain requested"),
        ServerHello::AuthFailed => anyhow::bail!("authentication failed (wrong secret?)"),
        ServerHello::Error(e) => anyhow::bail!("server error: {e}"),
    };
    if let Some(sub) = &assigned_subdomain {
        *subdomain_cell().lock().unwrap() = Some(sub.clone());
    }

    // ---- Raw-TCP tunnel registration (optional) ----
    if let Some(tcp) = &opts.tcp {
        tx.send(Frame::TcpRegister(TcpRegister {
            requested_port: tcp.requested_port,
        }))
        .expect("sink task alive");
        // Wait for the server's allocation before serving streams so the
        // CLI can print the public endpoint.
        let port = match wait_tcp_reg_resp(&mut stream).await? {
            Some(TcpRegisterResp::Success { port }) => port,
            Some(TcpRegisterResp::PortInUse) => anyhow::bail!("requested TCP port already in use"),
            Some(TcpRegisterResp::InvalidPort) => {
                anyhow::bail!("requested TCP port outside server range")
            }
            Some(TcpRegisterResp::Error(e)) => anyhow::bail!("TCP registration failed: {e}"),
            None => anyhow::bail!("control connection closed during TCP registration"),
        };

        // Raw TCP: nothing structured to log request-wise, but tell the
        // inspector where the tunnel points so the UI can show it.
        let server_host =
            opts.control_addr.rsplit_once(':').map_or(opts.control_addr.as_str(), |(h, _)| h);
        crate::inspect::set_info(crate::inspect::AgentInfo {
            mode: "tcp".into(),
            local: opts.local.to_string(),
            remote: format!("tcp://{server_host}:{port}"),
        });
        info!(port, server_host, "tcp tunnel established — public: tcp://{server_host}:{port}");
        info!("use e.g.: ssh -p {port} user@{server_host}");
    }

    // ---- Serve frames ----
    loop {
        match stream.next().await {
            Some(Ok(WsMessage::Binary(data))) => {
                let frame = match decode(&data) {
                    Ok(f) => f,
                    Err(e) => {
                        debug!("bad frame from server: {e}");
                        continue;
                    }
                };
                if !handle_frame(&tx, &opts.local, frame).await {
                    break;
                }
            }
            Some(Ok(WsMessage::Close(_))) => {
                debug!("server closed control connection");
                break;
            }
            Some(Ok(WsMessage::Ping(_))) => {
                let _ = tx.send(Frame::Ping); // application-level ack is Pong
            }
            Some(Ok(WsMessage::Pong(_))) => {}
            Some(Ok(_)) => {}
            Some(Err(e)) => {
                info!("control connection error: {e}");
                break;
            }
            None => break,
        }
    }

    sink_task.abort();
    Ok(assigned_subdomain)
}

/// Choose the subdomain for this connection: explicit > persisted > None.
fn resolve_subdomain(opts: &AgentOptions) -> Option<String> {
    if let Some(s) = &opts.subdomain {
        if !validate_subdomain(s) {
            warn!("subdomain {s:?} is invalid, ignoring");
            return None;
        }
        return Some(s.clone());
    }
    // Reconnect after an assigned (random) subdomain: reuse it to keep the
    // public URL stable. The server replaces our stale connection anyway.
    subdomain_cell().lock().unwrap().clone()
}

async fn wait_server_hello<S>(stream: &mut S) -> Result<Option<ServerHello>>
where
    S: StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
        Ok(Some(Ok(WsMessage::Binary(data)))) => match decode(&data)? {
            Frame::ServerHello(h) => Ok(Some(h)),
            other => {
                anyhow::bail!("expected ServerHello, got {other:?}")
            }
        },
        Ok(Some(Ok(_))) => anyhow::bail!("expected ServerHello, got non-binary message"),
        _ => Ok(None),
    }
}

/// Wait for the server's reply to a `TcpRegister` request.
async fn wait_tcp_reg_resp<S>(stream: &mut S) -> Result<Option<TcpRegisterResp>>
where
    S: StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
        Ok(Some(Ok(WsMessage::Binary(data)))) => match decode(&data)? {
            Frame::TcpRegisterResp(r) => Ok(Some(r)),
            other => anyhow::bail!("expected TcpRegisterResp, got {other:?}"),
        },
        Ok(Some(Ok(_))) => anyhow::bail!("expected TcpRegisterResp, got non-binary message"),
        _ => Ok(None),
    }
}

/// Handle one frame from the server. Returns false to end the connection.
async fn handle_frame(
    tx: &mpsc::UnboundedSender<Frame>,
    local: &SocketAddr,
    frame: Frame,
) -> bool {
    match frame {
        Frame::Stream(StreamPacket::Init { stream_id }) => {
            // Register the outbound placeholder *synchronously*, then hand the
            // receiver to the spawned pump task. This guarantees the first
            // Data frame (which may follow Init back-to-back) never arrives
            // before its channel exists.
            let (ltx, lrx) = mpsc::unbounded_channel::<Vec<u8>>();
            local::streams().insert(stream_id, ltx);
            let tx = tx.clone();
            let local = *local;
            tokio::spawn(async move {
                local::handle_stream(stream_id, tx, local, lrx).await;
            });
            true
        }
        Frame::Stream(StreamPacket::Data { stream_id, data }) => {
            if let Some(t) = local::streams().get(&stream_id) {
                let _ = t.send(data);
            } else {
                debug!(stream_id, "data for unknown local stream, ignoring");
            }
            true
        }
        Frame::Stream(StreamPacket::End { stream_id }) => {
            local::streams().remove(&stream_id);
            true
        }
        Frame::Stream(StreamPacket::Refused { .. }) => true, // server informing us
        Frame::Ping => {
            let _ = tx.send(Frame::Pong);
            true
        }
        Frame::Pong => true,
        Frame::TcpRegister(_) | Frame::TcpRegisterResp(_) => {
            warn!("unexpected TCP register frame after handshake");
            true
        }
        Frame::Hello(_) | Frame::ServerHello(_) => {
            warn!("unexpected frame after handshake");
            true
        }
    }
}