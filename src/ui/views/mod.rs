//! 主界面视图模块：页头、三大分栏与未安装引导页。

pub mod exclusions;
pub mod not_installed;
pub mod settings;
pub mod volumes;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Phase};
use crate::ui::{text, theme};

/// 顶部状态与快捷工具栏（1 行）
pub fn render_header(frame: &mut Frame, app: &mut App, area: Rect) {
    frame.render_widget(Paragraph::new("").style(theme::header()), area);
    if area.width < 20 || area.height == 0 {
        return;
    }

    let compact = area.width < 96;
    let title = if compact {
        format!(" UWF v{} ", env!("CARGO_PKG_VERSION"))
    } else {
        format!(" {} ", text::app_title())
    };
    let badge = if compact {
        match app.backend_kind {
            crate::backend::BackendKind::Windows => "[管理]",
            crate::backend::BackendKind::Mock => "[Mock]",
        }
    } else {
        text::backend_badge(app.backend_kind)
    };

    let mut left_spans = vec![
        Span::styled(title, theme::header_title()),
        Span::styled(format!("{badge} "), theme::header().fg(theme::FG_MUTED)),
    ];

    if app.phase == Phase::BackendLost {
        left_spans.push(Span::styled(
            "⚠ 后端已断开",
            theme::header().fg(theme::STATUS_ERR),
        ));
    } else if app.is_busy() {
        left_spans.push(Span::styled(
            "⏳ 执行中…",
            theme::header().fg(theme::STATUS_WARN),
        ));
    } else if let Some(fs) = app.snapshot.filter_state {
        let cur_style = if fs.current_enabled {
            theme::header().fg(theme::STATUS_OK)
        } else {
            theme::header().fg(theme::FG_MUTED)
        };
        let next_style = if fs.next_enabled {
            theme::header().fg(theme::STATUS_OK)
        } else {
            theme::header().fg(theme::FG_MUTED)
        };
        let (cur_text, next_text) = if compact {
            (
                text::toggle_zh(fs.current_enabled),
                text::toggle_zh(fs.next_enabled),
            )
        } else {
            (
                text::enabled_zh(fs.current_enabled),
                text::enabled_zh(fs.next_enabled),
            )
        };
        left_spans.push(Span::styled("当前:", theme::header()));
        left_spans.push(Span::styled(format!("●{cur_text} "), cur_style));
        left_spans.push(Span::styled("重启:", theme::header()));
        left_spans.push(Span::styled(format!("●{next_text}"), next_style));
    }

    let btn_specs: [(&str, bool); 5] = if compact {
        [
            ("[C]提交", false),
            ("[F9]置", false),
            ("[F5]刷", false),
            ("[?]", false),
            ("[Q]", true),
        ]
    } else {
        [
            ("[C]提交", false),
            ("[F9]重置", false),
            ("[F5]刷新", false),
            ("[?]帮助", false),
            ("[Q]退出", true),
        ]
    };

    let btn_total: u16 = btn_specs
        .iter()
        .map(|(lbl, _)| Line::raw(*lbl).width() as u16 + 1)
        .sum();
    let right_w = btn_total.min(area.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(10), Constraint::Length(right_w)]).areas(area);

    frame.render_widget(
        Paragraph::new(Line::from(left_spans)).style(theme::header()),
        left_area,
    );

    let mut x = right_area.x;
    let mut right_spans = Vec::new();
    let rects = [
        &mut app.regions.commit_btn,
        &mut app.regions.reset_btn,
        &mut app.regions.refresh_btn,
        &mut app.regions.help_btn,
        &mut app.regions.quit_btn,
    ];
    for (idx, (lbl, danger)) in btn_specs.iter().enumerate() {
        let w = (Line::raw(*lbl).width() as u16).min(right_area.right().saturating_sub(x));
        *rects[idx] = Rect::new(x, right_area.y, w, 1);
        x += w + 1;
        right_spans.push(Span::styled(*lbl, theme::button(false, *danger, false)));
        right_spans.push(Span::styled(" ", theme::header()));
    }
    frame.render_widget(
        Paragraph::new(Line::from(right_spans)).style(theme::header()),
        right_area,
    );
}

/// 尚未持有可用快照时在面板内渲染统一加载/失败占位
pub fn render_load_placeholder(frame: &mut Frame, app: &App, area: Rect) -> bool {
    let (msg, style) = match app.phase {
        Phase::LoadFailed => (text::LOAD_FAILED, theme::err()),
        Phase::BackendLost if app.snapshot.filter_state.is_none() => {
            ("后端已断开，请退出重启", theme::err())
        }
        Phase::Loading => (text::LOADING, theme::dim()),
        _ => return false,
    };
    frame.render_widget(
        Paragraph::new(Line::styled(format!(" {msg}"), style)).style(theme::surface()),
        area,
    );
    true
}
