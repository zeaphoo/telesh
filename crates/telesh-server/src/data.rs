//! 数据面:agent 主动连入的 TCP 连接,内部用 yamux 分流。
//!
//! 见 `docs/design-dataplane.md`。要点:
//!
//! * agent 主动连入后立刻建立 yamux 会话(agent 是 `Mode::Client`);
//! * agent 打开的第一条 stream 是**信令流**,先用登录 session 鉴权,之后保活;
//! * 每条公网连接由服务端在这个会话上开一条新 stream,流里是纯字节;
//! * 拒绝(本地拨号失败)由 agent reset 该流表达,边缘据此回 502。

use std::future::Future as _;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::Poll;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::Framed;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tracing::{debug, info, warn};
use yamux::{Config as MuxConfig, Connection as MuxConnection, Mode};

use telesh_protocol::signal::{AuthStatus, SignalCodec, SignalFrame, SignalMessage};
use telesh_protocol::SignalAuthResponse;

use crate::state::registry;

/// 一条数据连接上的流上限(yamux 的 `max_num_streams` 统计入站+出站)。
const MAX_STREAMS: usize = 4096;
/// yamux 要求连接级接收窗口 `>= 256 KiB * max_num_streams`。
const MAX_CONNECTION_WINDOW: usize = MAX_STREAMS * 256 * 1024;
/// 一条流最多占用多少线路长度后 yamux 才去穿插别的流。
const SPLIT_SEND_SIZE: usize = 16 * 1024;
/// 信令流完成鉴权的超时;超时即关闭整条数据连接。
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
/// 信令流保活间隔。
const KEEPALIVE: Duration = Duration::from_secs(30);
/// 排队等待开流的公网连接上限;超出直接拒绝,不做无界排队。
const MAX_PENDING_OPENS: usize = 1024;

static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);

/// 交给 ingress 的句柄:在这条数据连接上开一条新流。
#[derive(Debug, Clone)]
pub struct DataHandle {
    cmd: mpsc::Sender<DataCmd>,
}

enum DataCmd {
    Open(oneshot::Sender<std::result::Result<yamux::Stream, String>>),
    Shutdown,
}

impl DataHandle {
    /// 开一条新流,并写入流首目标头(`[u16 BE len][target]`,与 iroh 前导同一编码)。
    ///
    /// 返回的流已经是 tokio I/O;调用方负责按 yamux 的语义使用(EOF 时显式半关闭)。
    pub async fn open_stream(
        &self,
        target: &str,
    ) -> Result<tokio_util::compat::Compat<yamux::Stream>> {
        let (reply, rx) = oneshot::channel();
        self.cmd
            .send(DataCmd::Open(reply))
            .await
            .map_err(|_| anyhow::anyhow!("data connection is gone"))?;
        let stream = rx
            .await
            .map_err(|_| anyhow::anyhow!("data connection dropped the open request"))?
            .map_err(|e| anyhow::anyhow!(e))?;
        let mut stream = stream.compat();
        telesh_protocol::write_service(&mut stream, target)
            .await
            .context("failed to write the stream target header")?;
        Ok(stream)
    }

    /// 请求驱动任务收尾(注册表在控制连接断开时调用)。
    pub fn shutdown(&self) {
        let _ = self.cmd.try_send(DataCmd::Shutdown);
    }
}

/// 接受数据连接,直到进程结束。
pub async fn run(listener: TcpListener) -> Result<()> {
    let addr = listener.local_addr()?;
    info!(%addr, "data plane listening (yamux over TCP)");
    loop {
        let (socket, peer) = listener.accept().await?;
        tokio::spawn(async move {
            if let Err(e) = handle_connection(socket, peer).await {
                debug!(%peer, "data connection ended: {e:#}");
            }
        });
    }
}

async fn handle_connection(socket: TcpStream, peer: SocketAddr) -> Result<()> {
    let _ = socket.set_nodelay(true);
    let mut cfg = MuxConfig::default();
    cfg.set_max_num_streams(MAX_STREAMS)
        .set_max_connection_receive_window(Some(MAX_CONNECTION_WINDOW))
        .set_split_send_size(SPLIT_SEND_SIZE);
    let conn = MuxConnection::new(socket.compat(), cfg, Mode::Server);
    let (tx, rx) = mpsc::channel(8);
    drive(conn, DataHandle { cmd: tx }, rx, peer).await
}

/// 驱动任务的一步。
enum Step {
    /// agent 开来一条流(只有第一条合法:信令流)。
    Inbound(yamux::Stream),
    /// 有一条开流请求被满足。
    Open(yamux::Stream, oneshot::Sender<std::result::Result<yamux::Stream, String>>),
    /// 连接结束。
    Closed,
}

/// 拥有 yamux `Connection` 的任务:驱动 I/O、接受信令流、按需开流。
///
/// 待处理的开流请求和连接 I/O 都需要 `&mut Connection`,所以不能用两个分支的
/// `select!`(会同时借用),只能放在同一个 `poll_fn` 里。
async fn drive(
    mut conn: MuxConnection<impl futures::io::AsyncRead + futures::io::AsyncWrite + Unpin>,
    handle: DataHandle,
    mut rx: mpsc::Receiver<DataCmd>,
    peer: SocketAddr,
) -> Result<()> {
    let run_id = NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed);
    let mut pending: std::collections::VecDeque<
        oneshot::Sender<std::result::Result<yamux::Stream, String>>,
    > = std::collections::VecDeque::new();
    let mut signal: Option<tokio::task::JoinHandle<()>> = None;
    let mut signal_open = false;
    let mut auth_timer = Box::pin(tokio::time::sleep(AUTH_TIMEOUT));

    loop {
        let step = std::future::poll_fn(|cx| {
            // 1) 取出排队的命令
            loop {
                match rx.poll_recv(cx) {
                    Poll::Ready(Some(DataCmd::Shutdown)) | Poll::Ready(None) => {
                        return Poll::Ready(Step::Closed);
                    }
                    Poll::Ready(Some(DataCmd::Open(reply))) => {
                        if pending.len() >= MAX_PENDING_OPENS {
                            let _ = reply.send(Err("too many pending stream opens".into()));
                        } else {
                            pending.push_back(reply);
                        }
                    }
                    Poll::Pending => break,
                }
            }
            // 2) 尽量满足排队的开流请求:每轮返回一条,driver 会立刻再进来处理下一条。
            while let Some(reply) = pending.pop_front() {
                match conn.poll_new_outbound(cx) {
                    Poll::Ready(Ok(stream)) => return Poll::Ready(Step::Open(stream, reply)),
                    Poll::Ready(Err(e)) => {
                        let _ = reply.send(Err(e.to_string()));
                    }
                    Poll::Pending => {
                        pending.push_front(reply);
                        break;
                    }
                }
            }
            // 3) 鉴权期限(未鉴权的连接不允许一直挂着)
            if !signal_open && auth_timer.as_mut().poll(cx).is_ready() {
                debug!(%peer, "no signalling stream within {AUTH_TIMEOUT:?}, closing");
                return Poll::Ready(Step::Closed);
            }
            // 4) 驱动 I/O 并接收入站流
            match conn.poll_next_inbound(cx) {
                Poll::Ready(Some(Ok(stream))) => Poll::Ready(Step::Inbound(stream)),
                Poll::Ready(Some(Err(e))) => {
                    debug!(%peer, "yamux error: {e}");
                    Poll::Ready(Step::Closed)
                }
                Poll::Ready(None) => Poll::Ready(Step::Closed),
                Poll::Pending => Poll::Pending,
            }
        })
        .await;

        match step {
            Step::Inbound(stream) => {
                if signal_open {
                    warn!(%peer, "agent opened a second stream; closing data connection");
                    break;
                }
                signal_open = true;
                signal = Some(tokio::spawn(serve_signal(
                    stream,
                    handle.clone(),
                    peer,
                    run_id,
                )));
            }
            Step::Open(stream, reply) => {
                let _ = reply.send(Ok(stream));
            }
            Step::Closed => break,
        }
    }

    if let Some(task) = signal {
        task.abort();
    }
    registry().remove_data_run(run_id);
    debug!(%peer, run_id, "data connection closed");
    Ok(())
}

/// 信令流:先鉴权,然后保活;退出时请求 driver 收尾。
async fn serve_signal(stream: yamux::Stream, handle: DataHandle, peer: SocketAddr, run_id: u64) {
    let mut framed = Framed::new(stream.compat(), SignalCodec::new());

    // ---- 鉴权 ----
    let auth = match tokio::time::timeout(AUTH_TIMEOUT, framed.next()).await {
        Ok(Some(Ok(SignalMessage::Known(SignalFrame::Auth(auth))))) => auth,
        Ok(other) => {
            debug!(%peer, "expected auth as the first signalling frame, got {other:?}");
            let _ = framed
                .send(SignalFrame::AuthResp(SignalAuthResponse {
                    status: AuthStatus::BadSession,
                }))
                .await;
            return;
        }
        Err(_) => {
            debug!(%peer, "auth frame timed out");
            return;
        }
    };
    let account = match crate::db::session_account(crate::state::db(), &auth.session).await {
        Ok(Some(account)) => account,
        Ok(None) => {
            debug!(%peer, "data connection presented an invalid session");
            let _ = framed
                .send(SignalFrame::AuthResp(SignalAuthResponse {
                    status: AuthStatus::BadSession,
                }))
                .await;
            return;
        }
        Err(e) => {
            warn!(%peer, "session lookup failed: {e:#}");
            let _ = framed
                .send(SignalFrame::AuthResp(SignalAuthResponse {
                    status: AuthStatus::Internal,
                }))
                .await;
            return;
        }
    };
    let agent_id = crate::db::account_handle(account);
    if registry().data_run(&agent_id).is_some() {
        // 本期每个 agent 只允许一条数据连接。
        debug!(%agent_id, "agent already has a data connection, refusing another");
        let _ = framed
            .send(SignalFrame::AuthResp(SignalAuthResponse {
                status: AuthStatus::TooManyConnections,
            }))
            .await;
        return;
    }
    if framed
        .send(SignalFrame::AuthResp(SignalAuthResponse {
            status: AuthStatus::Ok,
        }))
        .await
        .is_err()
    {
        return;
    }
    registry().set_data(&agent_id, run_id, handle.clone());
    info!(%agent_id, %peer, max_streams = auth.agent_max_streams, "data connection established");

    // ---- 保活 ----
    let mut tick = tokio::time::interval(KEEPALIVE);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await; // 立即的第一次 tick 丢掉
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
                    debug!(%agent_id, "signalling stream error: {e}");
                    break;
                }
                None => break,
            },
        }
    }

    info!(%agent_id, "data connection signalling ended");
    handle.shutdown();
}
