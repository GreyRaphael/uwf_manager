//! UWF 管理器 —— Windows 统一写入筛选器（UWF）的桌面管理工具。
//!
//! 架构：UI 线程运行 `eframe` / `egui` 渲染与交互；后端线程以 actor 模式独占 `dyn UwfBackend`
//! （Windows 下 WMI 连接为 `!Send`），两者通过 `std::sync::mpsc` 通信，后端回包后主动触发窗口重绘。
//!
//! unsafe 政策：crate 级 deny；唯一例外是 `backend::windows::elevate` 中提权检测的 Win32 只读调用。

#![deny(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod backend;
mod error;
mod model;
mod ui;

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Context as _;
use backend::{BackendCommand, create_backend_factory, spawn_backend_thread_with_wake};
use eframe::egui;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!(
            "UWF 管理器 v{}\n用法: uwf_manager [--mock]\n  --mock  使用内置模拟数据运行界面预览（跳过系统 WMI）",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }

    let force_mock = args.iter().any(|arg| arg == "--mock");

    // Windows 真机模式下校验管理员权限；无控制台窗口时采用原生系统错误框提示
    #[cfg(windows)]
    if !force_mock && !backend::is_elevated() {
        show_fatal_dialog(
            "需要管理员权限",
            "UWF 管理操作需要管理员权限。\n\n\
             请右键程序选择“以管理员身份运行”，\n\
             或带 --mock 参数启动界面演示：uwf_manager.exe --mock",
        );
        std::process::exit(2);
    }

    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let quit_tx = cmd_tx.clone();
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let (factory, kind) = create_backend_factory(force_mock);

    let wake_ctx: Arc<OnceLock<egui::Context>> = Arc::new(OnceLock::new());
    let wake_ref = Arc::clone(&wake_ctx);
    let backend = spawn_backend_thread_with_wake(factory, cmd_rx, event_tx, move || {
        if let Some(ctx) = wake_ref.get() {
            ctx.request_repaint();
        }
    });

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(ui::text::app_title())
        .with_inner_size([1060.0, 740.0])
        .with_min_inner_size([900.0, 600.0]);
    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    let native_options = eframe::NativeOptions {
        viewport,
        centered: true,
        persist_window: false,
        ..Default::default()
    };

    let gui_result = eframe::run_native(
        "UWF 管理器",
        native_options,
        Box::new(move |cc| {
            let _ = wake_ctx.set(cc.egui_ctx.clone());
            ui::theme::apply_theme(&cc.egui_ctx);
            let mut app = app::UwfApp::new(cmd_tx, event_rx, kind);
            app.refresh_all();
            Ok(Box::new(app))
        }),
    );

    let _ = quit_tx.send(BackendCommand::Quit);
    backend.wait(Duration::from_secs(2));

    if let Err(err) = gui_result {
        let msg = format!("无法启动桌面图形界面：{err}\n\n请检查显示环境或显卡驱动状态。");
        show_fatal_dialog("界面启动失败", &msg);
        return Err(anyhow::anyhow!("{err}")).context("图形界面运行失败");
    }

    Ok(())
}

/// 从内嵌 `icon.ico` 解析窗口运行时图标（与 Windows `.rc` 共用同一图标源）。
fn load_window_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../icon.ico");
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Ico).ok()?;
    let rgba = img.into_rgba8();
    Some(egui::IconData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

fn show_fatal_dialog(title: &str, message: &str) {
    eprintln!("{title}: {message}");
    #[cfg(any(windows, target_os = "macos"))]
    {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(title)
            .set_description(message)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(title)
            .set_description(message)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
}
