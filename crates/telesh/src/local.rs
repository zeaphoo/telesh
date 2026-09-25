//! 数据面的每条 payload 流:接上本地服务并双向泵字节。
//!
//! yamux 流是纯字节管道 —— **没有前导、没有 ack**。本地拨号失败时直接丢弃该流
//! (yamux 发 RST),边缘据此回 502。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Context;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::debug;

/// 本地拨号的超时。
const DIAL_TIMEOUT: Duration = Duration::from_secs(3);

/// 给 inspector 用的本地递增 id(yamux 不暴露可复用的 u64)。
static NEXT_LOCAL_STREAM: AtomicU64 = AtomicU64::new(1);

/// 处理一条数据面流:拨本地服务,然后双向泵。
pub async fn handle_stream(stream: yamux::Stream, local_addr: SocketAddr) {
    let stream_id = NEXT_LOCAL_STREAM.fetch_add(1, Ordering::Relaxed);
    let conn = match timeout(DIAL_TIMEOUT, TcpStream::connect(local_addr)).await {
        Ok(Ok(conn)) => conn,
        Ok(Err(e)) => {
            debug!(stream_id, %local_addr, "local dial failed: {e}");
            return; // 直接 drop = RST,边缘会回 502
        }
        Err(_) => {
            debug!(stream_id, %local_addr, "local dial timed out");
            return;
        }
    };
    debug!(stream_id, %local_addr, "data-plane stream accepted, pumping");
    pump(conn, stream, stream_id).await;
    crate::inspect::finish_stream(stream_id);
}

/// 双向泵,直到任一侧结束。
async fn pump(tcp: TcpStream, stream: yamux::Stream, stream_id: u64) {
    let (mut tcp_r, mut tcp_w) = tcp.into_split();
    let (mut s_r, mut s_w) = tokio::io::split(stream.compat());

    // 本地服务 -> 边缘
    let up = async {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match tcp_r.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    crate::inspect::observe_response(stream_id, &buf[..n]);
                    if s_w.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    debug!(stream_id, "local read error: {e}");
                    break;
                }
            }
        }
        // 显式半关闭(FIN);直接 drop 会发 RST 并可能丢数据。
        let _ = s_w.shutdown().await;
    };

    // 边缘 -> 本地服务
    let down = async {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match s_r.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    crate::inspect::observe_request(stream_id, &buf[..n]);
                    if tcp_w.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    debug!(stream_id, "data stream read error: {e}");
                    break;
                }
            }
        }
        let _ = tcp_w.shutdown().await;
    };

    tokio::join!(up, down);
}

/// Resolve `host:port` (host may be a hostname) to a socket address.
pub async fn resolve_addr(host: &str, port: u16) -> anyhow::Result<SocketAddr> {
    let mut addrs = tokio::net::lookup_host((host, port))
        .await
        .with_context(|| format!("failed to resolve local address {host}:{port}"))?;
    addrs
        .next()
        .ok_or_else(|| anyhow::anyhow!("no address resolved for local host {host}:{port}"))
}
