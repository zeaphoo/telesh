//! Registry of connected agents.
//!
//! Keyed by subdomain (for Host-header routing) and by `agent_id` (so a
//! reconnecting agent transparently replaces its stale connection).

use std::time::Instant;

use dashmap::DashMap;
use tokio::sync::mpsc::UnboundedSender;
use tracing::debug;

use telesh_protocol::Frame;

/// A live agent connection. `tx` is the sink task channel: every frame meant
/// for this agent goes through it (stream inits, incoming data, pings...).
#[derive(Debug, Clone)]
pub struct ConnectedClient {
    pub agent_id: String,
    pub sub_domain: String,
    pub tx: UnboundedSender<Frame>,
    pub connected_at: Instant,
}

#[derive(Default)]
pub struct Registry {
    by_host: DashMap<String, ConnectedClient>,
    by_agent: DashMap<String, ConnectedClient>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) an agent.
    ///
    /// * If an old connection with the same `agent_id` exists (a reconnect),
    ///   it is evicted and its subdomain slot is re-used.
    /// * If `sub_domain` is held by a *different* agent, the existing holder
    ///   wins and `Ok(false)` is returned so the caller can fail the
    ///   handshake with `SubDomainInUse`.
    pub fn insert(&self, agent_id: &str, sub_domain: &str, tx: UnboundedSender<Frame>) -> bool {
        // Replace stale connection of the same agent.
        if let Some((_, prev)) = self.by_agent.remove(agent_id) {
            crate::metrics::dec_agents();
            crate::metrics::dec_http_tunnels();
            self.by_host.remove(&prev.sub_domain);
        }
        // Reject if another agent owns the subdomain.
        if self
            .by_host
            .get(sub_domain)
            .is_some_and(|c| c.agent_id != agent_id)
        {
            return false;
        }

        let client = ConnectedClient {
            agent_id: agent_id.to_string(),
            sub_domain: sub_domain.to_string(),
            tx,
            connected_at: Instant::now(),
        };
        debug!(sub_domain, agent_id, "registered agent");
        crate::metrics::inc_agents();
        crate::metrics::inc_http_tunnels();
        self.by_host.insert(sub_domain.to_string(), client.clone());
        self.by_agent.insert(agent_id.to_string(), client);
        true
    }

    pub fn find_by_host(&self, sub_domain: &str) -> Option<ConnectedClient> {
        self.by_host.get(sub_domain).map(|c| c.clone())
    }

    pub fn remove(&self, agent_id: &str) {
        if let Some((_, client)) = self.by_agent.remove(agent_id) {
            self.by_host.remove(&client.sub_domain);
            crate::metrics::dec_agents();
            crate::metrics::dec_http_tunnels();
            debug!(agent_id, sub_domain = %client.sub_domain, "unregistered agent");
        }
    }

    pub fn len(&self) -> usize {
        self.by_agent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_agent.is_empty()
    }

    /// Snapshot of all connected agents.
    pub fn list(&self) -> Vec<ConnectedClient> {
        self.by_agent.iter().map(|e| e.value().clone()).collect()
    }
}