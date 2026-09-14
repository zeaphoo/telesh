//! telesh-server — tunnel control server and edge ingress.
//!
//! Agents register a subdomain over a WebSocket control connection; the edge
//! ingress routes HTTP Host-header traffic across that connection to the
//! agent, which dials its local service (MVP data plane: bytes multiplexed
//! over the same WebSocket).

pub mod config;
pub mod control;
pub mod dashboard;
pub mod ingress;
pub mod metrics;
pub mod registry;
pub mod state;
pub mod tcp;

pub use config::Config;