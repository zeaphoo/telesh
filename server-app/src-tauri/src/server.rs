//! telesh-server 后端进程(sidecar)的生命周期:探活 / 拉起 / 退出时关掉。
//!
//! 和 `agent-app` 的思路一致:窗口内容是 HTTP dashboard,外壳只负责把后端
//! 拉起来并开一个 webview。后端有两种来源:
//!
//! 1. **已经在跑** —— 装成服务(systemd / launchd / Windows Service),或者
//!    用户自己起了一个。这时我们只连,不碰它的生命周期;
//! 2. **没在跑** —— 拉起随包分发的 `telesh-server`,**退出时由我们负责关掉**
//!    (见 [`shutdown`])。
//!
//! 配置文件由 `TELESH_SERVER_CONFIG` 指定;没设时用 app 配置目录里的
//! `server.toml`,不存在就写一份默认的。dashboard 地址从配置的 `control_addr`
//! 推导(通配地址换成回环地址),这样用户改了端口 app 也能找对地方。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Manager};

/// telesh-server 的默认控制面端口;dashboard 和控制在同一个监听上。
pub const DEFAULT_CONTROL_ADDR: &str = "0.0.0.0:9009";

/// `/api/instance` 里认指纹用的 `app` 字段。和 agent 的 `"telesh"` 区分开,
/// 避免把两边的面板认混。
const APP_FINGERPRINT: &str = "telesh-server";

/// 等后端监听就绪的上限(server 还要建库,比 agent 稍长一点)。
const READY_TIMEOUT: Duration = Duration::from_secs(20);
/// 单次探测超时。
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// 就绪轮询间隔。
const READY_POLL: Duration = Duration::from_millis(150);
/// 探测响应最多读这么多。
const PROBE_READ: u64 = 8 * 1024;

/// 我们拉起的那个子进程。`None` = 后端是外部的(服务/别人起的),退出时不该动它。
static CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// 后端就绪情况。
pub enum Outcome {
    /// 确认端口后面是 telesh-server,可以开窗。
    Ready,
    /// 端口被别的程序占着 —— **不开窗**,否则窗口会指向别人的服务。
    Foreign(String),
    /// 没起来/没等到就绪 —— 照旧开窗,让用户看到错误。
    NotReady(String),
}

/// 一次探测的结论。
enum Probe {
    Telesh,
    Foreign(Option<String>),
    Nothing,
}

/// dashboard 的本机地址与 URL。
#[derive(Debug, Clone)]
pub struct Dashboard {
    pub addr: SocketAddr,
    pub url: String,
}

/// 配置文件路径:环境变量优先,否则 `<app_config_dir>/server.toml`。
pub fn config_path(app: &AppHandle) -> PathBuf {
    if let Some(path) = std::env::var_os("TELESH_SERVER_CONFIG") {
        return PathBuf::from(path);
    }
    app.path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("server.toml")
}

/// 确保配置文件存在。环境变量显式指定时**不自动创建**(那是用户给的文件);
/// app 自己的默认配置则在第一次启动时写一份,方便用户从托盘打开编辑。
pub fn ensure_config(app: &AppHandle) -> Result<PathBuf, String> {
    let path = config_path(app);
    if path.is_file() {
        return Ok(path);
    }
    if std::env::var_os("TELESH_SERVER_CONFIG").is_some() {
        return Err(format!(
            "TELESH_SERVER_CONFIG 指向的配置文件不存在: {}",
            path.display()
        ));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("无法创建 {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, default_config(app, &path))
        .map_err(|e| format!("无法写入 {}: {e}", path.display()))?;
    Ok(path)
}

/// 从配置里读 dashboard 地址;读不到就用默认值。
pub fn dashboard(app: &AppHandle) -> Dashboard {
    let addr = read_control_addr(&config_path(app))
        .unwrap_or_else(|| DEFAULT_CONTROL_ADDR.parse().expect("valid default"));
    let addr = loopback(addr);
    Dashboard {
        addr,
        url: format!("http://{addr}"),
    }
}

/// 确保后端可用:已经在跑就直接用,没跑就拉起 sidecar 并等它就绪。
pub fn ensure_running(app: &AppHandle) -> Outcome {
    let config = match ensure_config(app) {
        Ok(path) => path,
        Err(e) => return Outcome::NotReady(e),
    };
    let dashboard = dashboard(app);
    match probe(dashboard.addr) {
        Probe::Telesh => Outcome::Ready,
        Probe::Foreign(server) => Outcome::Foreign(format!(
            "{} 上的不是 telesh-server{}(可能有别的服务占了这个端口)",
            dashboard.url,
            server.map(|s| format!("({s})")).unwrap_or_default()
        )),
        Probe::Nothing => start_sidecar(&config, &dashboard),
    }
}

/// 端口上是不是**别的**服务。托盘在后台唤回窗口前用它兜底:主流程已经
/// 拒绝给陌生服务开窗,托盘入口不能绕过这个判断。
pub fn is_foreign(app: &AppHandle) -> bool {
    matches!(probe(dashboard(app).addr), Probe::Foreign(_))
}

/// app 退出时调用:只关我们自己拉起的那个,外部实例不动。
pub fn shutdown() {
    let child = CHILD.lock().expect("child mutex").take();
    if let Some(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// 探一下端口后面是不是 telesh-server:只看 `GET /api/instance` 的 JSON 里
/// `app == "telesh-server"`。裸 TCP connect 不够 —— 端口上可能站着别人的服务。
fn probe(addr: SocketAddr) -> Probe {
    let Ok(mut sock) = TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) else {
        return Probe::Nothing;
    };
    let _ = sock.set_read_timeout(Some(PROBE_TIMEOUT));
    let _ = sock.set_write_timeout(Some(PROBE_TIMEOUT));
    // HTTP/1.0 + `Connection: close`:响应不会被 chunked,读到底就是 body。
    let req = b"GET /api/instance HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    if sock.write_all(req).is_err() {
        return Probe::Foreign(None);
    }
    let mut buf = Vec::new();
    let _ = sock.take(PROBE_READ).read_to_end(&mut buf);
    classify(&buf)
}

/// 从 HTTP 响应里认指纹。
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
    match serde_json::from_str::<serde_json::Value>(body.trim()) {
        Ok(json) if json.get("app").and_then(|v| v.as_str()) == Some(APP_FINGERPRINT) => {
            Probe::Telesh
        }
        _ => Probe::Foreign(server),
    }
}

/// 拉起随包的 `telesh-server` 并等它就绪。
fn start_sidecar(config: &Path, dashboard: &Dashboard) -> Outcome {
    let Some(bin) = find_binary() else {
        return Outcome::NotReady(
            "找不到 telesh-server 可执行文件;用 TELESH_SERVER_BIN=/path/to/telesh-server 指定,或先跑 scripts/prepare-sidecar.sh"
                .to_string(),
        );
    };

    let mut cmd = Command::new(&bin);
    cmd.arg(config)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return Outcome::NotReady(format!("启动 {} 失败: {e}", bin.display())),
    };

    // 子进程输出转发到 app 的 stderr,方便在 `cargo tauri dev` 里排查。
    if let Some(out) = child.stdout.take() {
        forward(out);
    }
    if let Some(err) = child.stderr.take() {
        forward(err);
    }

    *CHILD.lock().expect("child mutex") = Some(child);
    wait_ready(dashboard)
}

fn forward<R: std::io::Read + Send + 'static>(reader: R) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            eprintln!("[telesh-server] {line}");
        }
    });
}

fn wait_ready(dashboard: &Dashboard) -> Outcome {
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        if matches!(probe(dashboard.addr), Probe::Telesh) {
            return Outcome::Ready;
        }
        std::thread::sleep(READY_POLL);
    }
    Outcome::NotReady(format!(
        "telesh-server 在 {READY_TIMEOUT:?} 内没有在 {} 上就绪({})",
        dashboard.url,
        match probe(dashboard.addr) {
            Probe::Telesh => "再去探一次就成功了".to_string(),
            Probe::Foreign(Some(server)) => format!(
                "端口上回的是 {server},不是 /api/instance 的指纹 —— 随包的 sidecar 是不是旧版?"
            ),
            Probe::Foreign(None) => "端口上回的不是 telesh-server 的 /api/instance".to_string(),
            Probe::Nothing => "端口上没有监听".to_string(),
        }
    ))
}

fn read_control_addr(path: &Path) -> Option<SocketAddr> {
    #[derive(Deserialize)]
    struct ConfigFile {
        control_addr: Option<String>,
    }

    let text = std::fs::read_to_string(path).ok()?;
    let file: ConfigFile = toml::from_str(&text).ok()?;
    file.control_addr?.trim().parse().ok()
}

/// 通配地址换成回环地址,其它地址原样保留。
fn loopback(addr: SocketAddr) -> SocketAddr {
    let ip = match addr.ip() {
        IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, addr.port())
}

/// 给 app 自己管的配置文件写一份默认内容。
fn default_config(app: &AppHandle, path: &Path) -> String {
    let dataroot = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("data");
    let dataroot = dataroot.to_string_lossy();

    format!(
        r#"# telesh-server 配置(由 telesh server 桌面外壳生成)
#
# 文件位置:{path}
# 改完后退出并重新打开 app 生效。
#
# 数据目录:数据库、ACME 证书缓存等都放这里。
dataroot = {dataroot:?}

# 控制面 + dashboard 共用的监听地址。app 的窗口连 127.0.0.1:<端口>。
control_addr = "0.0.0.0:9009"

# 边缘入口:公网 HTTP / TCP 从这里进来。
ingress_addr = "0.0.0.0:8080"

# 数据面:agent 主动连这里。
data_addr = "0.0.0.0:9008"

# 拼隧道 URL 用的公网后缀,例如 example.com。
public_host = ""

# TCP 隧道端口范围。
tcp_port_range = "18000-18100"

# 内嵌 iroh 中继(默认不跑):
# [relay]
# serve = "0.0.0.0:8081"
# url = "https://relay.example.com"
"#,
        path = path.display(),
        dataroot = dataroot,
    )
}

/// 找 sidecar:先看 `TELESH_SERVER_BIN`,再看 app 同目录(打包后的布局),最后
/// 是开发时的 `target/{debug,release}`。
fn find_binary() -> Option<PathBuf> {
    let ext = if cfg!(windows) { ".exe" } else { "" };
    let plain = format!("telesh-server{ext}");
    let triple = format!("telesh-server-{}{ext}", env!("TELESH_TARGET_TRIPLE"));

    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(p) = std::env::var("TELESH_SERVER_BIN") {
        candidates.push(PathBuf::from(p));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // 打包后:Tauri 把 externalBin 放在 app 二进制旁边。
            candidates.push(dir.join(&plain));
            candidates.push(dir.join(&triple));
            // macOS .app 的另一种摆法:Contents/Resources/。
            candidates.push(dir.join("../Resources").join(&plain));
        }
    }

    // 开发期:二进制在本 crate 的 target/,或工作区的 target/。
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for profile in ["debug", "release"] {
        candidates.push(manifest.join("target").join(profile).join(&plain));
        candidates.push(manifest.join("../../target").join(profile).join(&plain));
    }

    candidates.into_iter().find(|p| p.is_file())
}
