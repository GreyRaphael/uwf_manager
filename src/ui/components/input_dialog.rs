//! 通用单行路径/键值输入弹窗（支持原生系统文件/文件夹浏览按钮）。

use eframe::egui::{self, Align, FontId, Key, Layout, RichText};

use crate::ui::theme;

use super::show_modal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseKind {
    None,
    File,
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    AddFileExclusion,
    AddRegistryExclusion,
    CommitFile,
    CommitFileDeletion,
    CommitRegistry,
}

pub struct InputState {
    pub title: String,
    pub field_label: String,
    pub hint: String,
    pub value: String,
    pub action: InputAction,
    pub browse: BrowseKind,
    pub error: Option<String>,
    needs_initial_focus: bool,
}

pub enum InputOutcome {
    Stay,
    Cancel,
    Browse(BrowseKind),
    Submit(InputAction, String),
}

impl InputState {
    pub fn new(
        title: impl Into<String>,
        field_label: impl Into<String>,
        hint: impl Into<String>,
        action: InputAction,
        browse: BrowseKind,
    ) -> Self {
        Self {
            title: title.into(),
            field_label: field_label.into(),
            hint: hint.into(),
            value: String::new(),
            action,
            browse,
            error: None,
            needs_initial_focus: true,
        }
    }

    pub fn set_browsed_path(&mut self, path: &std::path::Path) {
        self.value = path.to_string_lossy().into_owned();
        self.error = None;
    }

    fn try_submit(&mut self) -> InputOutcome {
        let trimmed = self.value.trim().to_string();
        if trimmed.is_empty() {
            self.error = Some("输入内容不能为空".to_string());
            InputOutcome::Stay
        } else {
            self.error = None;
            InputOutcome::Submit(self.action, trimmed)
        }
    }
}

pub fn render(ctx: &egui::Context, state: &mut InputState, browse_busy: bool) -> InputOutcome {
    let mut outcome = InputOutcome::Stay;
    let is_danger = matches!(state.action, InputAction::CommitFileDeletion);
    let title_color = if is_danger {
        theme::ERR_FG
    } else {
        theme::TEXT_PRIMARY
    };
    let submit_label = if is_danger { "下一步" } else { "确认" };

    let title = state.title.clone();
    let (_, should_close) = show_modal(ctx, "input_modal", &title, title_color, 520.0, |ui| {
        ui.label(
            RichText::new(&state.hint)
                .font(FontId::proportional(12.5))
                .color(theme::TEXT_SECONDARY),
        );
        ui.add_space(6.0);

        let lbl = ui.label(
            RichText::new(&state.field_label)
                .font(FontId::proportional(12.5))
                .strong()
                .color(theme::TEXT_PRIMARY),
        );

        let mut enter_pressed = false;
        ui.horizontal(|ui| {
            let browse_lbl = match state.browse {
                BrowseKind::Folder => Some("选择目录…"),
                BrowseKind::File => Some("浏览文件…"),
                BrowseKind::None => None,
            };
            let browse_w = browse_lbl.map_or(0.0, |lbl| {
                theme::measure_button_width(ui, lbl) + ui.spacing().item_spacing.x
            });
            let input_w = (ui.available_width() - browse_w).max(180.0);
            let edit_resp = ui
                .add(
                    egui::TextEdit::singleline(&mut state.value)
                        .desired_width(input_w)
                        .hint_text("请输入路径或点击右侧浏览…"),
                )
                .labelled_by(lbl.id);

            if state.needs_initial_focus {
                edit_resp.request_focus();
                state.needs_initial_focus = false;
            }
            if edit_resp.changed() {
                state.error = None;
            }
            if edit_resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                enter_pressed = true;
            }

            if let Some(lbl) = browse_lbl {
                ui.add_enabled_ui(!browse_busy, |ui| {
                    if theme::secondary_button(ui, lbl).clicked() {
                        outcome = InputOutcome::Browse(state.browse);
                    }
                });
            }
        });

        if let Some(err) = &state.error {
            ui.add_space(4.0);
            ui.label(
                RichText::new(err)
                    .font(FontId::proportional(12.0))
                    .color(theme::ERR_FG),
            );
        }

        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let btn = if is_danger {
                theme::danger_button(ui, submit_label)
            } else {
                theme::primary_button(ui, submit_label)
            };
            if btn.clicked() || enter_pressed {
                outcome = state.try_submit();
            }
            if theme::secondary_button(ui, "取消").clicked() {
                outcome = InputOutcome::Cancel;
            }
        });
    });

    if matches!(outcome, InputOutcome::Stay) && should_close {
        InputOutcome::Cancel
    } else {
        outcome
    }
}
