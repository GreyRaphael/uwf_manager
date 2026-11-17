//! 右侧上方卡片：受保护分区表（Volumes）及每行重启后保护开关。

use eframe::egui::{self, Align, FontId, Layout, RichText};
use egui_extras::{Column, TableBuilder};

use crate::app::UwfApp;
use crate::ui::theme;

pub fn render(ui: &mut egui::Ui, app: &mut UwfApp, max_table_height: f32) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());

        theme::card_header(
            ui,
            "受保护磁盘分区",
            Some("拨动开关即可切换保护状态（下次重启后生效）"),
        );

        ui.add_space(8.0);

        if app.snapshot.volumes.is_empty() {
            ui.label(
                RichText::new("未发现可用磁盘分区")
                    .font(FontId::proportional(12.5))
                    .color(theme::TEXT_MUTED),
            );
            return;
        }

        let busy = app.is_busy();
        let mut pending_toggle: Option<(String, bool)> = None;

        ui.scope(|ui| {
            ui.style_mut().interaction.selectable_labels = false;
            ui.spacing_mut().item_spacing.y = 4.0;

            let table_h = max_table_height.max(120.0);
            TableBuilder::new(ui)
                .id_salt("volumes_table")
                .striped(true)
                .cell_layout(Layout::left_to_right(Align::Center))
                .max_scroll_height(table_h)
                .min_scrolled_height(68.0)
                .column(Column::initial(64.0).at_least(56.0))
                .column(Column::remainder().at_least(150.0))
                .column(Column::initial(84.0).at_least(76.0))
                .column(Column::initial(142.0).at_least(132.0))
                .column(Column::initial(92.0).at_least(84.0))
                .header(24.0, |mut header| {
                    for title in ["分区", "保护效果说明", "当前状态", "开关", "重启后状态"]
                    {
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

                        row.col(|ui| {
                            ui.label(
                                RichText::new(&vol.drive_letter)
                                    .font(FontId::proportional(13.0))
                                    .strong()
                                    .color(theme::TEXT_PRIMARY),
                            );
                        });

                        row.col(|ui| {
                            let note = if vol.current_protected != vol.next_protected {
                                if vol.next_protected {
                                    "重启后将开始保护（改动不保留）"
                                } else {
                                    "重启后将取消保护（改动直接保存）"
                                }
                            } else if vol.current_protected {
                                "保护中（重启后自动还原全部改动）"
                            } else {
                                "未保护（直接保存到物理硬盘）"
                            };
                            ui.label(
                                RichText::new(note)
                                    .font(FontId::proportional(12.0))
                                    .color(theme::TEXT_SECONDARY),
                            );
                        });

                        row.col(|ui| {
                            theme::state_status_pill(
                                ui,
                                vol.current_protected,
                                false,
                                "保护",
                                "不保护",
                            );
                        });

                        row.col(|ui| {
                            ui.add_enabled_ui(!busy, |ui| {
                                let a11y_label = format!("切换 {} 重启后保护", vol.drive_letter);
                                let tip = if vol.next_protected {
                                    "点击切换：将在下次重启后解除该分区保护"
                                } else {
                                    "点击切换：将在下次重启后保护该分区"
                                };
                                let switch_resp =
                                    theme::toggle_switch(ui, vol.next_protected, &a11y_label)
                                        .on_hover_text(tip)
                                        .on_disabled_hover_text("有操作正在执行，请稍候");
                                if switch_resp.clicked() {
                                    pending_toggle =
                                        Some((vol.drive_letter.clone(), !vol.next_protected));
                                }
                            });

                            if vol.current_protected != vol.next_protected {
                                theme::status_pill(
                                    ui,
                                    "待重启生效",
                                    theme::WARN_FG,
                                    theme::WARN_BG,
                                );
                            }
                        });

                        row.col(|ui| {
                            let pending = vol.current_protected != vol.next_protected;
                            theme::state_status_pill(
                                ui,
                                vol.next_protected,
                                pending,
                                "保护",
                                "不保护",
                            );
                        });
                    });
                });
        });

        if let Some((drive, protect)) = pending_toggle {
            app.set_volume_protected(&drive, protect);
        }
    });
}
