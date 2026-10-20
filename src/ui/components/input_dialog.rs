//! 通用单行输入弹窗。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
    /// 请求切换到文件选择器（只有 AddFileExclusion 会构造）
    Browse,
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
            KeyCode::F(2) if self.browse => InputOutcome::Browse,
            KeyCode::Backspace => {
                self.buffer.pop();
                InputOutcome::Stay
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
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
    // 横向滚动：只显示能放进输入框的尾部，避免长路径溢出。
    let inner_width = rect.width.saturating_sub(4) as usize; // border 2 + "> " 2
    let chars: Vec<char> = state.buffer.chars().collect();
    let shown = inner_width.min(chars.len());
    let display: String = chars[chars.len() - shown..].iter().collect();

    let lines = vec![
        Line::styled(state.hint.clone(), theme::dim()),
        Line::from(""),
        Line::from(format!("> {display}")),
        Line::from(""),
        Line::styled(keys_hint, theme::dim()),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), rect);

    // 显示文本光标：border(1) + "> "(2) = rect.x + 3
    // CJK 宽字符仍按 1 计（路径以 ASCII 为主），超出右边界时钳制。
    let cursor_x = rect.x + 3 + shown as u16;
    frame.set_cursor_position(Position::new(
        cursor_x.min(rect.x + rect.width - 2),
        rect.y + 3,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_and_alt_chars_are_not_inserted() {
        let mut state = InputState::new("t", "h", InputAction::AddFileExclusion, true);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(state.on_key(ctrl_c), InputOutcome::Stay));
        assert!(state.buffer.is_empty());

        let alt_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT);
        assert!(matches!(state.on_key(alt_x), InputOutcome::Stay));
        assert!(state.buffer.is_empty());

        let plain = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(matches!(state.on_key(plain), InputOutcome::Stay));
        assert_eq!(state.buffer, "a");
    }

    #[test]
    fn browse_is_only_returned_when_enabled() {
        let mut browse = InputState::new("t", "h", InputAction::AddFileExclusion, true);
        assert!(matches!(
            browse.on_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE)),
            InputOutcome::Browse
        ));
        let mut no_browse = InputState::new("t", "h", InputAction::CommitRegistry, false);
        assert!(matches!(
            no_browse.on_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE)),
            InputOutcome::Stay
        ));
    }
}
