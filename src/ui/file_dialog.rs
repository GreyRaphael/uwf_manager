//! 异步系统原生文件与文件夹选择器抽象。
//!
//! - 运行期默认使用 [`RfdFileDialogService`]：在后台线程调用 `rfd::FileDialog`，
//!   避免阻塞主 UI 渲染循环，并在关闭时通过 [`egui::Context::request_repaint`] 唤醒窗口；
//! - 测试环境可注入模拟实现，零弹窗完成端到端交互验证。

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use eframe::egui;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileDialogKind {
    PickFile { title: String },
    PickFolder { title: String },
    PickImportTxt { title: String },
    SaveExportTxt { title: String, default_name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileDialogTarget {
    /// 回填当前打开的 [`crate::ui::components::input_dialog::InputState`] 文本框
    InputFill,
    /// 选择导入 txt 文件后立即发起批量导入
    ImportExclusions,
    /// 选择导出目标文件后立即写出排除列表
    ExportExclusions,
}

pub trait FileDialogService: 'static {
    fn open(&mut self, kind: FileDialogKind, ctx: &egui::Context) -> Receiver<Option<PathBuf>>;
}

/// 基于 `rfd` 的原生文件/目录对话框实现。
#[derive(Default)]
pub struct RfdFileDialogService;

impl FileDialogService for RfdFileDialogService {
    fn open(&mut self, kind: FileDialogKind, ctx: &egui::Context) -> Receiver<Option<PathBuf>> {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = run_rfd_dialog(kind);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        rx
    }
}

fn run_rfd_dialog(kind: FileDialogKind) -> Option<PathBuf> {
    match kind {
        FileDialogKind::PickFile { title } => rfd::FileDialog::new().set_title(title).pick_file(),
        FileDialogKind::PickFolder { title } => {
            rfd::FileDialog::new().set_title(title).pick_folder()
        }
        FileDialogKind::PickImportTxt { title } => rfd::FileDialog::new()
            .set_title(title)
            .add_filter("文本文件 (*.txt)", &["txt"])
            .pick_file(),
        FileDialogKind::SaveExportTxt {
            title,
            default_name,
        } => rfd::FileDialog::new()
            .set_title(title)
            .add_filter("文本文件 (*.txt)", &["txt"])
            .set_file_name(default_name)
            .save_file(),
    }
}
