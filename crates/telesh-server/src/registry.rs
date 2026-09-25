//! Registry of connected agents and tunnel routes.
//!
//! 一个 agent 现在可以发布多条隧道,所以路由表按**目标**记:
//! `subdomain → (agent_id, target)`,其中 `target` 是 agent 侧的标识(本地端口),
//! 服务端只存不解释,开流时原样回传给 agent。

use std::time::Instant;

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use tracing::debug;

use crate::data::DataHandle;

/// 一条隧道路由。
#[derive(Debug, Clone)]
pub struct Route {
    pub agent_id: String,
    /// 发布这条路由的控制连接;旧连接的清理只删自己的路由。
    pub conn_id: u64,
    /// agent 分配的目标标识(通常是本地端口的十进制字符串)。
    pub target: String,
}

/// 一个已认证的控制连接(metrics / dashboard 用)。
#[derive(Debug, Clone)]
pub struct AgentConn {
    pub agent_id: String,
    pub conn_id: u64,
    pub connected_at: Instant,
}

/// 一条数据连接:开流句柄 + 拥有它的连接 id。
#[derive(Debug, Clone)]
pub struct DataEntry {
    pub run_id: u64,
    pub handle: DataHandle,
}

#[derive(Default)]
pub struct Registry {
    agents: DashMap<String, AgentConn>,
    /// HTTP 路由:子域 → 目标。
    by_host: DashMap<String, Route>,
    /// agent 主动连入的数据连接。
    data: DashMap<String, DataEntry>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一条已认证的控制连接(重连时替换旧记录)。
    pub fn insert_agent(&self, agent_id: &str, conn_id: u64) {
        let fresh = self
            .agents
            .insert(
                agent_id.to_string(),
                AgentConn {
                    agent_id: agent_id.to_string(),
                    conn_id,
                    connected_at: Instant::now(),
                },
            )
            .is_none();
        if fresh {
            crate::metrics::inc_agents();
        }
        debug!(agent_id, "agent registered");
    }

    /// 控制连接断开:清掉它那条数据连接与路由。
    /// 只有**当前**连接(conn_id 相同)才会清掉 agent 记录与路由:
    /// 重连时新连接可能已经抢先注册,旧连接的清理不能把新状态删掉。
    pub fn remove(&self, agent_id: &str, conn_id: u64) {
        if self.agents.get(agent_id).is_some_and(|a| a.conn_id != conn_id) {
            debug!(agent_id, conn_id, "stale connection teardown ignored");
            return;
        }
        if let Some((_, entry)) = self.data.remove(agent_id) {
            entry.handle.shutdown();
        }
        let before = self.by_host.len();
        self.by_host
            .retain(|_, r| !(r.agent_id == agent_id && r.conn_id == conn_id));
        for _ in self.by_host.len()..before {
            crate::metrics::dec_http_tunnels();
        }
        if self.agents.remove(agent_id).is_some() {
            crate::metrics::dec_agents();
        }
        debug!(agent_id, "agent unregistered");
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Snapshot of all connected agents.
    pub fn list(&self) -> Vec<AgentConn> {
        self.agents.iter().map(|e| e.value().clone()).collect()
    }

    // ---- HTTP 路由 ----

    /// 注册一条子域路由。子域已被**另一个 agent** 占用时返回 `false`。
    pub fn add_route(&self, sub_domain: &str, route: Route) -> bool {
        match self.by_host.entry(sub_domain.to_string()) {
            Entry::Occupied(mut slot) => {
                if slot.get().agent_id == route.agent_id {
                    slot.insert(route);
                    true
                } else {
                    false
                }
            }
            Entry::Vacant(slot) => {
                slot.insert(route);
                crate::metrics::inc_http_tunnels();
                true
            }
        }
    }

    /// 取消一条子域路由(只有属主能删)。
    pub fn remove_route(&self, sub_domain: &str, conn_id: u64) {
        if let Entry::Occupied(slot) = self.by_host.entry(sub_domain.to_string()) {
            if slot.get().conn_id == conn_id {
                slot.remove();
                crate::metrics::dec_http_tunnels();
            }
        }
    }

    /// 按 Host 查路由。
    pub fn route(&self, sub_domain: &str) -> Option<Route> {
        self.by_host.get(sub_domain).map(|e| e.value().clone())
    }

    /// 全部 HTTP 路由(dashboard 用)。
    pub fn routes(&self) -> Vec<(String, Route)> {
        self.by_host
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect()
    }

    /// 某个 agent 的全部子域(dashboard 的 clients 页用)。
    pub fn sub_domains_of(&self, agent_id: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .by_host
            .iter()
            .filter(|e| e.value().agent_id == agent_id)
            .map(|e| e.key().clone())
            .collect();
        v.sort();
        v
    }

    // ---- 数据面 ----

    /// Register a data connection (an older run never replaces a newer one).
    pub fn set_data(&self, agent_id: &str, run_id: u64, handle: DataHandle) {
        match self.data.entry(agent_id.to_string()) {
            Entry::Occupied(mut slot) => {
                if slot.get().run_id < run_id {
                    slot.insert(DataEntry { run_id, handle });
                }
            }
            Entry::Vacant(slot) => {
                slot.insert(DataEntry { run_id, handle });
            }
        }
    }

    /// The handle for opening streams on this agent's data connection.
    pub fn data_handle(&self, agent_id: &str) -> Option<DataHandle> {
        self.data.get(agent_id).map(|e| e.handle.clone())
    }

    /// The `run_id` of the live data connection, if any.
    pub fn data_run(&self, agent_id: &str) -> Option<u64> {
        self.data.get(agent_id).map(|e| e.run_id)
    }

    /// Drop a data connection registration (only if `run_id` still owns it).
    pub fn remove_data(&self, agent_id: &str, run_id: u64) {
        if let Entry::Occupied(slot) = self.data.entry(agent_id.to_string()) {
            if slot.get().run_id == run_id {
                slot.remove();
            }
        }
    }

    /// Drop whichever agent registered this `run_id` (the driver task does not
    /// know the agent id).
    pub fn remove_data_run(&self, run_id: u64) {
        let key = self
            .data
            .iter()
            .find(|e| e.value().run_id == run_id)
            .map(|e| e.key().clone());
        if let Some(key) = key {
            self.data.remove(&key);
        }
    }
}
