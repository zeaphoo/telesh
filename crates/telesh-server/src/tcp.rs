//! Raw-TCP tunnels: allocate a public port on the server, lazily bind it,
//! and forward every connection to the owning agent as a stream.
//!
//! Routing key here is the *port* (there is no HTTP Host header to peek).

use std::fmt;
use std::sync::OnceLock;

use dashmap::DashMap;
use rand::Rng;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::config::config;
use crate::ingress;
use crate::registry::AgentConn;

/// One managed TCP tunnel: a bound public port whose accept loop feeds
/// streams to `client`.
pub struct ManagedTunnel {
    pub port: u16,
    pub client: AgentConn,
    accept_task: JoinHandle<()>,
}

/// Public TCP port -> owning tunnel.
static TCP_TUNNELS: OnceLock<DashMap<u16, ManagedTunnel>> = OnceLock::new();

#[derive(Debug)]
pub enum TcpError {
    OutOfRange,
    InUse,
    Bind(std::io::Error),
}

impl fmt::Display for TcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TcpError::OutOfRange => write!(f, "port outside configured range"),
            TcpError::InUse => write!(f, "port already in use"),
            TcpError::Bind(e) => write!(f, "bind failed: {e}"),
        }
    }
}

fn tunnels() -> &'static DashMap<u16, ManagedTunnel> {
    TCP_TUNNELS.get_or_init(DashMap::new)
}

/// Allocate and lazily bind a port for `client`. Returns the public port.
pub async fn register(client: AgentConn, requested: Option<u16>) -> Result<u16, TcpError> {
    let active = tunnels();
    let range = config().tcp_port_range;

    // Choose a port: the requested one, or a random free one in range.
    let listener = match requested {
        Some(port) => {
            if !range.inclusive().contains(&port) {
                return Err(TcpError::OutOfRange);
            }
            bind(port).await.map_err(|e| classify_bind(&e))?
        }
        None => {
            // Try uniformly random ports; ~150 attempts gives 99.999% success
            // when only 15% of ports are free (same heuristic as bore).
            let mut last_err = std::io::Error::other("no free port");
            let mut found = None;
            for _ in 0..150 {
                let port = rand::thread_rng().gen_range(range.inclusive());
                if active.contains_key(&port) {
                    continue;
                }
                match bind(port).await {
                    Ok(l) => {
                        found = Some((port, l));
                        break;
                    }
                    Err(e) => last_err = e,
                }
            }
            match found {
                Some((_, l)) => l,
                None => return Err(TcpError::Bind(last_err)),
            }
        }
    };
    let port = listener
        .local_addr()
        .map_err(TcpError::Bind)?
        .port();

    let accept_task = tokio::spawn(accept_loop(listener, port, client.clone()));
    tunnels().insert(
        port,
        ManagedTunnel {
            port,
            client,
            accept_task,
        },
    );
    crate::metrics::inc_tcp_tunnels();
    info!(port, "tcp tunnel opened");
    Ok(port)
}

/// Stop and release all TCP tunnels owned by `agent_id` (agent disconnect).
pub fn unregister_all(agent_id: &str) {
    let ports: Vec<u16> = tunnels()
        .iter()
        .filter(|t| t.client.agent_id == agent_id)
        .map(|t| t.port)
        .collect();
    for port in ports {
        if let Some((_, t)) = tunnels().remove(&port) {
            t.accept_task.abort();
            crate::metrics::dec_tcp_tunnels();
            debug!(port, "tcp tunnel released");
        }
    }
}

/// Information about a live raw-TCP tunnel (for the dashboard).
#[derive(Debug, Clone)]
pub struct TcpTunnelView {
    pub port: u16,
    pub agent_id: String,
}

pub fn list_tunnels() -> Vec<TcpTunnelView> {
    tunnels()
        .iter()
        .map(|t| TcpTunnelView {
            port: t.value().port,
            agent_id: t.value().client.agent_id.clone(),
        })
        .collect()
}

async fn bind(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(("0.0.0.0", port)).await
}

fn classify_bind(e: &std::io::Error) -> TcpError {
    if e.kind() == std::io::ErrorKind::AddrInUse {
        TcpError::InUse
    } else {
        TcpError::Bind(std::io::Error::new(e.kind(), e.to_string()))
    }
}

async fn accept_loop(listener: TcpListener, port: u16, client: AgentConn) {
    loop {
        match listener.accept().await {
            Ok((socket, peer)) => {
                debug!(%peer, port, "tcp tunnel connection");
                let client = client.clone();
                let key = format!("tcp:{port}");
                tokio::spawn(async move {
                    ingress::open_stream(client, socket, key).await;
                });
            }
            Err(e) => {
                warn!(port, "tcp tunnel accept error: {e}");
                return; // listener broke; caller will unregister
            }
        }
    }
}