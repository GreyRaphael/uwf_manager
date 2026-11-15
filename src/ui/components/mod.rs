//! 模态组件：确认框、输入框、文件选择器、提交工具菜单、帮助。
//!
//! 每个组件 = 状态结构（含键盘与鼠标处理，返回 Outcome）+ 渲染函数，
//! 与 app 层通过 [`crate::app::Modal`] 组合。

pub mod commit_menu;
pub mod confirm;
pub mod file_picker;
pub mod help;
pub mod input_dialog;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::widgets::{Block, Clear};

use crate::app::Modal;
use crate::ui::theme;

/// 将模态渲染在屏幕中央的浮层上
pub fn render_modal(frame: &mut Frame, modal: &mut Modal, area: Rect) {
    match modal {
        Modal::Confirm(state) => confirm::render(frame, state, area),
        Modal::Input(state) => input_dialog::render(frame, state, area),
        Modal::FilePicker(state) => file_picker::render(frame, state, area),
        Modal::CommitMenu(state) => commit_menu::render(frame, state, area),
        Modal::Help(state) => help::render(frame, state, area),
    }
}

/// 清空背景并铺设亮色卡片底色
pub(crate) fn clear_surface(frame: &mut Frame, rect: Rect) {
    frame.render_widget(Clear, rect);
    frame.render_widget(Block::default().style(theme::surface()), rect);
}

/// 居中矩形：按百分比宽高取屏幕中心区域，并保证最小宽高不溢出
pub(crate) fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let margin_y = (100u16.saturating_sub(percent_y)) / 2;
    let margin_x = (100u16.saturating_sub(percent_x)) / 2;
    let [_, middle_v, _] = Layout::vertical([
        Constraint::Percentage(margin_y),
        Constraint::Percentage(percent_y),
        Constraint::Percentage(margin_y),
    ])
    .areas(area);
    let [_, center, _] = Layout::horizontal([
        Constraint::Percentage(margin_x),
        Constraint::Percentage(percent_x),
        Constraint::Percentage(margin_x),
    ])
    .areas(middle_v);
    center
}

pub(crate) fn is_left_click(mouse: &MouseEvent) -> Option<Position> {
    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        Some(Position::new(mouse.column, mouse.row))
    } else {
        None
    }
}
