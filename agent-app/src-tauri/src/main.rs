//! telesh agent 桌面外壳。
//!
//! - 后端就是 `crates/telesh`(无头 CLI),这里以 sidecar 方式拉起,不共享代码;
//! - 窗口内容是外部地址 `http://127.0.0.1:16345`,即 agent 自带的面板,
//!   所以 `agent-ui` 一份代码同时服务浏览器和这个 app;
//! - 保留系统边框(`decorations` 保持默认 true),不做自绘标题栏;
//! - `tray.rs` 管托盘与设置项,`prefs.rs` 管持久化,`agent.rs` 管后端进程。
//!
//! 退出语义(和产品要求一致):**app 退出时关掉我们拉起的 telesh**。
//! 窗口关闭按钮的行为由托盘里的"关闭窗口时最小化到托盘"决定。

// 发布版在 Windows 上不要带控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod prefs;
mod tray;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// 面板地址:必须和 `crates/telesh/src/main.rs` 的 `UI_ADDR` 一致。
const UI_URL: &str = "http://127.0.0.1:16345";
/// 主窗口 label。
const MAIN_WINDOW: &str = "main";

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            prefs::init(&handle);
            tray::init(&handle)?;

            // 先把后端拉起来、等它就绪,再开窗。反过来的话 webview 会先撞上
            // connection refused,而外部 http 页面不会自己重试。
            let worker = handle.clone();
            std::thread::spawn(move || {
                let outcome = agent::ensure_running();
                match &outcome {
                    agent::Outcome::Ready => {}
                    agent::Outcome::NotReady(e) => {
                        eprintln!("[telesh-app] 后端未就绪: {e}");
                    }
                    agent::Outcome::Foreign(e) => {
                        eprintln!("[telesh-app] {e};不打开窗口(托盘还在,可从菜单退出)");
                    }
                }
                // 端口后面不是 telesh 就别开窗 —— 否则窗口会指到别人的服务上。
                if matches!(outcome, agent::Outcome::Foreign(_)) {
                    return;
                }
                // 窗口必须在主线程建(macOS 尤其如此)。
                let opener = worker.clone();
                if let Err(e) = worker.run_on_main_thread(move || {
                    if let Err(e) = open_window(&opener) {
                        eprintln!("[telesh-app] 打开窗口失败: {e}");
                    }
                }) {
                    eprintln!("[telesh-app] 无法调度主线程: {e}");
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if prefs::close_to_tray() {
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    // 显式退出,保证"退出时关闭 telesh"在三平台都成立
                    // (不依赖"关掉最后一个窗口是否自动退出"的默认行为)。
                    window.app_handle().exit(0);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building telesh-app")
        .run(|_app, event| {
            // 唯一的退出收口:`Exit` 之后进程就结束了,这里把 sidecar 收掉。
            if let tauri::RunEvent::Exit = event {
                agent::shutdown();
            }
        });
}

/// 打开(或唤回)主窗口。系统边框保留。
pub fn open_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return Ok(());
    }

    let url: tauri::Url = UI_URL.parse().expect("valid dashboard url");
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::External(url))
        .title("telesh")
        .inner_size(1120.0, 740.0)
        .min_inner_size(900.0, 580.0)
        .center()
        .build()?;
    Ok(())
}
