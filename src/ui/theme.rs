//! 颜色主题与样式。

use ratatui::style::{Color, Modifier, Style};

use crate::model::UsageLevel;

pub fn ok() -> Style {
    Style::default().fg(Color::Green)
}

pub fn err() -> Style {
    Style::default().fg(Color::Red)
}

pub fn warn() -> Style {
    Style::default().fg(Color::Yellow)
}

pub fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn title() -> Style {
    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
}

pub fn selected() -> Style {
    Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
}

pub fn gauge(level: UsageLevel) -> Style {
    match level {
        UsageLevel::Normal => Style::default().fg(Color::Green),
        UsageLevel::Warning => Style::default().fg(Color::Yellow),
        UsageLevel::Critical => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

/// 焦点栏边框色：焦点青色，非焦点深灰
pub fn focus_border(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}
