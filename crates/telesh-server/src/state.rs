//! Shared server state.
//!
//! MVP uses process-global state (single instance), matching tunnelto's
//! self-hosted topology. A multi-instance edge would swap these for a
//! distributed registry + gossip later.

use std::sync::OnceLock;

use anyhow::Result;
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use iroh::Endpoint;
use sqlx::SqlitePool;
use tokio::sync::mpsc::UnboundedSender;

use telesh_protocol::{EndpointAddr, EndpointId, Frame, P2PAddrResp, P2PAnnounce};

use crate::registry::Registry;

/// Agents by subdomain / agent_id.
pub static REGISTRY: OnceLock<Registry> = OnceLock::new();

/// One registered P2P service, namespaced by account.
///
/// The directory key is `"<account>/<name>"`; ownership is by `endpoint_id`
/// (a cryptographic peer identity), so a different agent cannot take over the
/// name.
#[derive(Debug, Clone)]
pub struct P2pEntry {
    pub account: String,
    pub name: String,
    pub endpoint_id: EndpointId,
    pub announce: P2PAnnounce,
}

/// Outcome of claiming a P2P service name.
pub enum P2pClaim {
    /// The name is now owned by this endpoint.
    Ok,
    /// A different endpoint already owns the name.
    Taken,
}

/// P2P service directory: service name -> iroh address. The server never sees
/// P2P payload; it only brokers this address between agents and peers.
pub static P2P_DIR: OnceLock<DashMap<String, P2pEntry>> = OnceLock::new();

/// The server's own iroh endpoint, used to dial agents for the direct data
/// plane (HTTP/TCP ingress). `None` until the server binds it at startup.
pub static SERVER_ENDPOINT: OnceLock<Endpoint> = OnceLock::new();

/// iroh relay URL announced to clients after login (and the one the gateway
/// dials through). `None` means iroh's default (n0) relays.
pub static RELAY_URL: OnceLock<Option<String>> = OnceLock::new();

/// Data-plane address announced to agents after login (the agent-initiated
/// yamux endpoint). `None` means no data plane.
pub static DATA_ADDR: OnceLock<Option<String>> = OnceLock::new();

/// Accounts / keys / sessions.
pub static DB: OnceLock<SqlitePool> = OnceLock::new();

/// Stable server identifier, mixed into login challenges.
pub static SERVER_ID: OnceLock<[u8; 32]> = OnceLock::new();

/// Live peers subscribed to service-name address updates. When an agent
/// re-announces (new address), subscribers are pushed the new `P2PAddr` so
/// `telesh connect` never has to re-look-up.
pub static P2P_SUBS: OnceLock<DashMap<String, Vec<UnboundedSender<Frame>>>> = OnceLock::new();


pub async fn init() -> Result<()> {
    let _ = REGISTRY.set(Registry::new());
    let _ = P2P_DIR.set(DashMap::new());
    let _ = P2P_SUBS.set(DashMap::new());
    let _ = SERVER_ID.set(rand::random());
    crate::metrics::init();

    let cfg = crate::config::config();
    let db_path = cfg.db.clone().unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        format!("{home}/.telesh/server.sqlite")
    });
    let pool = crate::db::open(&db_path).await?;
    let _ = DB.set(pool);
    Ok(())
}

pub fn db() -> &'static SqlitePool {
    DB.get().expect("server db not initialized")
}

pub fn server_id() -> &'static [u8; 32] {
    SERVER_ID.get().expect("server id not initialized")
}

pub fn registry() -> &'static Registry {
    REGISTRY.get().expect("server state not initialized")
}

pub fn p2p_dir() -> &'static DashMap<String, P2pEntry> {
    P2P_DIR.get().expect("server state not initialized")
}

/// Atomically claim `name` for `entry`'s endpoint.
///
/// The same endpoint may re-claim/replace its own entry (needed for reconnects),
/// but a different endpoint is rejected rather than overwriting the owner.
pub fn p2p_claim(name: String, entry: P2pEntry) -> P2pClaim {
    match p2p_dir().entry(name) {
        Entry::Occupied(mut slot) => {
            if slot.get().endpoint_id == entry.endpoint_id {
                slot.insert(entry);
                P2pClaim::Ok
            } else {
                P2pClaim::Taken
            }
        }
        Entry::Vacant(slot) => {
            slot.insert(entry);
            P2pClaim::Ok
        }
    }
}

fn p2p_subs() -> &'static DashMap<String, Vec<UnboundedSender<Frame>>> {
    P2P_SUBS.get().expect("server state not initialized")
}

/// Register a peer's control channel as interested in `name`.
pub fn p2p_subscribe(name: &str, tx: UnboundedSender<Frame>) {
    p2p_subs().entry(name.to_string()).or_default().push(tx);
}

/// Push the current address to everyone subscribed to `key` (called when an
/// agent announces or re-announces a service). Each push carries a fresh token.
pub async fn p2p_notify(key: &str, account: &str, name: &str, addr: &EndpointAddr) {
    let token = issue_token(account, name).await;
    let owner_name = match account.parse::<u32>() {
        Ok(id) => crate::db::display_name(db(), id)
            .await
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    if let Some(mut subs) = p2p_subs().get_mut(key) {
        subs.retain(|tx| !tx.is_closed());
        for tx in subs.iter() {
            let _ = tx.send(Frame::P2PAddr(P2PAddrResp::Found {
                account: account.to_string(),
                name: name.to_string(),
                owner_name: owner_name.clone(),
                addr: addr.clone(),
                token: token.clone(),
            }));
        }
    }
}

/// Mint a short-lived, `account/name`-scoped **opaque** token and store its
/// hash. The agent cannot verify it locally; it asks the server (see
/// `Frame::TokenCheck`).
pub async fn issue_token(account: &str, name: &str) -> Option<Vec<u8>> {
    let account_id = account.parse::<u32>().ok()?;
    crate::db::create_token(db(), account_id, name).await.ok()
}

/// Drop closed subscriber channels (a peer disconnected).
pub fn p2p_prune_subs() {
    p2p_subs().retain(|_, subs| {
        subs.retain(|tx| !tx.is_closed());
        !subs.is_empty()
    });
}

pub fn set_server_endpoint(endpoint: Endpoint) {
    let _ = SERVER_ENDPOINT.set(endpoint);
}

/// The server's iroh endpoint for dialing agents, if it was started with one.
pub fn server_endpoint() -> Option<&'static Endpoint> {
    SERVER_ENDPOINT.get()
}

pub fn set_relay_url(url: Option<String>) {
    let _ = RELAY_URL.set(url);
}

/// The relay URL clients are told to use, if the server runs/uses one.
pub fn relay_url() -> Option<String> {
    RELAY_URL.get().cloned().flatten()
}

pub fn set_data_addr(addr: Option<String>) {
    let _ = DATA_ADDR.set(addr);
}

/// The data-plane address agents are told to dial, if the server runs one.
pub fn data_addr() -> Option<String> {
    DATA_ADDR.get().cloned().flatten()
}

/// Drop every P2P service owned by `account` (called when the agent's control
/// connection ends, so stale addresses do not linger in the directory).
pub fn p2p_remove_account(account: &str) {
    let keys: Vec<String> = p2p_dir()
        .iter()
        .filter(|e| e.value().account == account)
        .map(|e| e.key().clone())
        .collect();
    for key in keys {
        p2p_dir().remove(&key);
    }
}