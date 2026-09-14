//! Local stream session: dial the local service and pump bytes in both
//! directions, multiplexed over the agent's control connection.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Context;
use dashmap::DashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::time::timeout;
use tracing::debug;

use telesh_protocol::{Frame, StreamPacket};

/// Active local streams: stream_id -> channel used by the main loop to
/// forward `Data` frames from the server to this session's writer.
pub static STREAMS: std::sync::OnceLock<DashMap<u64, UnboundedSender<Vec<u8>>>> =
    std::sync::OnceLock::new();

pub fn streams() -> &'static DashMap<u64, UnboundedSender<Vec<u8>>> {
    STREAMS.get_or_init(DashMap::new)
}

const DIAL_TIMEOUT: Duration = Duration::from_secs(3);

/// Handle a `StreamPacket::Init`: dial the local service and run the pump.
/// `server_tx` lets this task send Data/End/Refused frames back to the edge.
///
/// The `rx` channel is created and registered by the caller (the agent's
/// main loop) *synchronously* when `Init` arrives, so the very next `Data`
/// frame for this stream can never be dropped while we dial. A failed dial
/// removes the placeholder and sends `Refused`.
pub async fn handle_stream(
    stream_id: u64,
    server_tx: UnboundedSender<Frame>,
    local_addr: SocketAddr,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let conn = match timeout(DIAL_TIMEOUT, TcpStream::connect(local_addr)).await {
        Ok(Ok(conn)) => conn,
        Ok(Err(e)) => {
            debug!(stream_id, %local_addr, "local dial failed: {e}");
            streams().remove(&stream_id);
            let _ = server_tx.send(Frame::stream(StreamPacket::Refused { stream_id }));
            return;
        }
        Err(_) => {
            debug!(stream_id, %local_addr, "local dial timed out");
            streams().remove(&stream_id);
            let _ = server_tx.send(Frame::stream(StreamPacket::Refused { stream_id }));
            return;
        }
    };

    debug!(stream_id, %local_addr, "stream accepted, pumping");
    let (mut local_r, mut local_w) = conn.into_split();

    let mut buf = vec![0u8; 8192];
    loop {
        tokio::select! {
            // Local service -> edge (Data frames).
            read_res = local_r.read(&mut buf) => {
                match read_res {
                    Ok(0) => break, // local closed
                    Ok(n) => {
                        crate::inspect::observe_response(stream_id, &buf[..n]);
                        let data = buf[..n].to_vec();
                        if server_tx.send(Frame::stream(StreamPacket::Data { stream_id, data })).is_err() {
                            debug!(stream_id, "server channel closed, stopping stream");
                            break;
                        }
                    }
                    Err(e) => {
                        debug!(stream_id, "local read error: {e}");
                        break;
                    }
                }
            }
            // Edge -> local service.
            data = rx.recv() => {
                match data {
                    Some(d) => {
                        crate::inspect::observe_request(stream_id, &d);
                        if local_w.write_all(&d).await.is_err() {
                            break;
                        }
                    }
                    None => break, // stream closed by the server
                }
            }
        }
    }

    crate::inspect::finish_stream(stream_id);
    streams().remove(&stream_id);
    let _ = local_w.shutdown().await;
    let _ = server_tx.send(Frame::stream(StreamPacket::End { stream_id }));
    debug!(stream_id, "stream closed");
}

/// Resolve `host:port` (host may be a hostname) to a socket address.
pub async fn resolve_addr(host: &str, port: u16) -> anyhow::Result<SocketAddr> {
    let mut addrs = tokio::net::lookup_host((host, port)).await.with_context(|| {
        format!("failed to resolve local address {host}:{port}")
    })?;
    addrs.next().ok_or_else(|| {
        anyhow::anyhow!("no address resolved for local host {host}:{port}")
    })
}