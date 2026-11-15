//! 模态弹窗组件统一入口与公共模态框架辅助。

pub mod commit_menu;
pub mod confirm;
pub mod help;
pub mod input_dialog;

use eframe::egui::{self, Color32, FontId, Id, Modal, RichText, Ui};

use crate::ui::theme;

/// 统一渲染居中模态窗口，处理遮罩、宽度约束与 `Esc`/外部点击关闭。
pub fn show_modal<T>(
    ctx: &egui::Context,
    id_salt: &str,
    title: &str,
    title_color: Color32,
    width: f32,
    add_contents: impl FnOnce(&mut Ui) -> T,
) -> (T, bool) {
    let resp = Modal::new(Id::unique(id_salt))
        .frame(theme::modal_frame())
        .show(ctx, |ui| {
            ui.set_width(width.min(ctx.content_rect().width() - 32.0));
            ui.label(
                RichText::new(title)
                    .font(FontId::proportional(16.0))
                    .strong()
                    .color(title_color),
            );
            ui.add_space(6.0);
            add_contents(ui)
        });
    let should_close = resp.should_close();
    (resp.inner, should_close)
}
