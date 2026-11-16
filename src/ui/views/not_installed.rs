//! UWF 未安装/未启用引导卡片。

use eframe::egui::{self, Align, CornerRadius, FontId, Layout, Margin, RichText, Stroke};

use crate::app::UwfApp;
use crate::ui::{text, theme};

pub fn render(ui: &mut egui::Ui, app: &mut UwfApp) {
    let available = ui.available_size();
    let card_w = 600.0_f32.min((available.x - 32.0).max(360.0));

    ui.vertical_centered(|ui| {
        let top_pad = ((available.y - 340.0) * 0.35).max(24.0);
        ui.add_space(top_pad);

        ui.allocate_ui(egui::vec2(card_w, 0.0), |ui| {
            theme::card_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::status_pill(ui, "未安装", theme::WARN_FG, theme::WARN_BG);
                    ui.label(
                        RichText::new(text::NI_TITLE)
                            .font(FontId::proportional(17.0))
                            .strong()
                            .color(theme::TEXT_PRIMARY),
                    );
                });

                ui.add_space(8.0);
                ui.label(
                    RichText::new(text::NI_LEAD)
                        .font(FontId::proportional(13.0))
                        .color(theme::TEXT_SECONDARY),
                );

                if let Some(os_ver) = &app.os_version {
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "当前系统：{os_ver}（提示：需要 Windows 企业版和教育版才能使用 UWF 功能）"
                            ))
                            .font(FontId::proportional(12.0))
                            .color(theme::TEXT_MUTED),
                        )
                        .wrap(),
                    );
                }

                ui.add_space(12.0);
                ui.label(
                    RichText::new(text::NI_REASON_TITLE)
                        .font(FontId::proportional(13.0))
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(4.0);
                for reason in [text::NI_REASON_1, text::NI_REASON_2] {
                    ui.label(
                        RichText::new(reason)
                            .font(FontId::proportional(12.5))
                            .color(theme::TEXT_SECONDARY),
                    );
                }

                ui.add_space(14.0);
                let cmd_label = ui.label(
                    RichText::new(text::NI_ENABLE_TITLE)
                        .font(FontId::proportional(13.0))
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(6.0);

                egui::Frame::NONE
                    .fill(theme::BG_SUBTLE)
                    .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        let copy_w = theme::measure_button_width(ui, "复制命令");
                        let spacing = ui.spacing().item_spacing.x;
                        ui.horizontal(|ui| {
                            let mut cmd_ref: &str = text::NI_ENABLE_CMD;
                            ui.add(
                                egui::TextEdit::singleline(&mut cmd_ref)
                                    .desired_width((ui.available_width() - copy_w - spacing).max(180.0))
                                    .font(FontId::monospace(12.0)),
                            )
                            .labelled_by(cmd_label.id);

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if theme::secondary_button(ui, "复制命令").clicked() {
                                    ui.ctx().copy_text(text::NI_ENABLE_CMD.to_string());
                                    app.push_ok("已将 PowerShell 启用命令复制到剪贴板");
                                }
                            });
                        });
                    });

                ui.add_space(12.0);
                ui.label(
                    RichText::new(text::NI_FOOTER)
                        .font(FontId::proportional(12.0))
                        .color(theme::TEXT_MUTED),
                );

                ui.add_space(14.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::primary_button(ui, "重新检测").clicked() {
                        app.refresh_all();
                    }
                });
            });
        });
    });
}
