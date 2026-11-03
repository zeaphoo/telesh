//! `telesh` 的子命令:发布 / 镜像 / P2P 规则的命令行前端。
//!
//! 所有子命令都只和**正在运行**的本机 agent 的 HTTP API 通信
//! (`127.0.0.1:16345`,可用 `--ui-port` 改),不自己起进程、不碰 dataroot。
//! 配置与复杂的观测留在本地 Web UI:`telesh open`。
//!
//! 这些命令不是旧版「一隧道一进程」的子命令:它们只是把条目写进 daemon 的
//! desired state(`PUT /api/publish`),由常驻 agent 统一 reconcile。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};

use crate::account::AccountInfo;
use crate::cli_client::AgentClient;
use crate::inspect::RequestRecord;
use crate::instance::{humanize, Instance};
use crate::ports::PortInfo;
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

    /// 查看 agent、账户与隧道的整体状态。
    Status,

    /// 列出本机监听端口,并标出已经发布的方式。
    Ports,

    /// 查看最近的请求记录;用 `--id` 查看单条请求/响应报文。
    #[command(visible_alias = "requests")]
    Logs {
        /// 只显示某个通道,例如 `publish:8000` / `p2p:5432`。
        #[arg(short = 'c', long, value_name = "CHANNEL")]
        channel: Option<String>,

        /// 最多显示多少条(最新在前)。
        #[arg(short = 'n', long, default_value_t = 30, value_name = "N")]
        limit: usize,

        /// 查看单条请求的完整请求/响应报文。
        #[arg(long, value_name = "ID", conflicts_with = "follow")]
        id: Option<u64>,

        /// 持续输出新请求(类似 `tail -f`)。
        #[arg(long, conflicts_with = "id")]
        follow: bool,
    },

    /// 查看账户、登录状态与会话信息。
    Account,

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
        Command::Status => show_status(&client, json).await,
        Command::Ports => show_ports(&client, json).await,
        Command::Logs {
            channel,
            limit,
            id,
            follow,
        } => show_logs(&client, channel, limit, id, follow, json).await,
        Command::Account => show_account(&client, json).await,
        Command::P2p { ports, all, none } => set_p2p(&client, ports, all, none, json).await,
        Command::Publish { command } => match command {
            PublishCommand::Ls => list(&client, json).await,
            PublishCommand::Rm { kind, port } => remove(&client, &kind, port, json).await,
        },
    }
}

// ---- 命令实现 -------------------------------------------------------------

/// `status`:一个不需要打开浏览器的「现在怎么样」快照。
async fn show_status(client: &AgentClient, json: bool) -> Result<()> {
    let (instance, account, publish) = tokio::try_join!(
        client.get::<Instance>("/api/instance"),
        client.get::<AccountInfo>("/api/account"),
        client.get::<Vec<EntryStatus>>("/api/publish"),
    )?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "instance": instance,
                "account": account,
                "publish": publish,
            }))?
        );
        return Ok(());
    }

    let live = publish.iter().filter(|s| s.state == State::Live).count();
    println!(
        "telesh {}  pid {}  已运行 {}",
        instance.version,
        instance.pid,
        humanize(instance.uptime_secs)
    );
    println!("面板        {}", instance.url());
    println!("控制服务器  {}", dash(&account.control_server));

    let account_id = account.account.as_deref().unwrap_or("—");
    let display_name = account
        .display_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("  {s}"))
        .unwrap_or_default();
    println!(
        "账户        {} ({}){}",
        account_id,
        account_state(&account.state),
        display_name
    );
    if let Some(error) = account.last_error.as_deref() {
        println!("最近错误    {error}");
    }
    println!("通道        {live} live / {} total", publish.len());
    for status in &publish {
        println!(
            "  {:<7} {:<6} {:<11} {}{}",
            status.entry.kind(),
            status.entry.local_port(),
            state_str(status.state),
            status.detail.as_deref().unwrap_or_else(|| "—"),
            if status.entry.kind() == "p2p" || status.entry.kind() == "mirror" {
                format!(" ({})", target_of(&status.entry))
            } else {
                String::new()
            }
        );
    }
    Ok(())
}

/// `ports`:本机发现了什么,以及它是否已被发布。
async fn show_ports(client: &AgentClient, json: bool) -> Result<()> {
    let (ports, publish) = tokio::try_join!(
        client.get::<Vec<PortInfo>>("/api/ports"),
        client.get::<Vec<EntryStatus>>("/api/publish"),
    )?;

    if json {
        let rows: Vec<_> = ports
            .iter()
            .map(|port| {
                let published: Vec<_> = publish
                    .iter()
                    .filter(|s| s.entry.local_port() == port.port)
                    .map(|s| {
                        serde_json::json!({
                            "kind": s.entry.kind(),
                            "state": state_str(s.state),
                            "detail": s.detail,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "port": port.port,
                    "addr": port.addr,
                    "pid": port.pid,
                    "process": port.process,
                    "http_hint": port.http_hint,
                    "ssh_hint": port.ssh_hint,
                    "server": port.server,
                    "probed": port.probed,
                    "v6_only": port.v6_only,
                    "published": published,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if ports.is_empty() {
        println!("没有发现本机监听端口");
        return Ok(());
    }

    let mut published: BTreeMap<u16, Vec<String>> = BTreeMap::new();
    for status in &publish {
        published
            .entry(status.entry.local_port())
            .or_default()
            .push(format!(
                "{}:{}",
                status.entry.kind(),
                state_str(status.state)
            ));
    }

    println!(
        "{:<6} {:<18} {:<7} {:<16} {:<7} {}",
        "PORT", "ADDR", "PID", "PROCESS", "PROTO", "PUBLISHED"
    );
    for port in ports {
        let proto = if port.ssh_hint {
            "ssh"
        } else if port.http_hint {
            if port.probed {
                "http"
            } else {
                "http?"
            }
        } else {
            "-"
        };
        let published = published
            .get(&port.port)
            .map(|v| v.join(", "))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<6} {:<18} {:<7} {:<16} {:<7} {}",
            port.port,
            format!("{}:{}", port.addr, port.port),
            port.pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".to_string()),
            elide(port.process.as_deref().unwrap_or("-"), 16),
            proto,
            published
        );
    }
    Ok(())
}

/// `account`:登录状态;`--json` 直接给脚本用。
async fn show_account(client: &AgentClient, json: bool) -> Result<()> {
    let account: AccountInfo = client.get("/api/account").await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&account)?);
        return Ok(());
    }

    println!("状态        {}", account_state(&account.state));
    println!("账户        {}", account.account.as_deref().unwrap_or("—"));
    println!(
        "显示名      {}",
        account
            .display_name
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("—")
    );
    println!("控制服务器  {}", dash(&account.control_server));
    println!("数据目录    {}", dash(&account.dataroot));
    println!("身份公钥    {}", dash(&account.public_key));
    println!("会话过期    {}", session_expiry(account.session_expires_at));
    println!("最近登录    {}", since(account.last_login_at));
    if let Some(error) = account.last_error.as_deref() {
        println!("最近错误    {error}");
    }
    Ok(())
}

/// `logs` / `logs --follow` / `logs --id`。
async fn show_logs(
    client: &AgentClient,
    channel: Option<String>,
    limit: usize,
    id: Option<u64>,
    follow: bool,
    json: bool,
) -> Result<()> {
    if let Some(id) = id {
        let record: RequestRecord = client.get(&format!("/api/requests/{id}")).await?;
        if json {
            println!("{}", serde_json::to_string_pretty(&record)?);
        } else {
            println!(
                "#{}  {} {}  →  {}  ({} ms)",
                record.id,
                record.method,
                record.path,
                record
                    .status
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                record.duration_ms
            );
            println!(
                "通道 {}  请求 {} B  响应 {} B",
                record.channel, record.req_bytes, record.resp_bytes
            );
            println!("\n--- 请求 ---\n{}", raw_or_placeholder(&record.payload));
            println!(
                "\n--- 响应 ---\n{}",
                raw_or_placeholder(&record.resp_payload)
            );
        }
        return Ok(());
    }

    let limit = limit.max(1);
    if follow {
        return follow_logs(client, channel.as_deref(), limit.max(100), json).await;
    }

    let records = fetch_logs(client, channel.as_deref(), limit).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&records)?);
        return Ok(());
    }
    if records.is_empty() {
        println!("没有请求记录");
        return Ok(());
    }
    print_log_header();
    for record in &records {
        print_log_row(record);
    }
    Ok(())
}

async fn fetch_logs(
    client: &AgentClient,
    channel: Option<&str>,
    limit: usize,
) -> Result<Vec<RequestRecord>> {
    let mut path = format!("/api/requests?limit={limit}");
    if let Some(channel) = channel {
        path.push_str("&channel=");
        path.push_str(
            &url::form_urlencoded::byte_serialize(channel.as_bytes()).collect::<String>(),
        );
    }
    Ok(client.get(&path).await?)
}

async fn follow_logs(
    client: &AgentClient,
    channel: Option<&str>,
    limit: usize,
    json: bool,
) -> Result<()> {
    let mut last_id = 0u64;
    let mut first = true;
    if !json {
        print_log_header();
    }
    loop {
        let records = fetch_logs(client, channel, limit).await?;
        let mut fresh: Vec<RequestRecord> =
            records.into_iter().filter(|r| r.id > last_id).collect();
        // 跟随模式按时间正序输出,和 `tail -f` 一致。
        fresh.sort_by_key(|r| r.id);
        for record in &fresh {
            if json {
                println!("{}", serde_json::to_string(record)?);
            } else {
                print_log_row(record);
            }
            last_id = last_id.max(record.id);
        }
        if first && !json && fresh.is_empty() {
            println!("等待新请求…(Ctrl-C 退出)");
        }
        first = false;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn print_log_header() {
    println!(
        "{:<6} {:<12} {:<7} {:<7} {:>6} {:>11} {:<14} {}",
        "ID", "TIME", "METHOD", "STATUS", "MS", "BYTES", "CHANNEL", "PATH"
    );
}

fn print_log_row(record: &RequestRecord) {
    println!(
        "{:<6} {:<12} {:<7} {:<7} {:>6} {:>11} {:<14} {}",
        record.id,
        elide(&record.started_at, 12),
        elide(&record.method, 7),
        record
            .status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string()),
        record.duration_ms,
        format!("{}/{}", record.req_bytes, record.resp_bytes),
        elide(&record.channel, 14),
        elide(&record.path, 80)
    );
}

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

// ---- 输出 / 时间小工具 ----------------------------------------------------

fn dash(value: &str) -> &str {
    if value.trim().is_empty() {
        "—"
    } else {
        value
    }
}

fn account_state(state: &str) -> &str {
    match state {
        "online" => "已登录",
        "connecting" => "连接中",
        _ => "未连接",
    }
}

/// 按字符截断,避免把多字节字符切成两半。
fn elide(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn session_expiry(timestamp: u64) -> String {
    if timestamp == 0 {
        return "—".to_string();
    }
    let now = unix_now();
    if timestamp <= now {
        "已过期".to_string()
    } else {
        format!("{} 后", humanize(timestamp - now))
    }
}

fn since(timestamp: u64) -> String {
    if timestamp == 0 {
        return "从未".to_string();
    }
    let now = unix_now();
    if timestamp >= now {
        "刚刚".to_string()
    } else {
        format!("{} 前", humanize(now - timestamp))
    }
}

fn raw_or_placeholder(raw: &str) -> &str {
    if raw.is_empty() {
        "(空)"
    } else {
        raw
    }
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
