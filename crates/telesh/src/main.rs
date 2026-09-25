//! telesh agent:常驻进程 —— 发现本机监听端口,按本地 UI 上的配置发布到 control
//! server(http / tcp / p2p)或镜像远端服务。
//!
//! 命令行只有一个开关:数据目录(dataroot),默认 `~/.telesh`。要连哪个 control
//! server 写在 `<dataroot>/config.toml` 里:
//!
//! ```toml
//! control_server = "tunnel.example.com:9009"
//! ```
//!
//! 发布与镜像在本地 UI(`http://127.0.0.1:16345`)上配置。

mod auth;
mod config;
mod daemon;
mod data;
mod inspect;
mod local;
mod p2p;
mod ports;
mod publish;
mod state;
mod token_check;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// 本地发布管理 UI 的地址(固定,不对外)。
const UI_ADDR: &str = "127.0.0.1:16345";

#[derive(Debug, Parser)]
#[command(name = "telesh", version, about = "暴露本地端口:常驻 agent + 本地发布管理 UI")]
struct Cli {
    /// 数据目录:配置、身份/会话、发布配置都在这里。默认 `~/.telesh`。
    #[arg(long, value_name = "DIR")]
    dataroot: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("telesh=info")),
        )
        .init();

    let dataroot = Cli::parse()
        .dataroot
        .unwrap_or_else(state::default_dataroot);
    std::fs::create_dir_all(&dataroot)
        .with_context(|| format!("cannot create dataroot {}", dataroot.display()))?;

    // 配置文件是唯一的地址来源,没有命令行参数、也没有环境变量。
    let cfg = config::Config::load(&dataroot)?;
    let Some(control_addr) = cfg.control_server() else {
        anyhow::bail!(
            "{} 里没有 control_server;请写一行:`control_server = \"host:port\"`",
            config::file_path(&dataroot).display()
        );
    };

    let state_path = state::state_path(&dataroot);
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

    let creds = daemon::Credentials {
        control_addr: control_addr.clone(),
        secret_key: state.secret_key,
        session: state.live_session(),
        node_name: state.name.clone(),
        state_path,
    };
    info!(server = %control_addr, dataroot = %dataroot.display(), ui = UI_ADDR, "telesh agent starting");
    daemon::run(creds).await
}
