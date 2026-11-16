//! 左侧控制与监控栏：筛选器总开关 + 写入缓存 (Overlay) 监控与配置。

use eframe::egui::{self, Align, CornerRadius, FontId, Layout, Margin, RichText, Stroke};

use crate::app::{UwfApp, is_overlay_locked};
use crate::backend::{EditionSupport, check_windows_edition_support};
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
        let busy = app.is_busy();

        theme::card_header_with_action(
            ui,
            "写入保护总开关",
            Some("开启后受保护分区的改动重启后自动还原"),
            |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    let reset_btn = ui.add(
                        egui::Button::new(
                            RichText::new("重置…")
                                .font(FontId::proportional(11.5))
                                .color(theme::TEXT_MUTED),
                        )
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(Stroke::NONE),
                    );
                    if reset_btn
                        .on_hover_text("将全部 UWF 配置恢复为默认初始值（需确认并重启）")
                        .clicked()
                    {
                        app.open_reset_confirm();
                    }
                });
            },
        );
        ui.add_space(8.0);

        let Some(fs) = app.snapshot.filter_state else {
            ui.label(
                RichText::new("尚未获取保护开关状态")
                    .font(FontId::proportional(12.5))
                    .color(theme::TEXT_MUTED),
            );
            return;
        };

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("当前保护")
                    .font(FontId::proportional(12.5))
                    .color(theme::TEXT_SECONDARY),
            );
            if fs.current_enabled {
                theme::status_pill(ui, "● 已开启", theme::OK_FG, theme::OK_BG);
            } else {
                theme::status_pill(ui, "○ 已关闭", theme::TEXT_SECONDARY, theme::BG_SUBTLE);
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    let switch_tip = if fs.next_enabled {
                        "点击切换：将在下次重启后停用写入保护"
                    } else {
                        "点击切换：将在下次重启后启用写入保护"
                    };
                    if theme::toggle_switch(ui, fs.next_enabled, "切换写入保护总开关")
                        .on_hover_text(switch_tip)
                        .on_disabled_hover_text("有操作正在执行，请稍候")
                        .clicked()
                    {
                        app.toggle_write_filter();
                    }
                });
            });
        });

        if fs.current_enabled != fs.next_enabled {
            ui.add_space(8.0);
            let pending_msg = if fs.next_enabled {
                "⏳ 已设为开启保护，将在下次重启后生效"
            } else {
                "⏳ 已设为关闭保护，将在下次重启后生效"
            };
            egui::Frame::NONE
                .fill(theme::WARN_BG)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(pending_msg)
                            .font(FontId::proportional(12.0))
                            .strong()
                            .color(theme::WARN_FG),
                    );
                });
        }
    });
}

fn render_overlay_card(ui: &mut egui::Ui, app: &mut UwfApp) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::card_header_with_action(
            ui,
            "保护缓存",
            Some("临时缓存占用与配额上限"),
            |ui| {
                if app.cache.dirty {
                    theme::status_pill(ui, "* 未保存", theme::WARN_FG, theme::WARN_BG);
                }
            },
        );
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
                RichText::new("当前用量")
                    .font(FontId::proportional(12.5))
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                theme::status_pill(ui, &format!("● {status_text} {percent}%"), fg, bg);
                ui.label(
                    RichText::new(format!("{} / {} MB", runtime.consumption_mb, max_size))
                        .font(FontId::proportional(12.0))
                        .color(theme::TEXT_SECONDARY),
                );
            });
        });

        ui.add_space(6.0);
        theme::slim_progress_bar(ui, ratio, bar_color);
    } else {
        egui::Frame::NONE
            .fill(theme::BG_SUBTLE)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("写入保护当前未开启，开启并重启后将实时监测缓存使用率")
                        .font(FontId::proportional(12.0))
                        .color(theme::TEXT_MUTED),
                );
            });
    }
}

fn render_overlay_config_section(ui: &mut egui::Ui, app: &mut UwfApp) {
    let busy = app.is_busy();
    let locked = is_overlay_locked(&app.snapshot);
    let mut edited = false;
    // 以最宽后缀「MB (100%)」动态预留单位列，保证三行输入框右边缘对齐且高 DPI 下不截断
    let unit_reserve_w = theme::measure_text_width(
        ui,
        RichText::new("MB (100%)").font(FontId::proportional(12.0)),
        egui::TextStyle::Body,
    );

    let render_locked_pair = |ui: &mut egui::Ui, app: &mut UwfApp, edited: &mut bool| {
        if let Some(cfg_next) = app.snapshot.overlay_config_next {
            let mut selected_type = cfg_next.overlay_type;
            ui.horizontal(|ui| {
                ui.add_sized(
                    [theme::FIELD_LABEL_WIDTH, 24.0],
                    egui::Label::new(
                        RichText::new("缓存位置")
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

        *edited |= theme::labeled_input_row(
            ui,
            "最大容量",
            &mut app.cache.max_size_text,
            Some("MB"),
            unit_reserve_w,
            !busy && !locked,
        )
        .changed();
    };

    if locked {
        egui::Frame::NONE
            .fill(theme::BG_SUBTLE)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(8, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(text::OVERLAY_LOCK_HINT)
                        .font(FontId::proportional(11.5))
                        .color(theme::TEXT_MUTED),
                );
                ui.add_space(6.0);
                render_locked_pair(ui, app, &mut edited);
            });
        ui.add_space(6.0);
    } else {
        render_locked_pair(ui, app, &mut edited);
        ui.add_space(2.0);
    }

    let warn_unit = format_threshold_unit(&app.cache.warning_text, app);
    let crit_unit = format_threshold_unit(&app.cache.critical_text, app);

    edited |= theme::labeled_input_row(
        ui,
        "警告阈值",
        &mut app.cache.warning_text,
        Some(&warn_unit),
        unit_reserve_w,
        !busy,
    )
    .changed();

    edited |= theme::labeled_input_row(
        ui,
        "严重阈值",
        &mut app.cache.critical_text,
        Some(&crit_unit),
        unit_reserve_w,
        !busy,
    )
    .changed();

    if edited {
        app.cache.on_edited();
    }

    if let Some(err) = &app.cache.error {
        ui.add_space(4.0);
        ui.label(
            RichText::new(err)
                .font(FontId::proportional(12.0))
                .color(theme::ERR_FG),
        );
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!busy && app.cache.dirty, |ui| {
            if theme::primary_button(ui, "应用缓存设置")
                .on_disabled_hover_text("当前无未保存改动")
                .clicked()
            {
                app.submit_cache_settings();
            }
        });
        ui.add_enabled_ui(!busy, |ui| {
            if app.cache.dirty && theme::secondary_button(ui, "还原").clicked() {
                app.reset_cache_form();
            }
        });
    });
}

fn format_threshold_unit(val_text: &str, app: &UwfApp) -> String {
    let max_mb = if is_overlay_locked(&app.snapshot) {
        app.snapshot
            .overlay_config_next
            .map(|c| c.max_size_mb)
            .filter(|&m| m > 0)
    } else {
        app.cache
            .max_size_text
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|&m| m > 0)
    };
    let val = val_text.trim().parse::<u32>().ok();
    match (val, max_mb) {
        (Some(v), Some(m)) => {
            let pct = (u64::from(v) * 100 + u64::from(m) / 2) / u64::from(m);
            format!("MB ({pct}%)")
        }
        _ => "MB".to_string(),
    }
}

/// 左下角系统版本与 UWF 版本兼容性提示面板。
pub fn render_os_card(ui: &mut egui::Ui, app: &UwfApp) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());

        let os_text = app
            .os_version
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("正在检测 Windows 版本…");
        let support = app
            .os_version
            .as_deref()
            .map(check_windows_edition_support)
            .unwrap_or(EditionSupport::Unknown);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("系统环境")
                    .font(FontId::proportional(13.5))
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| match support {
                EditionSupport::Supported => {
                    theme::status_pill(ui, "支持 UWF", theme::OK_FG, theme::OK_BG);
                }
                EditionSupport::Unsupported => {
                    theme::status_pill(ui, "需企业/教育版", theme::WARN_FG, theme::WARN_BG);
                }
                EditionSupport::Unknown => {
                    theme::status_pill(ui, "版本未知", theme::TEXT_SECONDARY, theme::BG_SUBTLE);
                }
            });
        });

        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                RichText::new(os_text)
                    .font(FontId::proportional(12.5))
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            )
            .wrap(),
        );

        ui.add_space(4.0);
        let (hint_text, hint_color) = match support {
            EditionSupport::Unsupported => (
                "提示：当前非企业版或教育版，需要 Windows 企业版或教育版才能使用 UWF 功能。",
                theme::WARN_FG,
            ),
            EditionSupport::Supported => (
                "当前版本符合要求（需要 Windows 企业版和教育版才能使用 UWF 功能）。",
                theme::TEXT_MUTED,
            ),
            EditionSupport::Unknown => (
                "提示：需要 Windows 企业版和教育版才能使用 UWF 功能。",
                theme::TEXT_MUTED,
            ),
        };
        ui.add(
            egui::Label::new(
                RichText::new(hint_text)
                    .font(FontId::proportional(11.5))
                    .color(hint_color),
            )
            .wrap(),
        );
    });
}
