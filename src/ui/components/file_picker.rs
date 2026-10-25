//! TUI 模态文件/目录浏览器。
//!
//! 跨平台：Linux 从 `/` 起导航；Windows 以盘符列表为根（`A:`-`Z:` 探测存在性）。

#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph};

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
    pub list_state: ListState,
    /// 可视高度（render 时更新，供 PageUp/PageDown 使用）
    pub page_size: usize,
    pub error: Option<String>,
    #[cfg(windows)]
    drive_rx: Option<std::sync::mpsc::Receiver<Vec<PickerEntry>>>,
    #[cfg(windows)]
    loading_drives: bool,
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
            list_state: ListState::default(),
            page_size: 10,
            error: None,
            #[cfg(windows)]
            drive_rx: None,
            #[cfg(windows)]
            loading_drives: false,
        };
        state.reload();
        state
    }

    fn sync_list_state(&mut self) {
        if self.entries.is_empty() {
            self.list_state.select(None);
        } else {
            self.selected = self.selected.min(self.entries.len() - 1);
            self.list_state.select(Some(self.selected));
        }
    }

    /// 重新读取当前目录：目录在前、文件在后，按名称（小写）排序
    fn reload(&mut self) {
        self.error = None;
        self.entries.clear();
        self.selected = 0;
        let Some(cwd) = &self.cwd else {
            #[cfg(windows)]
            {
                // 盘符探测放到后台线程，避免网络盘/软驱阻塞 UI。
                self.start_drive_probe();
                self.poll_drives();
            }
            self.sync_list_state();
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
        self.sync_list_state();
    }

    #[cfg(windows)]
    fn start_drive_probe(&mut self) {
        if self.drive_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(list_drives());
        });
        self.drive_rx = Some(rx);
        self.loading_drives = true;
    }

    #[cfg(windows)]
    fn poll_drives(&mut self) {
        let Some(rx) = &self.drive_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(drives) => {
                self.entries = drives;
                self.drive_rx = None;
                self.loading_drives = false;
                self.selected = 0;
                self.sync_list_state();
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.drive_rx = None;
                self.loading_drives = false;
                self.error = Some("盘符探测线程异常退出".into());
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
                self.sync_list_state();
                PickerOutcome::Stay
            }
            KeyCode::Down => {
                if !self.entries.is_empty() {
                    self.selected = (self.selected + 1) % self.entries.len();
                }
                self.sync_list_state();
                PickerOutcome::Stay
            }
            KeyCode::PageUp => {
                self.selected = self.selected.saturating_sub(self.page_size.max(1));
                self.sync_list_state();
                PickerOutcome::Stay
            }
            KeyCode::PageDown => {
                if !self.entries.is_empty() {
                    self.selected =
                        (self.selected + self.page_size.max(1)).min(self.entries.len() - 1);
                }
                self.sync_list_state();
                PickerOutcome::Stay
            }
            KeyCode::Home => {
                self.selected = 0;
                self.sync_list_state();
                PickerOutcome::Stay
            }
            KeyCode::End => {
                self.selected = self.entries.len().saturating_sub(1);
                self.sync_list_state();
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

fn entry_label(entry: &PickerEntry) -> String {
    if entry.is_dir && !entry.name.ends_with(std::path::MAIN_SEPARATOR) {
        format!("{}{}", entry.name, std::path::MAIN_SEPARATOR)
    } else {
        entry.name.clone()
    }
}

pub fn render(frame: &mut Frame, state: &mut FilePickerState, area: Rect) {
    #[cfg(windows)]
    if state.cwd.is_none() {
        state.poll_drives();
    }
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

    state.page_size = list_area.height.max(1) as usize;
    state.sync_list_state();

    let items: Vec<ListItem> = if state.entries.is_empty() {
        #[cfg(windows)]
        let empty = if state.loading_drives {
            "正在检测盘符…".to_string()
        } else {
            state
                .error
                .clone()
                .unwrap_or_else(|| "（空目录）".to_string())
        };
        #[cfg(not(windows))]
        let empty = state
            .error
            .clone()
            .unwrap_or_else(|| "（空目录）".to_string());
        vec![ListItem::new(Line::styled(empty, theme::dim()))]
    } else {
        state
            .entries
            .iter()
            .map(|entry| {
                let style = if entry.is_dir {
                    theme::title()
                } else {
                    ratatui::style::Style::default()
                };
                ListItem::new(Line::from(Span::styled(entry_label(entry), style)))
            })
            .collect()
    };
    let list = List::new(items)
        .highlight_style(theme::selected())
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, list_area, &mut state.list_state);

    let hint = match state.mode {
        PickerMode::File => "[Enter] 进入/选择  [Backspace] 上级  [Esc] 取消",
        PickerMode::Any => "[Enter] 进入/选择  [Space] 选中此项  [Backspace] 上级  [Esc] 取消",
    };
    frame.render_widget(Paragraph::new(Line::styled(hint, theme::dim())), hint_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn state_with_entries(n: usize) -> FilePickerState {
        let mut state = FilePickerState::new(PickerMode::File, PickerAction::CommitFile);
        state.entries = (0..n)
            .map(|i| PickerEntry {
                name: format!("f{i:02}"),
                path: PathBuf::from(format!("/tmp/f{i:02}")),
                is_dir: false,
            })
            .collect();
        state.selected = 0;
        state.page_size = 10;
        state.sync_list_state();
        state
    }

    #[test]
    fn page_home_end_move_selection_and_list_state() {
        let mut state = state_with_entries(30);
        state.on_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(state.selected, 10);
        assert_eq!(state.list_state.selected(), Some(10));

        state.on_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(state.selected, 20);

        state.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(state.selected, 29);

        state.on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(state.selected, 0);
        assert_eq!(state.list_state.selected(), Some(0));
    }

    #[test]
    fn directory_label_has_exactly_one_separator() {
        let separator = std::path::MAIN_SEPARATOR;
        let expected = format!("C:{separator}");
        // 同时覆盖普通目录名和 list_drives 已带尾部分隔符的真实盘符形态。
        for name in ["C:".to_string(), expected.clone()] {
            let entry = PickerEntry {
                name,
                path: PathBuf::from("C:\\"),
                is_dir: true,
            };
            assert_eq!(entry_label(&entry), expected);
        }
    }
}
