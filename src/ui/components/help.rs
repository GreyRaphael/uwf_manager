//! [?] / [F1] 快捷键与鼠标操作帮助弹窗。

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::ui::theme;

use super::{clear_surface, is_left_click};

const HELP_LINES: &[&str] = &[
    "全局操作（支持鼠标点击各区域与按钮）",
    "  [Tab / Shift+Tab] 切换聚焦区域    [F5] 全部刷新",
    "  [C] 提交固化工具   [F9] 重置配置   [F10] 重启   [F11] 关机",
    "  [? / F1] 帮助      [Q / Esc] 退出（输入取消修改时先还原）",
    "1. 基本设置",
    "  [↑/↓] 切换项目    [Enter/空格/←/→] 切换 写入过滤 / 覆盖类型",
    "2. 缓存设置 (MB)",
    "  [↑/↓] 切换输入框  [0-9/Backspace] 编辑  [Enter] 确认缓存",
    "3. 保护分区",
    "  [↑/↓] 选择分区    [P] 保护   [U] 不保护   [Enter/空格] 切换",
    "4. 排除列表",
    "  [A] 添加文件(F2浏览)  [R] 添加注册表  [D/Del] 删除选中",
    "  [S] 切换当前/下次视图  [I] 导入列表   [X] 导出列表",
    "弹窗通用: [Enter] 确认  [Esc] 取消/关闭  (亦支持鼠标点击)",
];

#[derive(Default)]
pub struct HelpState {
    close_btn: Rect,
}

impl HelpState {
    pub fn on_key(&self, key: KeyEvent) -> bool {
        matches!(
            key.code,
            KeyCode::Esc
                | KeyCode::Enter
                | KeyCode::Char('q')
                | KeyCode::Char('Q')
                | KeyCode::Char('?')
                | KeyCode::F(1)
        )
    }

    pub fn on_mouse(&self, mouse: MouseEvent) -> bool {
        is_left_click(&mouse).is_some_and(|pos| self.close_btn.contains(pos))
    }
}

pub fn render(frame: &mut Frame, state: &mut HelpState, area: Rect) {
    let width = 72.min(area.width);
    let height = (HELP_LINES.len() as u16 + 4).min(area.height);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    clear_surface(frame, rect);
    let block = Block::bordered()
        .title(" 操作说明 ")
        .title_style(theme::title())
        .border_style(theme::focus_border(true))
        .style(theme::surface());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let [text_area, btn_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(inner);

    let lines: Vec<Line> = HELP_LINES
        .iter()
        .map(|line| {
            if line.starts_with(' ') {
                Line::styled(*line, theme::text())
            } else {
                Line::styled(*line, theme::title())
            }
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(theme::surface()),
        text_area,
    );

    let close_lbl = " 关闭 (Esc) ";
    let close_w = (Line::raw(close_lbl).width() as u16).min(btn_area.width);
    state.close_btn = Rect::new(
        btn_area.x + btn_area.width.saturating_sub(close_w),
        btn_area.y,
        close_w,
        1,
    );
    frame.render_widget(
        Paragraph::new(close_lbl).style(theme::button(true, false, false)),
        state.close_btn,
    );
}
