//! telesh — tunnel agent: expose a local service through a public entry.
//!
//!   telesh http 8000                # random subdomain on 127.0.0.1:9009
//!   telesh http 8000 --to srv:9009  # against a remote server
//!   telesh http 22  -s myssh        # custom subdomain
//!   telesh tcp 22                   # raw TCP tunnel (random public port)
//!   telesh tcp 22 --remote-port 19022

mod client;
mod inspect;
mod local;
mod state;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use client::{run_once, AgentOptions};

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
    #[arg(long, global = true, env = "TELESH_INSPECTOR_ADDR", default_value = "127.0.0.1:7400")]
    dashboard_addr: SocketAddr,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// HTTP tunnel: expose a local port under a public URL (Host-header routed).
    Http(HttpArgs),
    /// Raw TCP tunnel: expose a local port as tcp://<server>:<port>.
    Tcp(TcpArgs),
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

    let opts = match &cli.command {
        Command::Http(args) => {
            // An explicit subdomain overrides any persisted one.
            if let Some(s) = &args.subdomain {
                st.subdomain = Some(s.clone());
            }
            AgentOptions {
                control_addr: args.to.clone(),
                local: local::resolve_addr(&args.local_host, args.port).await?,
                subdomain: st.subdomain.clone(),
                secret: args.secret.clone(),
                agent_id: st.agent_id.clone(),
                persist_file: Some(state_path.clone()),
                tcp: None,
            }
        }
        Command::Tcp(args) => AgentOptions {
            control_addr: args.to.clone(),
            local: local::resolve_addr(&args.local_host, args.port).await?,
            subdomain: None,
            secret: args.secret.clone(),
            agent_id: st.agent_id.clone(),
            persist_file: Some(state_path.clone()),
            tcp: Some(client::TcpOptions {
                requested_port: args.remote_port,
            }),
        },
    };

    info!(server = %opts.control_addr, local = %opts.local, "telesh agent starting");

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

    let mut attempt: u32 = 0;
    loop {
        match run_once(&opts).await {
            Ok(_) => info!("control connection closed"),
            Err(e) => warn!("connection failed: {e:#}"),
        }

        if cli.no_reconnect {
            return Ok(());
        }

        attempt += 1;
        let backoff = Duration::from_millis((500 * attempt.min(10)) as u64);
        warn!(ms = backoff.as_millis(), "reconnecting...");
        tokio::time::sleep(backoff).await;
    }
}