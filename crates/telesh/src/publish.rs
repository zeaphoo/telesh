//! 发布配置(desired state)、运行状态,以及本地 UI 共享的状态。

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::ports::PortInfo;

/// 一条发布配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    /// 挂到 control server 的一个 HTTP 子域。
    Http {
        local_port: u16,
        #[serde(default)]
        sub_domain: Option<String>,
    },
    /// 在 control server 上开一个公网 TCP 端口。
    Tcp {
        local_port: u16,
        #[serde(default)]
        remote_port: Option<u16>,
    },
    /// 暴露到 P2P 网络(服务名)。
    P2p { local_port: u16, name: String },
    /// 把远端服务镜像到本地端口(访问方角色)。
    Mirror {
        local_port: u16,
        remote: String,
    },
}

impl Entry {
    pub fn local_port(&self) -> u16 {
        match self {
            Entry::Http { local_port, .. }
            | Entry::Tcp { local_port, .. }
            | Entry::P2p { local_port, .. }
            | Entry::Mirror { local_port, .. } => *local_port,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Entry::Http { .. } => "http",
            Entry::Tcp { .. } => "tcp",
            Entry::P2p { .. } => "p2p",
            Entry::Mirror { .. } => "mirror",
        }
    }
}

/// 一条条目的运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Pending,
    Live,
    Error,
    /// 本地端口当前没有在监听。
    TargetGone,
}

/// 对外(GET/PUT `/api/publish`)的形状:配置字段 + 运行状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryStatus {
    #[serde(flatten)]
    pub entry: Entry,
    #[serde(default)]
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 本地 UI 与 daemon 之间的共享状态。
pub struct Shared {
    desired: Mutex<Vec<Entry>>,
    status: Mutex<Vec<EntryStatus>>,
    ports: Mutex<Vec<PortInfo>>,
    /// 配置有变化时唤醒 daemon。
    pub notify: Notify,
    /// 目标名 → 本地地址。数据面和 iroh 两条传输都用它选路:
    /// key 就是 dialer 写在流首的 service 字段。
    pub targets: Arc<DashMap<String, SocketAddr>>,
}

impl Shared {
    fn new() -> Self {
        Self {
            desired: Mutex::new(Vec::new()),
            status: Mutex::new(Vec::new()),
            ports: Mutex::new(Vec::new()),
            notify: Notify::new(),
            targets: Arc::new(DashMap::new()),
        }
    }

    pub fn desired(&self) -> Vec<Entry> {
        self.desired.lock().unwrap().clone()
    }

    /// UI 提交了新的期望状态:存下来并唤醒 daemon。
    pub fn set_desired(&self, entries: Vec<Entry>) {
        *self.desired.lock().unwrap() = entries;
        self.notify.notify_one();
    }

    pub fn status(&self) -> Vec<EntryStatus> {
        self.status.lock().unwrap().clone()
    }

    pub fn set_status(&self, status: Vec<EntryStatus>) {
        *self.status.lock().unwrap() = status;
    }

    /// 会话断开时把已发布条目标回 pending。
    pub fn mark_all_pending(&self) {
        let mut status = self.status.lock().unwrap();
        for s in status.iter_mut() {
            if s.state == State::Live {
                s.state = State::Pending;
                s.detail = None;
            }
        }
    }

    pub fn ports(&self) -> Vec<PortInfo> {
        self.ports.lock().unwrap().clone()
    }

    pub fn set_ports(&self, ports: Vec<PortInfo>) {
        *self.ports.lock().unwrap() = ports;
    }

    /// 某个本地端口当前在监听吗(用来标记 `target_gone`)。
    pub fn is_listening(&self, port: u16) -> bool {
        self.ports().iter().any(|p| p.port == port)
    }

    /// 该本地端口该拨到哪里:有 IPv4 监听就 `127.0.0.1`,否则 `::1`。
    pub fn dial_addr(&self, port: u16) -> SocketAddr {
        let v6_only = self
            .ports()
            .iter()
            .find(|p| p.port == port)
            .map(|p| p.v6_only)
            .unwrap_or(false);
        if v6_only {
            SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port))
        } else {
            SocketAddr::from(([127, 0, 0, 1], port))
        }
    }
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
static STATE_PATH: OnceLock<std::path::PathBuf> = OnceLock::new();

/// 记住 state file 位置,供 HTTP API 持久化配置。
pub fn set_state_path(path: std::path::PathBuf) {
    let _ = STATE_PATH.set(path);
}

pub fn state_path() -> Option<&'static std::path::Path> {
    STATE_PATH.get().map(|p| p.as_path())
}

pub fn shared() -> &'static Arc<Shared> {
    SHARED.get_or_init(|| Arc::new(Shared::new()))
}

/// 从 `publish.json` 读取上次的发布配置(文件不存在就是空配置)。
pub fn load(state_path: &Path) -> Result<Vec<Entry>> {
    let path = publish_path(state_path);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))
}

/// 持久化发布配置。
pub fn save(state_path: &Path, entries: &[Entry]) -> Result<()> {
    let path = publish_path(state_path);
    let text = serde_json::to_string_pretty(entries)?;
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))
}

/// 发布配置与 state file 同目录。
fn publish_path(state_path: &Path) -> std::path::PathBuf {
    state_path.with_file_name("publish.json")
}
