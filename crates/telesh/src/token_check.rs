//! On-line validation of opaque data-plane tokens.
//!
//! The server is the only holder of the token store, so the agent asks it over
//! the control connection instead of verifying a signature locally. There is no
//! server signing key.
//!
//! A data-plane stream calls [`check`], which parks on a oneshot until the
//! control loop routes the matching [`Frame::TokenCheckResp`] to [`resolve`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio::time::timeout;
use tracing::debug;

use telesh_protocol::{Frame, TokenCheck};

/// How long to wait for the server's verdict before refusing the stream.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

static CONTROL: OnceLock<Mutex<Option<UnboundedSender<Frame>>>> = OnceLock::new();
static PENDING: OnceLock<DashMap<u64, oneshot::Sender<Option<u64>>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn control() -> &'static Mutex<Option<UnboundedSender<Frame>>> {
    CONTROL.get_or_init(|| Mutex::new(None))
}

fn pending() -> &'static DashMap<u64, oneshot::Sender<Option<u64>>> {
    PENDING.get_or_init(DashMap::new)
}

/// Point the validator at the current control connection (one per session).
pub fn set_control(tx: UnboundedSender<Frame>) {
    *control().lock().unwrap() = Some(tx);
}

/// Deliver a server verdict (token expiry when granted) to a waiting task.
pub fn resolve(id: u64, grant: Option<u64>) {
    if let Some((_, tx)) = pending().remove(&id) {
        let _ = tx.send(grant);
    }
}

/// Ask the server whether `token` is a live grant for `account/name`.
///
/// Returns the grant's expiry (unix seconds) so the caller can cache it for the
/// rest of the connection, or `None` if it was refused.
pub async fn check(account: &str, name: &str, token: Vec<u8>) -> Option<u64> {
    let sender = match control().lock().unwrap().clone() {
        Some(tx) => tx,
        None => return None,
    };
    debug!(%account, %name, "asking server to validate access token");
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    pending().insert(id, tx);
    let frame = Frame::TokenCheck(TokenCheck {
        id,
        account: account.to_string(),
        name: name.to_string(),
        token,
    });
    if sender.send(frame).is_err() {
        pending().remove(&id);
        return None;
    }
    match timeout(CHECK_TIMEOUT, rx).await {
        Ok(Ok(grant)) => grant,
        _ => {
            pending().remove(&id);
            None
        }
    }
}
