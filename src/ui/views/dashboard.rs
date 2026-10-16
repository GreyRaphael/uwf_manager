//! Tab 1：状态概览（筛选器状态、Overlay 监测、卷保护摘要）。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, Paragraph};

use crate::app::App;
use crate::model::UsageLevel;
use crate::ui::{text, theme};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let [filter_area, overlay_area, volumes_area] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(7),
        Constraint::Min(3),
    ])
    .areas(area);

    render_filter(frame, app, filter_area);
    render_overlay(frame, app, overlay_area);
    render_volume_summary(frame, app, volumes_area);
}

fn render_filter(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title("[UWF 筛选器]");
    let lines = match app.snapshot.filter_state {
        Some(state) => vec![
            Line::from(vec![
                Span::raw("  当前状态: "),
                status_dot(state.current_enabled),
                Span::raw(format!(" {}      下次启动: ", text::enabled_zh(state.current_enabled))),
                status_dot(state.next_enabled),
                Span::raw(format!(" {}", text::enabled_zh(state.next_enabled))),
            ]),
            Line::from(vec![
                Span::raw("  "),
                Span::styled("[E] 启用   [D] 禁用", theme::dim()),
                Span::styled(
                    if app.pending { "   ⏳ 执行中…" } else { "" },
                    theme::warn(),
                ),
            ]),
        ],
        None => vec![Line::from(format!("  {}", text::LOADING))],
    };
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title("[Overlay 缓存]（每 2 秒刷新）");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (Some(next), Some(current), Some(runtime)) = (
        app.snapshot.overlay_config_next,
        app.snapshot.overlay_config_current,
        app.overlay_runtime,
    ) else {
        frame.render_widget(Paragraph::new(format!("  {}", text::LOADING)), inner);
        return;
    };

    let [info_area, gauge_area, avail_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    // 与原版一致：类型展示当前会话、上限展示下次会话（R1 锁定规则的直观呈现）
    let info = Line::from(format!(
        "  类型: {}（当前: {}）  上限: {} MB（当前: {} MB）  预警: {} MB   临界: {} MB",
        next.overlay_type.label(),
        current.overlay_type.label(),
        next.max_size_mb,
        current.max_size_mb,
        runtime.warning_threshold_mb,
        runtime.critical_threshold_mb,
    ));
    frame.render_widget(Paragraph::new(info), info_area);

    let percent = runtime.usage_percent(next.max_size_mb);
    let level = runtime.usage_level();
    let gauge = Gauge::default()
        .gauge_style(theme::gauge(level))
        .percent(percent)
        .label(format!(
            "已用: {} MB / {} MB  {:.1}%",
            runtime.consumption_mb, next.max_size_mb, percent as f64
        ));
    frame.render_widget(gauge, gauge_area);

    let (health, style) = match level {
        UsageLevel::Normal => ("● 状态健康", theme::ok()),
        UsageLevel::Warning => ("● 超过预警阈值", theme::warn()),
        UsageLevel::Critical => ("● 超过临界阈值", theme::err()),
    };
    let avail = Line::from(vec![
        Span::raw(format!("  剩余可用: {} MB  ", runtime.available_space_mb)),
        Span::styled(health, style),
    ]);
    frame.render_widget(Paragraph::new(avail), avail_area);
}

fn render_volume_summary(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title("[卷保护摘要]");
    let lines: Vec<Line> = if app.snapshot.volumes.is_empty() {
        vec![Line::from(format!("  {}", text::LOADING))]
    } else {
        app.snapshot
            .volumes
            .iter()
            .map(|v| {
                Line::from(vec![
                    Span::raw(format!("  {} ", v.drive_letter)),
                    status_dot(v.current_protected),
                    Span::raw(format!(
                        "{} (下次: {})",
                        text::protected_zh(v.current_protected),
                        text::protected_zh(v.next_protected)
                    )),
                ])
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn status_dot(on: bool) -> Span<'static> {
    if on {
        Span::styled("●", theme::ok())
    } else {
        Span::styled("○", theme::dim())
    }
}
