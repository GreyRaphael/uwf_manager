//! D6：UWF 功能未安装引导页。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::app::App;
use crate::ui::{text, theme};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .title(text::NI_TITLE)
        .title_style(theme::warn())
        .border_style(theme::focus_border(true))
        .style(theme::surface());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [content_area, btn_area] =
        Layout::vertical([Constraint::Min(6), Constraint::Length(1)]).areas(inner);

    let lines = vec![
        Line::from(""),
        Line::styled(format!("  {}", text::NI_LEAD), theme::text()),
        Line::from(""),
        Line::styled(format!("  {}", text::NI_REASON_TITLE), theme::title()),
        Line::styled(format!("   {}", text::NI_REASON_1), theme::dim()),
        Line::styled(format!("   {}", text::NI_REASON_2), theme::dim()),
        Line::from(""),
        Line::styled(format!("  {}", text::NI_ENABLE_TITLE), theme::title()),
        Line::from(Span::styled(
            format!("   {}", text::NI_ENABLE_CMD),
            theme::ok(),
        )),
        Line::from(""),
        Line::styled(format!("  {}", text::NI_FOOTER), theme::dim()),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(theme::surface()),
        content_area,
    );

    let retry_lbl = " 重新检测 (F5) ";
    let quit_lbl = " 退出 (Q) ";
    let retry_w = Line::raw(retry_lbl).width() as u16;
    let quit_w = Line::raw(quit_lbl).width() as u16;
    app.regions.ni_retry_btn = Rect::new(
        btn_area.x + 2,
        btn_area.y,
        retry_w.min(btn_area.width.saturating_sub(2)),
        1,
    );
    let q_x = btn_area.x + 2 + retry_w + 2;
    app.regions.ni_quit_btn = Rect::new(
        q_x,
        btn_area.y,
        quit_w.min(btn_area.right().saturating_sub(q_x)),
        1,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled(retry_lbl, theme::button(true, false, false)),
            Span::raw("  "),
            Span::styled(quit_lbl, theme::button(false, false, false)),
        ]))
        .style(theme::surface()),
        btn_area,
    );
}
