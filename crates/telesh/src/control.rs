//! Agent control client: connects to the server, registers the subdomain,
//! and multiplexes streams over one WebSocket.

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use iroh::Watcher;
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use telesh_protocol::{
    decode, encode, validate_subdomain, Frame, ServerHello, TcpRegister, TcpRegisterResp,
};


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
    /// Persistent iroh identity (used by `p2p` mode; carried so the same state
    /// file is written back intact for http/tcp modes too).
    pub secret_key: [u8; 32],
    /// Cached login session, if any.
    pub session: Option<Vec<u8>>,
    /// Enrollment secret for first registration.
    pub enrollment: Option<String>,
    /// Friendly node name sent at login (defaults to the generated state name).
    pub node_name: Option<String>,
    /// Disable UPnP/PCP/NAT-PMP gateway probing for the iroh endpoint.
    pub no_portmap: bool,
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
    info!(%url, "connecting to control server");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .with_context(|| format!("failed to connect to control server at {url}"))?;

    // ---- Login (keypair challenge-response, or a cached session) ----
    let subdomain = resolve_subdomain(opts);
    let (login, server_hello) = crate::auth::login(
        &mut ws,
        &opts.secret_key,
        opts.session.clone(),
        opts.enrollment.as_deref(),
        subdomain,
        opts.node_name.clone(),
    )
    .await?;
    info!(account = %login.account, name = %login.display_name, "logged in");

    let assigned_subdomain = match &server_hello {
        ServerHello::Success {
            sub_domain,
            hostname,
            ..
        } => {
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
            // Persist identity + session + assigned subdomain immediately.
            if let Some(path) = &opts.persist_file {
                if let Err(e) = crate::state::save(
                    path,
                    &crate::state::AgentState {
                        secret_key: opts.secret_key,
                        subdomain: Some(sub_domain.clone()),
                        session: Some(login.session.clone()),
                        session_exp: login.expires_at,
                        name: opts.node_name.clone(),
                    },
                ) {
                    warn!(path = %path.display(), "cannot persist state: {e:#}");
                }
            }
            Some(sub_domain.clone())
        }
        other => anyhow::bail!("server rejected tunnel: {other:?}"),
    };
    if let Some(sub) = &assigned_subdomain {
        *subdomain_cell().lock().unwrap() = Some(sub.clone());
    }
    // The relay to use for the direct data plane is the server's choice,
    // announced with the handshake — agents carry no relay configuration.
    let relay = server_hello.relay_url().map(str::to_string);
    if let Some(url) = &relay {
        info!(relay = %url, "using server-announced iroh relay");
    }

    // ---- Data plane (agent-initiated) ----
    // The server announces its data address with the handshake. http/tcp payload
    // takes this path whenever a direct iroh connection is not available.
    let data_task = match server_hello.data_addr() {
        Some(announced) => match crate::data::resolve_addr(announced, &opts.control_addr).await {
            Some(addr) => {
                info!(%addr, "starting the data-plane client");
                Some(tokio::spawn(crate::data::run(crate::data::DataOptions {
                    addr,
                    session: login.session.clone(),
                    local: opts.local,
                })))
            }
            None => {
                warn!(announced, "cannot resolve the announced data address");
                None
            }
        },
        None => {
            info!("server announced no data address; payload will require iroh");
            None
        }
    };
    // ---- Frame pump ----
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    let _ = OUTBOUND.set(tx.clone());
    // Let data-plane streams ask the server to validate opaque tokens.
    crate::token_check::set_control(tx.clone());
    let sink_task = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let text = match encode(&frame) {
                Ok(t) => t,
                Err(e) => {
                    error!(?e, "frame encode failed");
                    continue;
                }
            };
            if sink.send(WsMessage::Text(text)).await.is_err() {
                break;
            }
        }
        debug!("outbound sink task ended");
    });

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

    // ---- Direct data plane (iroh) ----
    // Announce an iroh endpoint so the edge can route ingress traffic
    // peer-to-peer; the WebSocket relay stays the fallback. Setup is
    // non-blocking: the current address is announced now and re-announced as
    // the endpoint learns routes (e.g. its home relay).
    let serve = match crate::p2p::build_endpoint(
        iroh::SecretKey::from_bytes(&opts.secret_key),
        relay.as_deref(),
        opts.no_portmap,
    )
    .await
    {
        Ok(endpoint) => {
            let mut watcher = endpoint.watch_addr();
            let addr = watcher.get();
            info!(endpoint = %addr.id.fmt_short(), "announced iroh data plane");
            let _ = tx.send(Frame::IrohReady(addr));
            let tx_updates = tx.clone();
            tokio::spawn(async move {
                while let Ok(addr) = watcher.updated().await {
                    if tx_updates.send(Frame::IrohReady(addr)).is_err() {
                        break;
                    }
                }
            });
            // http/tcp expose a single port; the edge sends an empty service
            // name, so map `""` to the one local target.
            let mut targets = std::collections::HashMap::new();
            targets.insert(String::new(), opts.local);
            Some(tokio::spawn(crate::p2p::serve_endpoint(
                endpoint,
                std::sync::Arc::new(targets),
                opts.secret.clone(),
                Some(login.account.clone()),
            )))
        }
        Err(e) => {
            warn!("iroh endpoint unavailable, relay only: {e:#}");
            None
        }
    };

    // ---- Serve frames ----
    loop {
        match stream.next().await {
            Some(Ok(WsMessage::Text(text))) => {
                let frame = match decode(&text) {
                    Ok(f) => f,
                    // Unknown message types must not kill the connection.
                    Err(e) => {
                        debug!("ignoring bad frame from server: {e}");
                        continue;
                    }
                };
                if !handle_frame(&tx, frame).await {
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

    if let Some(serve) = serve {
        serve.abort();
    }
    if let Some(task) = data_task {
        task.abort();
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

/// Wait for the server's reply to a `TcpRegister` request.
async fn wait_tcp_reg_resp<S>(stream: &mut S) -> Result<Option<TcpRegisterResp>>
where
    S: StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
        Ok(Some(Ok(WsMessage::Text(text)))) => match decode(&text)? {
            Frame::TcpRegisterResp(r) => Ok(Some(r)),
            other => anyhow::bail!("expected TcpRegisterResp, got {other:?}"),
        },
        Ok(Some(Ok(_))) => anyhow::bail!("expected TcpRegisterResp, got a non-text message"),
        _ => Ok(None),
    }
}

/// Handle one frame from the server. Returns false to end the connection.
async fn handle_frame(tx: &mpsc::UnboundedSender<Frame>, frame: Frame) -> bool {
    match frame {
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
        Frame::TokenCheckResp(r) => {
            crate::token_check::resolve(r.id, if r.ok { Some(r.expires_at) } else { None });
            true
        }
        Frame::TokenCheck(_) => {
            warn!("unexpected TokenCheck from server");
            true
        }
        Frame::P2PAnnounce(_)
        | Frame::P2PAnnounceResp(_)
        | Frame::P2PLookup(_)
        | Frame::P2PAddr(_)
        | Frame::Lookup(_)
        | Frame::IrohReady(_)
        | Frame::Traffic(_)
        | Frame::TcpMapped(_)
        | Frame::AuthChallenge(_)
        | Frame::AuthResponse(_)
        | Frame::AuthOk(_)
        | Frame::AuthError(_) => {
            // P2P frames are handled by the dedicated `p2p` module, not here.
            warn!("unexpected P2P frame after handshake");
            true
        }
    }
}