//! agent 的配置文件:`<dataroot>/config.toml`。
//!
//! 要连哪个 control server 写在这里(不是命令行参数,也不是 UI):
//!
//! ```toml
//! control_server = "tunnel.example.com:9009"
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// 配置文件名(放在 dataroot 下)。
pub const FILE_NAME: &str = "config.toml";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// control server 地址,`host:port`。空 = 还没配置。
    pub control_server: String,
}

impl Config {
    /// 读 `<dataroot>/config.toml`。
    pub fn load(dataroot: &Path) -> Result<Self> {
        let path = file_path(dataroot);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read agent config {}", path.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("cannot parse agent config {}", path.display()))?;
        Ok(cfg)
    }

    /// 地址规范化后的形式(`ws://…/api/tunnel` 这种粘贴也接受)。
    pub fn control_server(&self) -> Option<String> {
        normalize(&self.control_server)
    }
}

/// `<dataroot>/config.toml`。
pub fn file_path(dataroot: &Path) -> PathBuf {
    dataroot.join(FILE_NAME)
}

/// 把用户写的地址规范化成 `host:port`;只有主机名时补控制面的默认端口。
pub fn normalize(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let rest = s
        .strip_prefix("ws://")
        .or_else(|| s.strip_prefix("wss://"))
        .or_else(|| s.strip_prefix("http://"))
        .or_else(|| s.strip_prefix("https://"))
        .unwrap_or(s);
    // 去掉路径与查询串(`/api/tunnel` 之类),只留 host:port。
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host_port = host_port.trim_end_matches(':');
    if host_port.is_empty() {
        return None;
    }
    // 只给了主机名时补上控制面的默认端口。
    if host_port.contains(':') || host_port.starts_with('[') {
        Some(host_port.to_string())
    } else {
        Some(format!("{host_port}:9009"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_host_port_and_urls() {
        assert_eq!(normalize("10.0.0.7:9009").unwrap(), "10.0.0.7:9009");
        assert_eq!(
            normalize("ws://10.0.0.7:9009/api/tunnel").unwrap(),
            "10.0.0.7:9009"
        );
        assert_eq!(
            normalize("  https://srv.example.com  ").unwrap(),
            "srv.example.com:9009"
        );
        assert_eq!(
            normalize("srv.example.com:9009/").unwrap(),
            "srv.example.com:9009"
        );
        assert!(normalize("").is_none());
        assert!(normalize("ws://").is_none());
    }

    #[test]
    fn parses_the_config_file() {
        let cfg: Config = toml::from_str("control_server = \"srv:9009\"").unwrap();
        assert_eq!(cfg.control_server().unwrap(), "srv:9009");
        // 空配置合法,只是还没配地址。
        let empty: Config = toml::from_str("").unwrap();
        assert!(empty.control_server().is_none());
        // 字段名打错要报错。
        assert!(toml::from_str::<Config>("ctrl = \"srv:9009\"").is_err());
    }
}
