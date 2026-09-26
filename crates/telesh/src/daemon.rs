//! 常驻 agent:一条控制连接 + 一条数据连接,按本地 UI 的发布配置 reconcile。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, info, warn};

use telesh_protocol::{
    decode, encode, Frame, P2PAnnounce, P2PAnnounceResp, TunnelClose, TunnelKind, TunnelPublish,
    TunnelResp, TunnelResult,
};

use crate::publish::{self, Entry, EntryStatus, Shared, State};

/// daemon 需要的身份与会话材料。
#[derive(Debug, Clone)]
pub struct Credentials {
    pub control_addr: String,
    pub secret_key: [u8; 32],
    pub session: Option<Vec<u8>>,
    pub node_name: Option<String>,
    pub state_path: PathBuf,
}

/// 一条已提交给服务端(或已在本地运行)的条目。
struct Applied {
    entry: Entry,
    /// 发布请求的关联 id(服务端回复按它配对);mirror 没有。
    id: Option<u64>,
    /// 服务端分配的标识(subdomain / 公网端口 / 服务名),用于取消。
    key: Option<String>,
    /// 目标表的 key(`<port>` 或 `<account>/<name>`)。
    target_key: String,
    state: State,
    detail: Option<String>,
    /// mirror 的本地任务。
    task: Option<tokio::task::JoinHandle<()>>,
}

/// 常驻运行:加载配置 → 会话 → 断线退避重连。
pub async fn run(creds: Credentials) -> Result<()> {
    let shared = publish::shared();
    set_control_addr(&creds.control_addr);
    match publish::load(&creds.state_path) {
        Ok(entries) if !entries.is_empty() => {
            info!(count = entries.len(), "loaded the publish configuration");
            shared.set_desired(entries);
        }
        Ok(_) => {}
        Err(e) => warn!("cannot load the publish configuration: {e:#}"),
    }

    let mut backoff = Duration::from_secs(1);
    loop {
        shared.mark_all_pending();
        crate::account::set_connecting();
        let ended = match session(&creds, shared).await {
            Ok(()) => {
                backoff = Duration::from_secs(1);
                "control connection closed".to_string()
            }
            Err(e) => {
                let msg = format!("{e:#}");
                warn!("agent session ended: {msg}");
                msg
            }
        };
        crate::account::set_offline(ended);
        shared.mark_all_pending();
        warn!(ms = backoff.as_millis(), "reconnecting to the control server");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

async fn session(creds: &Credentials, shared: &Arc<Shared>) -> Result<()> {
    let url = format!("ws://{}/api/tunnel", creds.control_addr);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .with_context(|| format!("failed to connect to control server at {url}"))?;

    let (login, hello) = crate::auth::login(
        &mut ws,
        &creds.secret_key,
        creds.session.clone(),
        telesh_protocol::ClientKind::Agent,
        creds.node_name.clone(),
    )
    .await?;
    info!(account = %login.account, name = %login.display_name, "logged in");
    crate::account::set_online(
        login.account.clone(),
        login.display_name.clone(),
        login.expires_at,
    );
    if let Err(e) = crate::state::save_session(&creds.state_path, &login.session, login.expires_at) {
        warn!("cannot persist session: {e:#}");
    }
    let account = login.account.clone();
    // 镜像那条 lookup 连接复用本次登录的身份与 session,不再需要单独密钥。
    let peer_auth = crate::p2p::PeerAuth {
        secret_key: creds.secret_key,
        session: Some(login.session.clone()),
        display_name: Some(login.display_name.clone()),
    };

    shared.set_ports(crate::ports::scan_and_probe().await);

    // ---- 出站队列(数据面校验令牌也用它) ----
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    let sink_task = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            match encode(&frame) {
                Ok(text) => {
                    if sink.send(WsMessage::Text(text)).await.is_err() {
                        break;
                    }
                }
                Err(e) => debug!("frame encode failed: {e}"),
            }
        }
    });
    crate::token_check::set_control(tx.clone());

    // ---- 数据面(agent 主动连入) ----
    let data_task = match hello.data_addr() {
        Some(announced) => match crate::data::resolve_addr(announced, &creds.control_addr).await {
            Some(addr) => {
                info!(%addr, "data-plane client starting");
                Some(tokio::spawn(crate::data::run(crate::data::DataOptions {
                    addr,
                    session: login.session.clone(),
                    targets: shared.targets.clone(),
                })))
            }
            None => {
                warn!(announced, "cannot resolve the announced data address");
                None
            }
        },
        None => {
            info!("server announced no data address; http/tcp tunnels cannot be routed");
            None
        }
    };

    // ---- iroh(P2P 对等连接) ----
    let endpoint = match crate::p2p::build_endpoint(
        iroh::SecretKey::from_bytes(&creds.secret_key),
        hello.relay_url(),

    )
    .await
    {
        Ok(e) => {
            info!(endpoint = %e.addr().id.fmt_short(), "iroh endpoint ready");
            Some(e)
        }
        Err(e) => {
            warn!("iroh endpoint unavailable, data plane only: {e:#}");
            None
        }
    };
    let serve = endpoint.as_ref().map(|e| {
        tokio::spawn(crate::p2p::serve_endpoint(
            e.clone(),
            shared.targets.clone(),
            Some(account.clone()),
        ))
    });
    // iroh 地址会在拿到 home relay 后变化:变了要**重新 announce 所有 P2P 服务**,
    // 否则目录里留着旧地址(对端连不上)。
    let mut last_addr = endpoint.as_ref().map(|e| e.addr());

    // ---- reconcile ----
    let mut applied: Vec<Applied> = Vec::new();
    let mut next_id: u64 = 1;
    let mut tick = tokio::time::interval(Duration::from_secs(3));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let result = loop {
        tokio::select! {
            _ = shared.notify.notified() => {}
            _ = tick.tick() => {
                shared.set_ports(crate::ports::scan_and_probe().await);
                // P2P 流量服务端看不到,agent 定期上报。
                for t in crate::p2p::drain_traffic() {
                    let _ = tx.send(Frame::Traffic(t));
                }
                // iroh 地址变化 → 重新 announce P2P 服务。
                if let Some(ep) = &endpoint {
                    let addr = ep.addr();
                    if last_addr.as_ref() != Some(&addr) {
                        debug!(endpoint = %addr.id.fmt_short(), "iroh address changed, re-announcing");
                        last_addr = Some(addr.clone());
                        for a in applied.iter_mut() {
                            if let Entry::P2p { local_port, name } = &a.entry {
                                a.state = State::Pending;
                                let _ = tx.send(Frame::P2PAnnounce(P2PAnnounce {
                                    name: name.clone(),
                                    addr: addr.clone(),
                                    local_port: *local_port,
                                }));
                            }
                        }
                    }
                }
            }
            msg = stream.next() => match msg {
                Some(Ok(WsMessage::Text(text))) => match decode(&text) {
                    Ok(frame) => handle_frame(frame, &mut applied, &tx),
                    Err(e) => debug!("ignoring bad frame from server: {e}"),
                },
                Some(Ok(WsMessage::Close(_))) => break Ok(()),
                Some(Ok(_)) => {}
                Some(Err(e)) => break Err(anyhow::anyhow!("control connection error: {e}")),
                None => break Ok(()),
            },
        }
        reconcile(&mut applied, &tx, shared, &endpoint, &mut next_id, &peer_auth).await;
        publish_status(&applied, shared);
    };

    if let Some(t) = serve {
        t.abort();
    }
    if let Some(t) = data_task {
        t.abort();
    }
    for a in applied.iter_mut() {
        if let Some(t) = a.task.take() {
            t.abort();
        }
    }
    sink_task.abort();
    result
}

/// 处理服务端发来的一条帧。
fn handle_frame(frame: Frame, applied: &mut [Applied], tx: &mpsc::UnboundedSender<Frame>) {
    match frame {
        Frame::Ping => {
            let _ = tx.send(Frame::Pong);
        }
        Frame::Pong => {}
        Frame::TokenCheckResp(r) => {
            crate::token_check::resolve(r.id, if r.ok { Some(r.expires_at) } else { None })
        }
        Frame::TunnelResp(resp) => apply_tunnel_resp(resp, applied),
        Frame::P2PAnnounceResp(resp) => apply_p2p_resp(resp, applied),
        _ => debug!("ignoring unexpected frame from server"),
    }
}

fn apply_tunnel_resp(resp: TunnelResp, applied: &mut [Applied]) {
    let Some(a) = applied.iter_mut().find(|a| a.id == Some(resp.id)) else {
        debug!(id = resp.id, "response for an unknown publish request");
        return;
    };
    match resp.result {
        TunnelResult::Http { sub_domain, hostname } => {
            info!(sub_domain = %sub_domain, hostname = %hostname, "http tunnel live");
            a.key = Some(sub_domain);
            a.state = State::Live;
            a.detail = Some(hostname);
        }
        TunnelResult::Tcp { port } => {
            info!(port, "tcp tunnel live");
            a.key = Some(port.to_string());
            a.state = State::Live;
            a.detail = Some(format!("公网端口 {port}"));
        }
        TunnelResult::NameTaken => {
            a.state = State::Error;
            a.detail = Some("名字或端口已被占用".into());
        }
        TunnelResult::Invalid => {
            a.state = State::Error;
            a.detail = Some("名字或端口不合法".into());
        }
        TunnelResult::Error(e) => {
            a.state = State::Error;
            a.detail = Some(e);
        }
    }
}

fn apply_p2p_resp(resp: P2PAnnounceResp, applied: &mut [Applied]) {
    match resp {
        P2PAnnounceResp::Ok { name } => {
            if let Some(a) = applied.iter_mut().find(|a| a.key.as_deref() == Some(&name)) {
                a.state = State::Live;
                a.detail = Some(name);
            }
        }
        P2PAnnounceResp::NameTaken { name } => {
            if let Some(a) = applied.iter_mut().find(|a| a.key.as_deref() == Some(&name)) {
                a.state = State::Error;
                a.detail = Some("服务名已被占用".into());
            }
        }
        P2PAnnounceResp::InvalidName { name } => {
            if let Some(a) = applied.iter_mut().find(|a| a.key.as_deref() == Some(&name)) {
                a.state = State::Error;
                a.detail = Some("服务名不合法".into());
            }
        }
    }
}

/// 让实际状态收敛到配置:先撤销不再需要的,再发布新的。
async fn reconcile(
    applied: &mut Vec<Applied>,
    tx: &mpsc::UnboundedSender<Frame>,
    shared: &Arc<Shared>,
    endpoint: &Option<iroh::Endpoint>,
    next_id: &mut u64,
    peer_auth: &crate::p2p::PeerAuth,
) {
    let desired = shared.desired();

    // ---- 撤销 ----
    let mut i = 0;
    while i < applied.len() {
        if desired.iter().any(|d| d == &applied[i].entry) {
            i += 1;
            continue;
        }
        let a = applied.remove(i);
        if let Some(t) = a.task {
            t.abort();
        }
        if let Some(key) = a.key {
            let kind = match a.entry {
                Entry::Http { .. } => TunnelKind::Http,
                Entry::Tcp { .. } => TunnelKind::Tcp,
                Entry::P2p { .. } => TunnelKind::P2p,
                Entry::Mirror { .. } => continue,
            };
            info!(kind = ?kind, %key, "unpublishing tunnel");
            let _ = tx.send(Frame::TunnelClose(TunnelClose { kind, key }));
        }
    }

    // ---- 发布 ----
    for entry in &desired {
        if applied.iter().any(|a| &a.entry == entry) {
            continue;
        }
        let port = entry.local_port();
        let listening = shared.is_listening(port);
        let addr = shared.dial_addr(port);
        match entry {
            Entry::Http {
                local_port,
                sub_domain,
            } => {
                let target_key = local_port.to_string();
                let id = *next_id;
                *next_id += 1;
                let _ = tx.send(Frame::TunnelPublish(TunnelPublish {
                    id,
                    kind: TunnelKind::Http,
                    target: target_key.clone(),
                    name: sub_domain.clone(),
                    remote_port: None,
                }));
                applied.push(Applied {
                    entry: entry.clone(),
                    id: Some(id),
                    key: None,
                    target_key,
                    state: State::Pending,
                    detail: None,
                    task: None,
                });
            }
            Entry::Tcp {
                local_port,
                remote_port,
            } => {
                let target_key = local_port.to_string();
                let id = *next_id;
                *next_id += 1;
                let _ = tx.send(Frame::TunnelPublish(TunnelPublish {
                    id,
                    kind: TunnelKind::Tcp,
                    target: target_key.clone(),
                    name: None,
                    remote_port: *remote_port,
                }));
                applied.push(Applied {
                    entry: entry.clone(),
                    id: Some(id),
                    key: None,
                    target_key,
                    state: State::Pending,
                    detail: None,
                    task: None,
                });
            }
            Entry::P2p { local_port, name } => {
                // iroh 前导里的 service 是 `<account>/<name>`,但 agent 选目标用的是
                // 裸服务名(账号只用于归属校验),所以目标表的 key 就是 name。
                let target_key = name.clone();
                let Some(ep) = endpoint else {
                    applied.push(Applied {
                        entry: entry.clone(),
                        id: None,
                        key: Some(name.clone()),
                        target_key,
                        state: State::Error,
                        detail: Some("iroh 不可用".into()),
                        task: None,
                    });
                    continue;
                };
                let _ = tx.send(Frame::P2PAnnounce(P2PAnnounce {
                    name: name.clone(),
                    addr: ep.addr(),
                    local_port: *local_port,
                }));
                applied.push(Applied {
                    entry: entry.clone(),
                    id: None,
                    key: Some(name.clone()),
                    target_key,
                    state: State::Pending,
                    detail: None,
                    task: None,
                });
            }
            Entry::Mirror { local_port, remote } => {
                let (account_id, name) = match remote.split_once('/') {
                    Some((a, n)) if !a.is_empty() && !n.is_empty() => {
                        (a.to_string(), n.to_string())
                    }
                    _ => {
                        applied.push(Applied {
                            entry: entry.clone(),
                            id: None,
                            key: None,
                            target_key: String::new(),
                            state: State::Error,
                            detail: Some("目标必须是 ACCOUNT/NAME".into()),
                            task: None,
                        });
                        continue;
                    }
                };
                let opts = crate::p2p::ConnectOptions {
                    control_addr: shared_control_addr(),
                    account: account_id,
                    name,
                    local: addr,
                    auth: peer_auth.clone(),
                    retry_wait: Duration::from_secs(30),
                };
                let port = *local_port;
                let task = tokio::spawn(async move {
                    if let Err(e) = crate::p2p::run_connect(&opts).await {
                        warn!(port, "mirror ended: {e:#}");
                    }
                });
                applied.push(Applied {
                    entry: entry.clone(),
                    id: None,
                    key: None,
                    target_key: String::new(),
                    state: State::Live,
                    detail: Some(format!("本地监听 {addr}")),
                    task: Some(task),
                });
            }
        }
        if !listening && !matches!(entry, Entry::Mirror { .. }) {
            if let Some(a) = applied.last_mut() {
                a.state = State::TargetGone;
                a.detail = Some("本地端口当前没有在监听".into());
            }
        }
    }

    // ---- 目标表按"当前已应用的条目"整体重建 ----
    // 同一个本地端口可以同时发布为 http 和 tcp,逐条增删会互相误删。
    let mut wanted: HashMap<String, SocketAddr> = HashMap::new();
    for a in applied.iter() {
        if matches!(a.entry, Entry::Mirror { .. }) {
            continue;
        }
        wanted.insert(a.target_key.clone(), shared.dial_addr(a.entry.local_port()));
    }
    let stale: Vec<String> = shared
        .targets
        .iter()
        .filter(|e| !wanted.contains_key(e.key()))
        .map(|e| e.key().clone())
        .collect();
    for key in stale {
        shared.targets.remove(&key);
    }
    for (key, addr) in wanted {
        shared.targets.insert(key, addr);
    }
}

/// 把当前状态发布给 UI。
fn publish_status(applied: &[Applied], shared: &Arc<Shared>) {
    let out: Vec<EntryStatus> = applied
        .iter()
        .map(|a| {
            let mut state = a.state;
            let mut detail = a.detail.clone();
            if state == State::Live
                && !matches!(a.entry, Entry::Mirror { .. })
                && !shared.is_listening(a.entry.local_port())
            {
                state = State::TargetGone;
                detail = Some("本地端口当前没有在监听".into());
            }
            EntryStatus {
                entry: a.entry.clone(),
                state,
                detail,
            }
        })
        .collect();
    shared.set_status(out);
}

// 这两个是给 `publish_status` / mirror 用的连接参数,由 `run` 设置。
static CONTROL_ADDR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_control_addr(addr: &str) {
    let _ = CONTROL_ADDR.set(addr.to_string());
}

fn shared_control_addr() -> String {
    CONTROL_ADDR.get().cloned().unwrap_or_default()
}
