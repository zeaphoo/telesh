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

use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 面板地址,必须和 `crates/telesh/src/main.rs` 里的 `UI_ADDR` 一致。
pub const UI_ADDR: &str = "127.0.0.1:16345";

/// 等后端监听就绪的上限。
const READY_TIMEOUT: Duration = Duration::from_secs(15);
/// 单次探活超时(要短,否则每次启动都白等)。
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);
/// 就绪轮询间隔。
const READY_POLL: Duration = Duration::from_millis(150);

/// 我们拉起的那个子进程。`None` = 后端是外部的(服务/别人起的),退出时不该动它。
static CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// 端口已在监听 = 后端已经在跑(服务模式,或已经有一个实例)。
pub fn dashboard_up() -> bool {
    let Ok(addr) = UI_ADDR.parse::<SocketAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok()
}

/// 确保后端可用:在跑就直接返回,没跑就拉起 sidecar 并等端口就绪。
pub fn ensure_running() -> Result<(), String> {
    if dashboard_up() {
        return Ok(());
    }

    let bin = find_binary().ok_or_else(|| {
        "找不到 telesh 可执行文件;用 TELESH_BIN=/path/to/telesh 指定,或先跑 scripts/prepare-sidecar.sh"
            .to_string()
    })?;

    let mut cmd = Command::new(&bin);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 {} 失败: {e}", bin.display()))?;

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

fn wait_ready() -> Result<(), String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        if dashboard_up() {
            return Ok(());
        }
        std::thread::sleep(READY_POLL);
    }
    Err(format!("telesh 在 {READY_TIMEOUT:?} 内没有监听 {UI_ADDR}"))
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
