//! Tab 4：提交工具（文件提交 / 删除提交 / 注册表提交）。

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
        Line::from(vec![
            ratatui::text::Span::styled("  [F] 提交文件      ", theme::title()),
            ratatui::text::Span::raw("浏览并选择受保护卷中的文件，提交其修改"),
        ]),
        Line::from(vec![
            ratatui::text::Span::styled("  [D] 删除提交      ", theme::title()),
            ratatui::text::Span::raw("先真实删除文件，再固化该删除（两步操作，需确认）"),
        ]),
        Line::from(vec![
            ratatui::text::Span::styled("  [R] 注册表提交    ", theme::title()),
            ratatui::text::Span::raw("输入注册表完整路径（如 HKLM\\SOFTWARE\\X\\Value），提交键值修改"),
        ]),
        Line::from(""),
        Line::styled(
            "  提示：提交仅对当前已受 UWF 保护的卷有意义；删除提交会立即删除所选文件。",
            theme::dim(),
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title("[提交工具]")),
        area,
    );
}
