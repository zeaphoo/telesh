//! Server runtime configuration (CLI args).

use std::net::SocketAddr;
use std::sync::OnceLock;

use clap::Parser;
use std::ops::RangeInclusive;
use std::str::FromStr;

/// TCP port range for raw-TCP tunnels, e.g. `18000-18100`.
#[derive(Debug, Clone, Copy)]
pub struct TcpPortRange {
    pub start: u16,
    pub end: u16,
}

impl TcpPortRange {
    pub fn inclusive(&self) -> RangeInclusive<u16> {
        self.start..=self.end
    }
}

impl FromStr for TcpPortRange {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (a, b) = s
            .split_once('-')
            .ok_or_else(|| format!("expected START-END, got {s:?}"))?;
        let start: u16 = a.parse().map_err(|_| format!("bad port {a:?}"))?;
        let end: u16 = b.parse().map_err(|_| format!("bad port {b:?}"))?;
        if start > end {
            return Err(format!("start {start} > end {end}"));
        }
        Ok(TcpPortRange { start, end })
    }
}

/// telesh tunnel server: agents register a subdomain, the edge ingress routes
/// Host header traffic over the WebSocket control plane to the right agent.
#[derive(Debug, Clone, Parser)]
#[command(name = "telesh-server", version, about)]
pub struct Config {
    /// Address where agents dial the WebSocket control server (ws://.../tunnel).
    #[arg(long, env = "TELESH_CTRL_ADDR", default_value = "0.0.0.0:9009")]
    pub control_addr: SocketAddr,

    /// Address where the edge ingress listens for public TCP connections
    /// (HTTP in the MVP; the Host header selects the tunnel).
    #[arg(long, env = "TELESH_INGRESS_ADDR", default_value = "0.0.0.0:8080")]
    pub ingress_addr: SocketAddr,

    /// Optional public hostname suffix used when building tunnel URLs,
    /// e.g. `example.com` -> `http://<sub>.example.com:<ingress-port>`.
    /// Empty means the reported hostname is just the subdomain.
    #[arg(long, env = "TELESH_PUBLIC_HOST", default_value = "")]
    pub public_host: String,

    /// Optional master secret. When set, agents must present the same key in
    /// their `ClientHello`, otherwise the connection is rejected.
    #[arg(long, env = "TELESH_SECRET", default_value = "")]
    pub secret: String,

    /// Max lifetime of a pending stream that the agent never accepts (ms).
    #[arg(long, env = "TELESH_ACCEPT_TIMEOUT_MS", default_value = "10000")]
    pub accept_timeout_ms: u64,

    /// Port range for raw-TCP tunnels (e.g. 18000-18100). If the requested
    /// port is outside this range the registration is rejected.
    #[arg(long, value_parser = clap::value_parser!(TcpPortRange), env = "TELESH_TCP_RANGE", default_value = "18000-18100")]
    pub tcp_port_range: TcpPortRange,

    /// Address for the dashboard UI + API + /metrics. Defaults to loopback
    /// so the admin surface is not exposed to the public ingress.
    #[arg(long, env = "TELESH_DASHBOARD_ADDR", default_value = "127.0.0.1:7500")]
    pub dashboard_addr: SocketAddr,
}

pub static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn init(config: Config) {
    let _ = CONFIG.set(config);
}

pub fn config() -> &'static Config {
    CONFIG.get().expect("server config not initialized")
}