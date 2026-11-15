//! 右栏：4. 排除列表（文件与注册表统一展示、行内操作栏）与底部快捷按钮。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, Paragraph};

use crate::app::{App, FocusPanel};
use crate::ui::{text, theme};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let [box_area, btn_area] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(2)]).areas(area);

    let focused = app.focus == FocusPanel::Exclusions;
    app.regions.panel_exclusions = box_area;

    let read_only = app.exclusion_session_current;
    let title_spans = if read_only {
        vec![
            Span::styled(" 4.排除列表 ", theme::focus_title(focused)),
            Span::styled("[当前生效·只读] ", theme::warn()),
        ]
    } else {
        vec![
            Span::styled(" 4.排除列表 ", theme::focus_title(focused)),
            Span::styled("[重启后·可编辑] ", theme::ok()),
        ]
    };

    let block = Block::bordered()
        .title(Line::from(title_spans))
        .border_style(theme::focus_border(focused))
        .style(theme::surface());
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);

    if !super::render_load_placeholder(frame, app, inner) {
        let [list_area, toolbar_area] =
            Layout::vertical([Constraint::Min(2), Constraint::Length(1)]).areas(inner);
        app.regions.exclusion_rows = list_area;

        let visible = app.visible_exclusions();
        let items: Vec<ListItem> = if visible.is_empty() {
            vec![ListItem::new(text::READY_EMPTY).style(theme::dim())]
        } else {
            visible
                .iter()
                .map(|item| {
                    let style = if item.is_registry() {
                        theme::registry()
                    } else {
                        theme::text()
                    };
                    ListItem::new(item.display()).style(style)
                })
                .collect()
        };

        let list = List::new(items)
            .style(theme::surface())
            .highlight_style(theme::selected())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, list_area, &mut app.exclusion_state);

        render_toolbar(frame, app, toolbar_area, read_only);
    }

    render_bottom_buttons(
        frame,
        app,
        Rect::new(btn_area.x, btn_area.y + 1, btn_area.width, 1),
    );
}

fn render_toolbar(frame: &mut Frame, app: &mut App, area: Rect, read_only: bool) {
    let compact = area.width < 36;
    let view_lbl = if compact {
        " 视图 "
    } else {
        " 切换视图 "
    };
    let btns: [(&str, bool); 4] = if compact {
        [
            (" +文件 ", read_only),
            (" +注册 ", read_only),
            (" 删 ", read_only),
            (view_lbl, false),
        ]
    } else {
        [
            (" +文件 ", read_only),
            (" +注册表 ", read_only),
            (" 删除 ", read_only),
            (view_lbl, false),
        ]
    };

    let mut x = area.x;
    let mut spans = Vec::new();
    let rects = [
        &mut app.regions.excl_add_file_btn,
        &mut app.regions.excl_add_reg_btn,
        &mut app.regions.excl_delete_btn,
        &mut app.regions.excl_view_btn,
    ];
    for (i, (lbl, disabled)) in btns.iter().enumerate() {
        let w = (Line::raw(*lbl).width() as u16).min(area.right().saturating_sub(x));
        *rects[i] = Rect::new(x, area.y, w, 1);
        x += w + 1;
        spans.push(Span::styled(*lbl, theme::button(false, false, *disabled)));
        spans.push(Span::styled(" ", theme::surface()));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme::surface()),
        area,
    );
}

fn render_bottom_buttons(frame: &mut Frame, app: &mut App, area: Rect) {
    let ready = app.phase == crate::app::Phase::Ready;
    let import_disabled = !ready || app.exclusion_session_current;
    let compact = area.width < 38;
    let btns: [(&str, bool, bool); 4] = if compact {
        [
            (" 导入 ", false, import_disabled),
            (" 导出 ", false, !ready),
            (" 重启 ", true, !ready),
            (" 关机 ", true, !ready),
        ]
    } else {
        [
            (" 导入列表 ", false, import_disabled),
            (" 导出列表 ", false, !ready),
            (" 重启 ", true, !ready),
            (" 关机 ", true, !ready),
        ]
    };

    let total_w: u16 = btns
        .iter()
        .map(|(lbl, _, _)| Line::raw(*lbl).width() as u16 + 1)
        .sum::<u16>()
        .saturating_sub(1);
    let mut x = area.x + area.width.saturating_sub(total_w) / 2;
    let mut spans = Vec::new();
    let rects = [
        &mut app.regions.import_btn,
        &mut app.regions.export_btn,
        &mut app.regions.restart_btn,
        &mut app.regions.shutdown_btn,
    ];
    for (i, (lbl, danger, disabled)) in btns.iter().enumerate() {
        let w = (Line::raw(*lbl).width() as u16).min(area.right().saturating_sub(x));
        *rects[i] = Rect::new(x, area.y, w, 1);
        x += w + 1;
        spans.push(Span::styled(*lbl, theme::button(false, *danger, *disabled)));
        spans.push(Span::styled(" ", theme::canvas()));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme::canvas()),
        area,
    );
}
