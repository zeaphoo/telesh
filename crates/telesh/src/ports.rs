//! 本机监听端口发现(Linux `/proc`)。
//!
//! 只读 `/proc/net/tcp{,6}` 找监听态(`0A`),再扫 `/proc/<pid>/fd` 用 socket inode
//! 反查进程。非 root 只能看到本用户的进程,这属于正常;容器内看到的就是容器里的端口。

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};

use serde::Serialize;

/// 一个正在监听的本地端口。
#[derive(Debug, Clone, Serialize)]
pub struct PortInfo {
    pub port: u16,
    /// 监听地址(`0.0.0.0` 表示所有网卡)。
    pub addr: String,
    pub pid: Option<u32>,
    pub process: Option<String>,
    /// 端口表的启发式建议(**不是**探测结果)。
    pub http_hint: bool,
    /// 该端口只监听 IPv6 回环/任意地址(数据面拨号要用 `::1`)。
    pub v6_only: bool,
}

/// 常见 HTTP 端口(启发式建议,不代表真实协议)。
const HTTP_HINT_PORTS: &[u16] = &[
    80, 443, 3000, 3001, 4000, 4200, 5000, 5173, 5601, 7000, 8000, 8001, 8080, 8081, 8088, 8443,
    8888, 9000, 9090, 10000,
];

/// 扫描一次本机监听的 TCP 端口(按端口去重,IPv4 优先)。
pub fn scan() -> Vec<PortInfo> {
    let inodes = listen_inodes();
    if inodes.is_empty() {
        return Vec::new();
    }
    let owners = socket_owners();
    let mut by_port: HashMap<u16, PortInfo> = HashMap::new();
    for (ip, port, inode) in inodes {
        let (pid, process) = owners.get(&inode).cloned().unwrap_or((None, None));
        let v6 = ip.contains(':');
        let entry = by_port.entry(port).or_insert_with(|| PortInfo {
            port,
            addr: ip.clone(),
            pid,
            process: process.clone(),
            http_hint: HTTP_HINT_PORTS.contains(&port),
            v6_only: v6,
        });
        // IPv4 监听优先展示,并且只要有一个 v4 监听就不算 v6-only。
        if !v6 {
            entry.addr = ip;
            entry.v6_only = false;
        }
    }
    let mut out: Vec<PortInfo> = by_port.into_values().collect();
    out.sort_by_key(|p| p.port);
    out
}

/// `(address, port, inode)`,只取 LISTEN 状态。
fn listen_inodes() -> Vec<(String, u16, u64)> {
    let mut out = Vec::new();
    for (path, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 || f[3] != "0A" {
                continue;
            }
            let Some((ip, port)) = parse_addr(f[1], v6) else {
                continue;
            };
            let Ok(inode) = f[9].parse::<u64>() else {
                continue;
            };
            out.push((ip, port, inode));
        }
    }
    out
}

/// `/proc/net/tcp` 里的地址是 `hex_ip:hex_port`,IP 按小端逐字节排列。
fn parse_addr(field: &str, v6: bool) -> Option<(String, u16)> {
    let (ip_hex, port_hex) = field.split_once(':')?;
    let port = u16::from_str_radix(port_hex, 16).ok()?;
    let raw = (0..ip_hex.len() / 2)
        .map(|i| u8::from_str_radix(&ip_hex[i * 2..i * 2 + 2], 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    let ip = if v6 {
        if raw.len() != 16 {
            return None;
        }
        // 四个 32 位字,每个字内部小端。
        let mut bytes = [0u8; 16];
        for word in 0..4 {
            for b in 0..4 {
                bytes[word * 4 + b] = raw[word * 4 + (3 - b)];
            }
        }
        Ipv6Addr::from(bytes).to_string()
    } else {
        if raw.len() != 4 {
            return None;
        }
        Ipv4Addr::new(raw[3], raw[2], raw[1], raw[0]).to_string()
    };
    Some((ip, port))
}

/// socket inode → (pid, 进程名)。
fn socket_owners() -> HashMap<u64, (Option<u32>, Option<String>)> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let process = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()
            .map(|s| s.trim().to_string());
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(link) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let Some(inode) = link
                .to_str()
                .and_then(|s| s.strip_prefix("socket:["))
                .and_then(|s| s.strip_suffix(']'))
                .and_then(|s| s.parse::<u64>().ok())
            else {
                continue;
            };
            out.entry(inode).or_insert((Some(pid), process.clone()));
        }
    }
    out
}
