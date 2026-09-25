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

    /// Optional separate secret for lookup-only peers, so they never need
    /// the master secret. Servers configured with `--secret` still accept the
    /// master secret from peers; when this is unset, peers must use it.
    #[arg(long, env = "TELESH_LOOKUP_SECRET", default_value = "")]
    pub lookup_secret: String,

    /// SQLite database holding accounts, keys and sessions.
    /// Defaults to `~/.telesh/server.sqlite`.
    #[arg(long, env = "TELESH_DB")]
    pub db: Option<String>,

    /// Optional secret required on first registration. When set, open signup is
    /// closed: an unknown key may not create an account without it.
    #[arg(long, env = "TELESH_ENROLLMENT_SECRET", default_value = "")]
    pub enrollment_secret: String,

    /// Also run an embedded iroh relay on this address. TLS and ACME/Let's
    /// Encrypt settings come from the relay config file
    /// (`$TELESH_RELAY_CONFIG`, default `~/.telesh/relay.toml`).
    #[arg(long, env = "TELESH_RELAY_SERVE")]
    pub relay_serve: Option<SocketAddr>,

    /// Public URL of the iroh relay announced to agents after login (and used
    /// by the server's own gateway). When empty it is derived from the relay
    /// config file's `url`, or from the embedded relay plus `--public-host`.
    /// Leave it empty to keep agents on iroh's default (n0) relays.
    #[arg(long, env = "TELESH_RELAY_URL", default_value = "")]
    pub relay_url: String,

    /// Address for the data plane: agents dial this and speak yamux over TCP.
    /// Announced to agents in `server_hello.success.data_addr`; an agent
    /// replaces an unspecified host (`0.0.0.0` / `::`) with the host it used
    /// for the control connection.
    #[arg(long, env = "TELESH_DATA_ADDR", default_value = "0.0.0.0:9008")]
    pub data_addr: SocketAddr,

    /// Port range for raw-TCP tunnels (e.g. 18000-18100). If the requested
    /// port is outside this range the registration is rejected.
    #[arg(long, value_parser = clap::value_parser!(TcpPortRange), env = "TELESH_TCP_RANGE", default_value = "18000-18100")]
    pub tcp_port_range: TcpPortRange,

    /// Address for the dashboard UI + API + /metrics. Defaults to loopback
    /// so the admin surface is not exposed to the public ingress.
    #[arg(long, env = "TELESH_DASHBOARD_ADDR", default_value = "127.0.0.1:16456")]
    pub dashboard_addr: SocketAddr,
}

pub static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn init(config: Config) {
    let _ = CONFIG.set(config);
}

pub fn config() -> &'static Config {
    CONFIG.get().expect("server config not initialized")
}