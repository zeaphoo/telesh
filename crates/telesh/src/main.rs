//! telesh — tunnel agent: expose a local service through a public entry.
//!
//!   telesh http 8000                # random subdomain on 127.0.0.1:9009
//!   telesh http 8000 --to srv:9009  # against a remote server
//!   telesh http 22  -s myssh        # custom subdomain
//!   telesh tcp 22                   # raw TCP tunnel (random public port)
//!   telesh tcp 22 --remote-port 19022
//!   telesh p2p 22 -s myssh          # expose directly over iroh (P2P)
//!   telesh connect myssh -l 2222    # mirror the remote service on localhost:2222

mod auth;
mod control;
mod data;
mod inspect;
mod local;
mod p2p;
mod state;
mod token_check;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use control::{run_once, AgentOptions};

/// Seconds an accepted local connection is held while the P2P link recovers.
const RETRY_WAIT_SECS: u64 = 30;

/// Read a boolean env flag ("1"/"true"/"yes").
fn env_flag(key: &str) -> bool {
    matches!(
        std::env::var(key).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

#[derive(Debug, Parser)]
#[command(name = "telesh", version, about = "Expose a local port via a public tunnel URL")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// File persisting agent identity + assigned subdomain across restarts.
    #[arg(long, global = true, env = "TELESH_STATE")]
    state_file: Option<PathBuf>,

    /// Exit instead of reconnecting when the connection drops.
    #[arg(long, global = true)]
    no_reconnect: bool,

    /// Address for the agent-local inspector dashboard (requests log + stats).
    #[arg(long, global = true, env = "TELESH_INSPECTOR_ADDR", default_value = "127.0.0.1:16345")]
    dashboard_addr: SocketAddr,

    /// Enrollment secret for first registration on servers that require one.
    #[arg(long, global = true, env = "TELESH_ENROLLMENT_SECRET")]
    enroll: Option<String>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// HTTP tunnel: expose a local port under a public URL (Host-header routed).
    Http(HttpArgs),
    /// Raw TCP tunnel: expose a local port as tcp://<server>:<port>.
    Tcp(TcpArgs),
    /// P2P: expose a local port directly over iroh (payload never hits the server).
    P2p(P2pArgs),
    /// P2P: mirror a remote service onto a local TCP port.
    Connect(ConnectArgs),
}

#[derive(Debug, clap::Args)]
struct HttpArgs {
    /// Local port to expose.
    port: u16,

    /// Local host to forward to.
    #[arg(long, env = "TELESH_LOCAL_HOST", default_value = "127.0.0.1")]
    local_host: String,

    /// Control server address (host:port of the telesh-server).
    #[arg(short, long, env = "TELESH_SERVER", default_value = "127.0.0.1:9009")]
    to: String,

    /// Requested subdomain; by default the server assigns a random one.
    #[arg(short, long, env = "TELESH_SUBDOMAIN")]
    subdomain: Option<String>,

    /// Authentication secret, if the server requires one.
    #[arg(short = 'k', long, env = "TELESH_SECRET")]
    secret: Option<String>,
}

#[derive(Debug, clap::Args)]
struct TcpArgs {
    /// Local port to expose.
    port: u16,

    /// Local host to forward to.
    #[arg(long, env = "TELESH_LOCAL_HOST", default_value = "127.0.0.1")]
    local_host: String,

    /// Control server address (host:port of the telesh-server).
    #[arg(short, long, env = "TELESH_SERVER", default_value = "127.0.0.1:9009")]
    to: String,

    /// Requested remote port on the server; by default one is assigned.
    #[arg(long, env = "TELESH_REMOTE_PORT")]
    remote_port: Option<u16>,

    /// Authentication secret, if the server requires one.
    #[arg(short = 'k', long, env = "TELESH_SECRET")]
    secret: Option<String>,
}

/// `NAME=PORT`, repeatable, for exposing several ports from one process.
#[derive(Debug, Clone)]
struct ExposeSpec {
    name: String,
    port: u16,
}

impl std::str::FromStr for ExposeSpec {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name, port) = s
            .split_once('=')
            .ok_or_else(|| format!("expected NAME=PORT, got {s:?}"))?;
        let port: u16 = port.parse().map_err(|_| format!("bad port {port:?}"))?;
        if name.is_empty() {
            return Err("service name must not be empty".into());
        }
        Ok(ExposeSpec {
            name: name.to_string(),
            port,
        })
    }
}

#[derive(Debug, clap::Args)]
struct P2pArgs {
    /// Local port to expose over the P2P network (single-service form).
    port: Option<u16>,

    /// Expose an additional port as `NAME=PORT` (repeatable). When present,
    /// <PORT> and -s/--name are ignored and every service needs a name.
    #[arg(long = "expose", value_name = "NAME=PORT")]
    expose: Vec<ExposeSpec>,

    /// Local host to forward to.
    #[arg(long, env = "TELESH_LOCAL_HOST", default_value = "127.0.0.1")]
    local_host: String,

    /// Control server address (host:port of the telesh-server).
    #[arg(short, long, env = "TELESH_SERVER", default_value = "127.0.0.1:9009")]
    to: String,

    /// Service name peers use to reach this port; defaults to the endpoint id.
    #[arg(short = 's', long, env = "TELESH_NAME")]
    name: Option<String>,

    /// Authentication secret, if one is required.
    #[arg(short = 'k', long, env = "TELESH_SECRET")]
    secret: Option<String>,

    /// Also open a router port mapping (UPnP/PCP/NAT-PMP) for the local port,
    /// exposing it publicly without iroh or the server.
    #[arg(long, env = "TELESH_MAP_TCP")]
    map_tcp: bool,
}

#[derive(Debug, clap::Args)]
struct ConnectArgs {
    /// Target to mirror locally, as `ACCOUNT/NAME` (e.g. `000012345/web`).
    target: String,

    /// Local port to bind the mirror on.
    #[arg(short = 'l', long)]
    local_port: u16,

    /// Local host to bind the mirror on.
    #[arg(long, env = "TELESH_LOCAL_HOST", default_value = "127.0.0.1")]
    local_host: String,

    /// Control server address (host:port of the telesh-server).
    #[arg(short, long, env = "TELESH_SERVER", default_value = "127.0.0.1:9009")]
    to: String,

    /// Authentication secret, if one is required.
    #[arg(short = 'k', long, env = "TELESH_SECRET")]
    secret: Option<String>,

}

/// Run `body` (one tunnel session) and reconnect with backoff until
/// `--no-reconnect` is set or the process is stopped.
macro_rules! reconnect_loop {
    ($no_reconnect:expr, $body:expr) => {{
        let mut attempt: u32 = 0;
        loop {
            let fut = $body;
            match fut.await {
                Ok(_) => info!("session closed"),
                Err(e) => warn!("session error: {e:#}"),
            }
            if $no_reconnect {
                return Ok(());
            }
            attempt += 1;
            let backoff = Duration::from_millis((500 * attempt.min(10)) as u64);
            warn!(ms = backoff.as_millis(), "reconnecting...");
            tokio::time::sleep(backoff).await;
        }
    }};
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("telesh=info")))
        .init();

    let cli = Cli::parse();
    let state_path = cli.state_file.clone().unwrap_or_else(state::default_path);

    // Load persisted identity (agent_id + last subdomain).
    let mut st = match state::load(&state_path) {
        Ok(s) => s,
        Err(e) => {
            warn!("cannot load state from {}: {e:#}", state_path.display());
            state::AgentState::fresh()
        }
    };

    // Local debugging dashboard (request log + stats), independent of the
    // tunnel control connection.
    inspect::init();
    {
        let addr = cli.dashboard_addr;
        tokio::spawn(async move {
            if let Err(e) = inspect::serve_dashboard(addr).await {
                warn!("inspector dashboard exited: {e:#}");
            }
        });
    }

    match &cli.command {
        Command::Http(args) => {
            // An explicit subdomain overrides any persisted one.
            if let Some(s) = &args.subdomain {
                st.subdomain = Some(s.clone());
            }
            let opts = AgentOptions {
                control_addr: args.to.clone(),
                local: local::resolve_addr(&args.local_host, args.port).await?,
                subdomain: st.subdomain.clone(),
                secret: args.secret.clone(),
                secret_key: st.secret_key,
                session: st.live_session(),
                enrollment: cli.enroll.clone(),
                node_name: st.name.clone(),
                no_portmap: env_flag("TELESH_NO_PORTMAP"),
                persist_file: Some(state_path.clone()),
                tcp: None,
            };
            info!(server = %opts.control_addr, local = %opts.local, "telesh agent starting");
            reconnect_loop!(cli.no_reconnect, run_once(&opts))
        }
        Command::Tcp(args) => {
            let opts = AgentOptions {
                control_addr: args.to.clone(),
                local: local::resolve_addr(&args.local_host, args.port).await?,
                subdomain: None,
                secret: args.secret.clone(),
                secret_key: st.secret_key,
                session: st.live_session(),
                enrollment: cli.enroll.clone(),
                node_name: st.name.clone(),
                no_portmap: env_flag("TELESH_NO_PORTMAP"),
                persist_file: Some(state_path.clone()),
                tcp: Some(control::TcpOptions {
                    requested_port: args.remote_port,
                }),
            };
            info!(server = %opts.control_addr, local = %opts.local, "telesh agent starting");
            reconnect_loop!(cli.no_reconnect, run_once(&opts))
        }
        Command::P2p(args) => {
            let mut services = Vec::new();
            if !args.expose.is_empty() {
                for spec in &args.expose {
                    services.push(p2p::P2PExpose {
                        name: Some(spec.name.clone()),
                        local: local::resolve_addr(&args.local_host, spec.port).await?,
                    });
                }
            } else if let Some(port) = args.port {
                services.push(p2p::P2PExpose {
                    name: args.name.clone(),
                    local: local::resolve_addr(&args.local_host, port).await?,
                });
            } else {
                anyhow::bail!("provide <PORT> or at least one --expose NAME=PORT");
            }
            let opts = p2p::ExposeOptions {
                control_addr: args.to.clone(),
                services,
                secret: args.secret.clone(),
                no_portmap: env_flag("TELESH_NO_PORTMAP"),
                secret_key: st.secret_key,
                session: st.live_session(),
                enrollment: cli.enroll.clone(),
                node_name: st.name.clone(),
                persist_file: Some(state_path.clone()),
                map_tcp: args.map_tcp,
            };
            info!(server = %opts.control_addr, "telesh p2p starting");
            reconnect_loop!(cli.no_reconnect, p2p::run_expose(&opts))
        }
        Command::Connect(args) => {
            let (account, name) = match args.target.split_once('/') {
                Some((a, n)) if !a.is_empty() && !n.is_empty() => (a.to_string(), n.to_string()),
                _ => anyhow::bail!("target must be ACCOUNT/NAME, e.g. 000012345/web"),
            };
            let local = local::resolve_addr(&args.local_host, args.local_port).await?;
            let opts = p2p::ConnectOptions {
                control_addr: args.to.clone(),
                account,
                name,
                local,
                secret: args.secret.clone(),
                no_portmap: env_flag("TELESH_NO_PORTMAP"),
                retry_wait: Duration::from_secs(RETRY_WAIT_SECS),
            };
            info!(server = %opts.control_addr, account = %opts.account, service = %opts.name, local = %opts.local, "telesh connect starting");
            reconnect_loop!(cli.no_reconnect, p2p::run_connect(&opts))
        }
    }
}