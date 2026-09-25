//! telesh agent:常驻进程 —— 发现本机监听端口,按本地 UI 上的配置发布到 control
//! server(http / tcp / p2p)或镜像远端服务。
//!
//! ```text
//! telesh <control-server>
//! ```
//!
//! 发布与镜像都在本地 UI(`http://127.0.0.1:16345`)上配置。

mod auth;
mod daemon;
mod data;
mod inspect;
mod local;
mod p2p;
mod ports;
mod publish;
mod state;
mod token_check;

use std::net::SocketAddr;
use std::path::PathBuf;
use anyhow::Result;
use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// 读一个布尔环境变量("1"/"true"/"yes")。
fn env_flag(key: &str) -> bool {
    matches!(
        std::env::var(key).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

#[derive(Debug, Parser)]
#[command(name = "telesh", version, about = "暴露本地端口:常驻 agent + 本地发布管理 UI")]
struct Cli {
    /// 控制服务器地址,如 `srv:9009`。
    control_server: Option<String>,

    /// 身份与会话文件(identity + session;publish 配置放在同目录)。
    #[arg(long, global = true, env = "TELESH_STATE")]
    state_file: Option<PathBuf>,

    /// 本地发布管理 UI 的监听地址。
    #[arg(long, global = true, env = "TELESH_INSPECTOR_ADDR", default_value = "127.0.0.1:16345")]
    dashboard_addr: SocketAddr,

}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("telesh=info")),
        )
        .init();

    let cli = Cli::parse();
    let state_path = cli.state_file.clone().unwrap_or_else(state::default_path);

    let state = match state::load(&state_path) {
        Ok(s) => s,
        Err(e) => {
            warn!("cannot load state from {}: {e:#}", state_path.display());
            state::AgentState::fresh()
        }
    };

    // 本地发布管理 UI(常驻;由 daemon 和 API 共用一份共享状态)。
    inspect::init();
    publish::set_state_path(state_path.clone());
    {
        let addr = cli.dashboard_addr;
        tokio::spawn(async move {
            if let Err(e) = inspect::serve_dashboard(addr).await {
                warn!("inspector dashboard exited: {e:#}");
            }
        });
    }

    let Some(control_addr) = cli.control_server.clone() else {
        anyhow::bail!("用法:telesh <control-server>,例如 `telesh srv:9009`");
    };
    let creds = daemon::Credentials {
        control_addr: control_addr.clone(),
        secret_key: state.secret_key,
        session: state.live_session(),
        node_name: state.name.clone(),
        state_path,
        no_portmap: env_flag("TELESH_NO_PORTMAP"),
    };
    info!(server = %control_addr, ui = %cli.dashboard_addr, "telesh agent starting");
    daemon::run(creds).await
}
