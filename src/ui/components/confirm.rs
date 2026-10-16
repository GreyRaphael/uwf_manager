//! 危险操作确认弹窗（重置/重启/关机/删除提交）。

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::ui::theme;

use super::centered_rect;

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    ResetSettings,
    Restart,
    Shutdown,
    /// 携带待删除文件的绝对路径
    CommitFileDeletion(PathBuf),
}

pub struct ConfirmState {
    pub title: String,
    pub message: String,
    pub action: ConfirmAction,
}

pub enum ConfirmOutcome {
    Stay,
    Cancel,
    Confirm(ConfirmAction),
}

impl ConfirmState {
    pub fn new(title: impl Into<String>, message: impl Into<String>, action: ConfirmAction) -> Self {
        Self { title: title.into(), message: message.into(), action }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> ConfirmOutcome {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                ConfirmOutcome::Confirm(self.action.clone())
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => ConfirmOutcome::Cancel,
            _ => ConfirmOutcome::Stay,
        }
    }
}

pub fn render(frame: &mut Frame, state: &ConfirmState, area: Rect) {
    let rect = centered_rect(56, 32, area);
    let block = Block::bordered()
        .title(format!(" {} ", state.title))
        .border_style(theme::warn());
    frame.render_widget(Clear, rect);
    let lines = vec![
        Line::from(""),
        Line::from(state.message.clone()),
        Line::from(""),
        Line::styled("[Enter] 确认    [Esc / N] 取消", theme::dim()),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(block).alignment(Alignment::Center),
        rect,
    );
}
