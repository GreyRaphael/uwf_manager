//! Ratatui 界面层：简洁明亮的单屏四区布局与模态渲染。

pub mod components;
pub mod text;
mod theme;
mod views;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Phase, UiRegions};

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.regions = UiRegions::default();

    // 铺设统一明亮浅色底板
    frame.render_widget(Block::default().style(theme::canvas()), area);

    let [header_area, body_area, notice_area, log_area, hotkey_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    views::render_header(frame, app, header_area);

    if app.phase == Phase::NotInstalled {
        views::not_installed::render(frame, app, body_area);
        render_log(frame, app, log_area);
        render_hotkeys(frame, hotkey_area, true);
    } else {
        let [col_left, col_mid, col_right] = if body_area.width >= 96 {
            Layout::horizontal([
                Constraint::Length(28),
                Constraint::Length(28),
                Constraint::Min(36),
            ])
        } else {
            Layout::horizontal([
                Constraint::Length(24),
                Constraint::Length(24),
                Constraint::Min(28),
            ])
        }
        .spacing(1)
        .areas(body_area);

        views::settings::render(frame, app, col_left);
        views::volumes::render(frame, app, col_mid);
        views::exclusions::render(frame, app, col_right);

        render_notice(frame, notice_area);
        render_log(frame, app, log_area);
        render_hotkeys(frame, hotkey_area, false);
    }

    if let Some(modal) = &mut app.modal {
        components::render_modal(frame, modal, area);
    }
}

fn render_notice(frame: &mut Frame, area: Rect) {
    let msg = if area.width < 98 {
        text::NOTICE_BAR_SHORT
    } else {
        text::NOTICE_BAR
    };
    frame.render_widget(
        Paragraph::new(Line::styled(format!(" {msg}"), theme::warn())).style(theme::canvas()),
        area,
    );
}

fn render_log(frame: &mut Frame, app: &App, area: Rect) {
    let line = match app.last_log.as_ref() {
        Some(entry) => {
            let (label, style) = match entry.level {
                crate::app::LogLevel::Ok => ("成功", theme::ok()),
                crate::app::LogLevel::Warn => ("警告", theme::warn()),
                crate::app::LogLevel::Err => ("失败", theme::err()),
            };
            Line::from(vec![
                Span::styled(format!(" {} [{label}] ", entry.time), style),
                Span::styled(&entry.text, theme::text()),
            ])
        }
        None => Line::from(Span::styled(format!(" {}", text::READY), theme::dim())),
    };
    frame.render_widget(Paragraph::new(line).style(theme::canvas()), area);
}

fn render_hotkeys(frame: &mut Frame, area: Rect, not_installed: bool) {
    let bar = if not_installed {
        text::NI_HOTKEY_BAR
    } else if area.width < 88 {
        text::HOTKEY_BAR_SHORT
    } else {
        text::HOTKEY_BAR
    };
    frame.render_widget(
        Paragraph::new(format!(" {bar}")).style(theme::header().fg(theme::FG_MUTED)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{demo_snapshot, ready_app};
    use crate::app::{FocusPanel, Modal, Phase};
    use crate::backend::{BackendCmd, BackendCommand, BackendKind};
    use crate::model::{OverlayConfig, OverlayRuntime, OverlayType};
    use crate::ui::components::confirm::{ConfirmAction, ConfirmState};
    use crate::ui::components::help::HelpState;
    use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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
        out.chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn single_screen_renders_all_four_panels_at_100x30() {
        let (mut app, _rx) = ready_app(BackendKind::Mock);
        let screen = render_to_string(&mut app, 100, 30);
        assert!(screen.contains("1.基本设置"), "{screen}");
        assert!(screen.contains("2.缓存设置(MB)"), "{screen}");
        assert!(screen.contains("3.保护分区"), "{screen}");
        assert!(screen.contains("4.排除列表"), "{screen}");
        assert!(screen.contains("[Mock演示]"), "{screen}");
        assert!(screen.contains("C:\\Windows\\Temp"), "{screen}");
        assert!(
            screen.contains("HKLM\\SOFTWARE\\UwfManagerDemo"),
            "{screen}"
        );
        assert!(screen.contains("已用:512/2048MB"), "{screen}");
        assert!(screen.contains("25%"), "{screen}");
    }

    #[test]
    fn single_screen_fits_cleanly_at_80x24() {
        let (mut app, _rx) = ready_app(BackendKind::Mock);
        let screen = render_to_string(&mut app, 80, 24);
        assert!(screen.contains("1.基本设置"), "{screen}");
        assert!(screen.contains("2.缓存设置(MB)"), "{screen}");
        assert!(screen.contains("3.保护分区"), "{screen}");
        assert!(screen.contains("4.排除列表"), "{screen}");
        assert!(screen.contains("确认缓存"), "{screen}");
        assert!(screen.contains("保护(P)"), "{screen}");
        assert!(screen.contains("重启"), "{screen}");
        assert!(screen.contains("关机"), "{screen}");
    }

    #[test]
    fn not_installed_guide_page_replaces_panels() {
        let (cmd_tx, _cmd_rx) = std::sync::mpsc::channel();
        let (_, ui_rx) = std::sync::mpsc::channel();
        let mut app = App::new(cmd_tx, ui_rx, BackendKind::Mock);
        app.phase = Phase::NotInstalled;
        let screen = render_to_string(&mut app, 100, 30);
        assert!(screen.contains("UWF功能未安装"), "{screen}");
        assert!(screen.contains("Enable-WindowsOptionalFeature"), "{screen}");
        assert!(screen.contains("Client-UnifiedWriteFilter"), "{screen}");
        assert!(!screen.contains("3.保护分区"), "{screen}");
    }

    #[test]
    fn gauge_uses_current_session_max() {
        let (mut app, _rx) = ready_app(BackendKind::Mock);
        app.snapshot = demo_snapshot();
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
        assert!(screen.contains("已用:512/2048MB"), "{screen}");
        assert!(screen.contains("25%"), "{screen}");
        assert!(!screen.contains("已用:512/4096MB"), "{screen}");
    }

    #[test]
    fn initial_load_failure_shows_retry_hint() {
        let (cmd_tx, _cmd_rx) = std::sync::mpsc::channel();
        let (_, ui_rx) = std::sync::mpsc::channel();
        let mut app = App::new(cmd_tx, ui_rx, BackendKind::Mock);
        app.phase = Phase::LoadFailed;
        let screen = render_to_string(&mut app, 100, 30);
        assert!(screen.contains("加载失败，按[F5]重试"), "{screen}");
        assert!(!screen.contains("加载中…"), "{screen}");
    }

    #[test]
    fn confirm_wraps_multiline_long_path_at_80x24() {
        let (mut app, _rx) = ready_app(BackendKind::Mock);
        let long_path =
            r"C:\Users\Administrator\AppData\Local\Temp\very_long_folder_name\target_subfile.log";
        app.modal = Some(Modal::Confirm(ConfirmState::new(
            "确认删除提交",
            format!("确认从卷中删除并固化？\n{long_path}"),
            ConfirmAction::Quit,
        )));
        let screen = render_to_string(&mut app, 80, 24);
        assert!(screen.contains("确认从卷中删除并固化？"), "{screen}");
        assert!(screen.contains("target_subfile.log"), "{screen}");
        assert!(screen.contains("确认(Enter)"), "{screen}");
    }

    #[test]
    fn help_modal_shows_all_sections_at_80x24() {
        let (mut app, _rx) = ready_app(BackendKind::Mock);
        app.modal = Some(Modal::Help(HelpState::default()));
        let screen = render_to_string(&mut app, 80, 24);
        assert!(screen.contains("1.基本设置"), "{screen}");
        assert!(screen.contains("2.缓存设置"), "{screen}");
        assert!(screen.contains("3.保护分区"), "{screen}");
        assert!(screen.contains("4.排除列表"), "{screen}");
    }

    #[test]
    fn mouse_click_focuses_panels_and_triggers_buttons() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        let _ = render_to_string(&mut app, 100, 30);

        let click = |app: &mut App, pick: fn(&crate::app::UiRegions) -> Rect| {
            let rect = pick(&app.regions);
            assert!(rect.width > 0 && rect.height > 0);
            app.handle_ui_event(crate::app::UiEvent::Terminal(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x + 1,
                row: rect.y,
                modifiers: KeyModifiers::NONE,
            })));
        };

        // 点击 [保护(P)] 按钮保护选中的 C:
        click(&mut app, |r| r.protect_btn);
        assert_eq!(app.focus, FocusPanel::Volumes);
        match cmd_rx.try_recv().unwrap() {
            BackendCommand::Mutate {
                id,
                cmd: BackendCmd::Protect(drive),
            } => {
                assert_eq!(drive, "C:");
                app.handle_ui_event(crate::app::UiEvent::Backend(
                    crate::backend::BackendEvent::Finished {
                        id,
                        outcome: crate::backend::Outcome::Ok("ok".into()),
                    },
                ));
            }
            other => panic!("期望 Protect，实际 {other:?}"),
        }

        // 点击 [切换视图] 按钮切换为只读当前视图
        click(&mut app, |r| r.excl_view_btn);
        assert_eq!(app.focus, FocusPanel::Exclusions);
        assert!(app.exclusion_session_current);

        // 点击 [C]提交 打开提交菜单弹窗，再点取消
        click(&mut app, |r| r.commit_btn);
        assert!(matches!(app.modal, Some(Modal::CommitMenu(_))));
        let _ = render_to_string(&mut app, 100, 30);
    }

    #[test]
    fn tiny_terminal_renders_without_panic() {
        let (mut app, _rx) = ready_app(BackendKind::Mock);
        let _ = render_to_string(&mut app, 20, 10);
    }
}
