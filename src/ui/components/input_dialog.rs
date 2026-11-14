//! 通用单行输入弹窗。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
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
    /// 光标所在 UTF-8 字符偏移（`0..=buffer.chars().count()`）
    pub cursor: usize,
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
            cursor: 0,
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
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                InputOutcome::Stay
            }
            KeyCode::Right => {
                let len = self.buffer.chars().count();
                self.cursor = (self.cursor + 1).min(len);
                InputOutcome::Stay
            }
            KeyCode::Home => {
                self.cursor = 0;
                InputOutcome::Stay
            }
            KeyCode::End => {
                self.cursor = self.buffer.chars().count();
                InputOutcome::Stay
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    let idx = char_to_byte(&self.buffer, self.cursor);
                    self.buffer.remove(idx);
                }
                InputOutcome::Stay
            }
            KeyCode::Delete => {
                let idx = char_to_byte(&self.buffer, self.cursor);
                if idx < self.buffer.len() {
                    self.buffer.remove(idx);
                }
                InputOutcome::Stay
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let idx = char_to_byte(&self.buffer, self.cursor);
                self.buffer.insert(idx, c);
                self.cursor += 1;
                InputOutcome::Stay
            }
            _ => InputOutcome::Stay,
        }
    }
}

fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(i, _)| i)
}

/// 按 UTF-8 显示列宽计算可见子串及光标在可见串内的列偏移。
fn visible_window(s: &str, cursor_char: usize, max_cols: usize) -> (&str, u16) {
    let chars: Vec<(usize, usize, usize)> = s
        .char_indices()
        .map(|(i, c)| {
            let end = i + c.len_utf8();
            (i, end, Span::raw(&s[i..end]).width())
        })
        .collect();
    let cursor = cursor_char.min(chars.len());

    let mut start = cursor;
    let mut used = 0usize;
    while start > 0 {
        let w = chars[start - 1].2;
        if used + w > max_cols {
            break;
        }
        used += w;
        start -= 1;
    }
    let cursor_col = used;

    let mut end = cursor;
    while end < chars.len() {
        let w = chars[end].2;
        if used + w > max_cols {
            break;
        }
        used += w;
        end += 1;
    }

    let slice = if start >= end {
        ""
    } else {
        &s[chars[start].0..chars[end - 1].1]
    };
    (slice, cursor_col as u16)
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
    // 横向滚动：按显示列宽窗口展示，兼容中文宽字符路径。
    let inner_width = rect.width.saturating_sub(4) as usize; // border 2 + "> " 2
    let (display, cursor_col) = visible_window(&state.buffer, state.cursor, inner_width);

    let lines = vec![
        Line::styled(state.hint.clone(), theme::dim()),
        Line::from(""),
        Line::from(format!("> {display}")),
        Line::from(""),
        Line::styled(keys_hint, theme::dim()),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), rect);

    // 显示文本光标：border(1) + "> "(2) = rect.x + 3，窄终端下安全钳制避免下溢。
    let cursor_x = (rect.x + 3 + cursor_col).min(rect.x + rect.width.saturating_sub(2));
    let cursor_y = (rect.y + 3).min(rect.y + rect.height.saturating_sub(2));
    frame.set_cursor_position(Position::new(cursor_x, cursor_y));
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

    #[test]
    fn cursor_edits_utf8_mid_string_and_computes_cjk_width() {
        let mut state = InputState::new("t", "h", InputAction::AddFileExclusion, false);
        for c in ['C', ':', '\\', '中', '文'] {
            state.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(state.cursor, 5);
        assert_eq!(
            visible_window(&state.buffer, state.cursor, 10),
            ("C:\\中文", 7)
        );

        // 移到 "中" 后插入 'X'，再用 Delete 删 "文"
        state.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        state.on_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE));
        assert_eq!(state.buffer, "C:\\中X文");
        assert_eq!(
            visible_window(&state.buffer, state.cursor, 10),
            ("C:\\中X文", 6)
        );

        state.on_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(state.buffer, "C:\\中X");
        state.on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(
            visible_window(&state.buffer, state.cursor, 5),
            ("C:\\中", 0)
        );
        state.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(state.cursor, 5);
    }

    #[test]
    fn render_on_tiny_terminal_does_not_underflow() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let state = InputState::new("t", "h", InputAction::AddFileExclusion, false);
        let mut terminal = Terminal::new(TestBackend::new(4, 4)).unwrap();
        terminal.draw(|f| render(f, &state, f.area())).unwrap();
    }
}
