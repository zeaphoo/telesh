//! `telesh-server` 的本地运维 / 配置子命令。
//!
//! 读操作走正在运行的 dashboard HTTP API(和控制面同一个监听),所以 CLI
//! 不会去碰进程内状态;配置操作直接安全地编辑同一个 TOML 文件,写前用
//! [`Config`] 做完整校验。这样用户不必手工改配置文件,也不会把服务写坏。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::config::{Command, Config, ConfigCommand, TcpPortRange};
use crate::metrics::MetricsSnapshot;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// 所有可由 `config set` 修改的键。顺序也是 `config keys` 的输出顺序。
const CONFIG_KEYS: &[(&str, &str, &str)] = &[
    ("dataroot", "path", "数据根目录(数据库、ACME 缓存等)"),
    ("control_addr", "addr", "控制面 + dashboard 监听地址"),
    ("ingress_addr", "addr", "公网 HTTP/TCP 入口监听地址"),
    ("data_addr", "addr", "agent 数据面监听地址"),
    ("public_host", "host", "拼隧道 URL 用的公网后缀"),
    ("tcp_port_range", "START-END", "公网 TCP 端口范围"),
    ("relay.serve", "addr", "内嵌 iroh 中继监听地址;unset 关闭"),
    ("relay.url", "url", "通告给 agent 的中继 URL"),
    ("relay.tls_cert", "path", "中继 TLS 证书链(PEM)"),
    ("relay.tls_key", "path", "中继 TLS 私钥(PEM)"),
    ("relay.https_addr", "addr", "中继 HTTPS 监听地址"),
    ("relay.quic_addr", "addr", "中继 QUIC 地址发现监听地址"),
    ("relay.acme", "domain,...", "ACME 域名列表(逗号分隔)"),
    ("relay.acme_email", "email", "ACME 账户联系邮箱"),
    ("relay.acme_staging", "bool", "使用 Let's Encrypt staging"),
    ("relay.acme_cache", "path", "ACME 证书缓存目录"),
];

/// 执行一个运维 / 配置子命令。`Serve` 由 `main` 自己处理。
pub async fn run(command: Command, config_path: PathBuf, json: bool) -> Result<()> {
    match command {
        Command::Serve => unreachable!("serve is handled by main"),
        Command::Status => status(&config_path, json).await,
        Command::Clients => clients(&config_path, json).await,
        Command::Tunnels => tunnels(&config_path, json).await,
        Command::Metrics { prometheus } => metrics(&config_path, prometheus, json).await,
        Command::Open { print_only } => open(&config_path, print_only).await,
        Command::Config { command } => config_command(&config_path, command, json),
    }
}

// ---------------------------------------------------------------------------
// 运行时观察:只读 dashboard API
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Instance {
    app: String,
    version: String,
    pid: u32,
    control_addr: String,
    dashboard_url: String,
    uptime_secs: u64,
    started_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClientRow {
    agent_id: String,
    sub_domain: String,
    online_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TunnelRow {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    remote: String,
    status: String,
    in_bytes: u64,
    out_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TunnelsResponse {
    tunnels: Vec<TunnelRow>,
}

async fn status(config_path: &Path, json: bool) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let addr = cfg.dashboard_addr();
    let probe = ServerClient::probe(addr).await;

    if json {
        let payload = match probe {
            Ok(info) => {
                let metrics = ServerClient::new(addr)
                    .get_json::<MetricsSnapshot>("/api/metrics")
                    .await
                    .ok();
                serde_json::json!({
                    "config_path": config_path.display().to_string(),
                    "config": cfg,
                    "running": true,
                    "instance": info,
                    "metrics": metrics,
                })
            }
            Err(e) => serde_json::json!({
                "config_path": config_path.display().to_string(),
                "config": cfg,
                "running": false,
                "error": format!("{e:#}"),
            }),
        };
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    print_config_summary(&cfg, config_path);
    match probe {
        Ok(info) => {
            println!("状态        运行中");
            println!("版本        {}  pid {}", info.version, info.pid);
            println!("面板        {}", info.dashboard_url);
            println!("已运行      {}", humanize(info.uptime_secs));
            match ServerClient::new(addr)
                .get_json::<MetricsSnapshot>("/api/metrics")
                .await
            {
                Ok(m) => {
                    println!("在线 agent  {}", m.agents_online);
                    println!("活动连接    {}", m.active_connections);
                    println!(
                        "隧道        {} ({} http / {} tcp)",
                        m.http_tunnels + m.tcp_tunnels,
                        m.http_tunnels,
                        m.tcp_tunnels
                    );
                    println!("HTTP 请求   {}", m.requests_total);
                    println!(
                        "流量        ↑ {}  ↓ {}",
                        human_bytes(m.traffic_in_bytes),
                        human_bytes(m.traffic_out_bytes)
                    );
                }
                Err(e) => println!("指标        读取失败: {e:#}"),
            }
        }
        Err(e) => {
            println!("状态        未运行");
            println!("{e:#}");
        }
    }
    Ok(())
}

fn print_config_summary(cfg: &Config, path: &Path) {
    println!("配置        {}", path.display());
    println!("数据目录    {}", cfg.dataroot.display());
    println!("控制面      {}", cfg.control_addr);
    println!("入口        {}", cfg.ingress_addr);
    println!("数据面      {}", cfg.data_addr);
    println!(
        "公网后缀    {}",
        if cfg.public_host.trim().is_empty() {
            "—"
        } else {
            &cfg.public_host
        }
    );
    println!(
        "TCP 端口    {}-{}",
        cfg.tcp_port_range.start, cfg.tcp_port_range.end
    );
    println!(
        "内嵌中继    {}",
        cfg.relay
            .serve
            .map(|a| a.to_string())
            .unwrap_or_else(|| "关闭".to_string())
    );
}

async fn clients(config_path: &Path, json: bool) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let rows: Vec<ClientRow> = ServerClient::connect(cfg.dashboard_addr())
        .await?
        .get_json("/api/clients")
        .await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("没有在线 agent");
        return Ok(());
    }
    println!("{:<18} {:<36} {}", "AGENT", "SUBDOMAINS", "ONLINE");
    for row in rows {
        println!(
            "{:<18} {:<36} {}",
            elide(&row.agent_id, 18),
            elide(&row.sub_domain, 36),
            humanize(row.online_secs)
        );
    }
    Ok(())
}

async fn tunnels(config_path: &Path, json: bool) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let response: TunnelsResponse = ServerClient::connect(cfg.dashboard_addr())
        .await?
        .get_json("/api/tunnels")
        .await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&response.tunnels)?);
        return Ok(());
    }
    if response.tunnels.is_empty() {
        println!("没有活动隧道");
        return Ok(());
    }
    println!(
        "{:<6} {:<30} {:<20} {:<8} {:>10} {:>10}",
        "TYPE", "NAME", "PUBLIC", "STATUS", "IN", "OUT"
    );
    for tunnel in response.tunnels {
        println!(
            "{:<6} {:<30} {:<20} {:<8} {:>10} {:>10}",
            elide(&tunnel.kind, 6),
            elide(&tunnel.name, 30),
            elide(&tunnel.remote, 20),
            elide(&tunnel.status, 8),
            human_bytes(tunnel.in_bytes),
            human_bytes(tunnel.out_bytes)
        );
    }
    Ok(())
}

async fn metrics(config_path: &Path, prometheus: bool, json: bool) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let client = ServerClient::connect(cfg.dashboard_addr()).await?;

    if prometheus {
        let text = client.get_text("/api/metrics/prometheus").await?;
        print!("{text}");
        return Ok(());
    }

    let m: MetricsSnapshot = client.get_json("/api/metrics").await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&m)?);
        return Ok(());
    }
    println!("在线 agent   {}", m.agents_online);
    println!("活动连接     {}", m.active_connections);
    println!("HTTP 隧道    {}", m.http_tunnels);
    println!("TCP 隧道     {}", m.tcp_tunnels);
    println!("HTTP 请求    {}", m.requests_total);
    println!("入站流量     {}", human_bytes(m.traffic_in_bytes));
    println!("出站流量     {}", human_bytes(m.traffic_out_bytes));
    Ok(())
}

async fn open(config_path: &Path, print_only: bool) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let info = ServerClient::probe(cfg.dashboard_addr()).await?;
    let url = info.dashboard_url;
    println!("{url}");
    if !print_only {
        open_panel(&url)?;
    }
    Ok(())
}

fn open_panel(url: &str) -> Result<()> {
    if !has_display() {
        println!("没有图形会话,跳过打开浏览器;面板地址:{url}");
        return Ok(());
    }
    let mut command = if cfg!(target_os = "macos") {
        let mut c = StdCommand::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        let mut c = StdCommand::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else {
        let mut c = StdCommand::new("xdg-open");
        c.arg(url);
        c
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().context("无法调起浏览器")?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    println!("已在默认浏览器里打开面板");
    Ok(())
}

fn has_display() -> bool {
    if cfg!(target_os = "linux") {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    } else {
        true
    }
}

// ---------------------------------------------------------------------------
// 配置编辑:保留注释、写前校验、原子替换
// ---------------------------------------------------------------------------

fn config_command(config_path: &Path, command: ConfigCommand, json: bool) -> Result<()> {
    match command {
        ConfigCommand::Path => {
            println!("{}", config_path.display());
            Ok(())
        }
        ConfigCommand::Show => show_config(config_path, json),
        ConfigCommand::Get { key } => get_config(config_path, &key, json),
        ConfigCommand::Set { key, value } => set_config(config_path, &key, &value, json),
        ConfigCommand::Unset { key } => unset_config(config_path, &key, json),
        ConfigCommand::Keys => {
            println!("{:<22} {:<12} {}", "KEY", "TYPE", "说明");
            for (key, kind, description) in CONFIG_KEYS {
                println!("{key:<22} {kind:<12} {description}");
            }
            Ok(())
        }
        ConfigCommand::Validate => {
            let cfg = Config::load(config_path)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "path": config_path.display().to_string(),
                        "valid": true,
                        "config": cfg,
                    }))?
                );
            } else {
                println!("配置有效:{}", config_path.display());
            }
            Ok(())
        }
    }
}

fn show_config(config_path: &Path, json: bool) -> Result<()> {
    let cfg = Config::load(config_path)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&cfg)?);
    } else {
        println!("# 有效配置(含默认值):{}", config_path.display());
        print!("{}", toml::to_string_pretty(&cfg)?);
    }
    Ok(())
}

fn get_config(config_path: &Path, key: &str, json: bool) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let root = toml::Value::try_from(&cfg)?;
    let Some(value) = lookup_value(&root, key) else {
        bail!("配置项 `{key}` 不存在;用 `telesh-server config keys` 查看可用的键");
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "key": key,
                "value": value,
            }))?
        );
    } else {
        println!("{}", format_value(value)?);
    }
    Ok(())
}

fn set_config(config_path: &Path, key: &str, raw: &str, json: bool) -> Result<()> {
    let item = parse_config_item(key, raw)?;
    let mut doc = load_document(config_path, true)?;
    set_value_path(&mut doc, key, item)?;

    let text = doc.to_string();
    toml::from_str::<Config>(&text)
        .with_context(|| format!("写入 `{key}` 后配置校验失败,文件未修改"))?;
    write_config(config_path, &text)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "path": config_path.display().to_string(),
                "key": key,
                "value": raw,
                "restart_required": true,
            }))?
        );
    } else {
        println!("已更新 {key} = {raw}");
        println!("配置文件:{}", config_path.display());
        println!("重启 telesh-server 后生效。");
    }
    Ok(())
}

fn unset_config(config_path: &Path, key: &str, json: bool) -> Result<()> {
    if !config_path.is_file() {
        bail!("配置文件不存在:{}", config_path.display());
    }
    let mut doc = load_document(config_path, false)?;
    if !remove_value_path(&mut doc, key)? {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "path": config_path.display().to_string(),
                    "key": key,
                    "removed": false,
                })
            );
        } else {
            println!("{key} 本来就没有显式设置");
        }
        return Ok(());
    }

    let text = doc.to_string();
    toml::from_str::<Config>(&text)
        .with_context(|| format!("删除 `{key}` 后配置校验失败,文件未修改"))?;
    write_config(config_path, &text)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "path": config_path.display().to_string(),
                "key": key,
                "removed": true,
                "restart_required": true,
            }))?
        );
    } else {
        println!("已删除 {key},将回落到默认值");
        println!("配置文件:{}", config_path.display());
        println!("重启 telesh-server 后生效。");
    }
    Ok(())
}

fn load_document(path: &Path, create: bool) -> Result<toml_edit::DocumentMut> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse::<toml_edit::DocumentMut>()
            .with_context(|| format!("无法解析 {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            Ok(toml_edit::DocumentMut::new())
        }
        Err(e) => Err(e).with_context(|| format!("无法读取 {}", path.display())),
    }
}

fn parse_config_item(key: &str, raw: &str) -> Result<toml_edit::Item> {
    let raw = raw.trim();
    let item = match key {
        "dataroot" | "public_host" | "relay.url" | "relay.tls_cert" | "relay.tls_key"
        | "relay.acme_email" | "relay.acme_cache" => toml_edit::Item::Value(raw.into()),
        "control_addr" | "ingress_addr" | "data_addr" | "relay.serve" | "relay.https_addr"
        | "relay.quic_addr" => {
            raw.parse::<SocketAddr>()
                .with_context(|| format!("`{raw}` 不是合法的 `IP:端口` 地址"))?;
            toml_edit::Item::Value(raw.into())
        }
        "tcp_port_range" => {
            let range = raw
                .parse::<TcpPortRange>()
                .map_err(|e| anyhow!("`{raw}` 不是合法的端口范围: {e}"))?;
            toml_edit::Item::Value(format!("{}-{}", range.start, range.end).into())
        }
        "relay.acme" => {
            let mut array = toml_edit::Array::new();
            if !raw.is_empty() {
                for domain in raw.split(',').map(str::trim) {
                    if domain.is_empty() {
                        bail!("`relay.acme` 里的域名不能为空");
                    }
                    array.push(domain);
                }
            }
            toml_edit::Item::Value(toml_edit::Value::Array(array))
        }
        "relay.acme_staging" => toml_edit::Item::Value(parse_bool(raw)?.into()),
        _ => bail!("未知配置项 `{key}`;用 `telesh-server config keys` 查看可用的键"),
    };
    Ok(item)
}

fn parse_bool(raw: &str) -> Result<bool> {
    match raw.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => bail!("`{raw}` 不是布尔值;用 true / false"),
    }
}

fn set_value_path(
    doc: &mut toml_edit::DocumentMut,
    key: &str,
    item: toml_edit::Item,
) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    let (last, parents) = parts
        .split_last()
        .ok_or_else(|| anyhow!("配置项不能为空"))?;
    let mut table = doc.as_table_mut();
    for part in parents {
        let entry = table
            .entry(part)
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
        table = entry
            .as_table_mut()
            .ok_or_else(|| anyhow!("`{part}` 不是 TOML table,无法写入 `{key}`"))?;
    }
    // 替换已有值时保留原来的空白和注释前缀;否则 `config set` 会把
    // 文件头部/行尾注释一起冲掉。
    let item = match (table.get(last), item) {
        (Some(toml_edit::Item::Value(old)), toml_edit::Item::Value(mut new)) => {
            *new.decor_mut() = old.decor().clone();
            toml_edit::Item::Value(new)
        }
        (_, item) => item,
    };
    // 用已有的 Key 走 `insert_formatted`,连键上的注释/缩进一起保留;
    // 普通 `insert` 会 `fmt()` 键,把行首注释冲掉。
    match table.get_key_value(last).map(|(key, _)| key.clone()) {
        Some(key) => {
            table.insert_formatted(&key, item);
        }
        None => {
            table.insert(last, item);
        }
    }
    Ok(())
}

fn remove_value_path(doc: &mut toml_edit::DocumentMut, key: &str) -> Result<bool> {
    let parts: Vec<&str> = key.split('.').collect();
    let (last, parents) = parts
        .split_last()
        .ok_or_else(|| anyhow!("配置项不能为空"))?;
    let mut table = doc.as_table_mut();
    for part in parents {
        let Some(item) = table.get_mut(part) else {
            return Ok(false);
        };
        table = item
            .as_table_mut()
            .ok_or_else(|| anyhow!("`{part}` 不是 TOML table,无法删除 `{key}`"))?;
    }
    Ok(table.remove(last).is_some())
}

fn lookup_value<'a>(root: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut value = root;
    for part in key.split('.') {
        value = value.get(part)?;
    }
    Some(value)
}

fn format_value(value: &toml::Value) -> Result<String> {
    Ok(match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(d) => d.to_string(),
        toml::Value::Array(a) => {
            if a.is_empty() {
                "[]".to_string()
            } else {
                a.iter()
                    .map(format_value)
                    .collect::<Result<Vec<_>>>()?
                    .join(", ")
            }
        }
        toml::Value::Table(_) => toml::to_string(value)?.trim_end().to_string(),
    })
}

fn write_config(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建目录 {}", parent.display()))?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, text).with_context(|| format!("无法写入 {}", tmp.display()))?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("无法替换 {}", path.display()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 最小 dashboard HTTP 客户端
// ---------------------------------------------------------------------------

struct ServerClient {
    addr: SocketAddr,
}

impl ServerClient {
    fn new(addr: SocketAddr) -> Self {
        Self { addr }
    }

    async fn connect(addr: SocketAddr) -> Result<Self> {
        Self::probe(addr).await?;
        Ok(Self::new(addr))
    }

    async fn probe(addr: SocketAddr) -> Result<Instance> {
        let client = Self::new(addr);
        let value: serde_json::Value = client
            .get_json("/api/instance")
            .await
            .map_err(|e| unavailable(addr, e))?;
        let app = value
            .get("app")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if app != "telesh-server" {
            bail!(
                "http://{addr} 上不是 telesh-server(app = `{app}`)。检查配置里的 control_addr,或换一个端口。"
            );
        }
        serde_json::from_value(value)
            .with_context(|| format!("http://{addr}: /api/instance 字段不完整"))
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self.request(path).await?;
        serde_json::from_slice(&response.body)
            .with_context(|| format!("{path}: 返回的不是合法 JSON"))
    }

    async fn get_text(&self, path: &str) -> Result<String> {
        let response = self.request(path).await?;
        Ok(String::from_utf8_lossy(&response.body).into_owned())
    }

    async fn request(&self, path: &str) -> Result<Response> {
        let mut stream =
            tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect(self.addr))
                .await
                .map_err(|_| anyhow!("连接 http://{} 超时", self.addr))?
                .with_context(|| format!("连接 http://{} 失败", self.addr))?;

        let exchange = async {
            let req =
                format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
            stream.write_all(req.as_bytes()).await?;
            let mut raw = Vec::new();
            stream.read_to_end(&mut raw).await?;
            Ok::<_, std::io::Error>(raw)
        };
        let raw = tokio::time::timeout(REQUEST_TIMEOUT, exchange)
            .await
            .map_err(|_| anyhow!("请求 http://{} 超时", self.addr))?
            .with_context(|| format!("请求 http://{} 失败", self.addr))?;
        parse_http_response(&raw)
    }
}

struct Response {
    body: Vec<u8>,
}

fn parse_http_response(raw: &[u8]) -> Result<Response> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("server 返回了非 HTTP 响应"))?;
    let head =
        std::str::from_utf8(&raw[..split]).map_err(|_| anyhow!("server 返回了非 UTF-8 响应头"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| anyhow!("server 返回了无法解析的状态行"))?;
    let body = raw[split + 4..].to_vec();
    if !(200..300).contains(&status) {
        bail!(
            "dashboard 返回 HTTP {status}: {}",
            String::from_utf8_lossy(&body).trim()
        );
    }
    Ok(Response { body })
}

fn unavailable(addr: SocketAddr, source: anyhow::Error) -> anyhow::Error {
    anyhow!(
        "无法连接 telesh-server 面板 http://{addr}:{source:#}\n\n\
         server 可能没有在运行。启动:\n  \
         systemctl start telesh-server\n  \
         telesh-server --config <FILE>\n\n\
         面板地址由配置里的 control_addr 决定(通配地址按回环访问)。"
    )
}

// ---------------------------------------------------------------------------
// 输出小工具
// ---------------------------------------------------------------------------

fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes < KIB {
        format!("{bytes:.0} B")
    } else if bytes < MIB {
        format!("{:.1} KiB", bytes / KIB)
    } else if bytes < GIB {
        format!("{:.1} MiB", bytes / MIB)
    } else {
        format!("{:.2} GiB", bytes / GIB)
    }
}

fn humanize(secs: u64) -> String {
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    if secs < MIN {
        format!("{secs}s")
    } else if secs < HOUR {
        format!("{}m", secs / MIN)
    } else if secs < DAY {
        format!("{}h{:02}m", secs / HOUR, (secs % HOUR) / MIN)
    } else {
        format!("{}d{:02}h", secs / DAY, (secs % DAY) / HOUR)
    }
}

fn elide(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_set_round_trips_and_keeps_comments() {
        let dir = std::env::temp_dir().join(format!("telesh-server-cli-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("server.toml");
        std::fs::write(
            &path,
            "# keep me\npublic_host = \"old.example\"\n\n[relay]\nurl = \"https://relay.example\"\n",
        )
        .unwrap();

        set_config(&path, "public_host", "new.example", false).unwrap();
        set_config(&path, "relay.serve", "0.0.0.0:8081", false).unwrap();
        set_config(&path, "tcp_port_range", "19000-19010", false).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep me"));
        assert!(text.contains("new.example"));
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.public_host, "new.example");
        assert_eq!(cfg.relay.serve.unwrap().port(), 8081);
        assert_eq!(cfg.tcp_port_range.inclusive(), 19000..=19010);

        assert!(parse_config_item("control_addr", "not-an-addr").is_err());
        assert!(parse_config_item("nope", "x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_unset_removes_only_that_key() {
        let mut doc: toml_edit::DocumentMut = "public_host = \"x\"\n[relay]\nurl = \"y\"\n"
            .parse()
            .unwrap();
        assert!(remove_value_path(&mut doc, "relay.url").unwrap());
        assert!(!remove_value_path(&mut doc, "relay.url").unwrap());
        let text = doc.to_string();
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.public_host, "x");
        assert_eq!(cfg.relay.url, None);
    }

    #[test]
    fn bytes_have_readable_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MiB");
    }
}
