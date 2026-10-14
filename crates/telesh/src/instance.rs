//! 「这个端口上是不是 telesh」——只看 `GET /api/instance` 的指纹。
//!
//! 没有锁文件、没有 pid 文件:端口被占 + 指纹不匹配就是全部判据。
//!
//! - `telesh` 启动时端口被占:是 telesh → 提示已在运行(可选打开面板)并正常退出;
//!   是别的程序 → 报错,让用户换 `--ui-port`;
//! - `telesh open`:确认是 telesh 才打开浏览器,避免把陌生服务当成面板;
//! - 桌面外壳(`agent-app`)用同一套判据,避免把窗口指向别人的服务。

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// 探测的超时:这是启动路径上的阻塞调用,必须短。
const TIMEOUT: Duration = Duration::from_millis(500);
/// 响应最多读这么多(面板只会回一小段 JSON/HTML)。
const MAX_READ: u64 = 8 * 1024;

/// `/api/instance` 的响应,同时也是「这是 telesh」的指纹。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    /// 恒为 `telesh`;认指纹就看这个字段。
    pub app: String,
    pub version: String,
    pub pid: u32,
    /// 面板实际绑定的地址。
    pub ui_addr: String,
    /// 面板已运行秒数。
    pub uptime_secs: u64,
    /// 面板启动时刻(unix 秒),给脚本用。
    pub started_unix: u64,
}

impl Instance {
    /// 面板地址。
    pub fn url(&self) -> String {
        format!("http://{}", self.ui_addr)
    }
}

/// 一次探测的结论。
#[derive(Debug)]
pub enum Probe {
    /// 是 telesh。
    Telesh(Box<Instance>),
    /// 端口被别的程序占着(能读到 `Server:` 头就带上)。
    Foreign(Option<String>),
    /// 没人监听。
    Nothing,
}

/// 探一下 `127.0.0.1:<port>` 上是什么。
pub fn probe(port: u16) -> Probe {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut sock) = TcpStream::connect_timeout(&addr, TIMEOUT) else {
        return Probe::Nothing;
    };
    let _ = sock.set_read_timeout(Some(TIMEOUT));
    let _ = sock.set_write_timeout(Some(TIMEOUT));
    // HTTP/1.0 + `Connection: close`:响应不会被 chunked,读到底就是body。
    let req = b"GET /api/instance HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    if sock.write_all(req).is_err() {
        return Probe::Foreign(None);
    }
    let mut buf = Vec::new();
    let _ = sock.take(MAX_READ).read_to_end(&mut buf);
    classify(&buf)
}

/// 从 HTTP 响应里认指纹:JSON body 的 `app == "telesh"` 才算,否则当陌生服务。
fn classify(data: &[u8]) -> Probe {
    let text = String::from_utf8_lossy(data);
    let Some((headers, body)) = text.split_once("\r\n\r\n") else {
        return Probe::Foreign(None);
    };
    let server = headers
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("server:"))
        .map(|l| l[l.find(':').map_or(0, |i| i + 1)..].trim().to_string())
        .filter(|s| !s.is_empty());
    match serde_json::from_str::<Instance>(body.trim()) {
        Ok(info) if info.app == "telesh" => Probe::Telesh(Box::new(info)),
        _ => Probe::Foreign(server),
    }
}

/// 启动时端口被占用:`main` 已经知道 bind 失败了,这里给出人话并收尾。
///
/// 返回 `Ok` = 「已经有一个 telesh 在跑」,这不是错误(退出码 0)。
pub fn handle_occupied(port: u16, open: bool) -> Result<()> {
    match probe(port) {
        Probe::Telesh(info) => {
            println!(
                "telesh 已在运行:pid {},已运行 {}",
                info.pid,
                humanize(info.uptime_secs)
            );
            println!("面板:{}", info.url());
            if open {
                open_panel(&info.url());
            } else {
                println!("打开面板:`telesh open`(或直接用浏览器访问上面的地址)");
            }
            Ok(())
        }
        Probe::Foreign(server) => bail!(
            "127.0.0.1:{port} 上占着端口的不是 telesh{};换一个端口 `telesh --ui-port <PORT>`(若那其实是旧版 telesh —— 没有 /api/instance —— 升级后再试)",
            server.map(|s| format!("({s})")).unwrap_or_default()
        ),
        Probe::Nothing => bail!(
            "127.0.0.1:{port} 刚才还是被占用的,现在又没人监听了;重试,或用 `--ui-port` 换端口"
        ),
    }
}

/// `telesh open`:在浏览器里打开正在运行的面板。
pub fn open(port: u16, print_only: bool) -> Result<()> {
    match probe(port) {
        Probe::Telesh(info) => {
            let url = info.url();
            println!("{url}");
            if !print_only {
                open_panel(&url);
            }
            Ok(())
        }
        Probe::Foreign(server) => bail!(
            "127.0.0.1:{port} 上不是 telesh{},没有可以打开的面板(旧版 telesh 没有 /api/instance,升级后再试)",
            server.map(|s| format!("({s})")).unwrap_or_default()
        ),
        Probe::Nothing => bail!(
            "127.0.0.1:{port} 上没有 telesh 在跑;先起一个,或用 `--ui-port` 指定端口"
        ),
    }
}

/// 有图形会话就调起默认浏览器,没有就把地址打出来 —— SSH 里真正需要的是那行地址。
pub fn open_panel(url: &str) {
    if !has_display() {
        println!("没有图形会话,跳过打开浏览器;面板地址:{url}");
        return;
    }
    match open_browser(url) {
        Ok(()) => println!("已在默认浏览器里打开面板"),
        Err(e) => println!("调起浏览器失败({e});直接访问 {url}"),
    }
}

/// 有没有图形会话可以弹浏览器。Linux 看 `DISPLAY`/`WAYLAND_DISPLAY`,
/// macOS/Windows 假定有。
pub fn has_display() -> bool {
    if cfg!(target_os = "linux") {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    } else {
        true
    }
}

/// 调起系统默认浏览器(不等它退出,也不显示它的输出)。
fn open_browser(url: &str) -> Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        // `start` 是 cmd 的内建命令;那个空参数是窗口标题的占位。
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().context("cannot launch a browser")?;
    // 收掉它,免得留僵尸。
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// 「42s」/「12m」/「3h07m」/「2d04h」。
pub fn humanize(secs: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 拼一个和 axum 实际返回形式一致的响应。
    fn http(body: &str) -> String {
        format!(
            "HTTP/1.0 200 OK\r\ncontent-type: application/json\r\nserver: telesh/0.1.0\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    const BODY: &str = r#"{"app":"telesh","version":"0.1.0","pid":42,"ui_addr":"127.0.0.1:16345","uptime_secs":90,"started_unix":1760000000}"#;

    #[test]
    fn recognises_telesh() {
        match classify(http(BODY).as_bytes()) {
            Probe::Telesh(info) => {
                assert_eq!(info.pid, 42);
                assert_eq!(info.url(), "http://127.0.0.1:16345");
            }
            other => panic!("应该是 telesh,得到 {other:?}"),
        }
    }

    #[test]
    fn foreign_service_is_not_telesh() {
        let raw = "HTTP/1.1 200 OK\r\nserver: nginx/1.24.0\r\ncontent-length: 12\r\n\r\nhello world\n";
        match classify(raw.as_bytes()) {
            Probe::Foreign(server) => assert_eq!(server.as_deref(), Some("nginx/1.24.0")),
            other => panic!("应该是陌生服务,得到 {other:?}"),
        }
    }

    #[test]
    fn other_json_is_not_telesh() {
        // 别的程序也可能回 JSON(dev server 之类),没有 app 字段就不认。
        assert!(matches!(
            classify(http(r#"{"status":"ok"}"#).as_bytes()),
            Probe::Foreign(_)
        ));
    }

    #[test]
    fn old_telesh_without_the_route_is_foreign() {
        // 旧版没有 /api/instance:404 + 空 body,不能误判成 telesh。
        let raw = "HTTP/1.1 404 Not Found\r\nserver: telesh/0.0.9\r\ncontent-length: 0\r\n\r\n";
        assert!(matches!(classify(raw.as_bytes()), Probe::Foreign(_)));
    }

    #[test]
    fn garbage_is_foreign() {
        assert!(matches!(classify(b"\x16\x03\x01\x00\x00"), Probe::Foreign(None)));
    }

    #[test]
    fn humanize_buckets() {
        assert_eq!(humanize(0), "0s");
        assert_eq!(humanize(59), "59s");
        assert_eq!(humanize(60), "1m");
        assert_eq!(humanize(3599), "59m");
        assert_eq!(humanize(3600), "1h00m");
        assert_eq!(humanize(11220), "3h07m");
        assert_eq!(humanize(86400), "1d00h");
        assert_eq!(humanize(190_000), "2d04h");
    }
}
