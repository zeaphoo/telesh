//! 服务端配置。
//!
//! 默认从 TOML 文件读;字段都可以省略(省略即用默认值)。用户也可以用
//! `telesh-server config set/get/unset` 修改,不必手工编辑文件。所有需要
//! 落盘的数据都在 `dataroot` 下。
//!
//! ```toml
//! dataroot = "/var/lib/telesh"
//! public_host = "example.com"
//! secret = "…"
//!
//! [relay]
//! serve = "0.0.0.0:8081"
//! url   = "https://relay.example.com"
//! ```

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::OnceLock;

use anyhow::Context;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

use crate::relay_config::RelayConfig;

/// 命令行:默认前台跑 server;子命令是本地运维/配置入口。
#[derive(Debug, Parser)]
#[command(
    name = "telesh-server",
    version,
    about = "telesh tunnel server: control plane + edge ingress",
    subcommand_precedence_over_arg = true
)]
pub struct Cli {
    /// TOML 配置文件路径。默认依次查找 `./server.toml`、`/etc/telesh/server.toml`、
    /// `~/.telesh/server.toml`。
    #[arg(short = 'c', long = "config", global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// 兼容旧用法:`telesh-server /etc/telesh/server.toml`。
    #[arg(value_name = "CONFIG", hide = true)]
    pub legacy_config: Option<PathBuf>,

    /// 以 JSON 输出(用于脚本;适用于运维子命令)。
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// 本次调用使用的配置文件路径(已展开 `~`)。
    pub fn config_path(&self) -> PathBuf {
        resolve_config_path(self.config.as_deref().or(self.legacy_config.as_deref()))
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 前台运行 control server + edge ingress(默认)。
    Serve,

    /// 查看配置、运行状态和流量摘要。
    Status,

    /// 列出当前在线的 agent。
    Clients,

    /// 列出当前活动的 HTTP/TCP 隧道。
    Tunnels,

    /// 查看服务指标。
    Metrics {
        /// 输出 Prometheus 文本格式(给抓取器或 `curl` 用)。
        #[arg(long)]
        prometheus: bool,
    },

    /// 在浏览器里打开 dashboard。
    Open {
        /// 只打印 URL,不调起浏览器。
        #[arg(long)]
        print_only: bool,
    },

    /// 查看或修改服务器配置(保留 TOML 注释)。
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// 打印配置文件路径。
    Path,

    /// 打印含默认值的有效配置。
    Show,

    /// 读取一个配置项,例如 `public_host` / `relay.serve`。
    Get {
        #[arg(value_name = "KEY")]
        key: String,
    },

    /// 写入一个配置项;写前会做完整校验,不会留下半截配置。
    Set {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(value_name = "VALUE")]
        value: String,
    },

    /// 删除一个配置项,让它回落到默认值。
    Unset {
        #[arg(value_name = "KEY")]
        key: String,
    },

    /// 列出所有可配置项。
    Keys,

    /// 只校验当前配置文件,不启动服务。
    Validate,
}

/// 找配置文件:显式参数 → `TELESH_SERVER_CONFIG` → 常见默认位置。
fn resolve_config_path(explicit: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit {
        return expand_tilde(path);
    }
    if let Some(path) = std::env::var_os("TELESH_SERVER_CONFIG") {
        return expand_tilde(Path::new(&path));
    }
    let mut candidates = vec![PathBuf::from("server.toml")];
    candidates.push(PathBuf::from("/etc/telesh/server.toml"));
    if let Some(home) = home_dir() {
        candidates.push(home.join(".telesh/server.toml"));
    }
    for path in &candidates {
        if path.is_file() {
            return path.clone();
        }
    }
    candidates.remove(0)
}

/// TCP 隧道可用的端口范围,配置文件里写成 `"18000-18100"`。
#[derive(Debug, Clone, Copy)]
pub struct TcpPortRange {
    pub start: u16,
    pub end: u16,
}

impl TcpPortRange {
    pub fn inclusive(&self) -> RangeInclusive<u16> {
        self.start..=self.end
    }
}

impl FromStr for TcpPortRange {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (a, b) = s
            .split_once('-')
            .ok_or_else(|| format!("expected START-END, got {s:?}"))?;
        let start: u16 = a.trim().parse().map_err(|_| format!("bad port {a:?}"))?;
        let end: u16 = b.trim().parse().map_err(|_| format!("bad port {b:?}"))?;
        if start > end {
            return Err(format!("start {start} > end {end}"));
        }
        Ok(TcpPortRange { start, end })
    }
}

impl<'de> Deserialize<'de> for TcpPortRange {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        TcpPortRange::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl Serialize for TcpPortRange {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{}-{}", self.start, self.end))
    }
}

/// 服务端配置。字段省略时用 [`Default`] 里的值。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// 数据根目录:数据库、证书缓存等都放这里。默认 `~/.telesh`。
    pub dataroot: PathBuf,

    /// agent 连控制面的地址(`ws://.../api/tunnel`)。同一个端口也提供 dashboard
    /// (`/` 前端、`/api/*`、`/metrics`)。
    pub control_addr: SocketAddr,

    /// 边缘入口监听公网连接(HTTP 用 Host 头选隧道,TCP 按端口)。
    pub ingress_addr: SocketAddr,

    /// 数据面地址:agent 主动连这里,yamux over TCP。
    pub data_addr: SocketAddr,

    /// 拼隧道 URL 用的公网后缀,例如 `example.com` → `http://<sub>.example.com:8080`。
    /// 空表示上报的 hostname 就只有子域。
    pub public_host: String,

    /// TCP 隧道的端口范围,例如 `"18000-18100"`。
    pub tcp_port_range: TcpPortRange,

    /// 中继相关配置;`serve` 为空表示不跑内嵌中继。
    pub relay: RelayConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dataroot: default_dataroot(),
            control_addr: "0.0.0.0:9009".parse().expect("valid default"),
            ingress_addr: "0.0.0.0:8080".parse().expect("valid default"),
            data_addr: "0.0.0.0:9008".parse().expect("valid default"),
            public_host: String::new(),
            tcp_port_range: TcpPortRange {
                start: 18000,
                end: 18100,
            },
            relay: RelayConfig::default(),
        }
    }
}

impl Config {
    /// 读配置文件。文件必须存在;字段可以省略。
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let path = expand_tilde(path);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read config {}", path.display()))?;
        let mut cfg: Config = toml::from_str(&text)
            .with_context(|| format!("cannot parse config {}", path.display()))?;
        // 配置里的路径也支持 `~`:展开成用户主目录,而不是建一个叫 `~` 的目录。
        cfg.dataroot = expand_tilde(&cfg.dataroot);
        cfg.relay.tls_cert = cfg.relay.tls_cert.as_deref().map(expand_tilde);
        cfg.relay.tls_key = cfg.relay.tls_key.as_deref().map(expand_tilde);
        cfg.relay.acme_cache = cfg.relay.acme_cache.as_deref().map(expand_tilde);
        Ok(cfg)
    }

    /// 数据库文件:`<dataroot>/server.sqlite`。
    pub fn db_path(&self) -> PathBuf {
        self.dataroot.join("server.sqlite")
    }

    /// dashboard 在本机可访问的地址。`control_addr` 绑在通配地址(`0.0.0.0` /
    /// `::`)时换成回环地址 —— 桌面外壳拿它开 webview。
    pub fn dashboard_addr(&self) -> SocketAddr {
        let ip = match self.control_addr.ip() {
            IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        SocketAddr::new(ip, self.control_addr.port())
    }

    /// dashboard 的 URL(`http://<dashboard_addr>`)。
    pub fn dashboard_url(&self) -> String {
        format!("http://{}", self.dashboard_addr())
    }

    /// ACME 证书缓存目录:`<dataroot>/acme`,除非配置文件里另指了地方。
    pub fn acme_cache(&self) -> PathBuf {
        self.relay
            .acme_cache
            .clone()
            .unwrap_or_else(|| self.dataroot.join("acme"))
    }

    /// 通告给 agent 的中继 URL:`[relay] url` → ACME 域名 → `public_host` 加端口,
    /// 都推不出来就是 `None`(agent 用 iroh 默认中继)。
    pub fn advertised_relay_url(&self) -> Option<String> {
        if let Some(url) = self.relay.advertised_url(&self.public_host) {
            return Some(url);
        }
        if self.relay.serve.is_some() {
            tracing::warn!(
                "内嵌中继在跑,但不知道它的公网 URL;agent 会用 iroh 默认中继。\
                 请在配置文件里设置 [relay] url 或 public_host。"
            );
        }
        None
    }

    /// 启动时打一行配置摘要(不含密钥)。
    pub fn log_summary(&self) {
        tracing::info!(
            dataroot = %self.dataroot.display(),
            control = %self.control_addr,
            ingress = %self.ingress_addr,
            data = %self.data_addr,
            tcp_range = %format_args!("{}-{}", self.tcp_port_range.start, self.tcp_port_range.end),
            public_host = %if self.public_host.is_empty() { "(none)" } else { &self.public_host },
            embedded_relay = ?self.relay.serve,
            "server config loaded"
        );
    }
}

/// 用户主目录:Unix 用 `HOME`,Windows 用 `USERPROFILE`。
fn home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        return Some(PathBuf::from(home));
    }
    #[cfg(windows)]
    if let Some(home) = std::env::var_os("USERPROFILE") {
        return Some(PathBuf::from(home));
    }
    None
}

fn default_dataroot() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".telesh")
}

pub static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn init(config: Config) {
    let _ = CONFIG.set(config);
}

pub fn config() -> &'static Config {
    CONFIG.get().expect("server config not initialized")
}

/// 把路径开头的 `~` 展开成用户主目录(`~` 与 `~/…`;`~user` 不处理)。
///
/// 配置文件里写 `dataroot = "~/telesh"` 时,要落到 `/home/you/telesh`,
/// 而不是在当前目录下建一个叫 `~` 的目录。没有 `HOME` 时原样返回。
pub fn expand_tilde(path: &Path) -> PathBuf {
    let Some(home) = home_dir() else {
        return path.to_path_buf();
    };
    let mut comps = path.components();
    match comps.next() {
        Some(std::path::Component::Normal(first)) if first == "~" => {
            let rest = comps.as_path();
            if rest.as_os_str().is_empty() {
                PathBuf::from(home)
            } else {
                PathBuf::from(home).join(rest)
            }
        }
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `~` 要展开成主目录,别的地方原样保留。
    #[test]
    fn expands_a_leading_tilde() {
        let home = home_dir().expect("home dir is set in tests");
        assert_eq!(expand_tilde(Path::new("~/telesh")), home.join("telesh"));
        assert_eq!(expand_tilde(Path::new("~")), home);
        assert_eq!(
            expand_tilde(Path::new("/var/lib/telesh")),
            PathBuf::from("/var/lib/telesh")
        );
        assert_eq!(
            expand_tilde(Path::new("rel/acme")),
            PathBuf::from("rel/acme")
        );
        // `~user` 不处理(拿不到别人的主目录)。
        assert_eq!(expand_tilde(Path::new("~root/x")), PathBuf::from("~root/x"));
    }

    /// 配置里的相对/`~` 路径要被展开。
    #[test]
    fn config_paths_are_expanded() {
        let home = home_dir().expect("home dir is set in tests");
        let cfg: Config =
            toml::from_str("dataroot = \"~/telesh\"\n[relay]\ntls_cert = \"~/relay.crt\"\n")
                .unwrap();
        let cfg = {
            let mut c = cfg;
            c.dataroot = expand_tilde(&c.dataroot);
            c.relay.tls_cert = c.relay.tls_cert.as_deref().map(expand_tilde);
            c
        };
        assert_eq!(cfg.dataroot, home.join("telesh"));
        assert_eq!(cfg.relay.tls_cert.unwrap(), home.join("relay.crt"));
    }

    /// 示例配置文件必须能解析:字段名或取值写错会在这里炸。
    #[test]
    fn example_config_parses() {
        let cfg: Config =
            toml::from_str(include_str!("../server.toml.example")).expect("example parses");
        assert_eq!(cfg.public_host, "example.com");
        assert_eq!(cfg.tcp_port_range.inclusive(), 18000..=18100);
        assert!(cfg.db_path().ends_with("server.sqlite"));
        assert_eq!(cfg.acme_cache().file_name().unwrap(), "acme");
    }

    /// 字段名打错要报错,不能静默用默认值。
    #[test]
    fn unknown_field_is_rejected() {
        assert!(toml::from_str::<Config>("ctrl_addr = \"0.0.0.0:1\"").is_err());
    }

    /// 只给 dataroot 也要能跑起来(其余全默认)。
    #[test]
    fn dataroot_alone_is_valid() {
        let cfg: Config = toml::from_str("dataroot = \"/tmp/ts\"").expect("parses");
        assert_eq!(cfg.control_addr.port(), 9009);
        assert_eq!(cfg.data_addr.port(), 9008);
        assert_eq!(cfg.db_path(), std::path::Path::new("/tmp/ts/server.sqlite"));
    }

    /// dashboard 地址:通配绑定换成回环,具体地址原样保留。
    #[test]
    fn dashboard_addr_uses_loopback_for_wildcards() {
        let mut cfg = Config::default();

        cfg.control_addr = "0.0.0.0:9009".parse().unwrap();
        assert_eq!(cfg.dashboard_addr(), "127.0.0.1:9009".parse().unwrap());
        assert_eq!(cfg.dashboard_url(), "http://127.0.0.1:9009");

        cfg.control_addr = "[::]:9009".parse().unwrap();
        assert_eq!(cfg.dashboard_addr(), "[::1]:9009".parse().unwrap());

        cfg.control_addr = "192.0.2.1:9009".parse().unwrap();
        assert_eq!(cfg.dashboard_addr(), cfg.control_addr);
    }
}
