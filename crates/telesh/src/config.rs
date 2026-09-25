//! agent 的配置文件:`<dataroot>/config.toml`。
//!
//! 要连哪个 control server 写在这里(不是命令行参数,也不是 UI):
//!
//! ```toml
//! control_server = "tunnel.example.com:9009"
//! ```
//!
//! 文件不存在时自动创建一份,默认连本机 `127.0.0.1:9009`。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// 配置文件名(放在 dataroot 下)。
pub const FILE_NAME: &str = "config.toml";

/// 自动生成的配置里写的默认地址:本机控制面。
pub const DEFAULT_CONTROL_SERVER: &str = "127.0.0.1:9009";

/// 新配置文件的内容。
const TEMPLATE: &str = "\
# telesh agent 配置。文件不存在时会自动生成这一份。
#
# 要连的 control server,写 host:port(只写 host 时端口默认 9009;
# 粘贴 ws://host:port/api/tunnel 也能认)。
control_server = \"127.0.0.1:9009\"
";

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

    /// 读配置;文件不存在就先写一份默认的(连本机 9009)。
    pub fn load_or_create(dataroot: &Path) -> Result<Self> {
        let path = file_path(dataroot);
        if !path.exists() {
            std::fs::create_dir_all(dataroot)
                .with_context(|| format!("cannot create dataroot {}", dataroot.display()))?;
            std::fs::write(&path, TEMPLATE)
                .with_context(|| format!("cannot write agent config {}", path.display()))?;
            tracing::info!(
                path = %path.display(),
                server = DEFAULT_CONTROL_SERVER,
                "created a default agent config"
            );
        }
        Self::load(dataroot)
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

/// 把路径开头的 `~` 展开成用户主目录(`~` 与 `~/…`;`~user` 不处理)。
///
/// `--dataroot "~/telesh"` 要落到 `/home/you/telesh`,而不是在当前目录下建一个
/// 叫 `~` 的目录。没有 `HOME` 时原样返回。
pub fn expand_tilde(path: &Path) -> PathBuf {
    let Ok(home) = std::env::var("HOME") else {
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
    fn expands_a_leading_tilde() {
        let home = PathBuf::from(std::env::var("HOME").expect("HOME is set in tests"));
        assert_eq!(expand_tilde(Path::new("~/telesh")), home.join("telesh"));
        assert_eq!(expand_tilde(Path::new("~")), home);
        assert_eq!(
            expand_tilde(Path::new("/srv/telesh")),
            PathBuf::from("/srv/telesh")
        );
        assert_eq!(expand_tilde(Path::new("~root/x")), PathBuf::from("~root/x"));
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

    #[test]
    fn creates_a_default_config_when_missing() {
        let dir = std::env::temp_dir().join(format!("telesh-cfg-{}", rand::random::<u64>()));
        let cfg = Config::load_or_create(&dir).expect("creates the file");
        assert_eq!(cfg.control_server().unwrap(), DEFAULT_CONTROL_SERVER);
        assert!(file_path(&dir).exists());
        // 第二次读的是文件本身(改过的地址要被认)。
        std::fs::write(file_path(&dir), "control_server = \"srv.example.com\"\n").unwrap();
        let cfg = Config::load_or_create(&dir).unwrap();
        assert_eq!(cfg.control_server().unwrap(), "srv.example.com:9009");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
