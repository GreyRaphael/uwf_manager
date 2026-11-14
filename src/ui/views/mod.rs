//! 各 Tab 视图与页头组件。

pub mod commit;
pub mod dashboard;
pub mod exclusions;
pub mod not_installed;
pub mod volumes;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Tabs};

use crate::app::{App, Phase, Tab};
use crate::ui::{text, theme};

/// 顶部选项卡 + 右侧全局状态徽标（断线 / 忙态在全部 Tab 可见）
pub fn render_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let labels = Tab::ALL.iter().map(|t| t.label());
    let tabs = Tabs::new(labels)
        .select(app.tab.index())
        .style(theme::dim())
        .highlight_style(theme::title())
        .divider("  ");
    let [tabs_area, badge_area] = ratatui::layout::Layout::horizontal([
        ratatui::layout::Constraint::Min(40),
        ratatui::layout::Constraint::Length(18),
    ])
    .areas(area);
    frame.render_widget(tabs, tabs_area);
    let (badge, style) = if app.phase == Phase::BackendLost {
        ("⚠ 后端已断开", theme::err())
    } else if app.is_busy() {
        ("⏳ 执行中…", theme::warn())
    } else {
        (text::backend_badge(app.backend_kind), theme::dim())
    };
    frame.render_widget(Paragraph::new(badge).style(style), badge_area);
}

/// 尚未持有可用快照时（加载中 / 加载失败 / 未加载即断线）渲染统一占位并返回 `true`。
pub fn render_load_placeholder(frame: &mut Frame, app: &App, area: Rect, block: Block<'_>) -> bool {
    let (msg, style) = match app.phase {
        Phase::LoadFailed => (text::LOAD_FAILED, theme::err()),
        Phase::BackendLost if app.snapshot.filter_state.is_none() => {
            ("后端已断开，请退出重启", theme::err())
        }
        Phase::Loading => (text::LOADING, Style::default()),
        _ => return false,
    };
    frame.render_widget(
        Paragraph::new(Line::styled(format!("  {msg}"), style)).block(block),
        area,
    );
    true
}
