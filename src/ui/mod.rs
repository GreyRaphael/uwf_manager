//! Ratatui 界面层：布局主框架与各视图。

pub mod components;
pub mod text;
mod theme;
mod views;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Phase, Tab};

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
    if app.phase == Phase::NotInstalled {
        views::not_installed::render(frame, tabs_area, body_area);
        render_hotkeys(frame, hotkey_area, true);
    } else {
        views::render_tabs(frame, app, tabs_area);
        match app.tab {
            Tab::Dashboard => views::dashboard::render(frame, app, body_area),
            Tab::Volumes => views::volumes::render(frame, app, body_area),
            Tab::Exclusions => views::exclusions::render(frame, app, body_area),
            Tab::Commit => views::commit::render(frame, app, body_area),
        }
        render_hotkeys(frame, hotkey_area, false);
    }
    render_log(frame, app, log_area);

    // 模态浮层最后渲染（置于所有视图之上）
    if let Some(modal) = &mut app.modal {
        components::render_modal(frame, modal, area);
    }
}

/// 底部操作日志（最近一条）
fn render_log(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let line = match app.last_log.as_ref() {
        Some(entry) => {
            let (label, style) = match entry.level {
                crate::app::LogLevel::Ok => ("成功", theme::ok()),
                crate::app::LogLevel::Warn => ("警告", theme::warn()),
                crate::app::LogLevel::Err => ("失败", theme::err()),
            };
            ratatui::text::Line::from(vec![
                ratatui::text::Span::styled(format!("{} [{label}] ", entry.time), style),
                ratatui::text::Span::raw(&entry.text),
            ])
        }
        None => ratatui::text::Line::from(ratatui::text::Span::styled(text::READY, theme::dim())),
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// 底部全局快捷键栏（未安装页使用独立提示，避免误导）
fn render_hotkeys(frame: &mut Frame, area: ratatui::layout::Rect, not_installed: bool) {
    let bar = if not_installed {
        text::NI_HOTKEY_BAR
    } else {
        text::HOTKEY_BAR
    };
    frame.render_widget(Paragraph::new(bar).style(theme::dim()), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::demo_snapshot;
    use crate::app::{App, Modal, Phase};
    use crate::backend::BackendKind;
    use crate::ui::components::overlay_settings::OverlaySettingsState;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn test_app() -> App {
        let (cmd_tx, _cmd_rx) = std::sync::mpsc::channel();
        let (_, ui_rx) = std::sync::mpsc::channel();
        App::new(cmd_tx, ui_rx, BackendKind::Mock)
    }

    /// 渲染整个界面到字符串（逐行拼接 TestBackend 缓冲）
    fn render_to_string(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let area = buffer.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        // CJK 宽字符占两格、后一格符号为空格：去除全部空白再做包含断言
        out.chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn dashboard_renders_snapshot() {
        let mut app = test_app();
        app.snapshot = demo_snapshot();
        app.phase = Phase::Ready;
        let screen = render_to_string(&mut app, 100, 30);
        assert!(screen.contains("[1]状态概览"), "缺少 Tab 标签:\n{screen}");
        assert!(screen.contains("Mock演示数据"), "缺少后端徽标:\n{screen}");
        assert!(screen.contains("当前状态"), "缺少筛选器状态:\n{screen}");
        assert!(screen.contains("2048MB"), "缺少 Overlay 上限:\n{screen}");
        assert!(
            screen.contains("已用:512MB/2048MB"),
            "缺少 Gauge 文案:\n{screen}"
        );
        assert!(screen.contains("C:"), "缺少卷摘要:\n{screen}");
    }

    #[test]
    fn not_installed_guide_page_replaces_tabs() {
        let mut app = test_app();
        app.phase = Phase::NotInstalled;
        let screen = render_to_string(&mut app, 100, 30);
        // D6 引导页：启用命令完整可见
        assert!(
            screen.contains("UWF功能未安装"),
            "缺少引导页标题:\n{screen}"
        );
        assert!(
            screen.contains("Enable-WindowsOptionalFeature"),
            "缺少启用命令:\n{screen}"
        );
        assert!(
            screen.contains("Client-UnifiedWriteFilter"),
            "缺少功能名:\n{screen}"
        );
        // 正常 Tab 视图被取代
        assert!(
            !screen.contains("[2]卷保护管理"),
            "引导页不应渲染 Tab 栏:\n{screen}"
        );
    }

    #[test]
    fn overlay_settings_modal_shows_r1_lock_hint() {
        let mut app = test_app();
        app.snapshot = demo_snapshot();
        app.phase = Phase::Ready;
        app.modal = Some(Modal::OverlaySettings(OverlaySettingsState::from_snapshot(
            &app.snapshot,
        )));
        let screen = render_to_string(&mut app, 100, 30);
        assert!(screen.contains("Overlay配置"), "缺少弹窗标题:\n{screen}");
        assert!(screen.contains("（已锁定）"), "缺少 R1 锁定标记:\n{screen}");
        assert!(screen.contains("仅阈值可改"), "缺少 R1 提示:\n{screen}");
    }
    #[test]
    fn gauge_uses_current_session_max() {
        use crate::model::{OverlayConfig, OverlayRuntime, OverlayType};
        let mut app = test_app();
        app.snapshot = demo_snapshot();
        app.phase = Phase::Ready;
        app.snapshot.overlay_config_current = Some(OverlayConfig {
            overlay_type: OverlayType::Ram,
            max_size_mb: 2048,
        });
        app.snapshot.overlay_config_next = Some(OverlayConfig {
            overlay_type: OverlayType::Ram,
            max_size_mb: 4096,
        });
        app.snapshot.overlay_runtime = Some(OverlayRuntime {
            available_space_mb: 1536,
            consumption_mb: 512,
            warning_threshold_mb: 1536,
            critical_threshold_mb: 1843,
        });
        let screen = render_to_string(&mut app, 100, 30);
        assert!(
            screen.contains("已用:512MB/2048MB"),
            "Gauge 应使用 current 上限:\n{screen}"
        );
        assert!(screen.contains("25%"), "百分比应为 25%:\n{screen}");
        assert!(
            !screen.contains("已用:512MB/4096MB"),
            "不应使用 next 上限:\n{screen}"
        );
    }
    #[test]
    fn initial_load_failure_shows_retry_hint() {
        let mut app = test_app();
        app.phase = Phase::LoadFailed;
        let screen = render_to_string(&mut app, 100, 30);
        assert!(
            screen.contains("加载失败，按[F5]重试"),
            "加载失败应提示重新检测:\n{screen}"
        );
        assert!(
            !screen.contains("加载中…"),
            "失败态不应再显示加载中:\n{screen}"
        );
    }
}
