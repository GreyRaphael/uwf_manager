//! 高危/不可逆操作确认弹窗（重置配置 / 重启 / 关机 / 删除固化文件 / 强制退出）。

use std::path::PathBuf;

use eframe::egui::{self, Align, FontId, Key, Layout, Modifiers, RichText};

use crate::ui::theme;

use super::show_modal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    ResetSettings,
    Restart,
    Shutdown,
    /// 有变更在途时的窗口关闭/退出确认
    Quit,
    /// 携带待删除文件的绝对路径（R4 两步确认）
    CommitFileDeletion(PathBuf),
}

pub struct ConfirmState {
    pub title: String,
    pub message: String,
    pub confirm_label: String,
    pub action: ConfirmAction,
}

pub enum ConfirmOutcome {
    Stay,
    Cancel,
    Confirm(ConfirmAction),
}

impl ConfirmState {
    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        action: ConfirmAction,
    ) -> Self {
        let confirm_label = match &action {
            ConfirmAction::ResetSettings => "确认重置",
            ConfirmAction::Restart => "确认重启",
            ConfirmAction::Shutdown => "确认关机",
            ConfirmAction::Quit => "强行退出",
            ConfirmAction::CommitFileDeletion(_) => "确认删除并固化",
        };
        Self {
            title: title.into(),
            message: message.into(),
            confirm_label: confirm_label.to_string(),
            action,
        }
    }
}

pub fn render(ctx: &egui::Context, state: &ConfirmState) -> ConfirmOutcome {
    let mut outcome = ConfirmOutcome::Stay;
    let (_, should_close) = show_modal(
        ctx,
        "confirm_modal",
        &state.title,
        theme::WARN_FG,
        440.0,
        |ui| {
            ui.label(
                RichText::new(&state.message)
                    .font(FontId::proportional(13.0))
                    .color(theme::TEXT_PRIMARY),
            );
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::danger_button(ui, &state.confirm_label).clicked()
                    || ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter))
                {
                    outcome = ConfirmOutcome::Confirm(state.action.clone());
                }
                if theme::secondary_button(ui, "取消").clicked() {
                    outcome = ConfirmOutcome::Cancel;
                }
            });
        },
    );

    if matches!(outcome, ConfirmOutcome::Stay) && should_close {
        ConfirmOutcome::Cancel
    } else {
        outcome
    }
}
