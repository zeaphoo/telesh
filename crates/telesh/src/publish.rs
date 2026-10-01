//! 发布配置(desired state)、运行状态,以及本地 UI 共享的状态。

use std::collections::{BTreeMap, BTreeSet};
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

/// P2P 入站规则:哪些本地端口允许被远端 peer 通过 P2P 直连。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum P2pMode {
    /// 所有在监听的本地端口(服务名 = 端口号)。
    All,
    /// 只有 `ports` 里列出的端口(服务名 = 端口号)。
    #[default]
    Selected,
    /// 不允许任何 P2P 入站。
    None,
}

/// P2P 入站规则。`ports` 只在 `mode == selected` 时有意义。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct P2pPolicy {
    #[serde(default)]
    pub mode: P2pMode,
    #[serde(default)]
    pub ports: Vec<u16>,
}

/// 设置的作用域:对外发布,还是 P2P 允许远程连接。
///
/// 同一个本地端口可以两者都有,所以按 section 分开存,互不影响。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Publish,
    P2p,
}

impl Section {
    /// 通道 key,与 `Settings` 里的 section 对应:`publish:<port>` / `p2p:<port>`。
    pub fn channel(self, port: u16) -> String {
        let name = match self {
            Section::Publish => "publish",
            Section::P2p => "p2p",
        };
        format!("{name}:{port}")
    }
}

/// 一个本地端口的设置。以后会加更多字段(比如 publish 的固定子域名)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortSettings {
    /// 是否开启请求追踪。默认开。
    #[serde(default = "default_true")]
    pub trace: bool,
}

impl Default for PortSettings {
    fn default() -> Self {
        Self { trace: true }
    }
}

fn default_true() -> bool {
    true
}

/// 单文件的端口设置:publish 与 p2p 分开记「哪些端口开了追踪」。
/// 没有记录的端口 = 默认开启。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub publish: BTreeMap<u16, PortSettings>,
    #[serde(default)]
    pub p2p: BTreeMap<u16, PortSettings>,
}

impl Settings {
    fn map(&self, section: Section) -> &BTreeMap<u16, PortSettings> {
        match section {
            Section::Publish => &self.publish,
            Section::P2p => &self.p2p,
        }
    }

    /// 这个端口是否开启追踪(没有记录 = 默认开)。
    pub fn trace_enabled(&self, section: Section, port: u16) -> bool {
        self.map(section)
            .get(&port)
            .map(|s| s.trace)
            .unwrap_or(true)
    }
}

/// 本地 UI 与 daemon 之间的共享状态。
pub struct Shared {
    desired: Mutex<Vec<Entry>>,
    status: Mutex<Vec<EntryStatus>>,
    ports: Mutex<Vec<PortInfo>>,
    /// P2P 入站规则(由 UI 设置)。
    p2p: Mutex<P2pPolicy>,
    /// 配置有变化时唤醒 daemon。
    pub notify: Notify,
    /// 目标名 → 本地地址。数据面和 iroh 两条传输都用它选路:
    /// key 就是 dialer 写在流首的 service 字段。
    pub targets: Arc<DashMap<String, SocketAddr>>,
    /// 端口设置(publish / p2p 分开),持久化到 `settings.json`。
    settings: Mutex<Settings>,
}

impl Shared {
    fn new() -> Self {
        Self {
            desired: Mutex::new(Vec::new()),
            status: Mutex::new(Vec::new()),
            ports: Mutex::new(Vec::new()),
            p2p: Mutex::new(P2pPolicy::default()),
            notify: Notify::new(),
            targets: Arc::new(DashMap::new()),
            settings: Mutex::new(Settings::default()),
        }
    }

    /// 当前设置(publish / p2p 分开)。
    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// 覆盖式写入设置。
    pub fn set_settings(&self, settings: Settings) {
        *self.settings.lock().unwrap() = settings;
    }

    /// 这个 section 下的端口是否开启追踪(没记录 = 默认开)。
    pub fn trace_enabled(&self, section: Section, port: u16) -> bool {
        self.settings.lock().unwrap().trace_enabled(section, port)
    }

    /// 覆盖式写入设置并落盘(UI 提交)。
    pub fn apply_settings(&self, settings: Settings) {
        let mut guard = self.settings.lock().unwrap();
        *guard = settings;
        self.persist(&guard);
    }

    /// 把某个 section 的端口集合同步成 `ports`:新端口补默认值,消失的删掉,
    /// 已有端口保留用户的选择。集合变了才落盘。daemon 每次 reconcile 调用。
    pub fn sync_settings(&self, section: Section, ports: &[u16]) {
        let mut guard = self.settings.lock().unwrap();
        let desired: BTreeSet<u16> = ports.iter().copied().collect();
        let map = match section {
            Section::Publish => &mut guard.publish,
            Section::P2p => &mut guard.p2p,
        };
        if map.keys().copied().collect::<BTreeSet<_>>() == desired {
            return;
        }
        map.retain(|p, _| desired.contains(p));
        for &p in &desired {
            map.entry(p).or_default();
        }
        self.persist(&guard);
    }

    /// 把当前设置写回 `settings.json`(调用时已持有锁)。
    fn persist(&self, settings: &Settings) {
        if let Some(path) = state_path() {
            if let Err(e) = save_settings(path, settings) {
                tracing::warn!("cannot persist settings: {e:#}");
            }
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

    pub fn p2p_policy(&self) -> P2pPolicy {
        self.p2p.lock().unwrap().clone()
    }

    /// UI 改了 P2P 入站规则:存下来并唤醒 daemon。
    pub fn set_p2p_policy(&self, policy: P2pPolicy) {
        *self.p2p.lock().unwrap() = policy;
        self.notify.notify_one();
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

/// 从 `p2p.json` 读 P2P 入站规则。
///
/// 文件不存在时兼容旧配置:把 `publish.json` 里的显式 P2P 条目当作「选定端口」,
/// 并落盘成规则文件(之后 P2P 只由这条规则决定)。
pub fn load_policy(state_path: &Path) -> Result<P2pPolicy> {
    let path = p2p_path(state_path);
    if path.exists() {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        return serde_json::from_str(&text)
            .with_context(|| format!("cannot parse {}", path.display()));
    }

    let ports: Vec<u16> = load(state_path)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| match e {
            Entry::P2p { local_port, .. } => Some(local_port),
            _ => None,
        })
        .collect();
    let policy = P2pPolicy {
        mode: P2pMode::Selected,
        ports,
    };
    if !policy.ports.is_empty() {
        let _ = save_policy(state_path, &policy);
    }
    Ok(policy)
}

/// 持久化 P2P 入站规则。
pub fn save_policy(state_path: &Path, policy: &P2pPolicy) -> Result<()> {
    let path = p2p_path(state_path);
    let text = serde_json::to_string_pretty(policy)?;
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))
}

/// P2P 规则与 state file 同目录。
fn p2p_path(state_path: &Path) -> std::path::PathBuf {
    state_path.with_file_name("p2p.json")
}

/// 从 `settings.json` 读端口设置(文件不存在 = 全部默认)。
pub fn load_settings(state_path: &Path) -> Settings {
    let path = settings_path(state_path);
    if !path.exists() {
        return Settings::default();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// 持久化端口设置。
pub fn save_settings(state_path: &Path, settings: &Settings) -> Result<()> {
    let path = settings_path(state_path);
    let text = serde_json::to_string_pretty(settings)?;
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))
}

/// 设置文件与 state file 同目录。
fn settings_path(state_path: &Path) -> std::path::PathBuf {
    state_path.with_file_name("settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认开;publish 与 p2p 的同一个端口互不影响;JSON 往返保持。
    #[test]
    fn settings_trace_is_per_section_and_port() {
        let mut s = Settings::default();
        assert!(s.trace_enabled(Section::Publish, 3000));
        assert!(s.trace_enabled(Section::P2p, 3000));

        s.publish.insert(3000, PortSettings { trace: false });
        assert!(!s.trace_enabled(Section::Publish, 3000));
        assert!(s.trace_enabled(Section::P2p, 3000)); // p2p 不受影响

        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert!(!back.trace_enabled(Section::Publish, 3000));
        assert!(back.trace_enabled(Section::P2p, 3000));
    }

    /// daemon 同步会把当前隧道补进文件(默认开)、删掉消失的,且保留用户的选择。
    #[test]
    fn sync_materializes_the_current_tunnels() {
        let shared = Shared::new();
        shared.sync_settings(Section::Publish, &[3000, 8080]);
        let s = shared.settings();
        assert_eq!(s.publish.len(), 2);
        assert!(s.trace_enabled(Section::Publish, 3000));

        // 用户把 3000 关掉。
        shared.sync_settings(Section::Publish, &[3000, 8080]);
        let mut settings = shared.settings();
        settings.publish.get_mut(&3000).unwrap().trace = false;
        shared.apply_settings(settings);
        assert!(!shared.trace_enabled(Section::Publish, 3000));

        // 端口集变化:去掉 8080、加上 9090。3000 的选择要保留。
        shared.sync_settings(Section::Publish, &[3000, 9090]);
        let s = shared.settings();
        assert_eq!(s.publish.len(), 2);
        assert!(!s.trace_enabled(Section::Publish, 3000));
        assert!(s.trace_enabled(Section::Publish, 9090));
        assert!(!s.publish.contains_key(&8080));
    }
}
