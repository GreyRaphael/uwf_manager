//! 左栏：1. 基本设置 + 2. 缓存设置(MB) 与实时用量监测。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, Paragraph};

use crate::app::{App, BasicField, CacheField, FocusPanel, NumericField, is_overlay_locked};
use crate::model::UsageLevel;
use crate::ui::{text, theme};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let [basic_area, cache_area] =
        Layout::vertical([Constraint::Length(4), Constraint::Min(8)]).areas(area);

    render_basic(frame, app, basic_area);
    render_cache(frame, app, cache_area);
}

fn render_basic(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == FocusPanel::BasicSettings;
    app.regions.panel_basic = area;
    let block = Block::bordered()
        .title(" 1.基本设置 ")
        .title_style(theme::focus_title(focused))
        .border_style(theme::focus_border(focused))
        .style(theme::surface());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if super::render_load_placeholder(frame, app, inner) {
        return;
    }

    let Some(fs) = app.snapshot.filter_state else {
        return;
    };
    let locked = fs.current_enabled;
    let overlay_type = app
        .snapshot
        .overlay_config_next
        .map(|c| c.overlay_type.label())
        .unwrap_or("-");

    let [row0, row1] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    let wf_selected = focused && app.basic_field == BasicField::WriteFilter;
    let wf_btn = format!(" {} ⇄ ", text::toggle_zh(fs.next_enabled));
    let wf_style = if wf_selected {
        theme::button(true, false, false)
    } else if fs.next_enabled {
        theme::button(false, false, false).fg(theme::STATUS_OK)
    } else {
        theme::button(false, false, false)
    };
    let label_w = 10u16.min(row0.width);
    app.regions.filter_toggle = Rect::new(
        row0.x + label_w,
        row0.y,
        row0.width.saturating_sub(label_w),
        1,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" 写入过滤:", theme::text()),
            Span::styled(wf_btn, wf_style),
        ]))
        .style(theme::surface()),
        row0,
    );

    let ot_selected = focused && app.basic_field == BasicField::OverlayType;
    let compact = row1.width < 24;
    let ot_text = if compact {
        match app.snapshot.overlay_config_next.map(|c| c.overlay_type) {
            Some(crate::model::OverlayType::Ram) => "内存",
            Some(crate::model::OverlayType::Disk) => "磁盘",
            None => "-",
        }
    } else {
        overlay_type
    };
    let lock_mark = if locked { "[锁]" } else { "⇄" };
    let ot_btn = format!(" {ot_text} {lock_mark} ");
    let ot_style = theme::button(ot_selected, false, locked);
    app.regions.overlay_type_toggle = Rect::new(
        row1.x + label_w,
        row1.y,
        row1.width.saturating_sub(label_w),
        1,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" 覆盖类型:", theme::text()),
            Span::styled(ot_btn, ot_style),
        ]))
        .style(theme::surface()),
        row1,
    );
}

fn render_cache(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == FocusPanel::CacheSettings;
    app.regions.panel_cache = area;

    let mut title_spans = vec![Span::styled(" 2.缓存设置(MB)", theme::focus_title(focused))];
    if app.cache.dirty {
        title_spans.push(Span::styled("*未保存", theme::warn()));
    }
    title_spans.push(Span::raw(" "));

    let block = Block::bordered()
        .title(Line::from(title_spans))
        .border_style(theme::focus_border(focused))
        .style(theme::surface());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if super::render_load_placeholder(frame, app, inner) {
        return;
    }

    let [
        r_max,
        r_warn,
        r_crit,
        r_msg,
        r_btn,
        _gap,
        r_usage_txt,
        r_gauge,
        r_avail,
    ] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(inner);

    let locked = is_overlay_locked(&app.snapshot);

    let max_cursor = render_numeric_row(
        frame,
        r_max,
        " 最大缓存:",
        &app.cache.max_size,
        focused && app.cache.field == CacheField::MaxSize && !locked,
        locked,
        &mut app.regions.max_size_input,
    );
    let warn_cursor = render_numeric_row(
        frame,
        r_warn,
        " 警告阈值:",
        &app.cache.warning,
        focused && app.cache.field == CacheField::WarningThreshold,
        false,
        &mut app.regions.warning_input,
    );
    let crit_cursor = render_numeric_row(
        frame,
        r_crit,
        " 严重阈值:",
        &app.cache.critical,
        focused && app.cache.field == CacheField::CriticalThreshold,
        false,
        &mut app.regions.critical_input,
    );

    if let Some(err) = &app.cache.error {
        frame.render_widget(
            Paragraph::new(Line::styled(format!(" {err}"), theme::err())).style(theme::surface()),
            r_msg,
        );
    } else if locked {
        frame.render_widget(
            Paragraph::new(Line::styled(" 启用时最大缓存已锁定", theme::dim()))
                .style(theme::surface()),
            r_msg,
        );
    }

    // [ 确认缓存 ] 按钮
    let btn_label = " 确认缓存 ";
    let btn_w = (Line::raw(btn_label).width() as u16).min(r_btn.width);
    let btn_x = r_btn.x + r_btn.width.saturating_sub(btn_w) / 2;
    app.regions.cache_confirm_btn = Rect::new(btn_x, r_btn.y, btn_w, 1);
    let btn_focused = focused && app.cache.field == CacheField::ConfirmButton;
    frame.render_widget(
        Paragraph::new(btn_label).style(theme::button(btn_focused, false, false)),
        app.regions.cache_confirm_btn,
    );

    // 实时用量条（R7：consumption / current.max_size_mb）
    if let (Some(current), Some(runtime)) = (
        app.snapshot.overlay_config_current,
        app.snapshot.overlay_runtime,
    ) {
        let max_size = current.max_size_mb.max(1);
        let percent = runtime.usage_percent(max_size);
        let level = runtime.usage_level();

        frame.render_widget(
            Paragraph::new(format!(
                " 已用: {} / {} MB",
                runtime.consumption_mb, max_size
            ))
            .style(theme::surface().fg(theme::FG_MUTED)),
            r_usage_txt,
        );
        let [_, gauge_inner, _] = Layout::horizontal([
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .areas(r_gauge);
        let gauge = Gauge::default()
            .gauge_style(theme::gauge(level))
            .percent(percent)
            .label(format!("{percent}%"));
        frame.render_widget(gauge, gauge_inner);

        let (health, h_style) = match level {
            UsageLevel::Normal => ("● 健康", theme::ok()),
            UsageLevel::Warning => ("● 预警", theme::warn()),
            UsageLevel::Critical => ("● 临界", theme::err()),
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(" 剩余: {} MB ", runtime.available_space_mb),
                    theme::dim(),
                ),
                Span::styled(health, h_style),
            ]))
            .style(theme::surface()),
            r_avail,
        );
    }

    // 在无弹窗且聚焦于未锁定的输入框时展示实体输入光标
    if app.modal.is_none() && focused {
        let cursor_pos = match app.cache.field {
            CacheField::MaxSize if !locked => max_cursor,
            CacheField::WarningThreshold => warn_cursor,
            CacheField::CriticalThreshold => crit_cursor,
            _ => None,
        };
        if let Some(pos) = cursor_pos {
            frame.set_cursor_position(pos);
        }
    }
}

fn render_numeric_row(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    field: &NumericField,
    is_focused: bool,
    is_locked: bool,
    hit_rect: &mut Rect,
) -> Option<Position> {
    if area.height == 0 || area.width < 12 {
        *hit_rect = Rect::default();
        return None;
    }
    let label_w = 10u16;
    let box_area = Rect::new(
        area.x + label_w,
        area.y,
        area.width.saturating_sub(label_w + 1),
        1,
    );
    *hit_rect = box_area;

    let inner_w = box_area.width.saturating_sub(2) as usize;
    let (visible, cursor_off) = slice_input(&field.text, field.cursor, inner_w);
    let pad = inner_w.saturating_sub(visible.len());
    let bracket_l = if is_focused { "▸" } else { "[" };
    let bracket_r = if is_focused { "◂" } else { "]" };
    let box_text = format!("{bracket_l}{visible}{:pad$}{bracket_r}", "");

    let line = Line::from(vec![
        Span::styled(label, theme::text()),
        Span::styled(box_text, theme::input(is_focused, is_locked)),
    ]);
    frame.render_widget(Paragraph::new(line).style(theme::surface()), area);

    if is_focused && !is_locked && inner_w > 0 {
        let cx = (box_area.x + 1 + cursor_off as u16).min(box_area.right().saturating_sub(2));
        Some(Position::new(cx, box_area.y))
    } else {
        None
    }
}

fn slice_input(s: &str, cursor: usize, max_cols: usize) -> (&str, usize) {
    if max_cols == 0 || s.is_empty() {
        return ("", 0);
    }
    let cursor = cursor.min(s.len());
    if s.len() <= max_cols {
        return (s, cursor);
    }
    let start = cursor.saturating_sub(max_cols);
    let end = (start + max_cols).min(s.len());
    (&s[start..end], cursor - start)
}
