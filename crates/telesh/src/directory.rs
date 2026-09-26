//! 目录查询:向控制服务器要「某个账户当前提供哪些服务」。
//!
//! 与 [`crate::token_check`] 同一套路 —— 在控制连接上发一条带 id 的请求,等专用任务
//! 把 [`Frame::P2PListResp`] 路由回来。本地 UI 的「连接远程端口」用它列出可选服务。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio::time::timeout;
use tracing::debug;

use telesh_protocol::{Frame, P2PList};

/// 等服务端答复的上限。
const LIST_TIMEOUT: Duration = Duration::from_secs(8);

static CONTROL: OnceLock<Mutex<Option<UnboundedSender<Frame>>>> = OnceLock::new();
static PENDING: OnceLock<DashMap<u64, oneshot::Sender<Vec<String>>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn control() -> &'static Mutex<Option<UnboundedSender<Frame>>> {
    CONTROL.get_or_init(|| Mutex::new(None))
}

fn pending() -> &'static DashMap<u64, oneshot::Sender<Vec<String>>> {
    PENDING.get_or_init(DashMap::new)
}

/// 指向当前控制连接(每个会话一次)。
pub fn set_control(tx: UnboundedSender<Frame>) {
    *control().lock().unwrap() = Some(tx);
}

/// 把服务端的结果交给等待中的任务。
pub fn resolve(id: u64, names: Vec<String>) {
    if let Some((_, tx)) = pending().remove(&id) {
        let _ = tx.send(names);
    }
}

/// 查询 `account` 当前提供的服务名。未连接 / 超时 / 出错都返回空列表。
pub async fn list(account: &str) -> Vec<String> {
    let sender = match control().lock().unwrap().clone() {
        Some(tx) => tx,
        None => return Vec::new(),
    };
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    pending().insert(id, tx);
    debug!(account, "asking the server for this account's services");
    let frame = Frame::P2PList(P2PList {
        id,
        account: account.to_string(),
    });
    if sender.send(frame).is_err() {
        pending().remove(&id);
        return Vec::new();
    }
    match timeout(LIST_TIMEOUT, rx).await {
        Ok(Ok(names)) => names,
        _ => {
            pending().remove(&id);
            Vec::new()
        }
    }
}
