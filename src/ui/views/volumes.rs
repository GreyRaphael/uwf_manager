//! 右侧上方卡片：受保护分区表（Volumes）及保护切换操作。

use eframe::egui::{self, Align, FontId, Layout, RichText};
use egui_extras::{Column, TableBuilder};

use crate::app::UwfApp;
use crate::ui::{text, theme};

pub fn render(ui: &mut egui::Ui, app: &mut UwfApp, max_table_height: f32) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());

        ui.horizontal(|ui| {
            theme::card_header(
                ui,
                "受保护分区 (Volumes)",
                Some("保护设定将在下次重启后生效"),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let busy = app.is_busy();
                let selected = app.selected_volume().cloned();
                let has_sel = selected.is_some();

                ui.add_enabled_ui(!busy && has_sel, |ui| {
                    if theme::secondary_button(ui, "取消保护").clicked() {
                        app.protect_selected(false);
                    }
                    if theme::primary_button(ui, "保护选中分区").clicked() {
                        app.protect_selected(true);
                    }
                });
            });
        });

        ui.add_space(8.0);

        if app.snapshot.volumes.is_empty() {
            ui.label(
                RichText::new("未发现可用磁盘分区")
                    .font(FontId::proportional(12.5))
                    .color(theme::TEXT_MUTED),
            );
            return;
        }

        // 在表格作用域内使用柔和浅蓝行高亮，保证状态标签与文字清晰可读
        let mut clicked_drive: Option<String> = None;
        ui.scope(|ui| {
            // 禁止整行内嵌文本截获点击用于划词，确保点击行内任意文字都能触发整行选中
            ui.style_mut().interaction.selectable_labels = false;
            ui.visuals_mut().selection.bg_fill = theme::BG_SELECTED;
            ui.visuals_mut().selection.stroke = egui::Stroke::new(1.0, theme::ACCENT);

            let table_h = max_table_height.max(120.0);
            TableBuilder::new(ui)
                .id_salt("volumes_table")
                .striped(true)
                .sense(egui::Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .max_scroll_height(table_h)
                .min_scrolled_height(100.0)
                .column(Column::initial(95.0).at_least(75.0))
                .column(Column::remainder().at_least(120.0))
                .column(Column::initial(110.0).at_least(95.0))
                .column(Column::initial(120.0).at_least(100.0))
                .header(26.0, |mut header| {
                    for title in ["分区", "状态说明", "当前会话", "重启后状态"] {
                        header.col(|ui| {
                            ui.label(
                                RichText::new(title)
                                    .font(FontId::proportional(12.0))
                                    .strong()
                                    .color(theme::TEXT_SECONDARY),
                            );
                        });
                    }
                })
                .body(|body| {
                    let row_count = app.snapshot.volumes.len();
                    body.rows(30.0, row_count, |mut row| {
                        let vol = &app.snapshot.volumes[row.index()];
                        let is_selected = app
                            .selected_volume_drive
                            .as_ref()
                            .is_some_and(|d| d.eq_ignore_ascii_case(&vol.drive_letter));
                        row.set_selected(is_selected);

                        row.col(|ui| {
                            let prefix = if is_selected { "▸ " } else { "   " };
                            ui.label(
                                RichText::new(format!("{prefix}{}", vol.drive_letter))
                                    .font(FontId::proportional(13.0))
                                    .strong()
                                    .color(if is_selected {
                                        theme::ACCENT
                                    } else {
                                        theme::TEXT_PRIMARY
                                    }),
                            );
                        });

                        row.col(|ui| {
                            let note = if vol.current_protected != vol.next_protected {
                                if vol.next_protected {
                                    "已加入待保护队列（重启后启用）"
                                } else {
                                    "已计划取消保护（重启后解除）"
                                }
                            } else if vol.current_protected {
                                "正在拦截该分区全部写入"
                            } else {
                                "直接写入物理磁盘"
                            };
                            ui.label(
                                RichText::new(note)
                                    .font(FontId::proportional(12.0))
                                    .color(theme::TEXT_SECONDARY),
                            );
                        });

                        row.col(|ui| {
                            if vol.current_protected {
                                theme::status_pill(
                                    ui,
                                    text::protected_zh(true),
                                    theme::OK_FG,
                                    theme::OK_BG,
                                );
                            } else {
                                theme::status_pill(
                                    ui,
                                    text::protected_zh(false),
                                    theme::TEXT_SECONDARY,
                                    theme::BG_SUBTLE,
                                );
                            }
                        });

                        row.col(|ui| {
                            let pending = vol.current_protected != vol.next_protected;
                            if pending {
                                let label = if vol.next_protected {
                                    "将保护 (待重启)"
                                } else {
                                    "将取消 (待重启)"
                                };
                                theme::status_pill(ui, label, theme::WARN_FG, theme::WARN_BG);
                            } else if vol.next_protected {
                                theme::status_pill(ui, "已保护", theme::OK_FG, theme::OK_BG);
                            } else {
                                theme::status_pill(
                                    ui,
                                    "未保护",
                                    theme::TEXT_SECONDARY,
                                    theme::BG_SUBTLE,
                                );
                            }
                        });

                        if row.response().clicked() {
                            clicked_drive = Some(vol.drive_letter.clone());
                        }
                    });
                });
        });

        if let Some(drive) = clicked_drive {
            app.select_volume(drive);
        }
    });
}
