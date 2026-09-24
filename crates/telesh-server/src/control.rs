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
    self as proto, decode, encode, login_message, AuthChallenge, AuthError, AuthMethod, AuthOk,
    ClientHello, Frame, P2PAddrResp, P2PAnnounceResp, PublicKey, ServerHello, Signature,
    StreamPacket, TcpRegisterResp, TokenCheckResp, LookupHello, PROTOCOL_VERSION,
};

use crate::config::config;
use crate::registry::AgentConn;
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

/// One control connection: dispatch on the first frame, which selects the role
/// (tunnel agent vs. lookup-only peer).
async fn handle_connection(addr: SocketAddr, mut socket: WebSocket) {
    let Some(first) = recv_first(&mut socket).await else {
        return;
    };
    let hello = match first {
        FirstFrame::Lookup(v) => return handle_lookup(addr, socket, v).await,
        FirstFrame::Agent(h) => h,
    };

    let reg = registry();

    // ---- Login: keypair challenge-response, or a session fast path ----
    if hello.protocol_version != PROTOCOL_VERSION {
        let _ = send(
            &mut socket,
            &Frame::ServerHello(ServerHello::Error(format!(
                "unsupported protocol version {}, expected {}",
                hello.protocol_version, PROTOCOL_VERSION
            ))),
        )
        .await;
        return;
    }
    let account = match login(&mut socket, &hello).await {
        Some(a) => a,
        None => return,
    };
    // The account handle (zero-padded 9-digit short id) replaces the old
    // self-declared agent_id everywhere downstream.
    let agent_id = crate::db::account_handle(account);

    let (sub_domain, assigned) = match resolve_subdomain(&mut socket, &hello).await {
        Some(r) => r,
        None => return,
    };

    // A single channel carries every frame bound for this agent.
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    if !reg.insert(&agent_id, &sub_domain, tx.clone()) {
        warn!(agent_id = %agent_id, sub_domain, "subdomain already in use");
        let _ = send(&mut socket, &Frame::ServerHello(ServerHello::SubDomainInUse)).await;
        return;
    }
    debug!(agent_id = %agent_id, sub_domain, "agent registered");

    let hostname = public_hostname(&sub_domain);
    if let Err(e) = send(
        &mut socket,
        &Frame::ServerHello(ServerHello::Success {
            sub_domain: sub_domain.clone(),
            hostname,
            agent_id: agent_id.clone(),
        }),
    )
    .await
    {
        debug!("failed to send server hello: {e}");
        reg.remove(&agent_id);
        return;
    }

    // ---- Pump loops ----
    let (mut sink, mut stream) = socket.split();

    // Sink task: drains the agent's outbound queue into the WebSocket.
    let agent_id_for_sink = agent_id.clone();
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
        reg_clone.remove(&agent_id_for_sink);
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
                warn!(agent_id = %agent_id, "control connection idle, closing");
                break;
            }
            Ok(Some(Ok(Message::Binary(data)))) => {
                if let Err(e) = dispatch(&tx, &agent_id, &data).await {
                    debug!("frame dispatch error: {e}");
                }
            }
            Ok(Some(Ok(Message::Close(_)))) => {
                debug!(agent_id = %agent_id, "agent closed connection");
                break;
            }
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Text(_)))) => {}
            Ok(Some(Err(e))) => {
                debug!(agent_id = %agent_id, "websocket error: {e}");
                break;
            }
            Ok(None) => break,
        }
    }

    let _ = tx.send(Frame::Ping); // nudge the sink task to notice the closed ws
    reg.remove(&agent_id);
    crate::tcp::unregister_all(&agent_id);
    crate::state::p2p_remove_account(&agent_id);
    info!(agent_id = %agent_id, sub_domain, %addr, "agent disconnected");

    let _ = assigned;
}

/// The first frame on a control connection, which sets the connection's role.
enum FirstFrame {
    Agent(ClientHello),
    Lookup(LookupHello),
}

/// Read the first message; must be a `Hello` (agent) or `Peer` (lookup-only).
async fn recv_first(socket: &mut WebSocket) -> Option<FirstFrame> {
    match tokio::time::timeout(Duration::from_secs(10), socket.next()).await {
        Ok(Some(Ok(Message::Binary(data)))) => match decode(&data) {
            Ok(Frame::Hello(h)) => Some(FirstFrame::Agent(h)),
            Ok(Frame::Lookup(v)) => Some(FirstFrame::Lookup(v)),
            other => {
                warn!("expected Hello/Peer, got {:?}", other.map(|_| "other frame"));
                None
            }
        },
        _ => None,
    }
}

/// Constant-time comparison of a presented key against an expected secret.
fn key_eq(key: Option<&str>, expected: &str) -> bool {
    key.map(|k| {
        k.as_bytes().iter().zip(expected.as_bytes()).all(|(a, b)| a == b)
            && k.len() == expected.len()
    }) == Some(true)
}

/// Auth for lookup-only peers. Accepts the master secret or, if configured,
/// a separate lower-privilege peer secret. Actual data access is gated by the
/// short-lived token the server mints, not by this secret.
fn lookup_auth_ok(key: Option<&str>) -> bool {
    let cfg = config();
    if cfg.secret.is_empty() && cfg.lookup_secret.is_empty() {
        return true;
    }
    (!cfg.secret.is_empty() && key_eq(key, &cfg.secret))
        || (!cfg.lookup_secret.is_empty() && key_eq(key, &cfg.lookup_secret))
}

/// Authenticate an agent: session fast path first, otherwise a keypair
/// challenge-response. Returns the account id, or `None` after notifying the
/// client of the failure.
async fn login(socket: &mut WebSocket, hello: &ClientHello) -> Option<u32> {
    if hello.method != AuthMethod::Keypair {
        let _ = send(socket, &Frame::AuthError(AuthError::UnsupportedMethod)).await;
        return None;
    }
    let pool = crate::state::db();

    // Fast path: a still-valid session token skips the challenge. A fresh token
    // is issued (rotation) so clients never keep a stale one for long.
    if let Some(session) = &hello.session {
        match crate::db::session_account(pool, session).await {
            Ok(Some(account)) => {
                return issue_session(socket, account, hello.display_name.as_deref()).await
            }
            Ok(None) => debug!("session token invalid/expired; falling back to keypair"),
            Err(e) => {
                warn!("session lookup failed: {e:#}");
                let _ = send(socket, &Frame::AuthError(AuthError::Error(e.to_string()))).await;
                return None;
            }
        }
    }

    let Some(pubkey_bytes) = hello.public_key else {
        let _ = send(socket, &Frame::AuthError(AuthError::BadSession)).await;
        return None;
    };
    let pubkey = match PublicKey::from_bytes(&pubkey_bytes) {
        Ok(pk) => pk,
        Err(_) => {
            let _ = send(socket, &Frame::AuthError(AuthError::BadSignature)).await;
            return None;
        }
    };

    // Challenge-response.
    let nonce: [u8; 32] = rand::random();
    let server_id = *crate::state::server_id();
    let ts = now_secs();
    if send(
        socket,
        &Frame::AuthChallenge(AuthChallenge {
            nonce,
            server_id,
            ts,
        }),
    )
    .await
    .is_err()
    {
        return None;
    }

    let signature = read_auth_response(socket).await?;
    let message = login_message(&server_id, &nonce, ts);
    if pubkey
        .verify(&message, &Signature::from_bytes(&signature))
        .is_err()
    {
        warn!(endpoint = %pubkey.fmt_short(), "login signature rejected");
        let _ = send(socket, &Frame::AuthError(AuthError::BadSignature)).await;
        return None;
    }

    // Find or create the account (TOFU). Registration is gated by the optional
    // enrollment secret; an existing key is never re-gated.
    let existing = match crate::db::account_for_key(pool, &pubkey_bytes).await {
        Ok(v) => v,
        Err(e) => {
            let _ = send(socket, &Frame::AuthError(AuthError::Error(e.to_string()))).await;
            return None;
        }
    };
    let account = match existing {
        Some(a) => a,
        None => {
            if !enrollment_ok(hello.enrollment.as_deref()) {
                warn!(endpoint = %pubkey.fmt_short(), "registration rejected (enrollment)");
                let _ = send(socket, &Frame::AuthError(AuthError::EnrollmentRejected)).await;
                return None;
            }
            match crate::db::account_for_key_or_create(pool, &pubkey_bytes).await {
                Ok(a) => {
                    info!(account = %crate::db::account_handle(a), endpoint = %pubkey.fmt_short(), "new account registered");
                    a
                }
                Err(e) => {
                    let _ = send(socket, &Frame::AuthError(AuthError::Error(e.to_string()))).await;
                    return None;
                }
            }
        }
    };

    issue_session(socket, account, hello.display_name.as_deref()).await
}

/// Mint a session and send `AuthOk`; returns the account id on success.
async fn issue_session(
    socket: &mut WebSocket,
    account: u32,
    display_name: Option<&str>,
) -> Option<u32> {
    let pool = crate::state::db();
    if let Some(name) = display_name.map(str::trim).filter(|n| !n.is_empty()) {
        if let Err(e) = crate::db::set_display_name(pool, account, name).await {
            warn!("cannot store display name: {e:#}");
        }
    }
    let display_name = crate::db::display_name(pool, account)
        .await
        .unwrap_or_default();
    match crate::db::create_session(pool, account, "agent", session_ttl()).await {
        Ok((session, expires_at)) => {
            let _ = send(
                socket,
                &Frame::AuthOk(AuthOk {
                    account: crate::db::account_handle(account),
                    session,
                    expires_at,
                    display_name,
                }),
            )
            .await;
            Some(account)
        }
        Err(e) => {
            let _ = send(socket, &Frame::AuthError(AuthError::Error(e.to_string()))).await;
            None
        }
    }
}

/// Read the `AuthResponse` to a challenge (tolerating keepalives).
async fn read_auth_response(socket: &mut WebSocket) -> Option<[u8; 64]> {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), socket.next()).await {
            Ok(Some(Ok(Message::Binary(data)))) => match decode(&data) {
                Ok(Frame::AuthResponse(r)) => {
                    return match <[u8; 64]>::try_from(r.signature.as_slice()) {
                        Ok(sig) => Some(sig),
                        Err(_) => {
                            let _ = send(socket, &Frame::AuthError(AuthError::BadSignature)).await;
                            None
                        }
                    };
                }
                Ok(Frame::Ping) => {
                    let _ = send(socket, &Frame::Pong).await;
                }
                Ok(other) => debug!("unexpected frame during login: {other:?}"),
                Err(e) => {
                    debug!("bad frame during login: {e}");
                    return None;
                }
            },
            _ => return None,
        }
    }
}

/// Whether first-time registration is allowed with the presented secret.
fn enrollment_ok(secret: Option<&str>) -> bool {
    let expected = &config().enrollment_secret;
    expected.is_empty() || key_eq(secret, expected)
}

/// Session lifetime: a fixed 12h ceiling.
fn session_ttl() -> u64 {
    crate::db::SESSION_TTL_SECS
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Answer a lookup-only peer. Peers never register a tunnel, so this
/// connection only ever serves `P2PLookup` (plus keepalives) until it closes.
async fn handle_lookup(addr: SocketAddr, mut socket: WebSocket, hello: LookupHello) {
    if hello.protocol_version != PROTOCOL_VERSION {
        let _ = send(
            &mut socket,
            &Frame::ServerHello(ServerHello::Error(format!(
                "unsupported protocol version {}, expected {}",
                hello.protocol_version, PROTOCOL_VERSION
            ))),
        )
        .await;
        return;
    }
    if !lookup_auth_ok(hello.auth_key.as_deref()) {
        warn!(peer = %hello.agent_id, "peer auth failed");
        let _ = send(&mut socket, &Frame::ServerHello(ServerHello::AuthFailed)).await;
        return;
    }
    if send(
        &mut socket,
        &Frame::ServerHello(ServerHello::Success {
            sub_domain: String::new(),
            hostname: String::new(),
            agent_id: hello.agent_id.clone(),
        }),
    )
    .await
    .is_err()
    {
        return;
    }
    // Replies and future push notifications go through a channel drained by a
    // sink task, so a re-announcing agent can notify this peer too.
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    let (mut sink, mut stream) = socket.split();
    let peer_id = hello.agent_id.clone();
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
    });
    debug!(peer = %peer_id, %addr, "peer connected (lookup-only)");

    loop {
        match tokio::time::timeout(PING_TIMEOUT, stream.next()).await {
            Err(_) => break,
            Ok(Some(Ok(Message::Binary(data)))) => match decode(&data) {
                Ok(Frame::P2PLookup(lookup)) => {
                    // Send the current address (may be NotFound) and subscribe
                    // for updates, so a later re-announce needs no re-lookup.
                    let key = format!("{}/{}", lookup.account, lookup.name);
                    let resp = lookup_p2p(&lookup.account, &lookup.name).await;
                    let _ = tx.send(Frame::P2PAddr(resp));
                    crate::state::p2p_subscribe(&key, tx.clone());
                }
                Ok(Frame::Ping) => {
                    if tx.send(Frame::Pong).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) => debug!("peer bad frame: {e}"),
            },
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => break,
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => {
                debug!(peer = %peer_id, "peer websocket error: {e}");
                break;
            }
        }
    }
    crate::state::p2p_prune_subs();
    debug!(peer = %peer_id, "peer disconnected");
}

/// Resolve an account-namespaced service against the P2P directory.
async fn lookup_p2p(account: &str, name: &str) -> P2PAddrResp {
    let key = format!("{account}/{name}");
    let addr = match crate::state::p2p_dir().get(&key) {
        Some(entry) => entry.announce.addr.clone(),
        None => return P2PAddrResp::NotFound,
    };
    // Issue the token and read the owner name outside the DashMap guard.
    let token = crate::state::issue_token(account, name).await;
    let owner_name = match account.parse::<u32>() {
        Ok(id) => crate::db::display_name(crate::state::db(), id)
            .await
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    P2PAddrResp::Found {
        account: account.to_string(),
        name: name.to_string(),
        owner_name,
        addr,
        token,
    }
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
            let client = AgentConn {
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
        // P2P directory: store a service's iroh address, or answer a lookup.
        Frame::P2PAnnounce(a) => {
            let name = a.name.clone();
            let addr = a.addr.clone();
            // The account comes from the authenticated connection, never the frame.
            let account = agent_id.to_string();
            let resp = if !proto::validate_subdomain(&name) {
                debug!(%name, agent_id, "rejecting invalid p2p service name");
                P2PAnnounceResp::InvalidName { name }
            } else {
                let key = format!("{account}/{name}");
                let entry = crate::state::P2pEntry {
                    account: account.clone(),
                    name: name.clone(),
                    endpoint_id: addr.id,
                    announce: a,
                };
                match crate::state::p2p_claim(key.clone(), entry) {
                    crate::state::P2pClaim::Ok => {
                        debug!(%name, %account, "p2p service registered");
                        // Wake any peer already subscribed to this name.
                        crate::state::p2p_notify(&key, &account, &name, &addr).await;
                        P2PAnnounceResp::Ok { name }
                    }
                    crate::state::P2pClaim::Taken => {
                        warn!(
                            %name,
                            %account,
                            "p2p service name already in use by another endpoint"
                        );
                        P2PAnnounceResp::NameTaken { name }
                    }
                }
            };
            let _ = tx.send(Frame::P2PAnnounceResp(resp));
        }
        Frame::P2PLookup(lookup) => {
            let resp = lookup_p2p(&lookup.account, &lookup.name).await;
            debug!(
                account = %lookup.account,
                name = %lookup.name,
                found = !matches!(resp, P2PAddrResp::NotFound),
                "p2p lookup"
            );
            let _ = tx.send(Frame::P2PAddr(resp));
        }
        Frame::P2PAddr(_) => debug!("unexpected P2PAddr from agent"),
        // Agent asks whether an opaque token is a live grant for account/name.
        Frame::TokenCheck(tc) => {
            let (ok, expires_at) = match tc.account.parse::<u32>() {
                Ok(account_id) => match crate::db::check_token(
                    crate::state::db(),
                    account_id,
                    &tc.name,
                    &tc.token,
                )
                .await
                {
                    Ok(Some(expires_at)) => (true, expires_at),
                    _ => (false, 0),
                },
                Err(_) => (false, 0),
            };
            let _ = tx.send(Frame::TokenCheckResp(TokenCheckResp {
                id: tc.id,
                ok,
                expires_at,
            }));
        }
        // Direct data plane: remember where to reach this agent over iroh.
        Frame::IrohReady(addr) => {
            debug!(agent_id, endpoint = %addr.id.fmt_short(), "agent announced iroh data plane");
            registry().set_iroh_addr(agent_id, addr);
        }
        // Traffic the server cannot observe (pure P2P): account it per service.
        Frame::Traffic(t) => {
            crate::metrics::add_traffic(&format!("p2p:{}", t.name), t.up, t.down);
        }
        // Router port mapping: informational; the port is opened on the agent.
        Frame::TcpMapped(m) => {
            info!(service = %m.name, public = %m.addr, local_port = m.local_port, "p2p tcp port mapped");
        }
        Frame::Pong => { /* keepalive ack */ }
        Frame::Hello(_)
        | Frame::ServerHello(_)
        | Frame::Ping
        | Frame::Lookup(_)
        | Frame::P2PAnnounceResp(_)
        | Frame::AuthChallenge(_)
        | Frame::AuthResponse(_)
        | Frame::AuthOk(_)
        | Frame::AuthError(_)
        | Frame::TokenCheckResp(_) => {
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