//! WebSocket control plane: agent handshake, frame dispatch, keepalive.

use std::net::SocketAddr;
use std::time::Duration;

use axum::{
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
    ClientHello, ClientKind, Frame, P2PAddrResp, P2PAnnounceResp, P2PListResp, PublicKey,
    ServerHello,
    Signature, TokenCheckResp, TunnelKind, TunnelPublish, TunnelResp, TunnelResult,
};

use crate::config::config;
use crate::registry::Route;
use crate::state::registry;

const PING_INTERVAL: Duration = Duration::from_secs(30);
const PING_TIMEOUT: Duration = Duration::from_secs(90);

/// axum router mounted at `/api/tunnel`.
pub fn router() -> axum::Router {
    axum::Router::new()
        .route("/api/tunnel", axum::routing::get(tunnel_ws))
        .route("/api/healthz", axum::routing::get(|| async { "ok" }))
}

async fn tunnel_ws(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_connection(addr, socket))
}

/// One control connection. 所有连接(agent 的和 peer 的)都先登录,登录之后按
/// `hello.client_kind` 分流:agent 去发布/接收隧道,peer 只查 P2P 目录。
async fn handle_connection(addr: SocketAddr, mut socket: WebSocket) {
    let Some(hello) = recv_hello(&mut socket).await else {
        return;
    };

    // ---- Login: keypair challenge-response, or a session fast path ----
    let account = match login(&mut socket, &hello).await {
        Some(a) => a,
        None => return,
    };
    // 账户句柄(补零的 9 位短 id)在下面各处取代了自申报的 agent_id。
    let agent_id = crate::db::account_handle(account);

    // peer(镜像那条连接)登录后只查目录,不注册任何东西。
    if hello.client_kind == ClientKind::Lookup {
        if send(
            &mut socket,
            &Frame::ServerHello(ServerHello::Success {
                agent_id: agent_id.clone(),
                relay_url: crate::state::relay_url(),
                data_addr: crate::state::data_addr(),
            }),
        )
        .await
        .is_err()
        {
            return;
        }
        return handle_lookup(addr, socket, agent_id).await;
    }

    let reg = registry();

    // 每条控制连接一个 id:重连时新连接可能先注册,旧连接的清理不能删掉新状态。
    let conn_id = crate::state::next_conn_id();

    // A single channel carries every frame bound for this agent. Tunnels are
    // registered per target *after* login (see `Frame::TunnelPublish`).
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    reg.insert_agent(&agent_id, conn_id);

    if let Err(e) = send(
        &mut socket,
        &Frame::ServerHello(ServerHello::Success {
            agent_id: agent_id.clone(),
            relay_url: crate::state::relay_url(),
            data_addr: crate::state::data_addr(),
        }),
    )
    .await
    {
        debug!("failed to send server hello: {e}");
        reg.remove(&agent_id, conn_id);
        return;
    }

    // ---- Pump loops ----
    let (mut sink, mut stream) = socket.split();

    // Sink task: drains the agent's outbound queue into the WebSocket.
    let agent_id_for_sink = agent_id.clone();
    let reg_clone = reg;
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let msg = match wire_message(&frame) {
                Ok(m) => m,
                Err(e) => {
                    error!(?e, "frame encode failed");
                    continue;
                }
            };
            if sink.send(msg).await.is_err() {
                break;
            }
        }
        reg_clone.remove(&agent_id_for_sink, conn_id);
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
            Ok(Some(Ok(msg @ Message::Text(_)))) => match parse_message(msg) {
                Ok(frame) => {
                    if let Err(e) = dispatch(&tx, &agent_id, conn_id, frame).await {
                        debug!("frame dispatch error: {e}");
                    }
                }
                // Unknown message types must not kill the connection.
                Err(e) => debug!(agent_id = %agent_id, "ignoring bad frame: {e}"),
            },
            Ok(Some(Ok(Message::Close(_)))) => {
                debug!(agent_id = %agent_id, "agent closed connection");
                break;
            }
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_)))) => {}
            Ok(Some(Err(e))) => {
                debug!(agent_id = %agent_id, "websocket error: {e}");
                break;
            }
            Ok(None) => break,
        }
    }

    let _ = tx.send(Frame::Ping); // nudge the sink task to notice the closed ws
    reg.remove(&agent_id, conn_id);
    crate::tcp::unregister_all(&agent_id, conn_id);
    crate::state::p2p_remove_account(&agent_id, conn_id);
    info!(agent_id = %agent_id, %addr, "agent disconnected");
}

/// 读第一帧:必须是 `hello`(agent 和 peer 都发它,靠 `client_kind` 区分)。
async fn recv_hello(socket: &mut WebSocket) -> Option<ClientHello> {
    match tokio::time::timeout(Duration::from_secs(10), socket.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => match decode(&text) {
            Ok(Frame::Hello(h)) => Some(h),
            Ok(other) => {
                warn!("expected hello as the first frame, got {other:?}");
                None
            }
            Err(e) => {
                warn!("first frame is not a valid control message: {e}");
                None
            }
        },
        _ => None,
    }
}

/// Authenticate a client: session fast path first, otherwise a keypair
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

    // Find or create the account (TOFU): 新公钥第一次登录就建账号并绑定它。
    // 更严的注册策略(匿名/邀请/OIDC)以后再加,不再是配置里的一个共享密钥。
    let account = match crate::db::account_for_key(pool, &pubkey_bytes).await {
        Ok(Some(a)) => a,
        Ok(None) => match crate::db::account_for_key_or_create(pool, &pubkey_bytes).await {
            Ok(a) => {
                info!(account = %crate::db::account_handle(a), endpoint = %pubkey.fmt_short(), "new account registered");
                a
            }
            Err(e) => {
                let _ = send(socket, &Frame::AuthError(AuthError::Error(e.to_string()))).await;
                return None;
            }
        },
        Err(e) => {
            let _ = send(socket, &Frame::AuthError(AuthError::Error(e.to_string()))).await;
            return None;
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
            Ok(Some(Ok(msg @ Message::Text(_)))) => match parse_message(msg) {
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
async fn handle_lookup(addr: SocketAddr, socket: WebSocket, agent_id: String) {
    // Replies and future push notifications go through a channel drained by a
    // sink task, so a re-announcing agent can notify this peer too.
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    let (mut sink, mut stream) = socket.split();
    let peer_id = agent_id;
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let msg = match wire_message(&frame) {
                Ok(m) => m,
                Err(e) => {
                    error!(?e, "frame encode failed");
                    continue;
                }
            };
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });
    debug!(peer = %peer_id, %addr, "peer connected (lookup-only)");

    loop {
        match tokio::time::timeout(PING_TIMEOUT, stream.next()).await {
            Err(_) => break,
            Ok(Some(Ok(msg @ Message::Text(_)))) => match parse_message(msg) {
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

/// 发布一条 HTTP 隧道:把一个本地目标挂到子域上。
async fn register_http(agent_id: &str, conn_id: u64, req: &TunnelPublish) -> TunnelResult {
    let sub_domain = match req.name.as_deref() {
        Some(s) if proto::validate_subdomain(s) => s.to_string(),
        Some(_) => return TunnelResult::Invalid,
        None => proto::random_subdomain(),
    };
    let route = Route {
        agent_id: agent_id.to_string(),
        conn_id,
        target: req.target.clone(),
    };
    if !registry().add_route(&sub_domain, route) {
        return TunnelResult::NameTaken;
    }
    let hostname = public_hostname(&sub_domain);
    info!(agent_id, %sub_domain, target = %req.target, "http tunnel registered");
    TunnelResult::Http {
        sub_domain,
        hostname,
    }
}

/// 发布一条裸 TCP 隧道:分配一个公网端口。
async fn register_tcp(agent_id: &str, conn_id: u64, req: &TunnelPublish) -> TunnelResult {
    match crate::tcp::register(agent_id, conn_id, &req.target, req.remote_port).await {
        Ok(port) => {
            info!(agent_id, port, target = %req.target, "tcp tunnel registered");
            TunnelResult::Tcp { port }
        }
        Err(crate::tcp::TcpError::InUse) => TunnelResult::NameTaken,
        Err(crate::tcp::TcpError::OutOfRange) => TunnelResult::Invalid,
        Err(e) => TunnelResult::Error(e.to_string()),
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
    conn_id: u64,
    frame: Frame,
) -> anyhow::Result<()> {
    match frame {
        // Multi-tunnel: publish one tunnel (HTTP or TCP); P2P uses `P2PAnnounce`.
        Frame::TunnelPublish(req) => {
            let result = match req.kind {
                TunnelKind::Http => register_http(agent_id, conn_id, &req).await,
                TunnelKind::Tcp => register_tcp(agent_id, conn_id, &req).await,
                TunnelKind::P2p => TunnelResult::Error("p2p tunnels use p2p_announce".into()),
            };
            let _ = tx.send(Frame::TunnelResp(TunnelResp { id: req.id, result }));
        }
        Frame::TunnelResp(_) => debug!("unexpected TunnelResp from agent"),
        // Unpublish a tunnel.
        Frame::TunnelClose(close) => match close.kind {
            TunnelKind::Http => {
                registry().remove_route(&close.key, conn_id);
                info!(agent_id, sub_domain = %close.key, "http tunnel closed");
            }
            TunnelKind::Tcp => {
                crate::tcp::unregister(&close.key, conn_id);
                info!(agent_id, port = %close.key, "tcp tunnel closed");
            }
            TunnelKind::P2p => {
                crate::state::p2p_remove_name(agent_id, &close.key, conn_id);
                info!(agent_id, name = %close.key, "p2p service closed");
            }
        },
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
                    conn_id,
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
        // Agent asks which services an account currently offers (for its local UI).
        Frame::P2PList(list) => {
            let names = crate::state::p2p_names(&list.account);
            debug!(account = %list.account, count = names.len(), "p2p list");
            let _ = tx.send(Frame::P2PListResp(P2PListResp {
                id: list.id,
                names,
            }));
        }
        Frame::P2PListResp(_) => debug!("unexpected P2PListResp from agent"),
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

/// 一条线上消息 → 一个 WebSocket text 帧(控制面全是 JSON)。
fn wire_message(frame: &Frame) -> anyhow::Result<Message> {
    Ok(Message::Text(encode(frame)?.into()))
}

/// WebSocket 消息 → 线上消息。控制面只用 text 帧。
fn parse_message(msg: Message) -> anyhow::Result<Frame> {
    match msg {
        Message::Text(text) => Ok(decode(&text)?),
        other => anyhow::bail!("unexpected websocket message: {other:?}"),
    }
}

async fn send(socket: &mut WebSocket, frame: &Frame) -> anyhow::Result<()> {
    socket.send(wire_message(frame)?).await?;
    Ok(())
}