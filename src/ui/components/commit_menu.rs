//! 提交固化选择弹窗（文件提交 / 删除提交 / 注册表提交）。

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};

use crate::ui::theme;

use super::{centered_rect, clear_surface, is_left_click};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitChoice {
    File,
    FileDeletion,
    Registry,
}

const ITEMS: [(CommitChoice, &str, &str); 3] = [
    (
        CommitChoice::File,
        "[1] 提交文件修改",
        "浏览并选择文件，将更改固化到物理磁盘（当前会话）",
    ),
    (
        CommitChoice::FileDeletion,
        "[2] 删除并固化文件",
        "先删除文件再固化该删除（两步操作，需二次确认）",
    ),
    (
        CommitChoice::Registry,
        "[3] 提交注册表键值",
        "输入完整路径（如 HKLM\\SOFTWARE\\X\\Value）",
    ),
];

pub struct CommitMenuState {
    pub list_state: ListState,
    list_area: Rect,
    cancel_btn: Rect,
}

pub enum CommitMenuOutcome {
    Stay,
    Cancel,
    Select(CommitChoice),
}

impl CommitMenuState {
    pub fn new() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            list_state,
            list_area: Rect::default(),
            cancel_btn: Rect::default(),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> CommitMenuOutcome {
        let cur = self.list_state.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => CommitMenuOutcome::Cancel,
            KeyCode::Up => {
                self.list_state
                    .select(Some((cur + ITEMS.len() - 1) % ITEMS.len()));
                CommitMenuOutcome::Stay
            }
            KeyCode::Down => {
                self.list_state.select(Some((cur + 1) % ITEMS.len()));
                CommitMenuOutcome::Stay
            }
            KeyCode::Enter => CommitMenuOutcome::Select(ITEMS[cur].0),
            KeyCode::Char('1') | KeyCode::Char('f') | KeyCode::Char('F') => {
                CommitMenuOutcome::Select(CommitChoice::File)
            }
            KeyCode::Char('2') | KeyCode::Char('d') | KeyCode::Char('D') => {
                CommitMenuOutcome::Select(CommitChoice::FileDeletion)
            }
            KeyCode::Char('3') | KeyCode::Char('r') | KeyCode::Char('R') => {
                CommitMenuOutcome::Select(CommitChoice::Registry)
            }
            _ => CommitMenuOutcome::Stay,
        }
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) -> CommitMenuOutcome {
        let Some(pos) = is_left_click(&mouse) else {
            return CommitMenuOutcome::Stay;
        };
        if self.cancel_btn.contains(pos) {
            return CommitMenuOutcome::Cancel;
        }
        if self.list_area.contains(pos) {
            let idx = (pos.y - self.list_area.y) as usize / 2;
            if idx < ITEMS.len() {
                return CommitMenuOutcome::Select(ITEMS[idx].0);
            }
        }
        CommitMenuOutcome::Stay
    }
}

pub fn render(frame: &mut Frame, state: &mut CommitMenuState, area: Rect) {
    let rect = centered_rect(66, 42, area);
    let rect = Rect {
        width: rect.width.max(36).min(area.width),
        height: rect.height.max(10).min(area.height),
        ..rect
    };
    clear_surface(frame, rect);
    let block = Block::bordered()
        .title(" 提交固化工具（当前受保护卷即时生效） ")
        .title_style(theme::title())
        .border_style(theme::focus_border(true))
        .style(theme::surface());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let [list_area, hint_area] =
        Layout::vertical([Constraint::Min(6), Constraint::Length(1)]).areas(inner);
    state.list_area = list_area;

    let items: Vec<ListItem> = ITEMS
        .iter()
        .map(|(_, title, desc)| {
            ListItem::new(vec![
                Line::from(Span::styled(*title, theme::title())),
                Line::from(Span::styled(format!("    {desc}"), theme::dim())),
            ])
        })
        .collect();
    let list = List::new(items)
        .style(theme::surface())
        .highlight_style(theme::selected())
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, list_area, &mut state.list_state);

    let [hint_left, cancel_area] =
        Layout::horizontal([Constraint::Min(10), Constraint::Length(12)]).areas(hint_area);
    state.cancel_btn = cancel_area;
    frame.render_widget(
        Paragraph::new(" [↑/↓] 选择  [Enter/1-3] 执行").style(theme::dim()),
        hint_left,
    );
    frame.render_widget(
        Paragraph::new(" 取消 (Esc) ").style(theme::button(false, false, false)),
        cancel_area,
    );
}
