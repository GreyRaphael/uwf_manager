//! UWF Manager —— Windows 统一写入筛选器（UWF）的简洁 TUI 管理工具。
//!
//! 架构：UI 线程负责渲染与键鼠交互，后端线程以 actor 模式独占 `dyn UwfBackend`
//! （WMI 连接为 `!Send`），两者经 `std::sync::mpsc` 通信，全程序无异步运行时。
//!
//! unsafe 政策：crate 级 deny；唯一例外是 `backend::windows::elevate` 中
//! 提权检测的 Win32 只读调用（该模块内显式 allow）。

#![deny(unsafe_code)]

mod app;
mod backend;
mod error;
mod model;
mod ui;

use std::time::Duration;

use anyhow::Context;
use backend::{create_backend_factory, spawn_backend_thread};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;

/// RAII 守卫：退出或异常返回时自动关闭终端鼠标捕获。
struct MouseCaptureGuard;

impl MouseCaptureGuard {
    fn enter() -> Self {
        let _ = execute!(std::io::stdout(), EnableMouseCapture);
        Self
    }
}

impl Drop for MouseCaptureGuard {
    fn drop(&mut self) {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
}

fn main() -> anyhow::Result<()> {
    // `--mock`：任意平台强制使用 Mock 后端。
    // 注意：Windows 下 manifest 仍是 requireAdministrator，--mock 只跳过运行时检测。
    let force_mock = std::env::args().any(|arg| arg == "--mock");

    // D5 兜底：Windows 真机模式（非 --mock）下检测管理员权限。
    #[cfg(windows)]
    if !force_mock && !backend::is_elevated() {
        eprintln!("错误：UWF 管理操作需要管理员权限。");
        eprintln!("请右键“以管理员身份运行”终端（Windows Terminal / cmd / PowerShell）后重试，");
        eprintln!("或以 Mock 模式预览界面：uwf_manager --mock");
        std::process::exit(2);
    }

    // 先安装自定义 hook（关闭鼠标捕获并打印错误）；ratatui::init 的 hook 会在 panic 时
    // 先恢复终端再链入本 hook。
    std::panic::set_hook(Box::new(|info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        eprintln!("\n程序发生内部错误并已退出: {info}\n");
    }));

    let mut terminal = ratatui::init();
    let _mouse_guard = MouseCaptureGuard::enter();

    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let (ui_tx, ui_rx) = std::sync::mpsc::channel();
    let (factory, kind) = create_backend_factory(force_mock);
    let backend = spawn_backend_thread(factory, cmd_rx, ui_tx.clone());

    // 终端事件直汇入 UI channel，与后端回包一起立即唤醒主循环。
    std::thread::spawn(move || {
        while let Ok(evt) = event::read() {
            if ui_tx.send(app::UiEvent::Terminal(evt)).is_err() {
                break;
            }
        }
    });

    let mut app = app::App::new(cmd_tx, ui_rx, kind);
    app.refresh_all();
    let result = app.run(&mut terminal);

    // 无论 run 正常/异常返回，都先关闭鼠标捕获并通知后端退出，再恢复终端。
    drop(_mouse_guard);
    app.send_quit();
    ratatui::restore();
    backend.wait(Duration::from_secs(2));
    result.context("应用运行错误")
}
