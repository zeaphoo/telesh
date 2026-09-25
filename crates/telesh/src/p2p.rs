//! P2P transport over iroh.
//!
//! Two roles share the same peer-to-peer data plane:
//!
//! * **expose** (`telesh p2p <port>`): publish a local port under a service
//!   name. The server stores the iroh [`EndpointAddr`]; inbound iroh streams
//!   are dialed into the local service.
//! * **connect** (`telesh connect <name> --local-port <n>`): mirror a remote
//!   service onto a local TCP port. Payload flows peer-to-peer; the server is
//!   only asked once, for the address.
//!
//! iroh handles NAT hole punching, relay fallback and (optionally) UPnP/PCP/
//! NAT-PMP port mapping. The WebSocket control plane is used only to exchange
//! the address and to keep the directory entry alive.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use dashmap::DashMap;

use anyhow::{anyhow, bail, Context, Result};
use futures::{SinkExt, StreamExt};
use iroh::endpoint::{presets, Connection, PortmapperConfig, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, info, warn};

use telesh_protocol::{
    decode, encode, Frame, LookupHello, P2PAddrResp, P2PLookup, ServerHello, TrafficReport,
    STREAM_ACK_ERR, STREAM_ACK_OK,
};

/// ALPN identifying the telesh P2P data plane.
pub(crate) const ALPN: &[u8] = b"telesh/p2p/1";

/// Upper bound on the auth prelude we will read from a peer, in bytes.
const MAX_PRELUDE: usize = 256;

/// How long to wait for a direct/relayed iroh connection to a peer.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How often a peer refreshes its access token (well within the server TTL).
const TOKEN_REFRESH_SECS: u64 = 150;

/// Per-service P2P byte counters (sent, received). The server cannot observe
/// this traffic, so the agent reports deltas periodically.
static P2P_TRAFFIC: OnceLock<DashMap<String, (AtomicU64, AtomicU64)>> = OnceLock::new();

fn p2p_traffic() -> &'static DashMap<String, (AtomicU64, AtomicU64)> {
    P2P_TRAFFIC.get_or_init(DashMap::new)
}

fn record_traffic(name: &str, up: u64, down: u64) {
    if up == 0 && down == 0 {
        return;
    }
    let entry = p2p_traffic()
        .entry(name.to_string())
        .or_insert_with(|| (AtomicU64::new(0), AtomicU64::new(0)));
    if up > 0 {
        entry.0.fetch_add(up, Ordering::Relaxed);
    }
    if down > 0 {
        entry.1.fetch_add(down, Ordering::Relaxed);
    }
}

/// 上报并清空 P2P 流量计数(服务端看不到这些字节)。
pub(crate) fn drain_traffic() -> Vec<TrafficReport> {
    let mut out = Vec::new();
    for entry in p2p_traffic().iter() {
        let up = entry.value().0.swap(0, Ordering::Relaxed);
        let down = entry.value().1.swap(0, Ordering::Relaxed);
        if up == 0 && down == 0 {
            continue;
        }
        out.push(TrafficReport {
            name: entry.key().clone(),
            up,
            down,
        });
    }
    out
}

/// Parameters for mirroring a remote service onto a local port.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub control_addr: String,
    /// 9-digit account handle of the service owner.
    pub account: String,
    /// Service name within that account.
    pub name: String,
    /// Address the local mirror listens on.
    pub local: SocketAddr,
    pub secret: Option<String>,
    pub no_portmap: bool,
    /// How long to hold an accepted local connection while the P2P link
    /// recovers, retrying the stream setup (never after payload flows).
    pub retry_wait: Duration,
}

/// Run one connect session: subscribe to a service, keep a live iroh connection
/// to its latest address, and mirror the local port.
///
/// Address updates are pushed by the server (subscription), so an agent that
/// reconnects with a new address needs no re-lookup. Returns when the control
/// subscription ends; the caller resubscribes.
pub async fn run_connect(opts: &ConnectOptions) -> Result<()> {
    let (info_tx, info_rx) = watch::channel::<Option<ServiceInfo>>(None);
    let (conn_tx, conn_rx) = watch::channel::<Option<Connection>>(None);

    // Control subscription: keeps the WS open, pushes address updates, and
    // refreshes the access token before it expires. The relay URL is announced
    // by the server during this handshake, so it must complete before the iroh
    // endpoint (which needs the relay) can be built.
    let (relay_tx, relay_rx) = tokio::sync::oneshot::channel::<Option<String>>();
    let mut control = tokio::spawn(subscribe(
        opts.control_addr.clone(),
        opts.account.clone(),
        opts.name.clone(),
        opts.secret.clone(),
        info_tx,
        relay_tx,
    ));
    let relay = match tokio::time::timeout(Duration::from_secs(15), relay_rx).await {
        Ok(Ok(relay)) => relay,
        Ok(Err(_)) => {
            control.abort();
            return Err(anyhow!("control subscription ended before the server handshake"));
        }
        Err(_) => {
            control.abort();
            return Err(anyhow!("timed out waiting for the control server handshake"));
        }
    };
    if let Some(url) = &relay {
        info!(relay = %url, "using server-announced iroh relay");
    }
    let endpoint = build_endpoint(SecretKey::generate(), relay.as_deref(), opts.no_portmap).await?;

    // Supervisor: maintains one live connection to the latest address.
    let supervisor = tokio::spawn(supervisor(endpoint, info_rx.clone(), conn_tx));

    let listener = TcpListener::bind(opts.local)
        .await
        .with_context(|| format!("cannot bind local mirror {}", opts.local))?;
    info!(local = %opts.local, service = %opts.name, "mirroring p2p service to local port");

    loop {
        tokio::select! {
            _ = &mut control => {
                supervisor.abort();
                return Err(anyhow!("control subscription ended"));
            }
            accepted = listener.accept() => {
                let (sock, peer) = match accepted {
                    Ok(v) => v,
                    Err(e) => {
                        control.abort();
                        supervisor.abort();
                        return Err(e.into());
                    }
                };
                let mut conn_rx = conn_rx.clone();
                let info_rx = info_rx.clone();
                let secret = opts.secret.clone();
                let service = format!("{}/{}", opts.account, opts.name);
                let retry_wait = opts.retry_wait;
                tokio::spawn(async move {
                    match attach(sock, &mut conn_rx, &info_rx, secret, service, retry_wait).await {
                        AttachOutcome::Attached => {}
                        AttachOutcome::Refused => {
                            debug!(%peer, "local connection refused by agent")
                        }
                        AttachOutcome::TimedOut => debug!(
                            %peer,
                            "gave up attaching local connection after {retry_wait:?}"
                        ),
                    }
                });
            }
        }
    }
}

/// A subscribed service's address plus the current access token.
#[derive(Debug, Clone)]
struct ServiceInfo {
    addr: EndpointAddr,
    token: Option<Vec<u8>>,
}

/// Keep exactly one live iroh connection to the most recent address, restoring
/// it automatically after the agent reconnects (or the path drops).
async fn supervisor(
    endpoint: Endpoint,
    mut info_rx: watch::Receiver<Option<ServiceInfo>>,
    conn_tx: watch::Sender<Option<Connection>>,
) {
    loop {
        // Wait until we have an address.
        while info_rx.borrow().is_none() {
            if info_rx.changed().await.is_err() {
                return;
            }
        }
        let addr = info_rx
            .borrow()
            .as_ref()
            .map(|i| i.addr.clone())
            .expect("checked above");

        let conn = match tokio::time::timeout(CONNECT_TIMEOUT, endpoint.connect(addr.clone(), ALPN))
            .await
        {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => {
                warn!("p2p connect failed: {e:#}");
                tokio::time::sleep(Duration::from_millis(1000)).await;
                continue;
            }
            Err(_) => {
                warn!("p2p connect timed out");
                tokio::time::sleep(Duration::from_millis(1000)).await;
                continue;
            }
        };
        info!(remote = %conn.remote_id().fmt_short(), "p2p connection established");
        let _ = conn_tx.send(Some(conn.clone()));

        // Hold the connection until it dies or the *address* changes (token
        // refreshes must not force a reconnect).
        loop {
            tokio::select! {
                _ = conn.closed() => {
                    debug!("p2p connection closed");
                    break;
                }
                changed = info_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let new = info_rx.borrow().as_ref().map(|i| i.addr.clone());
                    if new.as_ref() != Some(&addr) {
                        debug!("p2p address changed; reconnecting");
                        break;
                    }
                    // Same address re-announced: keep the existing connection.
                }
            }
        }
        let _ = conn_tx.send(None);
        conn.close(0u32.into(), b"reconnect");
    }
}

/// Subscribe to a service and feed its address (and later updates) to the
/// supervisor. Keeping this connection open is what makes re-lookup unnecessary.
async fn subscribe(
    control_addr: String,
    account: String,
    name: String,
    secret: Option<String>,
    info_tx: watch::Sender<Option<ServiceInfo>>,
    relay_tx: tokio::sync::oneshot::Sender<Option<String>>,
) -> Result<()> {
    let url = format!("ws://{control_addr}/tunnel");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .with_context(|| format!("failed to connect to control server at {url}"))?;

    // A lookup-only peer registers no tunnel on the server.
    let hello = LookupHello {
        agent_id: format!("peer-{}", random_id(12)),
        auth_key: secret,
    };
    ws.send(ws_text(&Frame::Lookup(hello))?).await?;
    let relay = loop {
        match ws.next().await {
            Some(Ok(WsMessage::Text(text))) => match decode(&text)? {
                Frame::ServerHello(hello @ ServerHello::Success { .. }) => {
                    break hello.relay_url().map(str::to_string)
                }
                Frame::ServerHello(other) => bail!("control server rejected peer: {other:?}"),
                Frame::Ping => ws.send(ws_text(&Frame::Pong)?).await?,
                _ => {}
            },
            Some(Ok(_)) => {}
            Some(Err(e)) => bail!("control connection error during handshake: {e}"),
            None => bail!("control connection closed during handshake"),
        }
    };
    // Hand the announced relay to `run_connect` before entering the long-lived
    // push loop.
    let _ = relay_tx.send(relay);

    // Split so we can read pushes and periodically re-request a fresh token.
    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<WsMessage>();
    let sink_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });
    out_tx
        .send(ws_text(&Frame::P2PLookup(P2PLookup {
            account: account.clone(),
            name: name.clone(),
        }))?)
        .map_err(|_| anyhow!("control sink closed"))?;

    // Refresh comfortably inside the token TTL.
    let refresh_every = Duration::from_secs(TOKEN_REFRESH_SECS);
    let mut refresh = tokio::time::interval(refresh_every);
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    refresh.tick().await; // consume the immediate first tick

    let result = loop {
        tokio::select! {
            _ = refresh.tick() => {
                let lookup = Frame::P2PLookup(P2PLookup {
                    account: account.clone(),
                    name: name.clone(),
                });
                if out_tx.send(ws_text(&lookup)?).is_err() {
                    break Err(anyhow!("control sink closed"));
                }
            }
            msg = stream.next() => match msg {
                Some(Ok(WsMessage::Text(text))) => match decode(&text)? {
                    Frame::P2PAddr(P2PAddrResp::Found {
                        owner_name,
                        addr,
                        token,
                        ..
                    }) => {
                        info!(owner = %owner_name, service = %name, endpoint = %addr.id.fmt_short(), "p2p service address updated");
                        let _ = info_tx.send(Some(ServiceInfo { addr, token }));
                    }
                    Frame::P2PAddr(P2PAddrResp::NotFound) => {
                        warn!(service = %name, "p2p service not registered yet; waiting for it");
                    }
                    Frame::Ping => {
                        let sent = out_tx.send(ws_text(&Frame::Pong)?).is_ok();
                        if !sent {
                            break Err(anyhow!("control sink closed"));
                        }
                    }
                    _ => {}
                },
                Some(Ok(WsMessage::Close(_))) => break Ok(()),
                Some(Ok(_)) => {}
                Some(Err(e)) => break Err(anyhow!("control connection error: {e}")),
                None => break Ok(()),
            }
        }
    };
    sink_task.abort();
    match result {
        Ok(()) => bail!("control subscription closed"),
        Err(e) => Err(e),
    }
}

/// Build an iroh endpoint with the telesh ALPN and (optional) custom relay.
pub(crate) async fn build_endpoint(
    secret: SecretKey,
    relay: Option<&str>,
    no_portmap: bool,
) -> Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .alpns(vec![ALPN.to_vec()]);
    builder = match relay {
        Some(url) => {
            let url: iroh::RelayUrl = url
                .parse()
                .with_context(|| format!("invalid relay url {url:?}"))?;
            builder.relay_mode(RelayMode::custom([url]))
        }
        None => builder.relay_mode(RelayMode::Default),
    };
    if no_portmap {
        builder = builder.portmapper_config(PortmapperConfig::Disabled);
    }
    builder.bind().await.context("failed to bind iroh endpoint")
}

/// Accept P2P connections and turn each inbound stream into a local dial.
pub(crate) async fn serve_endpoint(
    endpoint: Endpoint,
    targets: Arc<dashmap::DashMap<String, SocketAddr>>,
    secret: Option<String>,
    account: Option<String>,
) {
    while let Some(incoming) = endpoint.accept().await {
        let targets = targets.clone();
        let secret = secret.clone();
        let account = account.clone();
        tokio::spawn(async move {
            let conn = match incoming.await {
                Ok(conn) => conn,
                Err(e) => {
                    debug!("p2p incoming connection failed: {e}");
                    return;
                }
            };
            debug!(remote = %conn.remote_id().fmt_short(), "p2p peer connected");
            // Authorization is cached per iroh connection: the first stream pays
            // the server round-trip, later streams reuse the grant.
            let grants: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
            loop {
                let (send, recv) = match conn.accept_bi().await {
                    Ok(s) => s,
                    Err(e) => {
                        debug!("p2p connection closed: {e}");
                        break;
                    }
                };
                let targets = targets.clone();
                let secret = secret.clone();
                let account = account.clone();
                let grants = grants.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_stream(targets, secret, account, grants, send, recv).await {
                        debug!("p2p stream ended: {e:#}");
                    }
                });
            }
        });
    }
}

/// Authenticate a peer, resolve the named service, then pump both ways.
async fn serve_stream(
    targets: Arc<dashmap::DashMap<String, SocketAddr>>,
    secret: Option<String>,
    account: Option<String>,
    grants: Arc<Mutex<HashMap<String, u64>>>,
    mut send: SendStream,
    mut recv: RecvStream,
) -> Result<()> {
    // Every path answers with a one-byte ack, so the dialer can tell a
    // definitive refusal (do not retry) from a dead link (safe to retry).
    let preamble = match read_handshake(&mut recv, secret.as_deref()).await? {
        Some(preamble) => preamble,
        None => {
            warn!("peer rejected: bad or missing credential");
            let _ = send.write_all(&[STREAM_ACK_ERR]).await;
            let _ = send.finish();
            return Ok(());
        }
    };
    let service = preamble.service;
    // 服务名一律是 "<account>/<name>";没有斜杠就只当名字用。
    // 空服务名现在没有合法来源(公网 http/tcp 走数据连接,不连 iroh)。
    if service.is_empty() {
        warn!("peer requested an empty service");
        let _ = send.write_all(&[STREAM_ACK_ERR]).await;
        let _ = send.finish();
        return Ok(());
    }
    let (acct, name) = service
        .split_once('/')
        .unwrap_or(("", service.as_str()));
    if let Some(own) = &account {
        if !acct.is_empty() && acct != own {
            warn!(%service, "requested service belongs to another account");
            let _ = send.write_all(&[STREAM_ACK_ERR]).await;
            let _ = send.finish();
            return Ok(());
        }
    }
    // An opaque token is only trusted after the server (which owns the store)
    // confirms it for this `account/name`. The grant is cached for the rest of
    // the iroh connection, so only the first stream pays the round-trip.
    if !service.is_empty() {
        let now = now_secs();
        let cached = grants
            .lock()
            .unwrap()
            .get(&service)
            .copied()
            .unwrap_or(0)
            > now;
        if cached {
            debug!(%service, "reusing cached grant for this connection");
        } else if let Some(token) = preamble.token {
            match crate::token_check::check(acct, name, token).await {
                Some(expires_at) => {
                    grants.lock().unwrap().insert(service.clone(), expires_at);
                }
                None => {
                    warn!(%service, "access token rejected by the server");
                    let _ = send.write_all(&[STREAM_ACK_ERR]).await;
                    let _ = send.finish();
                    return Ok(());
                }
            }
        }
    }
    let Some(local) = targets.get(name).map(|e| *e.value()) else {
        warn!(%service, "p2p peer requested unknown service");
        let _ = send.write_all(&[STREAM_ACK_ERR]).await;
        let _ = send.finish();
        return Ok(());
    };
    let tcp = match TcpStream::connect(local).await {
        Ok(tcp) => tcp,
        Err(e) => {
            warn!(%service, %local, "cannot dial local service: {e}");
            let _ = send.write_all(&[STREAM_ACK_ERR]).await;
            let _ = send.finish();
            return Ok(());
        }
    };
    send.write_all(&[STREAM_ACK_OK])
        .await
        .context("send stream ack")?;
    let (up, down) = pump(tcp, send, recv).await;
    record_traffic(&service, up, down);
    Ok(())
}

/// Result of attaching an accepted local socket to a P2P stream.
enum AttachOutcome {
    /// Attached and pumped (possibly after retries).
    Attached,
    /// The agent definitively refused (bad credential/service or local dial
    /// failed); retrying would not help.
    Refused,
    /// The retry window expired with the link still down.
    TimedOut,
}

/// Attach an accepted local socket to a live P2P stream.
///
/// Opening the stream and writing the preamble are retried while the link
/// recovers, but only *before any payload is forwarded* — retrying after bytes
/// have flowed could duplicate them on the peer. The ack (`STREAM_ACK_OK`) is
/// the commit point: from there we pump and never retry.
async fn attach(
    sock: TcpStream,
    conn_rx: &mut watch::Receiver<Option<Connection>>,
    info_rx: &watch::Receiver<Option<ServiceInfo>>,
    secret: Option<String>,
    service: String,
    window: Duration,
) -> AttachOutcome {
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let Some(conn) = wait_for_connection_until(conn_rx, deadline).await else {
            return AttachOutcome::TimedOut;
        };
        match conn.open_bi().await {
            Ok((mut send, mut recv)) => {
                // Use the newest token; it is refreshed independently of the link.
                let token = info_rx.borrow().as_ref().and_then(|i| i.token.clone());
                if let Err(e) =
                    write_handshake(&mut send, secret.as_deref(), &service, token.as_deref()).await
                {
                    debug!("p2p preamble failed, retrying before any payload: {e:#}");
                } else {
                    // Wait for the agent to confirm it attached the local service.
                    // Until this ack arrives no payload has been forwarded, so a
                    // lost link here is safe to retry on a fresh stream.
                    let mut ack = [0u8; 1];
                    match recv.read_exact(&mut ack).await {
                        Ok(()) if ack[0] == STREAM_ACK_OK => {
                            let _ = pump(sock, send, recv).await;
                            return AttachOutcome::Attached;
                        }
                        Ok(()) => return AttachOutcome::Refused,
                        Err(e) => {
                            debug!("p2p ack lost, retrying before any payload: {e:#}");
                        }
                    }
                }
            }
            Err(e) => debug!("p2p stream open failed, retrying: {e:#}"),
        }
        // Back off so a persistently failing link does not spin.
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return AttachOutcome::TimedOut;
        }
        tokio::time::sleep(Duration::from_millis(300).min(remaining)).await;
    }
}

/// Wait (until `deadline`) for the supervisor to publish a live connection.
async fn wait_for_connection_until(
    rx: &mut watch::Receiver<Option<Connection>>,
    deadline: tokio::time::Instant,
) -> Option<Connection> {
    loop {
        if let Some(conn) = rx.borrow().as_ref().cloned() {
            // Skip a connection the supervisor has already torn down.
            if conn.close_reason().is_none() {
                return Some(conn);
            }
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        if tokio::time::timeout(remaining, rx.changed()).await.is_err() {
            return None;
        }
    }
}

/// Copy bytes in both directions until either side closes. Returns
/// `(sent_to_peer, received_from_peer)` for metrics.
async fn pump(tcp: TcpStream, mut send: SendStream, mut recv: RecvStream) -> (u64, u64) {
    let (mut tcp_r, mut tcp_w) = tcp.into_split();
    let up = async {
        let res = tokio::io::copy(&mut tcp_r, &mut send).await;
        // Signal EOF (rather than resetting) once the local side is done.
        let _ = send.finish();
        res
    };
    let down = async {
        let res = tokio::io::copy(&mut recv, &mut tcp_w).await;
        let _ = tcp_w.shutdown().await;
        res
    };
    let (up, down) = tokio::join!(up, down);
    (up.unwrap_or(0), down.unwrap_or(0))
}

/// Credential kinds for the stream preamble.
const CRED_SECRET: u8 = 0;
const CRED_TOKEN: u8 = 1;

/// A dialer's stream preamble: a credential plus the requested service.
struct Preamble {
    /// Opaque access token (kind 1).
    token: Option<Vec<u8>>,
    /// `"<account>/<name>"`。
    service: String,
}

/// Dialer -> acceptor stream preamble.
///
/// Kind 0 (shared secret): `[0][u16 secret_len][secret][u16 service_len][service]`.
/// Kind 1 (access token):  `[1][u16 token_len][token][u16 service_len][service]`.
async fn write_handshake(
    send: &mut SendStream,
    secret: Option<&str>,
    service: &str,
    token: Option<&[u8]>,
) -> Result<()> {
    if service.len() > MAX_PRELUDE {
        bail!("service name too long ({} bytes)", service.len());
    }
    match token {
        Some(token) => {
            if token.len() > MAX_PRELUDE {
                bail!("token too long ({} bytes)", token.len());
            }
            send.write_all(&[CRED_TOKEN]).await?;
            send.write_all(&(token.len() as u16).to_be_bytes()).await?;
            send.write_all(token).await?;
        }
        None => {
            let secret = secret.unwrap_or("").as_bytes();
            if secret.len() > MAX_PRELUDE {
                bail!("auth key too long ({} bytes)", secret.len());
            }
            send.write_all(&[CRED_SECRET]).await?;
            send.write_all(&(secret.len() as u16).to_be_bytes()).await?;
            send.write_all(secret).await?;
        }
    }
    send.write_all(&(service.len() as u16).to_be_bytes()).await?;
    send.write_all(service.as_bytes()).await?;
    Ok(())
}

/// Acceptor side of [`write_handshake`]. `None` means a rejected credential.
///
/// A kind-0 secret is checked here; a kind-1 token is **not** — the caller asks
/// the server to validate it (see `crate::token_check`).
async fn read_handshake(recv: &mut RecvStream, secret: Option<&str>) -> Result<Option<Preamble>> {
    let mut kind = [0u8; 1];
    recv.read_exact(&mut kind)
        .await
        .context("read credential kind")?;
    let token = match kind[0] {
        CRED_SECRET => {
            let presented = read_field(recv).await.context("read auth prelude")?;
            if let Some(expected) = secret {
                if !constant_time_eq(&presented, expected.as_bytes()) {
                    return Ok(None);
                }
            }
            None
        }
        CRED_TOKEN => Some(read_field(recv).await.context("read access token")?),
        other => bail!("unknown credential kind {other}"),
    };
    let service = read_field(recv).await.context("read service name")?;
    Ok(Some(Preamble {
        token,
        service: String::from_utf8_lossy(&service).into_owned(),
    }))
}

/// Unix seconds.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Read one length-prefixed (`u16`) field.
async fn read_field(recv: &mut RecvStream) -> Result<Vec<u8>> {
    let mut len = [0u8; 2];
    recv.read_exact(&mut len).await?;
    let len = u16::from_be_bytes(len) as usize;
    if len > MAX_PRELUDE {
        bail!("field length {len} exceeds maximum {MAX_PRELUDE}");
    }
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await?;
    Ok(buf)
}



/// 一条控制消息 → 一个 JSON text 帧。
fn ws_text(frame: &Frame) -> Result<WsMessage> {
    Ok(WsMessage::Text(encode(frame)?))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

fn random_id(len: usize) -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}