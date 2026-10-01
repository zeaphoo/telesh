//! 数据面客户端:主动连上服务端的数据端口,在里面用 yamux 分流。
//!
//! 见 `docs/design-dataplane.md`。agent 是 yamux 的 `Mode::Client`:先开第一条
//! stream 做鉴权(信令流),之后只接受服务端为每条公网连接开来的 payload 流。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio_util::codec::Framed;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tracing::{debug, info, warn};
use yamux::{Config as MuxConfig, Connection as MuxConnection, Mode};

use telesh_protocol::signal::{AuthStatus, SignalCodec, SignalFrame, SignalMessage};
use telesh_protocol::{AuthRequest, SignalAuthResponse};

/// 本端 yamux 的流上限,随鉴权消息上报,服务端据此钳制开流数量。
const MAX_STREAMS: u32 = 4096;
/// yamux 要求连接级接收窗口 `>= 256 KiB * max_num_streams`。
const MAX_CONNECTION_WINDOW: usize = MAX_STREAMS as usize * 256 * 1024;
const SPLIT_SEND_SIZE: usize = 16 * 1024;
/// 信令流保活间隔。
const KEEPALIVE: Duration = Duration::from_secs(30);
const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(30);

/// 数据面客户端参数。
#[derive(Debug, Clone)]
pub struct DataOptions {
    /// 服务端的数据端口。
    pub addr: SocketAddr,
    /// 登录拿到的会话令牌(数据连接复用它鉴权)。
    pub session: Vec<u8>,
    /// 目标名 → 本地地址(与控制面下发的 target 对应)。
    pub targets: Arc<DashMap<String, SocketAddr>>,
}

/// 解析服务端下发的数据地址。
///
/// 下发地址的主机是 `0.0.0.0` / `::` 时,用控制连接实际使用的主机替换 ——
/// 服务端只知道自己 bind 的地址,agent 才知道自己是从哪个主机连过去的。
pub async fn resolve_addr(announced: &str, control_addr: &str) -> Option<SocketAddr> {
    let announced: SocketAddr = announced.parse().ok()?;
    if !announced.ip().is_unspecified() {
        return Some(announced);
    }
    let host = control_addr
        .rsplit_once(':')
        .map_or(control_addr, |(h, _)| h);
    tokio::net::lookup_host((host, announced.port()))
        .await
        .ok()?
        .next()
}

/// 持续维持一条数据连接(断了就退避重连);调用方在会话结束时 abort 它。
pub async fn run(opts: DataOptions) {
    let mut backoff = RECONNECT_MIN;
    loop {
        match session(&opts).await {
            Ok(()) => backoff = RECONNECT_MIN,
            Err(e) => debug!("data connection ended: {e:#}"),
        }
        warn!(addr = %opts.addr, ms = backoff.as_millis(), "reconnecting the data plane");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

async fn session(opts: &DataOptions) -> Result<()> {
    let sock = TcpStream::connect(opts.addr)
        .await
        .with_context(|| format!("cannot reach the data port {}", opts.addr))?;
    let _ = sock.set_nodelay(true);

    let mut cfg = MuxConfig::default();
    cfg.set_max_num_streams(MAX_STREAMS as usize)
        .set_max_connection_receive_window(Some(MAX_CONNECTION_WINDOW))
        .set_split_send_size(SPLIT_SEND_SIZE);
    let mut conn = MuxConnection::new(sock.compat(), cfg, Mode::Client);

    // stream 1:信令流(鉴权 + 保活)。
    let signal = std::future::poll_fn(|cx| conn.poll_new_outbound(cx))
        .await
        .context("cannot open the signalling stream")?;
    let (done_tx, mut done_rx) = oneshot::channel::<()>();
    tokio::spawn(serve_signal(signal, opts.session.clone(), done_tx));

    // 服务端为每条公网连接开一条流;每条流接上本地目标。
    loop {
        tokio::select! {
            _ = &mut done_rx => {
                debug!("signalling stream ended, dropping the data connection");
                break;
            }
            incoming = std::future::poll_fn(|cx| conn.poll_next_inbound(cx)) => match incoming {
                Some(Ok(stream)) => {
                    let targets = opts.targets.clone();
                    tokio::spawn(async move {
                        // 流首是目标头([u16 BE len][target],与 iroh 前导同一编码)。
                        let mut stream = stream.compat();
                        let target = match telesh_protocol::read_service(&mut stream).await {
                            Ok(t) => t,
                            Err(e) => {
                                debug!("cannot read the stream target: {e}");
                                return;
                            }
                        };
                        // 目标 key 是 `http:<port>` / `tcp:<port>`。只有 http 隧道
                        // 够格被追踪;tcp 传 None。
                        let section = target
                            .starts_with("http:")
                            .then_some(crate::publish::Section::Publish);
                        match targets.get(&target).map(|e| *e.value()) {
                            Some(local) => {
                                crate::local::handle_stream(stream, local, section).await
                            }
                            None => debug!(%target, "unknown stream target"),
                        }
                    });
                }
                Some(Err(e)) => {
                    debug!("yamux error: {e}");
                    break;
                }
                None => break,
            },
        }
    }
    Ok(())
}

/// 信令流:鉴权,然后保活;退出时通知 accept 循环。
async fn serve_signal(stream: yamux::Stream, session: Vec<u8>, done: oneshot::Sender<()>) {
    let mut framed = Framed::new(stream.compat(), SignalCodec::new());
    if framed
        .send(SignalFrame::Auth(AuthRequest {
            session,
            agent_max_streams: MAX_STREAMS,
        }))
        .await
        .is_err()
    {
        return;
    }
    match framed.next().await {
        Some(Ok(SignalMessage::Known(SignalFrame::AuthResp(SignalAuthResponse {
            status: AuthStatus::Ok,
        })))) => info!("data connection authenticated"),
        other => {
            warn!("data connection rejected: {other:?}");
            return;
        }
    }

    let mut tick = tokio::time::interval(KEEPALIVE);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await; // 丢掉立即的第一次 tick
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if framed.send(SignalFrame::Ping).await.is_err() {
                    break;
                }
            }
            msg = framed.next() => match msg {
                Some(Ok(SignalMessage::Known(SignalFrame::Ping))) => {
                    if framed.send(SignalFrame::Pong).await.is_err() {
                        break;
                    }
                }
                // 已知但不关心 / 未知类型:忽略,不断连。
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    debug!("signalling stream error: {e}");
                    break;
                }
                None => break,
            },
        }
    }
    let _ = done.send(());
}
