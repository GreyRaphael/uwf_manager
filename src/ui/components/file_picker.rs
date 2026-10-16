//! TUI 模态文件/目录浏览器。
//!
//! 跨平台：Linux 从 `/` 起导航；Windows 以盘符列表为根（`A:`-`Z:` 探测存在性）。

use std::path::PathBuf;
#[cfg(windows)]
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph};
use ratatui::layout::{Constraint, Layout};

use crate::ui::theme;

use super::centered_rect;

/// 选择模式：File 仅可选文件；Any 文件与目录皆可（Space 选中目录）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    File,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerAction {
    CommitFile,
    CommitFileDeletion,
    AddFileExclusion,
    ImportExclusions,
}

#[derive(Debug, Clone)]
pub struct PickerEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

pub struct FilePickerState {
    pub mode: PickerMode,
    pub action: PickerAction,
    /// 当前目录；`None` 表示盘符列表（仅 Windows）
    pub cwd: Option<PathBuf>,
    pub entries: Vec<PickerEntry>,
    pub selected: usize,
    pub error: Option<String>,
}

pub enum PickerOutcome {
    Stay,
    Cancel,
    Pick(PickerAction, PathBuf),
}

impl FilePickerState {
    pub fn new(mode: PickerMode, action: PickerAction) -> Self {
        let cwd = std::env::current_dir()
            .ok()
            .or_else(|| Some(PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" })));
        let mut state = Self {
            mode,
            action,
            cwd,
            entries: Vec::new(),
            selected: 0,
            error: None,
        };
        state.reload();
        state
    }

    /// 重新读取当前目录：目录在前、文件在后，按名称（小写）排序
    fn reload(&mut self) {
        self.error = None;
        self.entries.clear();
        self.selected = 0;
        let Some(cwd) = &self.cwd else {
            #[cfg(windows)]
            {
                self.entries = list_drives();
            }
            return;
        };
        match std::fs::read_dir(cwd) {
            Ok(read_dir) => {
                for entry in read_dir.flatten() {
                    let path = entry.path();
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let name = entry.file_name().to_string_lossy().into_owned();
                    self.entries.push(PickerEntry { name, path, is_dir });
                }
                self.entries.sort_by(|a, b| {
                    b.is_dir
                        .cmp(&a.is_dir)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
            Err(e) => {
                self.error = Some(format!("无法读取目录: {e}"));
            }
        }
    }

    fn enter(&mut self, path: PathBuf) {
        self.cwd = Some(path);
        self.reload();
    }

    fn go_parent(&mut self) {
        let Some(cwd) = &self.cwd else { return };
        match cwd.parent() {
            Some(parent) => {
                let parent = parent.to_path_buf();
                self.cwd = Some(parent);
            }
            None => {
                // Windows：卷的上一级 = 盘符列表
                if cfg!(windows) {
                    self.cwd = None;
                }
            }
        }
        self.reload();
    }

    fn pick(&self, entry: &PickerEntry) -> Option<PickerOutcome> {
        match (self.mode, entry.is_dir) {
            (PickerMode::File, false) | (PickerMode::Any, _) => {
                Some(PickerOutcome::Pick(self.action, entry.path.clone()))
            }
            (PickerMode::File, true) => None,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> PickerOutcome {
        match key.code {
            KeyCode::Esc => PickerOutcome::Cancel,
            KeyCode::Up => {
                if !self.entries.is_empty() {
                    self.selected = (self.selected + self.entries.len() - 1) % self.entries.len();
                }
                PickerOutcome::Stay
            }
            KeyCode::Down => {
                if !self.entries.is_empty() {
                    self.selected = (self.selected + 1) % self.entries.len();
                }
                PickerOutcome::Stay
            }
            KeyCode::Backspace | KeyCode::Left => {
                self.go_parent();
                PickerOutcome::Stay
            }
            KeyCode::Enter => {
                let Some(entry) = self.entries.get(self.selected) else {
                    return PickerOutcome::Stay;
                };
                if entry.is_dir {
                    let path = entry.path.clone();
                    self.enter(path);
                    PickerOutcome::Stay
                } else {
                    self.pick(entry).unwrap_or(PickerOutcome::Stay)
                }
            }
            KeyCode::Char(' ') => {
                let Some(entry) = self.entries.get(self.selected) else {
                    return PickerOutcome::Stay;
                };
                self.pick(entry).unwrap_or(PickerOutcome::Stay)
            }
            _ => PickerOutcome::Stay,
        }
    }
}

/// Windows 盘符列表（A:-Z: 探测存在性）
#[cfg(windows)]
fn list_drives() -> Vec<PickerEntry> {
    (b'A'..=b'Z')
        .map(|letter| format!("{}:\\", letter as char))
        .filter(|drive| Path::new(drive).exists())
        .map(|drive| PickerEntry {
            name: drive.clone(),
            path: PathBuf::from(drive),
            is_dir: true,
        })
        .collect()
}

pub fn render(frame: &mut Frame, state: &FilePickerState, area: Rect) {
    let rect = centered_rect(72, 70, area);
    let cwd_label = match &state.cwd {
        Some(cwd) => cwd.display().to_string(),
        None => "盘符列表".to_string(),
    };
    let block = Block::bordered().title(format!(" 文件选择 - {cwd_label} "));
    frame.render_widget(Clear, rect);

    let [list_area, hint_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(block.inner(rect));
    frame.render_widget(block, rect);

    let items: Vec<ListItem> = if state.entries.is_empty() {
        vec![ListItem::new(Line::styled(
            state.error.clone().unwrap_or_else(|| "（空目录）".to_string()),
            theme::dim(),
        ))]
    } else {
        state
            .entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let sep = std::path::MAIN_SEPARATOR;
                let line = if entry.is_dir {
                    Line::from(vec![
                        Span::styled(if i == state.selected { "▶ " } else { "  " }, theme::title()),
                        Span::styled(format!("{}{sep}", entry.name), theme::title()),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled(if i == state.selected { "▶ " } else { "  " }, theme::title()),
                        Span::raw(&entry.name),
                    ])
                };
                let style = if i == state.selected {
                    theme::selected()
                } else {
                    ratatui::style::Style::default()
                };
                ListItem::new(line).style(style)
            })
            .collect()
    };
    frame.render_widget(List::new(items), list_area);

    let hint = match state.mode {
        PickerMode::File => "[Enter] 进入/选择  [Backspace] 上级  [Esc] 取消",
        PickerMode::Any => "[Enter] 进入/选择  [Space] 选中此项  [Backspace] 上级  [Esc] 取消",
    };
    frame.render_widget(Paragraph::new(Line::styled(hint, theme::dim())), hint_area);
}
