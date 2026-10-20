//! UWF Manager —— Windows 统一写入筛选器（UWF）的 TUI 管理工具。
//!
//! 架构（PLAN v2 D2/D4）：UI 线程负责渲染与按键，后端线程以 actor 模式独占
//! `dyn UwfBackend`（WMI 连接为 `!Send`），两者经 `std::sync::mpsc` 通信，
//! 全程序无异步运行时。
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
use backend::{BackendEvent, create_backend_factory, spawn_backend_thread};
use crossterm::event;

fn main() -> anyhow::Result<()> {
    // `--mock`：任意平台强制使用 Mock 后端。
    // 注意：Windows 下 manifest 仍是 requireAdministrator，--mock 只跳过运行时检测。
    let force_mock = std::env::args().any(|arg| arg == "--mock");

    // D5 兜底：Windows 真机模式（非 --mock）下检测管理员权限。
    // 正常发布版由 app.manifest 自动提权；此处仅兜住 manifest 缺失的 dev 场景。
    #[cfg(windows)]
    if !force_mock && !backend::is_elevated() {
        eprintln!("错误：UWF 管理操作需要管理员权限。");
        eprintln!("请右键“以管理员身份运行”终端（Windows Terminal / cmd / PowerShell）后重试，");
        eprintln!("或以 Mock 模式预览界面：uwf_manager --mock");
        std::process::exit(2);
    }

    // 先安装自定义 hook（打印错误）；ratatui::init 的 hook 会在 panic 时
    // 先恢复终端再链入本 hook，保证终端不留残影且错误可见。
    std::panic::set_hook(Box::new(|info| {
        eprintln!("\n程序发生内部错误并已退出: {info}\n");
    }));

    let mut terminal = ratatui::init();

    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let (backend_evt_tx, backend_evt_rx) = std::sync::mpsc::channel::<BackendEvent>();
    let (ui_tx, ui_rx) = std::sync::mpsc::channel();
    let (factory, kind) = create_backend_factory(force_mock);
    let backend = spawn_backend_thread(factory, cmd_rx, backend_evt_tx);

    // 后端回包转发到 UI 统一 channel；终端读取线程同样汇入，二者都能立即唤醒主循环。
    let forward_tx = ui_tx.clone();
    std::thread::spawn(move || {
        while let Ok(evt) = backend_evt_rx.recv() {
            if forward_tx.send(app::UiEvent::Backend(evt)).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        while let Ok(evt) = event::read() {
            if ui_tx.send(app::UiEvent::Terminal(evt)).is_err() {
                break;
            }
        }
    });

    let mut app = app::App::new(cmd_tx, ui_rx, kind);
    app.start_initial_load();
    let result = app.run(&mut terminal);

    // 无论 run 正常/异常返回，都通知后端退出；最多等 2 秒，避免 WMI 挂死拖住进程。
    app.send_quit();
    ratatui::restore();
    backend.wait(Duration::from_secs(2));
    result.context("应用运行错误")
}
