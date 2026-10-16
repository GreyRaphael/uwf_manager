//! 模态组件：确认框、输入框、Overlay 设置、文件选择器、帮助。
//!
//! 每个组件 = 状态结构（含按键处理，返回 Outcome）+ 渲染函数，
//! 与 app 层通过 [`crate::app::Modal`] 组合，便于独立测试。

pub mod confirm;
pub mod file_picker;
pub mod help;
pub mod input_dialog;
pub mod overlay_settings;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::Modal;

/// 将模态渲染在屏幕中央的浮层上
pub fn render_modal(frame: &mut Frame, modal: &Modal, area: Rect) {
    match modal {
        Modal::Confirm(state) => confirm::render(frame, state, area),
        Modal::Input(state) => input_dialog::render(frame, state, area),
        Modal::OverlaySettings(state) => overlay_settings::render(frame, state, area),
        Modal::FilePicker(state) => file_picker::render(frame, state, area),
        Modal::Help => help::render(frame, area),
    }
}

/// 居中矩形：按百分比宽高取屏幕中心区域
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
