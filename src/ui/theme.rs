//! 明亮简洁配色主题：参考经典桌面管理工具的浅色卡片风格。

use ratatui::style::{Color, Modifier, Style};

use crate::model::UsageLevel;

pub const BG_CANVAS: Color = Color::Rgb(240, 243, 248);
pub const BG_SURFACE: Color = Color::Rgb(255, 255, 255);
pub const BG_HEADER: Color = Color::Rgb(224, 234, 248);
pub const BG_DISABLED: Color = Color::Rgb(226, 230, 237);
pub const BG_BUTTON: Color = Color::Rgb(222, 228, 237);
pub const BG_SELECTED: Color = Color::Rgb(210, 228, 252);
pub const BG_ACTIVE_ROW: Color = Color::Rgb(234, 242, 254);
pub const BG_GAUGE_TRACK: Color = Color::Rgb(222, 228, 236);

pub const FG_TEXT: Color = Color::Rgb(24, 30, 42);
pub const FG_MUTED: Color = Color::Rgb(92, 102, 118);
pub const FG_DISABLED: Color = Color::Rgb(136, 144, 158);
pub const FG_REGISTRY: Color = Color::Rgb(29, 78, 172);

pub const ACCENT_BLUE: Color = Color::Rgb(0, 102, 204);
pub const BORDER_NORMAL: Color = Color::Rgb(176, 185, 198);

pub const STATUS_OK: Color = Color::Rgb(21, 128, 61);
pub const STATUS_WARN: Color = Color::Rgb(180, 83, 9);
pub const STATUS_ERR: Color = Color::Rgb(185, 28, 28);

pub fn canvas() -> Style {
    Style::default().bg(BG_CANVAS).fg(FG_TEXT)
}

pub fn surface() -> Style {
    Style::default().bg(BG_SURFACE).fg(FG_TEXT)
}

pub fn header() -> Style {
    Style::default().bg(BG_HEADER).fg(FG_TEXT)
}

pub fn header_title() -> Style {
    Style::default()
        .bg(BG_HEADER)
        .fg(ACCENT_BLUE)
        .add_modifier(Modifier::BOLD)
}

pub fn ok() -> Style {
    Style::default().fg(STATUS_OK).add_modifier(Modifier::BOLD)
}

pub fn err() -> Style {
    Style::default().fg(STATUS_ERR).add_modifier(Modifier::BOLD)
}

pub fn warn() -> Style {
    Style::default()
        .fg(STATUS_WARN)
        .add_modifier(Modifier::BOLD)
}

pub fn dim() -> Style {
    Style::default().fg(FG_MUTED)
}

pub fn text() -> Style {
    Style::default().fg(FG_TEXT)
}

pub fn registry() -> Style {
    Style::default().fg(FG_REGISTRY)
}

pub fn title() -> Style {
    Style::default()
        .fg(ACCENT_BLUE)
        .add_modifier(Modifier::BOLD)
}

pub fn selected() -> Style {
    Style::default()
        .bg(BG_SELECTED)
        .fg(FG_TEXT)
        .add_modifier(Modifier::BOLD)
}

pub fn active_row() -> Style {
    Style::default().bg(BG_ACTIVE_ROW).fg(FG_TEXT)
}

pub fn disabled() -> Style {
    Style::default().bg(BG_DISABLED).fg(FG_DISABLED)
}

pub fn input(focused: bool, is_disabled: bool) -> Style {
    if is_disabled {
        disabled()
    } else if focused {
        Style::default()
            .bg(BG_SURFACE)
            .fg(ACCENT_BLUE)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(BG_SURFACE).fg(FG_TEXT)
    }
}

pub fn button(focused: bool, danger: bool, is_disabled: bool) -> Style {
    if is_disabled {
        disabled()
    } else if focused && danger {
        Style::default()
            .bg(STATUS_ERR)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else if focused {
        Style::default()
            .bg(ACCENT_BLUE)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else if danger {
        Style::default().bg(BG_BUTTON).fg(STATUS_ERR)
    } else {
        Style::default().bg(BG_BUTTON).fg(FG_TEXT)
    }
}

pub fn gauge(level: UsageLevel) -> Style {
    let fg = match level {
        UsageLevel::Normal => STATUS_OK,
        UsageLevel::Warning => STATUS_WARN,
        UsageLevel::Critical => STATUS_ERR,
    };
    Style::default().fg(fg).bg(BG_GAUGE_TRACK)
}

pub fn focus_border(focused: bool) -> Style {
    if focused {
        Style::default().fg(ACCENT_BLUE)
    } else {
        Style::default().fg(BORDER_NORMAL)
    }
}

pub fn focus_title(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(ACCENT_BLUE)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(FG_TEXT).add_modifier(Modifier::BOLD)
    }
}
