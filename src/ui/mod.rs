//! 现代桌面 GUI 顶层调度：顶部全局命令栏（含日志弹窗与帮助入口）、双区工作台、底部状态栏、模态遮罩层。

pub mod components;
pub mod file_dialog;
pub mod text;
pub mod theme;
pub mod views;

use eframe::egui::{
    self, Align, FontId, Key, Layout, Margin, Modifiers, Panel, RichText, ScrollArea, Stroke,
};

use crate::app::{Phase, UwfApp};
use crate::ui::components::commit_menu::{self, CommitMenuOutcome};
use crate::ui::components::confirm::{self, ConfirmOutcome};
use crate::ui::components::help;
use crate::ui::components::input_dialog::{self, InputOutcome};
use crate::ui::components::logs;

/// 绘制整个应用窗口（供 [`eframe::App::ui`] 调用）。
pub fn render(ui: &mut egui::Ui, app: &mut UwfApp) {
    // Panel 声明顺序决定边框归属：
    // 1. bottom 独占最底部；
    // 2. Ready 状态下 left 独占左侧全高（使「筛选器控制」贴顶并把 top bar 左边界右移到右侧工作区上方）；
    // 3. top 占剩余宽度的顶部；4. CentralPanel 填满右下方工作区。
    render_footer_bar(ui, app);
    if app.phase == Phase::Ready {
        render_left_rail(ui, app);
    }
    render_top_bar(ui, app);
    render_central_workspace(ui, app);
    render_modals(ui.ctx(), app);
    handle_shortcuts(ui.ctx(), app);
}

fn render_top_bar(ui: &mut egui::Ui, app: &mut UwfApp) {
    Panel::top("uwf_top_bar")
        .show_separator_line(false)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_CANVAS)
                .inner_margin(Margin {
                    left: 14,
                    right: 14,
                    top: 14,
                    bottom: 0,
                }),
        )
        .show(ui, |ui| {
            theme::card_frame()
                .inner_margin(Margin::symmetric(14, 9))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        if app.is_busy() {
                            ui.spinner();
                        }

                        if app.phase == Phase::Ready {
                            let reasons = app.pending_reboot_reasons();
                            if reasons.is_empty() {
                                ui.label(
                                    RichText::new("✔ 当前设置已全部生效")
                                        .font(FontId::proportional(12.0))
                                        .color(theme::TEXT_MUTED),
                                );
                            } else {
                                let detail = format!(
                                    "以下更改将在下次重启计算机后生效：{}",
                                    reasons.join("、")
                                );
                                theme::status_pill(
                                    ui,
                                    "⚠ 待重启生效",
                                    theme::WARN_FG,
                                    theme::WARN_BG,
                                );
                                ui.add_enabled_ui(!app.is_busy(), |ui| {
                                    if theme::secondary_button(ui, "立即重启生效…")
                                        .on_hover_text(&detail)
                                        .clicked()
                                    {
                                        app.open_restart_confirm();
                                    }
                                });
                            }
                        }

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if theme::secondary_button(ui, "帮助 (F1)").clicked() {
                                app.open_help();
                            }
                            let log_lbl = format!("日志 ({})", app.logs.len());
                            if theme::secondary_button(ui, &log_lbl).clicked() {
                                app.open_logs();
                            }

                            let ready = app.phase == Phase::Ready && !app.is_busy();
                            ui.add_enabled_ui(ready, |ui| {
                                ui.menu_button("电源 ▾", |ui| {
                                    if ui.button("重启计算机…").clicked() {
                                        ui.close();
                                        app.open_restart_confirm();
                                    }
                                    if ui.button("关闭计算机…").clicked() {
                                        ui.close();
                                        app.open_shutdown_confirm();
                                    }
                                });

                                if theme::secondary_button(ui, "永久保存…").clicked() {
                                    app.open_commit_menu();
                                }
                            });
                        });
                    });
                });
        });
}

fn render_left_rail(ui: &mut egui::Ui, app: &mut UwfApp) {
    let left_w = ((ui.available_width() - 28.0) * 0.30).clamp(285.0, 330.0);
    Panel::left("uwf_left_rail")
        .resizable(false)
        .show_separator_line(false)
        .exact_size(left_w + 14.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_CANVAS)
                .inner_margin(Margin {
                    left: 14,
                    right: 0,
                    top: 14,
                    bottom: 14,
                }),
        )
        .show(ui, |ui| {
            let total_h = ui.available_height();
            let rail_w = ui.available_width();
            let os_h_id = ui.make_persistent_id("left_rail_os_card_h");
            let prev_os_h: f32 = ui.data(|d| d.get_temp::<f32>(os_h_id)).unwrap_or(122.0);
            let scroll_h = (total_h - prev_os_h - 10.0).max(120.0);

            ui.allocate_ui(egui::vec2(rail_w, scroll_h), |ui| {
                ScrollArea::vertical()
                    .id_salt("left_rail_scroll")
                    .max_height(scroll_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        views::settings::render(ui, app);
                    });
            });

            ui.add_space(10.0);
            let os_resp = views::settings::render_os_card(ui, app);
            let measured_h = os_resp.rect.height();
            if measured_h > 20.0 && (measured_h - prev_os_h).abs() > 0.5 {
                ui.data_mut(|d| d.insert_temp(os_h_id, measured_h));
                ui.ctx().request_repaint();
            }
        });
}

fn render_footer_bar(ui: &mut egui::Ui, app: &mut UwfApp) {
    Panel::bottom("uwf_bottom_bar")
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_CARD)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .inner_margin(Margin::symmetric(14, 7)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(entry) = app.last_log().cloned() {
                    let (icon, fg, bg) = theme::log_badge(entry.level);
                    theme::status_pill(ui, icon, fg, bg);
                    ui.label(
                        RichText::new(format!("[{}] {}", entry.time, entry.text))
                            .font(FontId::proportional(12.0))
                            .color(if entry.level == crate::app::LogLevel::Err {
                                theme::ERR_FG
                            } else {
                                theme::TEXT_PRIMARY
                            }),
                    );
                } else {
                    ui.label(
                        RichText::new("就绪")
                            .font(FontId::proportional(12.0))
                            .color(theme::TEXT_MUTED),
                    );
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(text::NOTICE_SHORT)
                            .font(FontId::proportional(11.5))
                            .color(theme::WARN_FG),
                    );
                });
            });
        });
}

fn render_central_workspace(ui: &mut egui::Ui, app: &mut UwfApp) {
    let margin = if app.phase == Phase::Ready {
        Margin {
            left: 14,
            right: 14,
            top: 12,
            bottom: 14,
        }
    } else {
        Margin::same(14)
    };
    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_CANVAS)
                .inner_margin(margin),
        )
        .show(ui, |ui| match app.phase {
            Phase::NotInstalled => views::not_installed::render(ui, app),
            Phase::Loading | Phase::LoadFailed | Phase::BackendLost => {
                render_status_card(ui, app);
            }
            Phase::Ready => render_ready_dashboard(ui, app),
        });
}

fn render_status_card(ui: &mut egui::Ui, app: &mut UwfApp) {
    let available = ui.available_size();
    let card_w = 460.0_f32.min((available.x - 24.0).max(280.0));
    ui.vertical_centered(|ui| {
        ui.add_space(((available.y - 160.0) * 0.4).max(20.0));
        ui.allocate_ui(egui::vec2(card_w, 0.0), |ui| {
            theme::card_frame().show(ui, |ui| {
                ui.vertical_centered(|ui| match app.phase {
                    Phase::Loading => {
                        ui.spinner();
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(text::LOADING)
                                .font(FontId::proportional(14.0))
                                .color(theme::TEXT_PRIMARY),
                        );
                    }
                    Phase::LoadFailed => {
                        theme::status_pill(ui, "加载失败", theme::ERR_FG, theme::ERR_BG);
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(text::LOAD_FAILED)
                                .font(FontId::proportional(13.0))
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(10.0);
                        if theme::primary_button(ui, "重新加载").clicked() {
                            app.refresh_all();
                        }
                    }
                    Phase::BackendLost => {
                        theme::status_pill(ui, "服务异常", theme::ERR_FG, theme::ERR_BG);
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(text::BACKEND_LOST)
                                .font(FontId::proportional(13.0))
                                .color(theme::ERR_FG),
                        );
                    }
                    _ => {}
                });
            });
        });
    });
}

fn render_ready_dashboard(ui: &mut egui::Ui, app: &mut UwfApp) {
    let total_h = ui.available_height();
    let vol_max_h = (total_h * 0.34).clamp(130.0, 220.0);
    views::volumes::render(ui, app, vol_max_h);
    ui.add_space(12.0);
    views::exclusions::render(ui, app);
}

fn render_modals(ctx: &egui::Context, app: &mut UwfApp) {
    let Some(modal) = app.modal.take() else {
        return;
    };
    match modal {
        crate::app::Modal::Help => {
            if !help::render(ctx) {
                app.modal = Some(crate::app::Modal::Help);
            }
        }
        crate::app::Modal::Logs => {
            if !logs::render(ctx, app) {
                app.modal = Some(crate::app::Modal::Logs);
            }
        }
        crate::app::Modal::CommitMenu => match commit_menu::render(ctx) {
            CommitMenuOutcome::Stay => app.modal = Some(crate::app::Modal::CommitMenu),
            CommitMenuOutcome::Cancel => {}
            CommitMenuOutcome::Select(choice) => app.execute_commit_choice(choice),
        },
        crate::app::Modal::Confirm(state) => match confirm::render(ctx, &state) {
            ConfirmOutcome::Stay => app.modal = Some(crate::app::Modal::Confirm(state)),
            ConfirmOutcome::Cancel => {}
            ConfirmOutcome::Confirm(action) => app.execute_confirm(action),
        },
        crate::app::Modal::Input(mut state) => {
            let browse_busy = app.is_file_dialog_open();
            match input_dialog::render(ctx, &mut state, browse_busy) {
                InputOutcome::Stay => app.modal = Some(crate::app::Modal::Input(state)),
                InputOutcome::Cancel => {}
                InputOutcome::Browse(kind) => {
                    app.modal = Some(crate::app::Modal::Input(state));
                    app.open_input_browse(kind, ctx);
                }
                InputOutcome::Submit(action, text) => {
                    if let Err(err) = app.validate_input_preflight(action, &text) {
                        state.error = Some(err);
                        app.modal = Some(crate::app::Modal::Input(state));
                    } else {
                        app.execute_input(action, text);
                    }
                }
            }
        }
    }
}

fn handle_shortcuts(ctx: &egui::Context, app: &mut UwfApp) {
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F1)) {
        if matches!(app.modal, Some(crate::app::Modal::Help)) {
            app.modal = None;
        } else {
            app.open_help();
        }
        return;
    }

    if app.modal.is_some() {
        return;
    }

    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F5)) && !app.is_pending() {
        app.refresh_all();
        return;
    }

    // 仅当没有任何文本输入控件正在编辑时才响应 Delete 快捷移除排除项
    let text_editing = ctx.egui_wants_keyboard_input();
    if !text_editing
        && app.phase == Phase::Ready
        && !app.is_busy()
        && app.selected_exclusion().is_some()
        && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete))
    {
        app.delete_selected_exclusion();
    }
}
