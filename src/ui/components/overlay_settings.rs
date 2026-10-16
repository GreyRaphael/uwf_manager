//! Overlay 配置弹窗：类型/上限/预警/临界四项。
//!
//! R1：筛选器当前启用时，类型与上限两行锁定（置灰且不可选中）。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::backend::FullSnapshot;
use crate::model::OverlayType;
use crate::ui::theme;

use super::centered_rect;

pub struct OverlaySettingsState {
    pub overlay_type: OverlayType,
    pub max_size: String,
    pub warning: String,
    pub critical: String,
    /// 当前选中行：0=类型 1=上限 2=预警 3=临界
    pub selected: usize,
    /// R1：筛选器当前启用时锁定类型/上限
    pub lock_type_size: bool,
    pub error: Option<String>,
}

pub enum OverlayOutcome {
    Stay,
    Cancel,
    Submit {
        overlay_type: OverlayType,
        max_mb: u32,
        warning_mb: u32,
        critical_mb: u32,
    },
}

impl OverlaySettingsState {
    /// 从快照预填：类型/上限取下次会话配置（可改的是下次），阈值取运行时值
    pub fn from_snapshot(snapshot: &FullSnapshot) -> Self {
        let (overlay_type, max_size) = snapshot
            .overlay_config_next
            .map(|c| (c.overlay_type, c.max_size_mb.to_string()))
            .unwrap_or((OverlayType::Ram, String::new()));
        let (warning, critical) = snapshot
            .overlay_runtime
            .map(|r| (r.warning_threshold_mb.to_string(), r.critical_threshold_mb.to_string()))
            .unwrap_or_default();
        let lock_type_size = snapshot
            .filter_state
            .map(|f| f.current_enabled)
            .unwrap_or(false);
        Self {
            overlay_type,
            max_size,
            warning,
            critical,
            selected: if lock_type_size { 2 } else { 0 },
            lock_type_size,
            error: None,
        }
    }

    fn row_enabled(&self, row: usize) -> bool {
        !(self.lock_type_size && row <= 1)
    }

    fn move_selection(&mut self, delta: isize) {
        let mut next = self.selected as isize;
        for _ in 0..4 {
            next = (next + delta).rem_euclid(4);
            if self.row_enabled(next as usize) {
                self.selected = next as usize;
                return;
            }
        }
    }

    fn text_field_mut(&mut self) -> Option<&mut String> {
        match self.selected {
            1 if self.row_enabled(1) => Some(&mut self.max_size),
            2 => Some(&mut self.warning),
            3 => Some(&mut self.critical),
            _ => None,
        }
    }

    fn submit(&mut self) -> OverlayOutcome {
        let parse = |s: &str, what: &str| -> std::result::Result<u32, String> {
            s.parse::<u32>()
                .map_err(|_| format!("{what}须为 0-4294967295 的整数"))
        };
        match (|| {
            Ok(OverlayOutcome::Submit {
                overlay_type: self.overlay_type,
                max_mb: parse(&self.max_size, "上限")?,
                warning_mb: parse(&self.warning, "预警阈值")?,
                critical_mb: parse(&self.critical, "临界阈值")?,
            })
        })() {
            Ok(outcome) => outcome,
            Err(msg) => {
                self.error = Some(msg);
                OverlayOutcome::Stay
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> OverlayOutcome {
        self.error = None;
        match key.code {
            KeyCode::Esc => OverlayOutcome::Cancel,
            KeyCode::Enter => self.submit(),
            KeyCode::Up => {
                self.move_selection(-1);
                OverlayOutcome::Stay
            }
            KeyCode::Down => {
                self.move_selection(1);
                OverlayOutcome::Stay
            }
            KeyCode::Left | KeyCode::Right
                if self.selected == 0 && self.row_enabled(0) =>
            {
                self.overlay_type = match self.overlay_type {
                    OverlayType::Ram => OverlayType::Disk,
                    OverlayType::Disk => OverlayType::Ram,
                };
                OverlayOutcome::Stay
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if let Some(field) = self.text_field_mut() {
                    field.push(c);
                }
                OverlayOutcome::Stay
            }
            KeyCode::Backspace => {
                if let Some(field) = self.text_field_mut() {
                    field.pop();
                }
                OverlayOutcome::Stay
            }
            _ => OverlayOutcome::Stay,
        }
    }
}

pub fn render(frame: &mut Frame, state: &OverlaySettingsState, area: Rect) {
    let rect = centered_rect(52, 44, area);
    let block = Block::bordered().title(" Overlay 配置 ");
    frame.render_widget(Clear, rect);

    let locked = state.lock_type_size;
    let row = |index: usize, label: &str, value: String, locked_row: bool| -> Line<'static> {
        let cursor = if state.selected == index { "▶ " } else { "  " };
        let style = if locked_row {
            theme::dim()
        } else if state.selected == index {
            theme::title()
        } else {
            ratatui::style::Style::default()
        };
        let lock_hint = if locked_row { "（已锁定）" } else { "" };
        Line::from(vec![
            Span::raw(cursor),
            Span::styled(format!("{label:<8}",), style),
            Span::styled(value, style),
            Span::styled(lock_hint, theme::dim()),
        ])
    };

    let type_value = if locked {
        state.overlay_type.label().to_string()
    } else {
        format!("← {} →", state.overlay_type.label())
    };
    let mut lines = vec![
        row(0, "类型", type_value, locked),
        row(1, "上限(MB)", state.max_size.clone(), locked),
        row(2, "预警(MB)", state.warning.clone(), false),
        row(3, "临界(MB)", state.critical.clone(), false),
        Line::from(""),
    ];
    if locked {
        lines.push(Line::styled("筛选器当前已启用，仅阈值可改（R1）", theme::warn()));
    }
    if let Some(error) = &state.error {
        lines.push(Line::styled(error.clone(), theme::err()));
    }
    lines.push(Line::styled("[↑/↓] 选择  [←/→] 切换类型  [Enter] 保存  [Esc] 取消", theme::dim()));
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}
