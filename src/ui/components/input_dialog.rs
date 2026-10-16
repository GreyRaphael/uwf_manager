//! 通用单行输入弹窗。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::ui::theme;

use super::centered_rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    AddFileExclusion,
    AddRegistryExclusion,
    CommitRegistry,
}

pub struct InputState {
    pub title: String,
    pub hint: String,
    pub buffer: String,
    pub action: InputAction,
    /// 是否支持 [F2] 打开文件选择器（仅 AddFileExclusion）
    pub browse: bool,
}

pub enum InputOutcome {
    Stay,
    Cancel,
    Submit(InputAction, String),
    /// 请求切换到文件选择器
    Browse(InputAction),
}

impl InputState {
    pub fn new(
        title: impl Into<String>,
        hint: impl Into<String>,
        action: InputAction,
        browse: bool,
    ) -> Self {
        Self {
            title: title.into(),
            hint: hint.into(),
            buffer: String::new(),
            action,
            browse,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> InputOutcome {
        match key.code {
            KeyCode::Enter => {
                if self.buffer.trim().is_empty() {
                    InputOutcome::Stay
                } else {
                    InputOutcome::Submit(self.action, self.buffer.trim().to_string())
                }
            }
            KeyCode::Esc => InputOutcome::Cancel,
            KeyCode::F(2) if self.browse => InputOutcome::Browse(self.action),
            KeyCode::Backspace => {
                self.buffer.pop();
                InputOutcome::Stay
            }
            KeyCode::Char(c) => {
                self.buffer.push(c);
                InputOutcome::Stay
            }
            _ => InputOutcome::Stay,
        }
    }
}

pub fn render(frame: &mut Frame, state: &InputState, area: Rect) {
    let rect = centered_rect(66, 30, area);
    let block = Block::bordered().title(format!(" {} ", state.title));
    frame.render_widget(Clear, rect);

    let keys_hint = if state.browse {
        "[Enter] 确认    [F2] 浏览…    [Esc] 取消"
    } else {
        "[Enter] 确认    [Esc] 取消"
    };
    let lines = vec![
        Line::styled(state.hint.clone(), theme::dim()),
        Line::from(""),
        Line::from(format!("> {}", state.buffer)),
        Line::from(""),
        Line::styled(keys_hint, theme::dim()),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), rect);

    // 显示文本光标（CJK 宽字符按 1 计，路径输入以 ASCII 为主，可接受）
    let cursor_x = rect.x + 4 + state.buffer.chars().count() as u16;
    frame.set_cursor_position(Position::new(cursor_x.min(rect.x + rect.width - 2), rect.y + 3));
}
