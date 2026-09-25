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

use anyhow::Result;
use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// 本地发布管理 UI 的地址(固定,不对外)。
const UI_ADDR: &str = "127.0.0.1:16345";

#[derive(Debug, Parser)]
#[command(name = "telesh", version, about = "暴露本地端口:常驻 agent + 本地发布管理 UI")]
struct Cli {
    /// 控制服务器地址,如 `srv:9009`。
    control_server: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("telesh=info")),
        )
        .init();

    let cli = Cli::parse();
    // 身份/会话文件固定在 <home>/.telesh/state;publish 配置放在同一目录。
    let state_path = state::default_path();

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
        let addr: std::net::SocketAddr = UI_ADDR.parse().expect("valid UI address");
        tokio::spawn(async move {
            if let Err(e) = inspect::serve_dashboard(addr).await {
                warn!("inspector dashboard exited: {e:#}");
            }
        });
    }

    let control_addr = cli.control_server;
    let creds = daemon::Credentials {
        control_addr: control_addr.clone(),
        secret_key: state.secret_key,
        session: state.live_session(),
        node_name: state.name.clone(),
        state_path,
    };
    info!(server = %control_addr, ui = UI_ADDR, "telesh agent starting");
    daemon::run(creds).await
}
