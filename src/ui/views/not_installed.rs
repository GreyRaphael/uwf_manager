//! D6：UWF 功能未安装引导页。
//!
//! 后端回包 `BackendEvent::NotInstalled` 置位 `app.not_installed` 后，
//! 本页取代正常 Tab 视图；[F5] 重新检测（RefreshAll），检测到后自动恢复。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::ui::{text, theme};

/// 页头（替代选项卡行）+ 引导正文
pub fn render(frame: &mut Frame, tabs_area: Rect, body_area: Rect) {
    let header = Line::from(vec![
        Span::styled("⚠ ", theme::warn()),
        Span::styled(text::NI_TAB_LABEL, theme::warn()),
    ]);
    frame.render_widget(Paragraph::new(header), tabs_area);

    let block = Block::bordered().title(text::NI_TITLE);
    let lines = vec![
        Line::from(format!("  {}", text::NI_LEAD)),
        Line::from(""),
        Line::from(format!("  {}", text::NI_REASON_TITLE)),
        Line::styled(format!("   {}", text::NI_REASON_1), theme::dim()),
        Line::styled(format!("   {}", text::NI_REASON_2), theme::dim()),
        Line::from(""),
        Line::from(format!("  {}", text::NI_ENABLE_TITLE)),
        Line::from(Span::styled(
            format!("   {}", text::NI_ENABLE_CMD),
            theme::title(),
        )),
        Line::from(""),
        Line::styled(format!("  {}", text::NI_FOOTER), theme::dim()),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), body_area);
}
