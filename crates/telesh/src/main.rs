//! telesh agent:常驻进程 —— 发现本机监听端口,按本地 UI 上的配置发布到 control
//! server(http / tcp / p2p)或镜像远端服务。
//!
//! 数据目录(默认 `~/.telesh`)里放配置、身份/会话、发布配置;
//! 要连哪个 control server 写在 `<dataroot>/config.toml` 里:
//!
//! ```toml
//! control_server = "tunnel.example.com:9009"
//! ```
//!
//! 命令:
//!
//! - `telesh`(无子命令):跑常驻 agent;面板在 `http://127.0.0.1:16345`
//!   (用 `--ui-port` 换)。端口上已经有另一个 telesh 时,提示并正常退出。
//! - `telesh open`:在浏览器里打开正在运行的面板(先确认那里真的是 telesh)。
//! - `telesh http|tcp|p2p|connect|publish`:发布 / 镜像 / P2P 规则的 CLI 前端,
//!   只对正在运行的 agent 生效(见 `cli.rs`)。

mod account;
mod auth;
mod cli;
mod cli_client;
mod config;
mod daemon;
mod data;
mod directory;
mod inspect;
mod instance;
mod local;
mod p2p;
mod ports;
mod publish;
mod state;
mod token_check;
mod trace_log;

use std::net::{Ipv4Addr, SocketAddr};

use anyhow::{Context, Result};
use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("telesh=info")),
        )
        .init();

    let cli = Cli::parse();

    // 子命令:不碰数据目录、不读配置,直接对已经在跑的实例干活。
    if let Some(command) = cli.command {
        return cli::run(command, cli.ui_port, cli.json).await;
    }

    // 先把面板端口抢下来。端口被占要在动 dataroot/state 之前就判掉:
    // 是另一个 telesh 就提示并退出,是别的程序就报错。
    let ui_addr = SocketAddr::from((Ipv4Addr::LOCALHOST, cli.ui_port));
    let listener = match inspect::bind_dashboard(ui_addr).await {
        Ok(listener) => listener,
        Err(_) => return instance::handle_occupied(cli.ui_port, cli.open),
    };

    let dataroot = config::expand_tilde(&cli.dataroot.unwrap_or_else(state::default_dataroot));
    std::fs::create_dir_all(&dataroot)
        .with_context(|| format!("cannot create dataroot {}", dataroot.display()))?;

    // 配置文件是唯一的地址来源;不存在就自动写一份默认的(连本机 9009)。
    let cfg = config::Config::load_or_create(&dataroot)?;
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

    // 本地发布管理 UI(dashboard 已经在跑了,这里接上剩余的接线)。
    // 请求日志按通道持久化到 <dataroot>/traces/。
    inspect::init(dataroot.join("traces"));
    publish::set_state_path(state_path.clone());
    tokio::spawn(async move {
        if let Err(e) = inspect::serve(listener).await {
            warn!("inspector dashboard exited: {e:#}");
        }
    });
    if cli.open {
        instance::open_panel(&format!("http://127.0.0.1:{}", cli.ui_port));
    }

    account::init(
        control_addr.clone(),
        dataroot.display().to_string(),
        *iroh::SecretKey::from_bytes(&state.secret_key).public().as_bytes(),
        state.name.clone(),
    );

    let creds = daemon::Credentials {
        control_addr: control_addr.clone(),
        secret_key: state.secret_key,
        session: state.live_session(),
        node_name: state.name.clone(),
        state_path,
    };
    info!(server = %control_addr, dataroot = %dataroot.display(), ui = %ui_addr, "telesh agent starting");
    daemon::run(creds).await
}
