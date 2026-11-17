//! 操作日志弹窗（支持按最新在前浏览与一键复制全部日志）。

use eframe::egui::{self, Align, FontId, Layout, RichText, ScrollArea};

use crate::app::UwfApp;
use crate::ui::theme;

use super::show_modal;

pub fn render(ctx: &egui::Context, app: &mut UwfApp) -> bool {
    let mut close_clicked = false;
    let (_, should_close) = show_modal(
        ctx,
        "logs_modal",
        "操作日志",
        theme::TEXT_PRIMARY,
        660.0,
        |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("操作记录（最新在前，共 {} 条）", app.logs.len()))
                        .font(FontId::proportional(12.5))
                        .color(theme::TEXT_SECONDARY),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::secondary_button(ui, "复制全部").clicked() {
                        let dumped = app
                            .logs
                            .iter()
                            .map(|e| format!("[{}] {}", e.time, e.text))
                            .collect::<Vec<_>>()
                            .join("\n");
                        ui.ctx().copy_text(dumped);
                        app.push_ok("已复制操作日志到剪贴板");
                    }
                });
            });

            ui.add_space(6.0);

            egui::Frame::NONE
                .fill(theme::BG_SUBTLE)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ScrollArea::vertical()
                        .id_salt("logs_modal_scroll")
                        .max_height(320.0)
                        .min_scrolled_height(220.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if app.logs.is_empty() {
                                ui.label(
                                    RichText::new("暂无操作记录")
                                        .font(FontId::proportional(12.5))
                                        .color(theme::TEXT_MUTED),
                                );
                                return;
                            }
                            for entry in app.logs.iter().rev() {
                                ui.horizontal(|ui| {
                                    theme::log_status_badge(ui, entry.level);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(format!(
                                                "[{}] {}",
                                                entry.time, entry.text
                                            ))
                                            .font(FontId::monospace(12.0))
                                            .color(
                                                if entry.level == crate::app::LogLevel::Err {
                                                    theme::ERR_FG
                                                } else {
                                                    theme::TEXT_PRIMARY
                                                },
                                            ),
                                        )
                                        .wrap(),
                                    );
                                });
                            }
                        });
                });

            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::primary_button(ui, "关闭").clicked() {
                    close_clicked = true;
                }
            });
        },
    );

    close_clicked || should_close
}
