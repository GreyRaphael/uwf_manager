//! [?] 快捷键帮助弹窗。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::ui::theme;

use super::centered_rect;

const HELP_LINES: &[&str] = &[
    "全局",
    "  [1-4] / [Tab]  切换页面        [F5] 全部刷新      [Q / Esc] 退出",
    "",
    "状态概览",
    "  [E] 启用筛选器  [D] 禁用筛选器  [R] 重置配置(需确认)",
    "  [S] Overlay 配置  [F10] 重启系统(需确认)  [F11] 关机(需确认)",
    "",
    "卷保护管理",
    "  [↑/↓] 选择卷     [P] 保护(下次生效)  [U] 取消保护(下次生效)",
    "",
    "排除项管理",
    "  [←/→] 切换文件/注册表栏   [S] 切换 当前生效/下次生效 视图",
    "  [A] 添加文件排除(可 F2 浏览)  [R] 添加注册表排除  [D] 删除选中项",
    "  [I] 导入列表(txt)  [X] 导出当前视图列表",
    "",
    "提交工具",
    "  [F] 提交文件  [D] 删除提交(先删文件再固化,需确认)  [R] 注册表提交",
    "",
    "弹窗通用: [Enter] 确认  [Esc] 取消/关闭",
];

pub fn render(frame: &mut Frame, area: Rect) {
    let rect = centered_rect(72, 72, area);
    let block = Block::bordered().title(" 快捷键帮助 ");
    frame.render_widget(Clear, rect);
    let lines: Vec<Line> = HELP_LINES
        .iter()
        .map(|line| {
            if line.is_empty() {
                Line::from("")
            } else if line.starts_with(' ') {
                Line::from(*line)
            } else {
                Line::styled(*line, theme::title())
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}
