//! telesh-server — tunnel control server and edge ingress.
//!
//! Agents register a subdomain over a WebSocket control connection; the edge
//! ingress routes HTTP Host-header traffic to the agent over the data
//! connection the agent established (`data.rs`, yamux over TCP). P2P tunnels
//! bypass the server entirely: peers dial the agent over iroh with a token the
//! server issued.

pub mod config;
pub mod control;
pub mod dashboard;
pub mod data;
pub mod db;
pub mod ingress;
pub mod metrics;
pub mod registry;
pub mod relay_config;
pub mod state;
pub mod tcp;

pub use config::{Cli, Config};