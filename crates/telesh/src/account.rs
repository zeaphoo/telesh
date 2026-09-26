//! 本机 agent 的登录状态与账户信息,供本地 UI(`GET /api/account`)展示。
//!
//! 静态字段(控制服务器、数据目录、身份公钥)在启动时写入;动态字段(登录状态、
//! 账户句柄、显示名、会话过期)在每次登录成功/断线时更新。

use std::sync::{Mutex, OnceLock};

use serde::Serialize;

const STATE_CONNECTING: &str = "connecting";
const STATE_ONLINE: &str = "online";
const STATE_OFFLINE: &str = "offline";

#[derive(Debug, Clone, Default, Serialize)]
pub struct AccountInfo {
    /// control server 地址(`host:port`)。
    pub control_server: String,
    /// 数据目录。
    pub dataroot: String,
    /// 本机身份公钥(hex,64 字符);同时就是账户密钥。
    pub public_key: String,
    /// `connecting` | `online` | `offline`。
    pub state: String,
    /// 9 位账户句柄(登录后才有)。
    pub account: Option<String>,
    /// 账户/节点显示名。
    pub display_name: Option<String>,
    /// 当前会话过期时间(unix 秒;0 = 未知)。
    pub session_expires_at: u64,
    /// 最近一次登录成功的 unix 秒。
    pub last_login_at: u64,
    /// 最近一次连接错误。
    pub last_error: Option<String>,
}

static INFO: OnceLock<Mutex<AccountInfo>> = OnceLock::new();

fn cell() -> &'static Mutex<AccountInfo> {
    INFO.get_or_init(|| {
        Mutex::new(AccountInfo {
            state: STATE_OFFLINE.to_string(),
            ..Default::default()
        })
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 启动时写入静态信息。
pub fn init(control_server: String, dataroot: String, public_key: [u8; 32]) {
    let mut info = cell().lock().unwrap();
    info.control_server = control_server;
    info.dataroot = dataroot;
    info.public_key = to_hex(&public_key);
    info.state = STATE_CONNECTING.to_string();
}

/// 正在连接(每次重连前)。
pub fn set_connecting() {
    cell().lock().unwrap().state = STATE_CONNECTING.to_string();
}

/// 登录成功。
pub fn set_online(account: String, display_name: String, session_expires_at: u64) {
    let mut info = cell().lock().unwrap();
    info.account = Some(account);
    info.display_name = Some(display_name);
    info.session_expires_at = session_expires_at;
    info.last_login_at = now_secs();
    info.last_error = None;
    info.state = STATE_ONLINE.to_string();
}

/// 会话结束(正常断开或出错)。保留账户信息用于展示,只把状态标成离线。
pub fn set_offline(error: impl Into<String>) {
    let mut info = cell().lock().unwrap();
    info.state = STATE_OFFLINE.to_string();
    info.last_error = Some(error.into());
}

/// 当前快照(UI 读取)。
pub fn snapshot() -> AccountInfo {
    cell().lock().unwrap().clone()
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
