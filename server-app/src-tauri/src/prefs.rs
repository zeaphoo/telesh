//! 托盘里那点"设置"的持久化。目前只有一项:关窗口是收进托盘还是直接退出。
//!
//! 存 `<app_config_dir>/prefs.json`(由 `tauri.conf.json` 的 identifier 决定),
//! 和 telesh-server 自己的 `<dataroot>` 无关,互不干扰。

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prefs {
    /// 点窗口关闭按钮时:true = 收进托盘继续跑,false = 直接退出(顺带关掉后端)。
    pub close_to_tray: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        // 托盘应用的常规行为;想一关就退,在托盘菜单里改一下。
        Self {
            close_to_tray: true,
        }
    }
}

static PREFS: Mutex<Option<Prefs>> = Mutex::new(None);

/// 启动时读一次。读不到就用默认值,并且**不写盘** —— 等用户真的改过再落盘。
pub fn init(app: &AppHandle) {
    let prefs = std::fs::read_to_string(path(app))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    *PREFS.lock().expect("prefs mutex") = Some(prefs);
}

pub fn close_to_tray() -> bool {
    current(|p| p.close_to_tray)
}

/// 写入并落盘。
pub fn set_close_to_tray(app: &AppHandle, on: bool) {
    let snapshot = {
        let mut guard = PREFS.lock().expect("prefs mutex");
        let prefs = guard.get_or_insert_with(Prefs::default);
        if prefs.close_to_tray == on {
            return;
        }
        prefs.close_to_tray = on;
        prefs.clone()
    };
    save(app, &snapshot);
}

fn current<T>(f: impl FnOnce(&Prefs) -> T) -> T {
    let guard = PREFS.lock().expect("prefs mutex");
    match guard.as_ref() {
        Some(prefs) => f(prefs),
        None => f(&Prefs::default()),
    }
}

fn path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("prefs.json")
}

fn save(app: &AppHandle, prefs: &Prefs) {
    let path = path(app);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(prefs) {
        Ok(body) => {
            if let Err(e) = std::fs::write(&path, body) {
                eprintln!("[telesh-server-app] 写 {} 失败: {e}", path.display());
            }
        }
        Err(e) => eprintln!("[telesh-server-app] 序列化设置失败: {e}"),
    }
}
