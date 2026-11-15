//! 左侧控制与监控栏：筛选器总开关 + 写入缓存 (Overlay) 监控与配置。

use eframe::egui::{
    self, Align, CornerRadius, FontId, Layout, Margin, ProgressBar, RichText, Stroke,
};

use crate::app::{UwfApp, is_overlay_locked};
use crate::model::OverlayType;
use crate::ui::{text, theme};

pub fn render(ui: &mut egui::Ui, app: &mut UwfApp) {
    render_filter_card(ui, app);
    ui.add_space(12.0);
    render_overlay_card(ui, app);
}

fn render_filter_card(ui: &mut egui::Ui, app: &mut UwfApp) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::card_header(ui, "筛选器控制", Some("系统写入保护总开关"));
        ui.add_space(8.0);

        let Some(fs) = app.snapshot.filter_state else {
            ui.label(
                RichText::new("尚未获取筛选器状态")
                    .font(FontId::proportional(12.5))
                    .color(theme::TEXT_MUTED),
            );
            return;
        };

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("当前会话")
                    .font(FontId::proportional(12.5))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if fs.current_enabled {
                    theme::status_pill(ui, "● 已启用", theme::OK_FG, theme::OK_BG);
                } else {
                    theme::status_pill(ui, "○ 已禁用", theme::TEXT_SECONDARY, theme::BG_SUBTLE);
                }
            });
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("重启后状态")
                    .font(FontId::proportional(12.5))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let pending_change = fs.current_enabled != fs.next_enabled;
                if pending_change {
                    let label = if fs.next_enabled {
                        "将启用 (待重启)"
                    } else {
                        "将禁用 (待重启)"
                    };
                    theme::status_pill(ui, label, theme::WARN_FG, theme::WARN_BG);
                } else if fs.next_enabled {
                    theme::status_pill(ui, "已启用", theme::OK_FG, theme::OK_BG);
                } else {
                    theme::status_pill(ui, "已禁用", theme::TEXT_SECONDARY, theme::BG_SUBTLE);
                }
            });
        });

        ui.add_space(10.0);
        let busy = app.is_busy();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!busy, |ui| {
                let clicked = if fs.next_enabled {
                    theme::secondary_button(ui, "停用筛选器").clicked()
                } else {
                    theme::primary_button(ui, "启用筛选器").clicked()
                };
                if clicked {
                    app.toggle_write_filter();
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::danger_button(ui, "重置配置").clicked() {
                        app.open_reset_confirm();
                    }
                });
            });
        });
    });
}

fn render_overlay_card(ui: &mut egui::Ui, app: &mut UwfApp) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        let subtitle = if app.cache.dirty {
            Some("* 有未保存改动")
        } else {
            Some("实时缓存监控与上限配额")
        };
        theme::card_header(ui, "写入缓存 (Overlay)", subtitle);
        ui.add_space(8.0);

        render_usage_section(ui, app);
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        render_overlay_config_section(ui, app);
    });
}

fn render_usage_section(ui: &mut egui::Ui, app: &UwfApp) {
    if let (Some(current), Some(runtime)) = (
        app.snapshot.overlay_config_current,
        app.snapshot.overlay_runtime,
    ) {
        let max_size = current.max_size_mb.max(1);
        let percent = runtime.usage_percent(max_size);
        let ratio = (percent as f32) / 100.0;
        let level = runtime.usage_level();
        let (fg, bar_color, status_text) = theme::usage_colors(level);
        let bg = match level {
            crate::model::UsageLevel::Normal => theme::OK_BG,
            crate::model::UsageLevel::Warning => theme::WARN_BG,
            crate::model::UsageLevel::Critical => theme::ERR_BG,
        };

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("当前缓存用量")
                    .font(FontId::proportional(12.5))
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                theme::status_pill(ui, &format!("● {status_text} {percent}%"), fg, bg);
            });
        });

        ui.add_space(4.0);
        ui.add(
            ProgressBar::new(ratio)
                .desired_width(ui.available_width())
                .desired_height(18.0)
                .fill(bar_color)
                .corner_radius(CornerRadius::same(6))
                .text(format!("已用 {} / {} MB", runtime.consumption_mb, max_size)),
        );

        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("可用剩余空间：{} MB", runtime.available_space_mb))
                .font(FontId::proportional(11.5))
                .color(theme::TEXT_MUTED),
        );
    } else {
        egui::Frame::NONE
            .fill(theme::BG_SUBTLE)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("筛选器当前未开启，启用并重启后将实时监测缓存使用率")
                        .font(FontId::proportional(12.0))
                        .color(theme::TEXT_MUTED),
                );
            });
    }
}

fn render_overlay_config_section(ui: &mut egui::Ui, app: &mut UwfApp) {
    let busy = app.is_busy();
    let locked = is_overlay_locked(&app.snapshot);

    // 缓存模式：内存(RAM) / 磁盘(Disk)（R1：当前启用时锁定）
    if let Some(cfg_next) = app.snapshot.overlay_config_next {
        let mut selected_type = cfg_next.overlay_type;
        ui.horizontal(|ui| {
            ui.add_sized(
                [78.0, 24.0],
                egui::Label::new(
                    RichText::new("缓存模式")
                        .font(FontId::proportional(12.5))
                        .color(if !locked {
                            theme::TEXT_PRIMARY
                        } else {
                            theme::TEXT_MUTED
                        }),
                ),
            );
            ui.add_enabled_ui(!busy && !locked, |ui| {
                if theme::segmented_toggle(
                    ui,
                    &mut selected_type,
                    &[
                        (OverlayType::Ram, "内存 RAM"),
                        (OverlayType::Disk, "磁盘 Disk"),
                    ],
                ) {
                    app.set_overlay_type(selected_type);
                }
            });
        });
        ui.add_space(4.0);
    }

    let input_w = (ui.available_width() - 118.0).max(80.0);
    let mut edited = false;

    edited |= theme::labeled_input_row(
        ui,
        "最大容量",
        &mut app.cache.max_size_text,
        Some("MB"),
        !busy && !locked,
        input_w,
    )
    .changed();

    edited |= theme::labeled_input_row(
        ui,
        "警告阈值",
        &mut app.cache.warning_text,
        Some("MB"),
        !busy,
        input_w,
    )
    .changed();

    edited |= theme::labeled_input_row(
        ui,
        "严重阈值",
        &mut app.cache.critical_text,
        Some("MB"),
        !busy,
        input_w,
    )
    .changed();

    if edited {
        app.cache.on_edited();
    }

    ui.add_space(4.0);
    if let Some(err) = &app.cache.error {
        ui.label(
            RichText::new(err)
                .font(FontId::proportional(12.0))
                .color(theme::ERR_FG),
        );
    } else if locked {
        ui.label(
            RichText::new(text::OVERLAY_LOCK_HINT)
                .font(FontId::proportional(11.5))
                .color(theme::TEXT_MUTED),
        );
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!busy, |ui| {
            if theme::primary_button(ui, "应用缓存设置").clicked() {
                app.submit_cache_settings();
            }
            if app.cache.dirty && theme::secondary_button(ui, "还原").clicked() {
                app.reset_cache_form();
            }
        });
    });
}
