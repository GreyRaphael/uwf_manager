//! Tab 2：卷保护管理（Table）。

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::widgets::{Block, Row, Table};

use crate::app::App;
use crate::ui::{text, theme};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let header = Row::new(["盘符", "当前保护", "下次保护"]).style(theme::title());
    let rows = app.snapshot.volumes.iter().map(|v| {
        Row::new([
            v.drive_letter.clone(),
            text::protected_zh(v.current_protected).to_string(),
            text::protected_zh(v.next_protected).to_string(),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Length(14),
        ],
    )
    .block(Block::bordered().title("[卷保护管理]  [P] 保护  [U] 取消保护"))
    .header(header)
    .row_highlight_style(theme::selected())
    .highlight_symbol("▶ ");
    frame.render_stateful_widget(table, area, &mut app.volumes_state);
}
