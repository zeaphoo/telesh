//! Server-side iroh gateway: the edge dials agents directly for HTTP/TCP
//! ingress traffic, instead of multiplexing over the agent's WebSocket.
//!
//! The control plane still carries only small JSON/frames; bulk payload that
//! used to relay through the server now takes a peer-to-peer iroh path when the
//! agent announced one, with the WebSocket relay as fallback.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use iroh::endpoint::{presets, Connection, PortmapperConfig, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, RelayMode};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

/// ALPN identifying the telesh direct data plane (shared with the agent).
pub const ALPN: &[u8] = b"telesh/p2p/1";

/// How long to wait for a direct/relayed connection to an agent.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound on the auth prelude, mirroring the agent side.
const MAX_PRELUDE: usize = 256;

/// Bind the server's iroh endpoint. The edge only dials agents, so port
/// mapping is disabled and an ephemeral identity is sufficient.
pub async fn build_endpoint(relay: &str) -> Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(iroh::SecretKey::generate())
        .alpns(vec![ALPN.to_vec()])
        .portmapper_config(PortmapperConfig::Disabled);
    builder = if relay.is_empty() {
        builder.relay_mode(RelayMode::Default)
    } else {
        let url: iroh::RelayUrl = relay
            .parse()
            .with_context(|| format!("invalid relay url {relay:?}"))?;
        builder.relay_mode(RelayMode::custom([url]))
    };
    builder
        .bind()
        .await
        .context("failed to bind server iroh endpoint")
}

/// Dial an agent and open one stream for a public connection. The returned
/// `Connection` must be kept alive for the lifetime of the stream.
pub async fn dial(
    endpoint: &Endpoint,
    addr: EndpointAddr,
) -> Result<(Connection, SendStream, RecvStream)> {
    let conn = tokio::time::timeout(CONNECT_TIMEOUT, endpoint.connect(addr, ALPN))
        .await
        .map_err(|_| anyhow!("timed out dialing agent over iroh"))?
        .context("iroh connect failed")?;
    let (send, recv) = conn.open_bi().await.context("failed to open iroh stream")?;
    Ok((conn, send, recv))
}

/// Dialer -> acceptor stream preamble, shared-secret kind:
/// `[0][u16 secret_len][secret][u16 service_len][service]`.
///
/// The edge exposes a single local port per agent, so the service name is
/// empty; the agent's target table maps `""` to that port.
pub async fn write_handshake(send: &mut SendStream, secret: &str, service: &str) -> Result<()> {
    let secret = secret.as_bytes();
    if secret.len() > MAX_PRELUDE || service.len() > MAX_PRELUDE {
        return Err(anyhow!("handshake field too long"));
    }
    send.write_all(&[0u8]).await?;
    send.write_all(&(secret.len() as u16).to_be_bytes()).await?;
    send.write_all(secret).await?;
    send.write_all(&(service.len() as u16).to_be_bytes()).await?;
    send.write_all(service.as_bytes()).await?;
    Ok(())
}

/// The acceptor's one-byte stream acknowledgement.
pub enum Ack {
    /// Preamble accepted and the local service attached; payload may flow.
    Ok,
    /// Refused (bad credential/service or local dial failed); do not retry.
    Refused,
}

/// Read the acceptor's ack after [`write_handshake`].
pub async fn read_ack(recv: &mut RecvStream) -> Result<Ack> {
    let mut byte = [0u8; 1];
    recv.read_exact(&mut byte)
        .await
        .context("failed to read stream ack")?;
    match byte[0] {
        telesh_protocol::STREAM_ACK_OK => Ok(Ack::Ok),
        telesh_protocol::STREAM_ACK_ERR => Ok(Ack::Refused),
        other => Err(anyhow!("unexpected stream ack {other}")),
    }
}

/// Pump a public TCP connection over an iroh stream until either side closes.
/// Returns `(bytes_from_public, bytes_to_public)` for metrics.
pub async fn pump(socket: TcpStream, mut send: SendStream, mut recv: RecvStream) -> (u64, u64) {
    let (mut r, mut w) = socket.into_split();
    let up = async {
        let res = tokio::io::copy(&mut r, &mut send).await;
        let _ = send.finish();
        res
    };
    let down = async {
        let res = tokio::io::copy(&mut recv, &mut w).await;
        let _ = w.shutdown().await;
        res
    };
    let (up, down) = tokio::join!(up, down);
    (up.unwrap_or(0), down.unwrap_or(0))
}