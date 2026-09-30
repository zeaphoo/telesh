//! 本机监听端口发现(Linux `/proc`)。
//!
//! 只读 `/proc/net/tcp{,6}` 找监听态(`0A`),再扫 `/proc/<pid>/fd` 用 socket inode
//! 反查进程。非 root 只能看到本用户的进程,这属于正常;容器内看到的就是容器里的端口。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;
use std::time::Duration;

use dashmap::DashMap;
use futures::StreamExt;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// 一个正在监听的本地端口。
#[derive(Debug, Clone, Serialize)]
pub struct PortInfo {
    pub port: u16,
    /// 监听地址(`0.0.0.0` 表示所有网卡)。
    pub addr: String,
    pub pid: Option<u32>,
    pub process: Option<String>,
    /// 看起来是 HTTP 吗:探测结果;还没探测过时回落到端口表启发式。
    pub http_hint: bool,
    /// 探测到是 SSH 吗(服务端先发 `SSH-2.0-...` banner)。
    pub ssh_hint: bool,
    /// 探测到的 `Server:` 头(探测结果里没有就是 None)。
    pub server: Option<String>,
    /// 是否已经真的探测过(UI 用来区分"建议"和"检测到")。
    pub probed: bool,
    /// 该端口只监听 IPv6 回环/任意地址(数据面拨号要用 `::1`)。
    pub v6_only: bool,
}

/// 一次探测的结论。
#[derive(Debug, Clone, Default)]
struct Probe {
    http: bool,
    ssh: bool,
    server: Option<String>,
}

/// port → 探测结论(进程内缓存;端口消失时清掉,服务重启后会重新探测)。
static PROBES: OnceLock<DashMap<u16, Probe>> = OnceLock::new();

fn probes() -> &'static DashMap<u16, Probe> {
    PROBES.get_or_init(DashMap::new)
}

/// 一次探测的连接与读取超时。
const PROBE_TIMEOUT: Duration = Duration::from_millis(700);
/// 同时探测多少个端口。
const PROBE_CONCURRENCY: usize = 8;

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
            ssh_hint: false,
            server: None,
            probed: false,
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

/// 扫一遍监听端口,并**对没探测过的端口做一次 HTTP 探测**。
///
/// 探测会真的向本地服务发一个 `HEAD /`(有副作用),所以:
/// 每个端口在一个进程生命周期内只探一次;端口消失后缓存清除,重新出现会再探一次。
pub async fn scan_and_probe() -> Vec<PortInfo> {
    let mut list = scan();

    // 端口消失 → 清掉缓存,服务重启后会重新探测。
    let live: std::collections::HashSet<u16> = list.iter().map(|p| p.port).collect();
    probes().retain(|port, _| live.contains(port));

    // 未知端口排队探测。
    let todo: Vec<(u16, bool)> = list
        .iter()
        .filter(|p| !probes().contains_key(&p.port))
        .map(|p| (p.port, p.v6_only))
        .collect();

    let results: Vec<(u16, Probe)> = futures::stream::iter(todo)
        .map(|(port, v6_only)| async move { (port, probe_protocol(port, v6_only).await) })
        .buffer_unordered(PROBE_CONCURRENCY)
        .collect()
        .await;
    for (port, probe) in results {
        probes().insert(port, probe);
    }

    for p in list.iter_mut() {
        if let Some(probe) = probes().get(&p.port) {
            p.http_hint = probe.http;
            p.ssh_hint = probe.ssh;
            p.server = probe.server.clone();
            p.probed = true;
        }
    }
    list
}

/// 探一下本地端口是什么协议:SSH(服务端先发 banner)或 HTTP(发一个最小的
/// `HEAD /`)。对 SSH 而言,我们在收到 banner 后读到的第一段字节就是
/// `SSH-2.0-...`,所以这次发 HTTP 请求不影响判定。
async fn probe_protocol(port: u16, v6_only: bool) -> Probe {
    let ip = if v6_only {
        IpAddr::from(Ipv6Addr::LOCALHOST)
    } else {
        IpAddr::from(Ipv4Addr::LOCALHOST)
    };
    let Ok(Ok(mut sock)) = timeout(PROBE_TIMEOUT, TcpStream::connect((ip, port))).await else {
        return Probe::default();
    };
    let req = b"HEAD / HTTP/1.0\r\nHost: localhost\r\nUser-Agent: telesh-probe\r\n\r\n";
    if timeout(PROBE_TIMEOUT, sock.write_all(req)).await.is_err() {
        return Probe::default();
    }
    let mut buf = [0u8; 1024];
    let Ok(Ok(n)) = timeout(PROBE_TIMEOUT, sock.read(&mut buf)).await else {
        return Probe::default();
    };
    classify(&buf[..n])
}

/// 按首段字节归类:`SSH-` → SSH,`HTTP/` → HTTP(顺带取 `Server:` 头)。
fn classify(data: &[u8]) -> Probe {
    let head = String::from_utf8_lossy(data);
    if head.starts_with("SSH-") {
        return Probe {
            ssh: true,
            ..Probe::default()
        };
    }
    if !head.starts_with("HTTP/") {
        return Probe::default();
    }
    let server = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("server:"))
        .map(|l| l[l.find(':').map_or(0, |i| i + 1)..].trim().to_string())
        .filter(|s| !s.is_empty());
    Probe {
        http: true,
        server,
        ..Probe::default()
    }
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
