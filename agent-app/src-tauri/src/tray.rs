//! 托盘图标与菜单。菜单就是"设置"入口:
//!
//! - 打开面板
//! - 关闭窗口时最小化到托盘(勾选项,落盘在 `prefs.rs`)
//! - 退出并关闭 telesh
//!
//! 菜单文案直接写死中文:原生托盘不走 `agent-ui` 的 i18next。

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

const MENU_SHOW: &str = "show";
const MENU_CLOSE_TO_TRAY: &str = "close-to-tray";
const MENU_QUIT: &str = "quit";

/// macOS 用 template 图标(只取 alpha,由系统按亮/暗菜单栏着色);
/// 其它平台用带底色的彩色图标 —— 深浅任务栏都能看清。
#[cfg(target_os = "macos")]
const TRAY_ICON: tauri::image::Image<'_> = tauri::include_image!("icons/tray-mac.png");
#[cfg(not(target_os = "macos"))]
const TRAY_ICON: tauri::image::Image<'_> = tauri::include_image!("icons/tray.png");

pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, MENU_SHOW, "打开面板", true, None::<&str>)?;
    let close_to_tray = CheckMenuItem::with_id(
        app,
        MENU_CLOSE_TO_TRAY,
        "关闭窗口时最小化到托盘",
        true,
        crate::prefs::close_to_tray(),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "退出并关闭 telesh", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &PredefinedMenuItem::separator(app)?,
            &close_to_tray,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    TrayIconBuilder::with_id("main")
        .icon(TRAY_ICON)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("telesh agent")
        .menu(&menu)
        // Linux(libappindicator)不支持这个开关:左键永远弹菜单,
        // 所以"打开面板"必须留在菜单里,不能只靠左键。
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_SHOW => open_panel(app),
            // CheckMenuItem 的勾选状态由 muda 自己切换,这里按"取反"落盘保持一致。
            MENU_CLOSE_TO_TRAY => {
                crate::prefs::set_close_to_tray(app, !crate::prefs::close_to_tray());
            }
            MENU_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                open_panel(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn open_panel(app: &AppHandle) {
    if let Err(e) = crate::open_window(app) {
        eprintln!("[telesh-app] 打开面板失败: {e}");
    }
}
