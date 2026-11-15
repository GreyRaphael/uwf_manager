//! [F1] 快捷键与使用说明弹窗。

use eframe::egui::{self, Align, FontId, Layout, RichText};

use crate::ui::theme;

use super::show_modal;

pub const GITHUB_URL: &str = "https://github.com/GreyRaphael/uwf_manager";

const SECTIONS: [(&str, &[(&str, &str)]); 3] = [
    (
        "全局快捷键",
        &[
            ("F5", "刷新全部 UWF 状态（含筛选器 / 缓存 / 卷 / 排除列表）"),
            ("F1", "打开或关闭本帮助窗口"),
            ("Esc", "关闭当前弹窗"),
            ("Delete", "在无文本输入焦点时移除当前选中的排除项规则"),
        ],
    ),
    (
        "生效机制与注意事项",
        &[
            (
                "重启生效",
                "筛选器启用/停用、缓存模式/上限、分区保护及排除项增删均作用于下次启动",
            ),
            (
                "R1 锁定规则",
                "筛选器当前会话已启用时，不可改缓存模式与最大容量；预警/严重阈值随时可调",
            ),
            (
                "当前/下次视图",
                "排除列表右上可切换「重启后生效 (可编辑)」与「当前生效 (只读)」",
            ),
        ],
    ),
    (
        "固化提交与导入格式",
        &[
            (
                "提交操作",
                "支持立即提交文件修改、两步安全提交文件删除、以及提交注册表键值",
            ),
            (
                "导入规则 (.txt)",
                "UTF-8 每行一条；以 HK 开头识别为注册表排除，其余识别为文件/目录绝对路径",
            ),
        ],
    ),
];

pub fn render(ctx: &egui::Context) -> bool {
    let mut close_clicked = false;
    let (_, should_close) = show_modal(
        ctx,
        "help_modal",
        "使用说明与快捷键",
        theme::TEXT_PRIMARY,
        540.0,
        |ui| {
            for (heading, rows) in SECTIONS {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(heading)
                        .font(FontId::proportional(13.5))
                        .strong()
                        .color(theme::ACCENT),
                );
                ui.add_space(2.0);
                egui::Grid::new(format!("help_{heading}"))
                    .num_columns(2)
                    .spacing([14.0, 5.0])
                    .show(ui, |ui| {
                        for (key, desc) in rows {
                            ui.label(
                                RichText::new(*key)
                                    .font(FontId::proportional(12.5))
                                    .strong()
                                    .color(theme::TEXT_PRIMARY),
                            );
                            ui.label(
                                RichText::new(*desc)
                                    .font(FontId::proportional(12.5))
                                    .color(theme::TEXT_SECONDARY),
                            );
                            ui.end_row();
                        }
                    });
                ui.add_space(4.0);
            }

            ui.add_space(4.0);
            ui.label(
                RichText::new("开源项目与反馈")
                    .font(FontId::proportional(13.5))
                    .strong()
                    .color(theme::ACCENT),
            );
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("GitHub 地址")
                        .font(FontId::proportional(12.5))
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                ui.hyperlink_to(GITHUB_URL, GITHUB_URL);
            });

            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::primary_button(ui, "知道了").clicked() {
                    close_clicked = true;
                }
            });
        },
    );
    close_clicked || should_close
}
