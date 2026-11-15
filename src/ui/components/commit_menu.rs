//! 提交固化菜单弹窗（提交文件修改 / 删除并固化文件 / 提交注册表键值）。

use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, FontId, Layout, Margin, RichText, Stroke,
};

use crate::ui::theme;

use super::show_modal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitChoice {
    File,
    FileDeletion,
    Registry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitMenuOutcome {
    Stay,
    Cancel,
    Select(CommitChoice),
}

const ITEMS: [(CommitChoice, &str, &str, &str, bool); 3] = [
    (
        CommitChoice::File,
        "提交文件修改",
        "将当前受保护卷中某文件的改动立即写回物理磁盘",
        "选择文件…",
        false,
    ),
    (
        CommitChoice::FileDeletion,
        "删除并固化文件",
        "立即删除目标文件并固化该删除，重启后不再恢复（需二次确认）",
        "选择待删文件…",
        true,
    ),
    (
        CommitChoice::Registry,
        "提交注册表键值",
        "将受保护注册表项下的指定键值立即写入物理配置（格式：键路径\\值名）",
        "输入键值…",
        false,
    ),
];

pub fn render(ctx: &egui::Context) -> CommitMenuOutcome {
    let mut outcome = CommitMenuOutcome::Stay;
    let (_, should_close) = show_modal(
        ctx,
        "commit_menu_modal",
        "提交固化操作（立即写入磁盘/注册表）",
        theme::TEXT_PRIMARY,
        460.0,
        |ui| {
            ui.label(
                RichText::new("以下操作直接作用于当前会话受保护对象，请选择要执行的固化类型：")
                    .font(FontId::proportional(12.5))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add_space(8.0);

            for (choice, title, desc, btn_label, danger) in ITEMS {
                let fill = if danger {
                    Color32::from_rgb(255, 250, 250)
                } else {
                    theme::BG_SUBTLE
                };
                let stroke = if danger {
                    Stroke::new(1.0, Color32::from_rgb(254, 202, 202))
                } else {
                    Stroke::new(1.0, theme::BORDER_STRONG)
                };
                let title_color = if danger { theme::ERR_FG } else { theme::ACCENT };

                egui::Frame::NONE
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.set_width((ui.available_width() - 118.0).max(180.0));
                                ui.label(
                                    RichText::new(title)
                                        .font(FontId::proportional(13.5))
                                        .strong()
                                        .color(title_color),
                                );
                                ui.label(
                                    RichText::new(desc)
                                        .font(FontId::proportional(12.0))
                                        .color(theme::TEXT_SECONDARY),
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let btn = if danger {
                                    ui.add(
                                        Button::new(RichText::new(btn_label).color(theme::ERR_FG))
                                            .fill(theme::ERR_BG)
                                            .stroke(Stroke::new(
                                                1.0,
                                                Color32::from_rgb(252, 165, 165),
                                            ))
                                            .corner_radius(CornerRadius::same(6)),
                                    )
                                } else {
                                    theme::secondary_button(ui, btn_label)
                                };
                                if btn.clicked() {
                                    outcome = CommitMenuOutcome::Select(choice);
                                }
                            });
                        });
                    });
                ui.add_space(6.0);
            }

            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::secondary_button(ui, "取消").clicked() {
                    outcome = CommitMenuOutcome::Cancel;
                }
            });
        },
    );

    if matches!(outcome, CommitMenuOutcome::Stay) && should_close {
        CommitMenuOutcome::Cancel
    } else {
        outcome
    }
}
