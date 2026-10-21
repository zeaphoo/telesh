//! `telesh` 的子命令:发布 / 镜像 / P2P 规则的命令行前端。
//!
//! 所有子命令都只和**正在运行**的本机 agent 的 HTTP API 通信
//! (`127.0.0.1:16345`,可用 `--ui-port` 改),不自己起进程、不碰 dataroot。
//! 配置与复杂的观测留在本地 Web UI:`telesh open`。
//!
//! 这些命令不是旧版「一隧道一进程」的子命令:它们只是把条目写进 daemon 的
//! desired state(`PUT /api/publish`),由常驻 agent 统一 reconcile。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};

use crate::cli_client::AgentClient;
use crate::publish::{Entry, EntryStatus, P2pMode, P2pPolicy, State};

/// 本地发布管理 UI 的默认端口(永远只监听 127.0.0.1)。
pub const DEFAULT_UI_PORT: u16 = 16345;

/// 等 daemon 把一条新发布 reconcile 到 live/error 的上限。
const READY_TIMEOUT: Duration = Duration::from_secs(5);
const READY_POLL: Duration = Duration::from_millis(200);

#[derive(Debug, Parser)]
#[command(
    name = "telesh",
    version,
    about = "暴露本地端口:常驻 agent + 本地发布管理 UI/CLI"
)]
pub struct Cli {
    /// 数据目录:配置、身份/会话、发布配置都在这里。默认 `~/.telesh`。
    #[arg(long, value_name = "DIR", global = true)]
    pub dataroot: Option<PathBuf>,

    /// 本地面板端口。
    #[arg(long, value_name = "PORT", default_value_t = DEFAULT_UI_PORT, global = true)]
    pub ui_port: u16,

    /// 就绪后在默认浏览器里打开面板(没有图形会话时自动跳过)。
    #[arg(long, global = true)]
    pub open: bool,

    /// 以 JSON 输出(用于脚本;仅子命令)。
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 在浏览器里打开正在运行的面板。
    Open {
        /// 只打印地址,不调起浏览器。
        #[arg(long)]
        print_only: bool,
    },

    /// 把本地端口发布为 HTTP 子域。
    Http {
        /// 本地端口。
        #[arg(value_name = "PORT")]
        port: u16,
        /// 固定子域名(不填由服务端分配)。
        #[arg(short = 's', long = "sub-domain", value_name = "NAME")]
        sub_domain: Option<String>,
    },

    /// 把本地端口发布为公网 TCP 端口。
    Tcp {
        /// 本地端口。
        #[arg(value_name = "PORT")]
        port: u16,
        /// 指定公网端口(不填由服务端分配)。
        #[arg(short = 'r', long = "remote-port", value_name = "PORT")]
        remote_port: Option<u16>,
    },

    /// 设置 P2P 入站规则:允许远端 peer 直连哪些本地端口。
    ///
    /// 带端口参数时是**覆盖式**的选定列表;`--all` / `--none` 是另外两种模式。
    P2p {
        /// 允许的本地端口(可写多个)。
        #[arg(value_name = "PORT", conflicts_with_all = ["all", "none"])]
        ports: Vec<u16>,
        /// 允许所有正在监听的本地端口。
        #[arg(long, conflicts_with = "none")]
        all: bool,
        /// 关闭 P2P 入站。
        #[arg(long)]
        none: bool,
    },

    /// 把远端账户的服务镜像到本地端口。
    Connect {
        /// 远端服务,格式 `ACCOUNT/NAME`,例如 `000012345/web`。
        #[arg(value_name = "ACCOUNT/NAME")]
        remote: String,
        /// 本地监听端口。
        #[arg(short = 'l', long = "local-port", value_name = "PORT")]
        local_port: u16,
    },

    /// 查看或删除发布条目。
    Publish {
        #[command(subcommand)]
        command: PublishCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum PublishCommand {
    /// 列出当前发布 / 镜像 / 由 P2P 规则推导出的条目。
    Ls,
    /// 删除一条发布条目(不能删 P2P;请用 `telesh p2p` 改规则)。
    Rm {
        /// 类型:`http` / `tcp` / `mirror`。
        #[arg(value_name = "KIND")]
        kind: String,
        /// 本地端口。
        #[arg(value_name = "PORT")]
        port: u16,
    },
}

/// 执行一个子命令。`open` 不依赖 agent 的 HTTP API,其余都需要。
pub async fn run(command: Command, ui_port: u16, json: bool) -> Result<()> {
    if let Command::Open { print_only } = command {
        let info = AgentClient::probe(ui_port).await?;
        let url = info.url();
        println!("{url}");
        if !print_only {
            crate::instance::open_panel(&url);
        }
        return Ok(());
    }

    let client = AgentClient::connect(ui_port).await?;
    match command {
        Command::Open { .. } => unreachable!("handled above"),
        Command::Http { port, sub_domain } => {
            publish(
                &client,
                Entry::Http {
                    local_port: port,
                    sub_domain,
                },
                json,
            )
            .await
        }
        Command::Tcp { port, remote_port } => {
            publish(
                &client,
                Entry::Tcp {
                    local_port: port,
                    remote_port,
                },
                json,
            )
            .await
        }
        Command::Connect { remote, local_port } => {
            validate_remote(&remote)?;
            publish(&client, Entry::Mirror { local_port, remote }, json).await
        }
        Command::P2p { ports, all, none } => set_p2p(&client, ports, all, none, json).await,
        Command::Publish { command } => match command {
            PublishCommand::Ls => list(&client, json).await,
            PublishCommand::Rm { kind, port } => remove(&client, &kind, port, json).await,
        },
    }
}

// ---- 命令实现 -------------------------------------------------------------

async fn publish(client: &AgentClient, entry: Entry, json: bool) -> Result<()> {
    let mut entries = desired(client).await?;
    upsert(&mut entries, entry.clone());
    let _: Vec<EntryStatus> = client.put("/api/publish", &as_statuses(entries)).await?;

    let status = wait_for_entry(client, &entry).await;
    let status = status.unwrap_or_else(|| EntryStatus {
        entry: entry.clone(),
        state: State::Pending,
        detail: None,
    });

    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!(
            "{:<7} {:<6} -> {:<24} [{}]{}",
            status.entry.kind(),
            status.entry.local_port(),
            target_of(&status.entry),
            state_str(status.state),
            status
                .detail
                .as_deref()
                .map(|d| format!(" {d}"))
                .unwrap_or_default()
        );
    }
    Ok(())
}

async fn list(client: &AgentClient, json: bool) -> Result<()> {
    let statuses: Vec<EntryStatus> = client.get("/api/publish").await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&statuses)?);
        return Ok(());
    }
    if statuses.is_empty() {
        println!("没有发布条目");
        return Ok(());
    }
    println!(
        "{:<7} {:<6} {:<24} {:<11} DETAIL",
        "KIND", "LOCAL", "TARGET", "STATE"
    );
    for s in statuses {
        println!(
            "{:<7} {:<6} {:<24} {:<11} {}",
            s.entry.kind(),
            s.entry.local_port(),
            target_of(&s.entry),
            state_str(s.state),
            s.detail.unwrap_or_default()
        );
    }
    Ok(())
}

async fn remove(client: &AgentClient, kind: &str, port: u16, json: bool) -> Result<()> {
    let kind = kind.to_ascii_lowercase();
    if kind == "p2p" {
        bail!("P2P 条目由入站规则生成,不能单独删除;用 `telesh p2p --none` 或重设端口列表");
    }
    if !matches!(kind.as_str(), "http" | "tcp" | "mirror") {
        bail!("未知类型 `{kind}`;可选 http / tcp / mirror");
    }

    let mut entries = desired(client).await?;
    if !remove_entry(&mut entries, &kind, port) {
        bail!("没有找到 {kind} 条目(本地端口 {port})");
    }
    let _: Vec<EntryStatus> = client.put("/api/publish", &as_statuses(entries)).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({ "removed": { "kind": kind, "local_port": port } })
        );
    } else {
        println!("已移除 {kind} 本地端口 {port}");
    }
    Ok(())
}

async fn set_p2p(
    client: &AgentClient,
    ports: Vec<u16>,
    all: bool,
    none: bool,
    json: bool,
) -> Result<()> {
    let policy = p2p_policy(ports, all, none)?;
    let out: P2pPolicy = client.put("/api/p2p-policy", &policy).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    match out.mode {
        P2pMode::All => println!("P2P 入站:允许所有正在监听的本地端口"),
        P2pMode::None => println!("P2P 入站:已关闭"),
        P2pMode::Selected => println!("P2P 入站:允许本地端口 {}", join_ports(&out.ports)),
    }
    Ok(())
}

// ---- 纯逻辑(单元测试在这里) ----------------------------------------------

/// 用 `new` 替换同一「槽位」(kind + local_port)的条目。
fn upsert(entries: &mut Vec<Entry>, new: Entry) {
    entries.retain(|e| !same_slot(e, &new));
    entries.push(new);
}

/// 删除匹配 kind + local_port 的条目;返回是否真的删掉了。
fn remove_entry(entries: &mut Vec<Entry>, kind: &str, port: u16) -> bool {
    let before = entries.len();
    entries.retain(|e| !(e.kind() == kind && e.local_port() == port));
    entries.len() != before
}

fn same_slot(a: &Entry, b: &Entry) -> bool {
    a.kind() == b.kind() && a.local_port() == b.local_port()
}

fn p2p_policy(mut ports: Vec<u16>, all: bool, none: bool) -> Result<P2pPolicy> {
    if all {
        return Ok(P2pPolicy {
            mode: P2pMode::All,
            ports: Vec::new(),
        });
    }
    if none {
        return Ok(P2pPolicy {
            mode: P2pMode::None,
            ports: Vec::new(),
        });
    }
    if ports.is_empty() {
        bail!("`telesh p2p` 需要至少一个端口,或用 `--all` / `--none`");
    }
    ports.sort_unstable();
    ports.dedup();
    Ok(P2pPolicy {
        mode: P2pMode::Selected,
        ports,
    })
}

fn validate_remote(remote: &str) -> Result<()> {
    match remote.split_once('/') {
        Some((account, name)) if !account.is_empty() && !name.is_empty() => Ok(()),
        _ => bail!("远端服务格式应为 ACCOUNT/NAME,例如 `000012345/web`"),
    }
}

fn as_statuses(entries: Vec<Entry>) -> Vec<EntryStatus> {
    entries
        .into_iter()
        .map(|entry| EntryStatus {
            entry,
            state: State::Pending,
            detail: None,
        })
        .collect()
}

/// 当前 desired state。`/api/publish` 还会带回由 P2P 规则推导出的条目,
/// 那些不是用户配置,写回时要去掉,否则会污染 `publish.json`。
async fn desired(client: &AgentClient) -> Result<Vec<Entry>> {
    let statuses: Vec<EntryStatus> = client.get("/api/publish").await?;
    Ok(statuses
        .into_iter()
        .map(|s| s.entry)
        .filter(|e| !matches!(e, Entry::P2p { .. }))
        .collect())
}

async fn wait_for_entry(client: &AgentClient, entry: &Entry) -> Option<EntryStatus> {
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut last = None;
    loop {
        if let Ok(statuses) = client.get::<Vec<EntryStatus>>("/api/publish").await {
            if let Some(status) = statuses.into_iter().find(|s| same_slot(&s.entry, entry)) {
                let done = matches!(status.state, State::Live | State::Error | State::TargetGone);
                last = Some(status);
                if done {
                    return last;
                }
            }
        }
        if Instant::now() >= deadline {
            return last;
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

fn target_of(entry: &Entry) -> String {
    match entry {
        Entry::Http { sub_domain, .. } => sub_domain
            .clone()
            .unwrap_or_else(|| "(自动分配)".to_string()),
        Entry::Tcp { remote_port, .. } => remote_port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "(自动分配)".to_string()),
        Entry::P2p { name, .. } => name.clone(),
        Entry::Mirror { remote, .. } => remote.clone(),
    }
}

fn state_str(state: State) -> &'static str {
    match state {
        State::Pending => "pending",
        State::Live => "live",
        State::Error => "error",
        State::TargetGone => "target_gone",
    }
}

fn join_ports(ports: &[u16]) -> String {
    ports
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_replaces_same_kind_and_port() {
        let mut entries = vec![
            Entry::Http {
                local_port: 8000,
                sub_domain: Some("old".into()),
            },
            Entry::Tcp {
                local_port: 8000,
                remote_port: None,
            },
        ];
        upsert(
            &mut entries,
            Entry::Http {
                local_port: 8000,
                sub_domain: Some("new".into()),
            },
        );
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&Entry::Http {
            local_port: 8000,
            sub_domain: Some("new".into()),
        }));
        assert!(entries.contains(&Entry::Tcp {
            local_port: 8000,
            remote_port: None,
        }));
    }

    #[test]
    fn remove_matches_kind_and_port() {
        let mut entries = vec![
            Entry::Http {
                local_port: 8000,
                sub_domain: None,
            },
            Entry::Mirror {
                local_port: 8000,
                remote: "000012345/web".into(),
            },
        ];
        assert!(remove_entry(&mut entries, "http", 8000));
        assert_eq!(entries.len(), 1);
        assert!(!remove_entry(&mut entries, "tcp", 8000));
    }

    #[test]
    fn p2p_policy_is_overwrite_style() {
        let policy = p2p_policy(vec![9000, 8000, 9000], false, false).unwrap();
        assert_eq!(policy.mode, P2pMode::Selected);
        assert_eq!(policy.ports, vec![8000, 9000]);

        assert!(p2p_policy(Vec::new(), false, false).is_err());

        let all = p2p_policy(Vec::new(), true, false).unwrap();
        assert_eq!(all.mode, P2pMode::All);
        assert!(all.ports.is_empty());

        let none = p2p_policy(Vec::new(), false, true).unwrap();
        assert_eq!(none.mode, P2pMode::None);
    }

    #[test]
    fn remote_needs_account_and_name() {
        assert!(validate_remote("000012345/web").is_ok());
        assert!(validate_remote("000012345/").is_err());
        assert!(validate_remote("/web").is_err());
        assert!(validate_remote("web").is_err());
    }
}
