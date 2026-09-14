//! Shared server state.
//!
//! MVP uses process-global state (single instance), matching tunnelto's
//! self-hosted topology. A multi-instance edge would swap these for a
//! distributed registry + gossip later.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use tokio::sync::mpsc::UnboundedSender;

use telesh_protocol::StreamPacket;

use crate::registry::Registry;

/// Agents by subdomain / agent_id.
pub static REGISTRY: OnceLock<Registry> = OnceLock::new();

/// Ingress-side stream pumps, keyed by stream_id. The sender side receives
/// `Data`/`End`/`Refused` packets from the agent and writes them to the
/// public TCP socket.
pub static ACTIVE_STREAMS: OnceLock<DashMap<u64, UnboundedSender<StreamPacket>>> = OnceLock::new();

static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(1);

pub fn init() {
    let _ = REGISTRY.set(Registry::new());
    let _ = ACTIVE_STREAMS.set(DashMap::new());
    crate::metrics::init();
}

/// Allocate a globally unique stream id.
pub fn next_stream_id() -> u64 {
    NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn registry() -> &'static Registry {
    REGISTRY.get().expect("server state not initialized")
}

pub fn active_streams() -> &'static DashMap<u64, UnboundedSender<StreamPacket>> {
    ACTIVE_STREAMS.get().expect("server state not initialized")
}