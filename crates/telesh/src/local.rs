//! 数据面的每条 payload 流:接上本地服务并双向泵字节。
//!
//! yamux 流是纯字节管道 —— **没有前导、没有 ack**。本地拨号失败时直接丢弃该流
//! (yamux 发 RST),边缘据此回 502。

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::debug;

/// 本地拨号的超时。
const DIAL_TIMEOUT: Duration = Duration::from_secs(3);

/// 处理一条数据面流:拨本地服务,然后双向泵。
///
/// 流已经是 tokio I/O(目标头已由调用方读完)。
pub async fn handle_stream<S>(
    stream: S,
    local_addr: SocketAddr,
    section: Option<crate::publish::Section>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let stream_id = crate::inspect::next_stream_id();
    // 本地目标由端口扫描得出(127.0.0.1 或 ::1),不必再解析主机名。
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
    crate::inspect::start_stream(stream_id, local_addr, section);
    pump(conn, stream, stream_id).await;
    crate::inspect::finish_stream(stream_id);
}

/// 双向泵,直到任一侧结束。
async fn pump<S>(tcp: TcpStream, stream: S, stream_id: u64)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut tcp_r, mut tcp_w) = tcp.into_split();
    let (mut s_r, mut s_w) = tokio::io::split(stream);

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

