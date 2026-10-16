//! Tab 4：提交工具（阶段 1 为说明页，阶段 2 接入 FilePicker 与提交动作）。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};

use crate::app::App;
use crate::ui::theme;

pub fn render(frame: &mut Frame, _app: &App, area: Rect) {
    let lines = vec![
        Line::from("  将受保护卷中的临时更改固化到物理磁盘（重启后不丢失）。"),
        Line::from(""),
        Line::styled("  [F] 提交文件        选择受保护卷中的文件，提交其修改", theme::dim()),
        Line::styled("  [D] 删除提交        删除文件并固化该删除（两步操作）", theme::dim()),
        Line::styled("  [R] 注册表提交      输入注册表完整路径，提交键值修改", theme::dim()),
        Line::from(""),
        Line::styled("  （以上操作将在阶段 2 开放）", theme::dim()),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title("[提交工具]")),
        area,
    );
}
