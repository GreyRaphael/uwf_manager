//! 各 Tab 视图与页头组件。

pub mod commit;
pub mod dashboard;
pub mod exclusions;
pub mod not_installed;
pub mod volumes;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Tabs;

use crate::app::{App, Tab};
use crate::ui::{text, theme};

/// 顶部选项卡 + 右侧后端徽标
pub fn render_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let labels = Tab::ALL.iter().map(|t| t.label());
    let tabs = Tabs::new(labels)
        .select(app.tab.index())
        .style(theme::dim())
        .highlight_style(theme::title())
        .divider("  ");
    let [tabs_area, badge_area] =
        ratatui::layout::Layout::horizontal([
            ratatui::layout::Constraint::Min(40),
            ratatui::layout::Constraint::Length(18),
        ])
        .areas(area);
    frame.render_widget(tabs, tabs_area);
    frame.render_widget(
        ratatui::widgets::Paragraph::new(text::backend_badge(app.backend_kind)).style(theme::dim()),
        badge_area,
    );
}
