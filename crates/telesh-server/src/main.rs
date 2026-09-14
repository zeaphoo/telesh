//! telesh-server binary entrypoint.

use std::net::SocketAddr;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use telesh_server::{config, control, dashboard, ingress, state, Config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("telesh_server=info,telesh_server::ingress=debug,tower_http=info")),
        )
        .init();

    let cfg = Config::parse();
    config::init(cfg.clone());

    state::init();

    let ctrl_addr = cfg.control_addr;
    let dash_addr = cfg.dashboard_addr;

    // HTTP control server (serves /tunnel WebSocket + /healthz).
    let app = control::router().into_make_service_with_connect_info::<SocketAddr>();
    let ctrl = async move {
        let listener = tokio::net::TcpListener::bind(ctrl_addr).await?;
        tracing::info!(%ctrl_addr, "control server listening (ws://.../tunnel)");
        axum::serve(listener, app).await?;
        Ok::<_, anyhow::Error>(())
    };

    // Dashboard (API + built frontend + prometheus).
    let dash = async move {
        let app = dashboard::router();
        let listener = tokio::net::TcpListener::bind(dash_addr).await?;
        tracing::info!(%dash_addr, "dashboard listening (http://.../)");
        axum::serve(listener, app).await?;
        Ok::<_, anyhow::Error>(())
    };

    tokio::try_join!(ctrl, ingress::run_ingress(), dash)?;
    Ok(())
}