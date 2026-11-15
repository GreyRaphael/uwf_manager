//! 现代桌面 GUI 顶层调度：顶部全局命令栏、双区工作台、底部反馈与可展开日志抽屉、模态遮罩层。

pub mod components;
pub mod file_dialog;
pub mod text;
pub mod theme;
pub mod views;

use eframe::egui::{
    self, Align, FontId, Key, Layout, Margin, Modifiers, Panel, RichText, ScrollArea, Stroke,
};

use crate::app::{Phase, UwfApp};
use crate::backend::BackendKind;
use crate::ui::components::commit_menu::{self, CommitMenuOutcome};
use crate::ui::components::confirm::{self, ConfirmOutcome};
use crate::ui::components::help;
use crate::ui::components::input_dialog::{self, InputOutcome};

/// 绘制整个应用窗口（供 [`eframe::App::ui`] 调用）。
pub fn render(ui: &mut egui::Ui, app: &mut UwfApp) {
    render_top_bar(ui, app);
    render_footer_bar(ui, app);
    if app.show_log_drawer {
        render_log_drawer(ui, app);
    }
    render_central_workspace(ui, app);
    render_modals(ui.ctx(), app);
    handle_shortcuts(ui.ctx(), app);
}

fn render_top_bar(ui: &mut egui::Ui, app: &mut UwfApp) {
    Panel::top("uwf_top_bar")
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_CARD)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .inner_margin(Margin::symmetric(16, 10)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(text::app_title())
                        .font(FontId::proportional(16.0))
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );

                let (b_fg, b_bg) = match app.backend_kind {
                    BackendKind::Windows => (theme::OK_FG, theme::OK_BG),
                    BackendKind::Mock => (theme::WARN_FG, theme::WARN_BG),
                };
                theme::status_pill(ui, text::backend_badge(app.backend_kind), b_fg, b_bg);

                if app.phase == Phase::Ready
                    && let Some(fs) = app.snapshot.filter_state
                {
                    ui.add_space(6.0);
                    let cur_txt = format!("当前: {}", text::enabled_zh(fs.current_enabled));
                    if fs.current_enabled {
                        theme::status_pill(ui, &cur_txt, theme::OK_FG, theme::OK_BG);
                    } else {
                        theme::status_pill(ui, &cur_txt, theme::TEXT_SECONDARY, theme::BG_SUBTLE);
                    }

                    if fs.current_enabled != fs.next_enabled {
                        let next_txt = format!("重启后: {}", text::enabled_zh(fs.next_enabled));
                        theme::status_pill(ui, &next_txt, theme::WARN_FG, theme::WARN_BG);
                    }
                }

                if app.is_busy() {
                    ui.add_space(6.0);
                    ui.spinner();
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::secondary_button(ui, "帮助 (F1)").clicked() {
                        app.open_help();
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

                        if theme::secondary_button(ui, "提交操作…").clicked() {
                            app.open_commit_menu();
                        }
                    });

                    let can_refresh = !app.is_pending();
                    ui.add_enabled_ui(can_refresh, |ui| {
                        if theme::secondary_button(ui, "刷新 (F5)").clicked() {
                            app.refresh_all();
                        }
                    });
                });
            });
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
                    let log_lbl = if app.show_log_drawer {
                        format!("收起日志 ({}) ▼", app.logs.len())
                    } else {
                        format!("操作日志 ({}) ▲", app.logs.len())
                    };
                    if theme::secondary_button(ui, &log_lbl).clicked() {
                        app.show_log_drawer = !app.show_log_drawer;
                    }

                    ui.label(
                        RichText::new(text::NOTICE_SHORT)
                            .font(FontId::proportional(11.5))
                            .color(theme::WARN_FG),
                    );
                });
            });
        });
}

fn render_log_drawer(ui: &mut egui::Ui, app: &mut UwfApp) {
    Panel::bottom("uwf_log_drawer")
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_SUBTLE)
                .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
                .inner_margin(Margin::symmetric(16, 8)),
        )
        .resizable(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("操作记录（最新在前）")
                        .font(FontId::proportional(12.5))
                        .strong()
                        .color(theme::TEXT_PRIMARY),
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
            ui.add_space(4.0);
            ScrollArea::vertical()
                .id_salt("log_drawer_scroll")
                .max_height(115.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if app.logs.is_empty() {
                        ui.label(
                            RichText::new("暂无操作记录")
                                .font(FontId::proportional(12.0))
                                .color(theme::TEXT_MUTED),
                        );
                        return;
                    }
                    for entry in app.logs.iter().rev() {
                        let (icon, fg, bg) = theme::log_badge(entry.level);
                        ui.horizontal(|ui| {
                            theme::status_pill(ui, icon, fg, bg);
                            ui.label(
                                RichText::new(format!("[{}] {}", entry.time, entry.text))
                                    .font(FontId::monospace(11.5))
                                    .color(theme::TEXT_PRIMARY),
                            );
                        });
                    }
                });
        });
}

fn render_central_workspace(ui: &mut egui::Ui, app: &mut UwfApp) {
    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_CANVAS)
                .inner_margin(Margin::same(14)),
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
    let total_w = ui.available_width();
    let total_h = ui.available_height();
    let left_w = (total_w * 0.31).clamp(275.0, 325.0);
    let right_w = (total_w - left_w - 14.0).max(320.0);

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;

        // 左侧控制与缓存监控边栏（可滚动防极小窗口裁剪）
        ui.allocate_ui_with_layout(
            egui::vec2(left_w, total_h),
            Layout::top_down(Align::Min),
            |ui| {
                ScrollArea::vertical()
                    .id_salt("left_rail_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        views::settings::render(ui, app);
                    });
            },
        );

        // 右侧核心工作区（上：受保护分区表，下：排除项规则卡片）
        ui.allocate_ui_with_layout(
            egui::vec2(right_w, total_h),
            Layout::top_down(Align::Min),
            |ui| {
                let vol_max_h = (total_h * 0.34).clamp(130.0, 220.0);
                views::volumes::render(ui, app, vol_max_h);
                ui.add_space(12.0);
                views::exclusions::render(ui, app);
            },
        );
    });
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
                InputOutcome::Submit(action, text) => app.execute_input(action, text),
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

    if app.show_log_drawer && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        app.show_log_drawer = false;
        return;
    }

    // 仅当没有任何文本输入控件正在编辑时才响应 Delete 快捷移除排除项
    let text_editing = ctx.egui_wants_keyboard_input();
    if !text_editing
        && app.phase == Phase::Ready
        && !app.is_busy()
        && !app.exclusion_session_current
        && app.selected_exclusion().is_some()
        && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete))
    {
        app.delete_selected_exclusion();
    }
}
