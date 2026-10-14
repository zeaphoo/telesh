//! telesh 后端进程(sidecar)的生命周期:探活 / 拉起 / 退出时关掉。
//!
//! 面板是 HTTP(`127.0.0.1:16345`),app 只是外壳,所以后端有两种来源:
//!
//! 1. **已经在跑** —— 装成了服务(systemd / launchd / Windows Service),或者
//!    用户自己起了一个。这时我们只连,不碰它的生命周期;
//! 2. **没在跑** —— 拉起随包分发的 `telesh`,**退出时由我们负责关掉**(见 [`shutdown`])。
//!
//! 已知缺口:app 被强杀(`kill -9` / 任务管理器)时这里不会执行。实测在
//! Linux/macOS 上子进程通常也会跟着走 —— 它的 stdout 管道断开,而下一次写日志
//! 就会拿到 EPIPE/SIGPIPE(telesh 连不上 control server 时会一直重试并打日志)。
//! 但这属于顺带效果,**别依赖**:Windows 上没有 SIGPIPE,强杀会真的留下孤儿。
//! 要兜住就得让 telesh 自带父进程看门狗,或者宿主用 Job Object。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 面板地址,必须和 `crates/telesh/src/main.rs` 里的 `DEFAULT_UI_PORT` 一致。
pub const UI_ADDR: &str = "127.0.0.1:16345";

/// 等后端监听就绪的上限。
const READY_TIMEOUT: Duration = Duration::from_secs(15);
/// 单次探测超时(要短,否则每次启动都白等)。
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// 就绪轮询间隔。
const READY_POLL: Duration = Duration::from_millis(150);
/// 探测响应最多读这么多(面板只回一小段 JSON)。
const PROBE_READ: u64 = 8 * 1024;

/// 我们拉起的那个子进程。`None` = 后端是外部的(服务/别人起的),退出时不该动它。
static CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// 后端就绪情况。
pub enum Outcome {
    /// 确认端口后面是 telesh,可以开窗。
    Ready,
    /// 端口被别的程序占着 —— **不开窗**,否则窗口会指向别人的服务。
    Foreign(String),
    /// 没起来/没等到就绪 —— 照旧开窗,让用户看到错误,总比什么都不弹强。
    NotReady(String),
}

/// 一次探测的结论。
enum Probe {
    Telesh,
    Foreign(Option<String>),
    Nothing,
}

/// 确保后端可用:已经在跑就直接用,没跑就拉起 sidecar 并等它就绪。
pub fn ensure_running() -> Outcome {
    match probe_dashboard() {
        // 服务模式,或者用户自己起了一个:只连,不碰它的生命周期。
        Probe::Telesh => Outcome::Ready,
        Probe::Foreign(server) => Outcome::Foreign(format!(
            "{UI_ADDR} 上的不是 telesh{}(可能有别的服务占了这个端口)",
            server.map(|s| format!("({s})")).unwrap_or_default()
        )),
        Probe::Nothing => start_sidecar(),
    }
}

/// 探一下端口后面是不是 telesh:只看 `GET /api/instance` 的 JSON 里
/// `app == "telesh"`。
///
/// 裸 TCP connect 不够 —— 端口上可能站着别人的服务,那样窗口就会被指到一个
/// 陌生页面。判据和 `crates/telesh/src/instance.rs` 保持一致。
fn probe_dashboard() -> Probe {
    let Ok(addr) = UI_ADDR.parse::<SocketAddr>() else {
        return Probe::Nothing;
    };
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
    match serde_json::from_str::<serde_json::Value>(body.trim()) {
        Ok(json) if json.get("app").and_then(|v| v.as_str()) == Some("telesh") => Probe::Telesh,
        _ => Probe::Foreign(server),
    }
}

/// 拉起随包的 `telesh` 并等它就绪。
fn start_sidecar() -> Outcome {
    let Some(bin) = find_binary() else {
        return Outcome::NotReady(
            "找不到 telesh 可执行文件;用 TELESH_BIN=/path/to/telesh 指定,或先跑 scripts/prepare-sidecar.sh"
                .to_string(),
        );
    };

    let mut cmd = Command::new(&bin);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return Outcome::NotReady(format!("启动 {} 失败: {e}", bin.display())),
    };

    // 子进程输出转发到 app 的 stderr,方便在 `cargo tauri dev` 里排查。
    // (顺带一个副作用:这两根管道会让被强杀的 app 带不走孤儿进程,见文件头注释。)
    if let Some(out) = child.stdout.take() {
        forward(out);
    }
    if let Some(err) = child.stderr.take() {
        forward(err);
    }

    *CHILD.lock().expect("child mutex") = Some(child);
    wait_ready()
}

/// app 退出时调用:只关我们自己拉起的那个,外部实例不动。
pub fn shutdown() {
    let child = CHILD.lock().expect("child mutex").take();
    if let Some(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn forward<R: std::io::Read + Send + 'static>(reader: R) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            eprintln!("[telesh] {line}");
        }
    });
}

fn wait_ready() -> Outcome {
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        if matches!(probe_dashboard(), Probe::Telesh) {
            return Outcome::Ready;
        }
        std::thread::sleep(READY_POLL);
    }
    // 超时了:再探一次,把「为什么不算就绪」写进消息 —— 最常见的原因是
    // 随包的 sidecar 是旧版(没有 /api/instance),只回 404。
    Outcome::NotReady(format!(
        "telesh 在 {READY_TIMEOUT:?} 内没有在 {UI_ADDR} 上就绪({})",
        match probe_dashboard() {
            Probe::Telesh => "再去探一次就成功了".to_string(),
            Probe::Foreign(Some(server)) => format!(
                "端口上回的是 {server},不是 /api/instance 的指纹 —— 随包的 sidecar 是不是旧版?"
            ),
            Probe::Foreign(None) => "端口上回的不是 telesh 的 /api/instance".to_string(),
            Probe::Nothing => "端口上没有监听".to_string(),
        }
    ))
}

/// 找 sidecar:先看 `TELESH_BIN`,再看 app 同目录(打包后的布局),最后是开发时
/// 的 `target/{debug,release}`。
fn find_binary() -> Option<PathBuf> {
    let ext = if cfg!(windows) { ".exe" } else { "" };
    let plain = format!("telesh{ext}");
    let triple = format!("telesh-{}{ext}", env!("TELESH_TARGET_TRIPLE"));

    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(p) = std::env::var("TELESH_BIN") {
        candidates.push(PathBuf::from(p));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // 打包后:Tauri 把 externalBin 放在 app 二进制旁边(triple 后缀已去掉)。
            candidates.push(dir.join(&plain));
            candidates.push(dir.join(&triple));
            // macOS .app 的另一种摆法:Contents/Resources/。
            candidates.push(dir.join("../Resources").join(&plain));
        }
    }

    // 开发期(`cargo tauri dev`):二进制在本 crate 的 target/,或工作区的 target/。
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for profile in ["debug", "release"] {
        candidates.push(manifest.join("target").join(profile).join(&plain));
        candidates.push(manifest.join("../../target").join(profile).join(&plain));
    }

    candidates.into_iter().find(|p| p.is_file())
}
