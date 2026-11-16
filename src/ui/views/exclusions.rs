//! 右侧下方卡片：排除项规则列表（文件/目录 + 注册表、行内生效状态、一键撤销修改与导入导出）。

use eframe::egui::{
    self, Align, CornerRadius, FontId, Layout, Margin, RichText, ScrollArea, Stroke,
};

use crate::app::{ExclusionItem, UwfApp};
use crate::ui::{text, theme};

pub fn render(ui: &mut egui::Ui, app: &mut UwfApp) {
    theme::card_frame().show(ui, |ui| {
        let avail_h = ui.available_height();
        ui.set_width(ui.available_width());
        ui.set_min_height(avail_h.max(200.0));

        // 第一行：标题与导入导出按钮；第二行：说明副标题
        let count = app.visible_exclusions().len();
        let busy = app.is_busy();
        let subtitle = format!("共 {count} 条 · 例外清单内的改动重启后仍会保留");
        theme::card_header_with_action(ui, "排除项规则", Some(&subtitle), |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                if theme::secondary_button(ui, "导出列表").clicked() {
                    app.open_export_dialog(ui.ctx());
                }
                if theme::secondary_button(ui, "导入列表").clicked() {
                    app.open_import_dialog(ui.ctx());
                }
            });
        });

        ui.add_space(8.0);

        // 第三行：高频添加与撤销操作工具栏（不换行）
        let revert_plan = app.exclusion_revert_plan();
        let revert_count = revert_plan.remove_count() + revert_plan.add_count();
        ui.horizontal(|ui| {
            let editable = !busy;
            let can_revert = editable && revert_count > 0;

            ui.add_enabled_ui(editable, |ui| {
                if theme::secondary_button(ui, "+ 文件").clicked() {
                    app.open_add_file_exclusion(false);
                }
                if theme::secondary_button(ui, "+ 文件夹").clicked() {
                    app.open_add_file_exclusion(true);
                }
                if theme::secondary_button(ui, "+ 注册表").clicked() {
                    app.open_add_registry_exclusion();
                }
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(can_revert, |ui| {
                    let revert_label = if revert_count > 0 {
                        format!("撤销修改 ({revert_count})")
                    } else {
                        "撤销修改".to_string()
                    };
                    let revert_btn = theme::secondary_button(ui, &revert_label);
                    let tip = if revert_plan.is_empty() {
                        "重启后排除列表已与当前生效一致，无需撤销".to_string()
                    } else {
                        format!(
                            "撤销修改并恢复为与当前生效一致（将移除 {} 项、恢复 {} 项）",
                            revert_plan.remove_count(),
                            revert_plan.add_count(),
                        )
                    };
                    if revert_btn
                        .on_hover_text(&tip)
                        .on_disabled_hover_text(&tip)
                        .clicked()
                    {
                        app.open_revert_exclusions_confirm();
                    }
                });
            });
        });

        ui.add_space(8.0);

        // 主体列表区（撑满卡片剩余空间）
        let rows = app.exclusion_rows();
        let filter_current_enabled = app
            .snapshot
            .filter_state
            .is_some_and(|fs| fs.current_enabled);

        let list_h = (ui.available_height() - 18.0).max(120.0);
        let mut removed_item: Option<ExclusionItem> = None;

        egui::Frame::NONE
            .fill(theme::BG_SUBTLE)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(list_h);

                if rows.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(((list_h - 46.0) * 0.5).max(8.0));
                            ui.label(
                                RichText::new(text::READY_EMPTY)
                                    .font(FontId::proportional(13.0))
                                    .strong()
                                    .color(theme::TEXT_SECONDARY),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("添加到此列表的文件、文件夹或注册表路径，在保护期间修改也会永久保留")
                                    .font(FontId::proportional(12.0))
                                    .color(theme::TEXT_MUTED),
                            );
                        });
                    });
                    return;
                }

                ScrollArea::vertical()
                    .id_salt("exclusions_scroll")
                    .max_height(list_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (idx, (item, in_current)) in rows.into_iter().enumerate() {
                            let full_text = item.display();
                            let (kind_lbl, kind_fg, kind_bg) = if item.is_registry() {
                                ("注册表", theme::WARN_FG, theme::WARN_BG)
                            } else {
                                ("文件/目录", theme::INFO_FG, theme::INFO_BG)
                            };

                            let row_fill = if idx % 2 == 0 {
                                theme::BG_CARD
                            } else {
                                theme::BG_SUBTLE
                            };

                            egui::Frame::NONE
                                .fill(row_fill)
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(Margin::symmetric(8, 5))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.with_layout(
                                        Layout::right_to_left(Align::Center),
                                        |ui| {
                                            ui.add_enabled_ui(!busy, |ui| {
                                                let rm_resp = ui
                                                    .add(
                                                        egui::Button::new(
                                                            RichText::new("移除")
                                                                .font(FontId::proportional(11.5))
                                                                .color(theme::ERR_FG),
                                                        )
                                                        .fill(theme::ERR_BG)
                                                        .stroke(Stroke::NONE)
                                                        .corner_radius(CornerRadius::same(5)),
                                                    )
                                                    .on_hover_text(format!(
                                                        "删除规则：{full_text}"
                                                    ));
                                                if rm_resp.clicked() {
                                                    removed_item = Some(item.clone());
                                                }
                                            });
                                            if !filter_current_enabled {
                                                theme::status_pill(
                                                    ui,
                                                    "已配置",
                                                    theme::TEXT_SECONDARY,
                                                    theme::BG_SUBTLE,
                                                );
                                            } else if in_current {
                                                theme::status_pill(
                                                    ui,
                                                    "已生效",
                                                    theme::OK_FG,
                                                    theme::OK_BG,
                                                );
                                            } else {
                                                theme::status_pill(
                                                    ui,
                                                    "待重启生效",
                                                    theme::WARN_FG,
                                                    theme::WARN_BG,
                                                );
                                            }

                                            let left_w = ui.available_width().max(80.0);
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(left_w, 20.0),
                                                Layout::left_to_right(Align::Center),
                                                |ui| {
                                                    theme::status_pill(
                                                        ui, kind_lbl, kind_fg, kind_bg,
                                                    );
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(&full_text)
                                                                .font(FontId::monospace(12.5))
                                                                .color(theme::TEXT_PRIMARY),
                                                        )
                                                        .truncate(),
                                                    )
                                                    .on_hover_text(&full_text);
                                                },
                                            );
                                        },
                                    );
                                });
                            ui.add_space(3.0);
                        }
                    });
            });

        if let Some(item) = removed_item {
            app.remove_exclusion(&item);
        }
    });
}
