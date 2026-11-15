//! 危险操作确认弹窗（重置/重启/关机/删除提交/退出确认）。

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::ui::theme;

use super::{centered_rect, clear_surface, is_left_click};

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    ResetSettings,
    Restart,
    Shutdown,
    /// 变更在途时的退出确认
    Quit,
    /// 携带待删除文件的绝对路径
    CommitFileDeletion(PathBuf),
}

pub struct ConfirmState {
    pub title: String,
    pub message: String,
    pub action: ConfirmAction,
    confirm_btn: Rect,
    cancel_btn: Rect,
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
        Self {
            title: title.into(),
            message: message.into(),
            action,
            confirm_btn: Rect::default(),
            cancel_btn: Rect::default(),
        }
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

    pub fn on_mouse(&mut self, mouse: MouseEvent) -> ConfirmOutcome {
        let Some(pos) = is_left_click(&mouse) else {
            return ConfirmOutcome::Stay;
        };
        if self.confirm_btn.contains(pos) {
            ConfirmOutcome::Confirm(self.action.clone())
        } else if self.cancel_btn.contains(pos) {
            ConfirmOutcome::Cancel
        } else {
            ConfirmOutcome::Stay
        }
    }
}

pub fn render(frame: &mut Frame, state: &mut ConfirmState, area: Rect) {
    let rect = centered_rect(64, 42, area);
    let rect = Rect {
        height: rect.height.max(8).min(area.height),
        width: rect.width.max(30).min(area.width),
        ..rect
    };
    clear_surface(frame, rect);
    let block = Block::bordered()
        .title(format!(" {} ", state.title))
        .title_style(theme::warn())
        .border_style(theme::warn())
        .style(theme::surface());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let [msg_area, btn_area] =
        Layout::vertical([Constraint::Min(2), Constraint::Length(1)]).areas(inner);

    let mut lines = vec![Line::from("")];
    lines.extend(
        state
            .message
            .lines()
            .map(|line| Line::from(line.to_string())),
    );
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false })
            .style(theme::surface()),
        msg_area,
    );

    let confirm_label = " 确认(Enter) ";
    let cancel_label = " 取消(Esc) ";
    let c_w = Line::raw(confirm_label).width() as u16;
    let x_w = Line::raw(cancel_label).width() as u16;
    let total_w = c_w + 2 + x_w;
    let start_x = btn_area.x + btn_area.width.saturating_sub(total_w) / 2;
    state.confirm_btn = Rect::new(start_x, btn_area.y, c_w.min(btn_area.width), 1);
    let cancel_x = (start_x + c_w + 2).min(btn_area.right());
    state.cancel_btn = Rect::new(
        cancel_x,
        btn_area.y,
        x_w.min(btn_area.right().saturating_sub(cancel_x)),
        1,
    );

    let btns = Line::from(vec![
        Span::styled(confirm_label, theme::button(true, false, false)),
        Span::raw("  "),
        Span::styled(cancel_label, theme::button(false, false, false)),
    ]);
    frame.render_widget(
        Paragraph::new(btns)
            .alignment(Alignment::Center)
            .style(theme::surface()),
        btn_area,
    );
}
