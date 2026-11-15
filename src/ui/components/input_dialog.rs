//! 通用单行输入弹窗。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::ui::theme;

use super::{centered_rect, clear_surface, is_left_click};

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
    confirm_btn: Rect,
    browse_btn: Rect,
    cancel_btn: Rect,
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
            confirm_btn: Rect::default(),
            browse_btn: Rect::default(),
            cancel_btn: Rect::default(),
        }
    }

    fn try_submit(&self) -> InputOutcome {
        if self.buffer.trim().is_empty() {
            InputOutcome::Stay
        } else {
            InputOutcome::Submit(self.action, self.buffer.trim().to_string())
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> InputOutcome {
        match key.code {
            KeyCode::Enter => self.try_submit(),
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

    pub fn on_mouse(&mut self, mouse: MouseEvent) -> InputOutcome {
        let Some(pos) = is_left_click(&mouse) else {
            return InputOutcome::Stay;
        };
        if self.confirm_btn.contains(pos) {
            self.try_submit()
        } else if self.browse && self.browse_btn.contains(pos) {
            InputOutcome::Browse
        } else if self.cancel_btn.contains(pos) {
            InputOutcome::Cancel
        } else {
            InputOutcome::Stay
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

pub fn render(frame: &mut Frame, state: &mut InputState, area: Rect) {
    let rect = centered_rect(68, 32, area);
    let rect = Rect {
        height: rect.height.max(7).min(area.height),
        width: rect.width.max(30).min(area.width),
        ..rect
    };
    clear_surface(frame, rect);
    let block = Block::bordered()
        .title(format!(" {} ", state.title))
        .title_style(theme::title())
        .border_style(theme::focus_border(true))
        .style(theme::surface());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let [hint_area, _, input_area, _, btn_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);

    frame.render_widget(
        Paragraph::new(Line::styled(state.hint.clone(), theme::dim())).style(theme::surface()),
        hint_area,
    );

    // 横向滚动：按显示列宽窗口展示，兼容中文宽字符路径。
    let inner_width = input_area.width.saturating_sub(2) as usize;
    let (display, cursor_col) = visible_window(&state.buffer, state.cursor, inner_width);
    frame.render_widget(
        Paragraph::new(Line::from(format!("> {display}"))).style(theme::active_row()),
        input_area,
    );

    let confirm_text = " 确认(Enter) ";
    let browse_text = " 浏览(F2) ";
    let cancel_text = " 取消(Esc) ";
    let confirm_w = Line::raw(confirm_text).width() as u16;
    let browse_w = if state.browse {
        Line::raw(browse_text).width() as u16
    } else {
        0
    };
    let cancel_w = Line::raw(cancel_text).width() as u16;

    let mut x = btn_area.x;
    state.confirm_btn = Rect::new(x, btn_area.y, confirm_w.min(btn_area.width), 1);
    x += confirm_w + 2;
    let mut spans = vec![
        Span::styled(confirm_text, theme::button(true, false, false)),
        Span::raw("  "),
    ];
    if state.browse {
        state.browse_btn = Rect::new(
            x,
            btn_area.y,
            browse_w.min(btn_area.right().saturating_sub(x)),
            1,
        );
        x += browse_w + 2;
        spans.push(Span::styled(
            browse_text,
            theme::button(false, false, false),
        ));
        spans.push(Span::raw("  "));
    } else {
        state.browse_btn = Rect::default();
    }
    state.cancel_btn = Rect::new(
        x,
        btn_area.y,
        cancel_w.min(btn_area.right().saturating_sub(x)),
        1,
    );
    spans.push(Span::styled(
        cancel_text,
        theme::button(false, false, false),
    ));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme::surface()),
        btn_area,
    );

    if input_area.width > 2 && input_area.height > 0 {
        let cursor_x = (input_area.x + 2 + cursor_col).min(input_area.right().saturating_sub(1));
        frame.set_cursor_position(Position::new(cursor_x, input_area.y));
    }
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
        let mut state = InputState::new("t", "h", InputAction::AddFileExclusion, false);
        let mut terminal = Terminal::new(TestBackend::new(4, 4)).unwrap();
        terminal.draw(|f| render(f, &mut state, f.area())).unwrap();
    }
}
