//! 内嵌 iroh 中继的配置:配置文件里的 `[relay]` 段。
//!
//! ```toml
//! [relay]
//! # 运行一个进程内的 iroh 中继。留空 = 不跑内嵌中继。
//! serve = "0.0.0.0:8081"
//!
//! # 通告给 agent 的公网 URL。可选:有 ACME 时由第一个域名推出,
//! # 纯 HTTP / 手动 TLS 时由 public_host 推出。中继在别的名字下面可达时写它。
//! url = "https://relay.example.com"
//!
//! # 手动 TLS(HTTPS + QUIC 地址发现):
//! tls_cert   = "/etc/telesh/relay.crt"
//! tls_key    = "/etc/telesh/relay.key"
//! https_addr = "0.0.0.0:443"
//! quic_addr  = "0.0.0.0:7842"
//!
//! # ……或者 ACME/Let's Encrypt(优先于手动 TLS):
//! acme         = ["relay.example.com"]
//! acme_email   = "admin@example.com"
//! acme_staging = false
//! acme_cache   = "/var/lib/telesh/acme"
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RelayConfig {
    /// 内嵌中继的 HTTP 监听地址;`None` 表示不运行内嵌中继。
    pub serve: Option<SocketAddr>,
    /// 这个中继对外可达的公网 URL,登录后通告给 agent。
    pub url: Option<String>,
    /// PEM 证书链;和 `tls_key` 一起启用 HTTPS + QUIC。
    pub tls_cert: Option<PathBuf>,
    /// `tls_cert` 对应的 PEM 私钥。
    pub tls_key: Option<PathBuf>,
    /// HTTPS 监听地址(配 TLS/ACME 时必填,通常 443)。
    pub https_addr: Option<SocketAddr>,
    /// QUIC 地址发现监听地址(默认 `0.0.0.0:7842`)。
    pub quic_addr: Option<SocketAddr>,
    /// ACME 域名;非空即启用 Let's Encrypt,并优先于手动 TLS。
    pub acme: Vec<String>,
    /// ACME 账户联系邮箱。
    pub acme_email: Option<String>,
    /// 用 Let's Encrypt 的 staging 环境。
    pub acme_staging: bool,
    /// ACME 证书缓存目录;默认 `<dataroot>/acme`。
    pub acme_cache: Option<PathBuf>,
}

impl RelayConfig {
    /// QUIC 绑定地址,带惯用默认值。
    pub fn quic_addr_or_default(&self) -> SocketAddr {
        self.quic_addr
            .unwrap_or_else(|| "0.0.0.0:7842".parse().expect("valid default"))
    }

    /// 光看这个配置能推出的公网中继 URL。
    ///
    /// 显式 `url` 永远优先。有 ACME 时第一个域名就是证书身份,是唯一可用的
    /// 主机名。手动 TLS 和纯 HTTP 不知道自己的主机名,除非 `public_host` 指明。
    pub fn advertised_url(&self, public_host: &str) -> Option<String> {
        if let Some(url) = self.url.as_deref().filter(|u| !u.is_empty()) {
            return Some(url.to_string());
        }
        // 没跑内嵌中继就没有可推导的 URL。
        let serve = self.serve?;
        if let Some(domain) = self.acme.first().filter(|d| !d.is_empty()) {
            let port = self.https_addr.map_or(443, |a| a.port());
            return Some(http_url("https", domain, port, 443));
        }
        let host = public_host.trim();
        if host.is_empty() {
            return None;
        }
        match self.https_addr {
            Some(addr) => Some(http_url("https", host, addr.port(), 443)),
            None => Some(http_url("http", host, serve.port(), 80)),
        }
    }
}

/// `scheme://host[:port]`,默认端口省掉。
fn http_url(scheme: &str, host: &str, port: u16, default_port: u16) -> String {
    if port == default_port {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    }
}
