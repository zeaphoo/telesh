//! telesh-server — tunnel control server and edge ingress.
//!
//! Agents register a subdomain over a WebSocket control connection; the edge
//! ingress routes HTTP Host-header traffic to the agent, which dials its local
//! service. Payload takes the direct iroh path when available, otherwise the
//! agent-initiated data plane (`data.rs`, yamux over TCP).

pub mod config;
pub mod control;
pub mod dashboard;
pub mod data;
pub mod db;
pub mod gateway;
pub mod ingress;
pub mod metrics;
pub mod registry;
pub mod relay_config;
pub mod state;
pub mod tcp;

pub use config::Config;