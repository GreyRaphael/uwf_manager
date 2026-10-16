//! UWF Manager —— Windows 统一写入筛选器（UWF）的 TUI 管理工具。
//!
//! 架构（PLAN v2 D2/D4）：UI 线程负责渲染与按键，后端线程以 actor 模式独占
//! `dyn UwfBackend`（WMI 连接为 `!Send`），两者经 `std::sync::mpsc` 通信，
//! 全程序无异步运行时、无 unsafe。

#![forbid(unsafe_code)]

mod app;
mod backend;
mod error;
mod model;
mod ui;

use anyhow::Context;
use backend::{BackendCmd, create_backend_factory, spawn_backend_thread};

fn main() -> anyhow::Result<()> {
    // `--mock`：任意平台强制使用 Mock 后端（Windows 上免管理员预览界面）
    let force_mock = std::env::args().any(|arg| arg == "--mock");

    // 先安装自定义 hook（打印错误）；ratatui::init 的 hook 会在 panic 时
    // 先恢复终端再链入本 hook，保证终端不留残影且错误可见。
    std::panic::set_hook(Box::new(|info| {
        eprintln!("\n程序发生内部错误并已退出: {info}\n");
    }));

    let mut terminal = ratatui::init();

    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let (evt_tx, evt_rx) = std::sync::mpsc::channel();
    let (factory, kind) = create_backend_factory(force_mock);
    let _backend_thread = spawn_backend_thread(factory, cmd_rx, evt_tx);

    let mut app = app::App::new(cmd_tx, evt_rx, kind);
    app.send(BackendCmd::RefreshAll);
    let result = app.run(&mut terminal);

    ratatui::restore();
    result.context("应用运行错误")
}
