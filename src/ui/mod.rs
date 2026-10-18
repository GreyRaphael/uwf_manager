//! Ratatui 界面层：布局主框架与各视图。

pub mod components;
pub mod text;
mod theme;
mod views;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Tab};

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let outer = Block::bordered()
        .title(text::app_title())
        .title_alignment(ratatui::layout::Alignment::Left)
        .style(Style::default().fg(Color::White));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let [tabs_area, body_area, log_area, hotkey_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(6),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    // D6：UWF 未安装时以引导页取代正常视图（[F5] 重新检测，检测到后自动恢复）
    if app.not_installed {
        views::not_installed::render(frame, tabs_area, body_area);
    } else {
        views::render_tabs(frame, app, tabs_area);
        match app.tab {
            Tab::Dashboard => views::dashboard::render(frame, app, body_area),
            Tab::Volumes => views::volumes::render(frame, app, body_area),
            Tab::Exclusions => views::exclusions::render(frame, app, body_area),
            Tab::Commit => views::commit::render(frame, app, body_area),
        }
    }
    render_log(frame, app, log_area);
    render_hotkeys(frame, hotkey_area);

    // 模态浮层最后渲染（置于所有视图之上）
    if let Some(modal) = &app.modal {
        components::render_modal(frame, modal, area);
    }
}

/// 底部操作日志（最近一条）
fn render_log(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let line = match app.log.back() {
        Some(entry) => ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(
                format!("{} [{}] ", entry.time, if entry.ok { "成功" } else { "失败" }),
                if entry.ok { theme::ok() } else { theme::err() },
            ),
            ratatui::text::Span::raw(&entry.text),
        ]),
        None => ratatui::text::Line::from(ratatui::text::Span::styled(text::READY, theme::dim())),
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// 底部全局快捷键栏
fn render_hotkeys(frame: &mut Frame, area: ratatui::layout::Rect) {
    frame.render_widget(
        Paragraph::new(text::HOTKEY_BAR).style(theme::dim()),
        area,
    );
}
