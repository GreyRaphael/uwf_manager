//! 中栏：3. 保护分区表与 [保护] / [不保护] 按钮。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table};

use crate::app::{App, FocusPanel};
use crate::ui::{text, theme};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let [table_area, btn_area] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(2)]).areas(area);

    let focused = app.focus == FocusPanel::Volumes;
    app.regions.panel_volumes = table_area;

    let block = Block::bordered()
        .title(" 3.保护分区 ")
        .title_style(theme::focus_title(focused))
        .border_style(theme::focus_border(focused))
        .style(theme::surface());
    let inner = block.inner(table_area);

    if app.phase != crate::app::Phase::Ready {
        frame.render_widget(block, table_area);
        super::render_load_placeholder(frame, app, inner);
    } else {
        // 表头占 inner 第 1 行，数据行从 inner.y + 1 开始
        app.regions.volume_rows = Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(1),
        );

        let compact = inner.width < 25;
        let header = if compact {
            Row::new(["分区", "当前", "重启后"])
        } else {
            Row::new(["分区", "当前状态", "重启状态"])
        }
        .style(theme::title());

        let rows = app.snapshot.volumes.iter().map(|v| {
            let cur_style = if v.current_protected {
                theme::ok()
            } else {
                theme::dim()
            };
            let next_style = if v.current_protected != v.next_protected {
                theme::warn()
            } else if v.next_protected {
                theme::ok()
            } else {
                theme::dim()
            };
            Row::new([
                Cell::from(v.drive_letter.clone()),
                Cell::from(text::protected_zh(v.current_protected)).style(cur_style),
                Cell::from(text::protected_zh(v.next_protected)).style(next_style),
            ])
        });

        let widths = if compact {
            [
                Constraint::Length(4),
                Constraint::Length(7),
                Constraint::Length(7),
            ]
        } else {
            [
                Constraint::Length(5),
                Constraint::Length(9),
                Constraint::Length(9),
            ]
        };

        let table = Table::new(rows, widths)
            .column_spacing(1)
            .block(block)
            .header(header)
            .row_highlight_style(theme::selected())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(table, table_area, &mut app.volumes_state);
    }

    // 底部操作按钮：[ 保护 ]  [ 不保护 ]
    let btn_row = Rect::new(btn_area.x, btn_area.y + 1, btn_area.width, 1);
    let p_lbl = " 保护(P) ";
    let u_lbl = " 不保护(U) ";
    let p_w = 9u16;
    let u_w = 11u16;
    let total_w = p_w + 2 + u_w;
    let start_x = btn_row.x + btn_row.width.saturating_sub(total_w) / 2;

    app.regions.protect_btn = Rect::new(start_x, btn_row.y, p_w.min(btn_row.width), 1);
    let u_x = (start_x + p_w + 2).min(btn_row.right());
    app.regions.unprotect_btn = Rect::new(
        u_x,
        btn_row.y,
        u_w.min(btn_row.right().saturating_sub(u_x)),
        1,
    );

    let ready = app.phase == crate::app::Phase::Ready && !app.snapshot.volumes.is_empty();
    let line = Line::from(vec![
        Span::styled(p_lbl, theme::button(false, false, !ready)),
        Span::styled("  ", theme::canvas()),
        Span::styled(u_lbl, theme::button(false, false, !ready)),
    ]);
    frame.render_widget(
        Paragraph::new(line)
            .alignment(ratatui::layout::Alignment::Center)
            .style(theme::canvas()),
        btn_row,
    );
}
