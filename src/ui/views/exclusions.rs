//! 右侧下方卡片：排除项规则列表（文件/目录 + 注册表、行内生效状态、一键撤销修改与导入导出）。

use eframe::egui::{
    self, Align, CornerRadius, FontId, Layout, Margin, RichText, ScrollArea, Sense, Stroke,
};

use crate::app::{ExclusionItem, UwfApp};
use crate::ui::{text, theme};

pub fn render(ui: &mut egui::Ui, app: &mut UwfApp) {
    theme::card_frame().show(ui, |ui| {
        let avail_h = ui.available_height();
        ui.set_width(ui.available_width());
        ui.set_min_height(avail_h.max(200.0));

        // 第一行：标题与生效机制说明（仅展示重启后配置视图，行内通过标签区分是否已生效）
        let count = app.visible_exclusions().len();
        theme::card_header(
            ui,
            "排除项规则",
            Some(&format!("共 {count} 条 · 修改在下次重启后生效")),
        );

        ui.add_space(8.0);

        // 第二行：单行分组操作工具栏（不换行，不提供类型过滤）
        let revert_plan = app.exclusion_revert_plan();
        ui.horizontal(|ui| {
            let busy = app.is_busy();
            let editable = !busy;
            let has_selected = app.selected_exclusion().is_some();
            let can_revert = editable && !revert_plan.is_empty();

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
                ui.add_enabled_ui(has_selected, |ui| {
                    if theme::danger_button(ui, "移除选中").clicked() {
                        app.delete_selected_exclusion();
                    }
                });
                ui.add_enabled_ui(can_revert, |ui| {
                    let revert_btn = theme::secondary_button(ui, "撤销修改");
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

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if theme::secondary_button(ui, "导出列表").clicked() {
                        app.open_export_dialog(ui.ctx());
                    }
                    if theme::secondary_button(ui, "导入列表").clicked() {
                        app.open_import_dialog(ui.ctx());
                    }
                });
            });
        });

        ui.add_space(8.0);

        // 主体列表区（撑满卡片剩余空间）
        let rows = app.exclusion_rows();
        let selected_key = app.selected_exclusion_key.clone();
        let filter_current_enabled = app
            .snapshot
            .filter_state
            .is_some_and(|fs| fs.current_enabled);

        let list_h = (ui.available_height() - 18.0).max(120.0);
        let mut clicked_item: Option<ExclusionItem> = None;

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
                        ui.label(
                            RichText::new(text::READY_EMPTY)
                                .font(FontId::proportional(13.0))
                                .color(theme::TEXT_MUTED),
                        );
                    });
                    return;
                }

                ScrollArea::vertical()
                    .id_salt("exclusions_scroll")
                    .max_height(list_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // 允许点击行内文本区域直接选中整行（完整路径仍可通过悬停提示或导出查看）
                        ui.style_mut().interaction.selectable_labels = false;
                        for (idx, (item, in_current)) in rows.into_iter().enumerate() {
                            let is_selected = selected_key.as_ref() == Some(&item);
                            let full_text = item.display();
                            let (kind_lbl, kind_fg, kind_bg) = if item.is_registry() {
                                ("注册表", theme::WARN_FG, theme::WARN_BG)
                            } else {
                                ("文件/目录", theme::INFO_FG, theme::INFO_BG)
                            };

                            let row_fill = if is_selected {
                                theme::BG_SELECTED
                            } else if idx % 2 == 0 {
                                theme::BG_CARD
                            } else {
                                theme::BG_SUBTLE
                            };
                            let row_stroke = if is_selected {
                                Stroke::new(1.0, theme::ACCENT)
                            } else {
                                Stroke::NONE
                            };

                            let row_resp = egui::Frame::NONE
                                .fill(row_fill)
                                .stroke(row_stroke)
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(Margin::symmetric(8, 5))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        theme::status_pill(ui, kind_lbl, kind_fg, kind_bg);

                                        let right_badge_w = 104.0;
                                        let text_w =
                                            (ui.available_width() - right_badge_w).max(80.0);
                                        ui.add_sized(
                                            [text_w, 20.0],
                                            egui::Label::new(
                                                RichText::new(&full_text)
                                                    .font(FontId::monospace(12.5))
                                                    .color(if is_selected {
                                                        theme::ACCENT
                                                    } else {
                                                        theme::TEXT_PRIMARY
                                                    }),
                                            )
                                            .truncate(),
                                        )
                                        .on_hover_text(&full_text);

                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
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
                                            },
                                        );
                                    });
                                })
                                .response
                                .interact(Sense::click());

                            if row_resp.clicked() {
                                clicked_item = Some(item);
                            }
                            ui.add_space(3.0);
                        }
                    });
            });

        if let Some(item) = clicked_item {
            app.select_exclusion(item);
        }
    });
}
