//! TUI 模态文件/目录浏览器。
//!
//! 跨平台：Linux 从 `/` 起导航；Windows 以盘符列表为根（`A:`-`Z:` 探测存在性）。

#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};

use crate::ui::theme;

use super::{centered_rect, clear_surface, is_left_click};

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
    pub list_state: ListState,
    /// 可视高度（render 时更新，供 PageUp/PageDown 使用）
    pub page_size: usize,
    pub error: Option<String>,
    list_area: Rect,
    enter_btn: Rect,
    pick_btn: Rect,
    parent_btn: Rect,
    cancel_btn: Rect,
    #[cfg(windows)]
    drive_rx: Option<std::sync::mpsc::Receiver<Vec<PickerEntry>>>,
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
            list_state: ListState::default(),
            page_size: 10,
            error: None,
            list_area: Rect::default(),
            enter_btn: Rect::default(),
            pick_btn: Rect::default(),
            parent_btn: Rect::default(),
            cancel_btn: Rect::default(),
            #[cfg(windows)]
            drive_rx: None,
        };
        state.reload();
        state
    }

    fn select(&mut self, index: Option<usize>) {
        if self.entries.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state
                .select(Some(index.unwrap_or(0).min(self.entries.len() - 1)));
        }
    }

    pub fn is_polling(&self) -> bool {
        #[cfg(windows)]
        {
            self.drive_rx.is_some()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// 推进异步盘符探测；状态有更新返回 `true` 以便事件循环及时重绘
    pub fn poll(&mut self) -> bool {
        #[cfg(windows)]
        {
            let Some(rx) = &self.drive_rx else {
                return false;
            };
            match rx.try_recv() {
                Ok(drives) => {
                    self.entries = drives;
                    self.drive_rx = None;
                    self.select(Some(0));
                    true
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => false,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.drive_rx = None;
                    self.error = Some("盘符探测线程异常退出".into());
                    true
                }
            }
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// 重新读取当前目录：目录在前、文件在后，按名称（小写）排序
    fn reload(&mut self) {
        self.error = None;
        self.entries.clear();
        let Some(cwd) = &self.cwd else {
            #[cfg(windows)]
            {
                self.start_drive_probe();
                let _ = self.poll();
            }
            self.select(Some(0));
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
        self.select(Some(0));
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
    }

    fn enter(&mut self, path: PathBuf) {
        self.cwd = Some(path);
        self.reload();
    }

    fn go_parent(&mut self) {
        let Some(cwd) = &self.cwd else { return };
        match cwd.parent() {
            Some(parent) => {
                self.cwd = Some(parent.to_path_buf());
            }
            None => {
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

    fn activate_selected(&mut self) -> PickerOutcome {
        let Some(entry) = self
            .list_state
            .selected()
            .and_then(|i| self.entries.get(i))
            .cloned()
        else {
            return PickerOutcome::Stay;
        };
        if entry.is_dir {
            self.enter(entry.path);
            PickerOutcome::Stay
        } else {
            self.pick(&entry).unwrap_or(PickerOutcome::Stay)
        }
    }

    fn pick_selected(&self) -> PickerOutcome {
        let Some(entry) = self.list_state.selected().and_then(|i| self.entries.get(i)) else {
            return PickerOutcome::Stay;
        };
        self.pick(entry).unwrap_or(PickerOutcome::Stay)
    }

    pub fn on_key(&mut self, key: KeyEvent) -> PickerOutcome {
        let cur = self.list_state.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc => PickerOutcome::Cancel,
            KeyCode::Up => {
                if !self.entries.is_empty() {
                    self.select(Some((cur + self.entries.len() - 1) % self.entries.len()));
                }
                PickerOutcome::Stay
            }
            KeyCode::Down => {
                if !self.entries.is_empty() {
                    self.select(Some((cur + 1) % self.entries.len()));
                }
                PickerOutcome::Stay
            }
            KeyCode::PageUp => {
                self.select(Some(cur.saturating_sub(self.page_size.max(1))));
                PickerOutcome::Stay
            }
            KeyCode::PageDown => {
                self.select(Some(cur + self.page_size.max(1)));
                PickerOutcome::Stay
            }
            KeyCode::Home => {
                self.select(Some(0));
                PickerOutcome::Stay
            }
            KeyCode::End => {
                self.select(Some(self.entries.len().saturating_sub(1)));
                PickerOutcome::Stay
            }
            KeyCode::Backspace | KeyCode::Left => {
                self.go_parent();
                PickerOutcome::Stay
            }
            KeyCode::Enter => self.activate_selected(),
            KeyCode::Char(' ') => self.pick_selected(),
            _ => PickerOutcome::Stay,
        }
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) -> PickerOutcome {
        match mouse.kind {
            MouseEventKind::ScrollUp if !self.entries.is_empty() => {
                let cur = self.list_state.selected().unwrap_or(0);
                self.select(Some(cur.saturating_sub(1)));
                return PickerOutcome::Stay;
            }
            MouseEventKind::ScrollDown if !self.entries.is_empty() => {
                let cur = self.list_state.selected().unwrap_or(0);
                self.select(Some((cur + 1).min(self.entries.len() - 1)));
                return PickerOutcome::Stay;
            }
            _ => {}
        }
        let Some(pos) = is_left_click(&mouse) else {
            return PickerOutcome::Stay;
        };
        if self.cancel_btn.contains(pos) {
            return PickerOutcome::Cancel;
        }
        if self.parent_btn.contains(pos) {
            self.go_parent();
            return PickerOutcome::Stay;
        }
        if self.enter_btn.contains(pos) {
            return self.activate_selected();
        }
        if self.mode == PickerMode::Any && self.pick_btn.contains(pos) {
            return self.pick_selected();
        }
        if self.list_area.contains(pos) && !self.entries.is_empty() {
            let row = (pos.y - self.list_area.y) as usize + self.list_state.offset();
            if row < self.entries.len() {
                let already = self.list_state.selected() == Some(row);
                self.select(Some(row));
                if already {
                    return self.activate_selected();
                }
            }
        }
        PickerOutcome::Stay
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
    let rect = centered_rect(78, 72, area);
    clear_surface(frame, rect);
    let cwd_label = match &state.cwd {
        Some(cwd) => cwd.display().to_string(),
        None => "盘符列表".to_string(),
    };
    let block = Block::bordered()
        .title(format!(" 文件选择 - {cwd_label} "))
        .title_style(theme::title())
        .border_style(theme::focus_border(true))
        .style(theme::surface());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let [list_area, btn_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(inner);

    state.list_area = list_area;
    state.page_size = list_area.height.max(1) as usize;

    let items: Vec<ListItem> = if state.entries.is_empty() {
        let empty = if state.is_polling() {
            "正在检测盘符…".to_string()
        } else {
            state
                .error
                .clone()
                .unwrap_or_else(|| "（空目录）".to_string())
        };
        vec![ListItem::new(Line::styled(empty, theme::dim()))]
    } else {
        state
            .entries
            .iter()
            .map(|entry| {
                let style = if entry.is_dir {
                    theme::title()
                } else {
                    theme::text()
                };
                ListItem::new(Line::from(Span::styled(entry_label(entry), style)))
            })
            .collect()
    };
    let list = List::new(items)
        .style(theme::surface())
        .highlight_style(theme::selected())
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, list_area, &mut state.list_state);

    let mut x = btn_area.x;
    let enter_lbl = " 打开(Enter) ";
    let pick_lbl = " 选中(Space) ";
    let parent_lbl = " 上级(Bksp) ";
    let cancel_lbl = " 取消(Esc) ";

    let enter_w = Line::raw(enter_lbl).width() as u16;
    let pick_w = Line::raw(pick_lbl).width() as u16;
    let parent_w = Line::raw(parent_lbl).width() as u16;
    let cancel_w = Line::raw(cancel_lbl).width() as u16;

    state.enter_btn = Rect::new(x, btn_area.y, enter_w.min(btn_area.width), 1);
    x += enter_w + 1;
    let mut spans = vec![
        Span::styled(enter_lbl, theme::button(true, false, false)),
        Span::raw(" "),
    ];
    if state.mode == PickerMode::Any {
        state.pick_btn = Rect::new(
            x,
            btn_area.y,
            pick_w.min(btn_area.right().saturating_sub(x)),
            1,
        );
        x += pick_w + 1;
        spans.push(Span::styled(pick_lbl, theme::button(false, false, false)));
        spans.push(Span::raw(" "));
    } else {
        state.pick_btn = Rect::default();
    }
    state.parent_btn = Rect::new(
        x,
        btn_area.y,
        parent_w.min(btn_area.right().saturating_sub(x)),
        1,
    );
    x += parent_w + 1;
    spans.push(Span::styled(parent_lbl, theme::button(false, false, false)));
    spans.push(Span::raw(" "));
    state.cancel_btn = Rect::new(
        x,
        btn_area.y,
        cancel_w.min(btn_area.right().saturating_sub(x)),
        1,
    );
    spans.push(Span::styled(cancel_lbl, theme::button(false, false, false)));

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme::surface()),
        btn_area,
    );
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
        state.page_size = 10;
        state.select(Some(0));
        state
    }

    #[test]
    fn page_home_end_move_selection_and_list_state() {
        let mut state = state_with_entries(30);
        state.on_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(state.list_state.selected(), Some(10));

        state.on_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(state.list_state.selected(), Some(20));

        state.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(state.list_state.selected(), Some(29));

        state.on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(state.list_state.selected(), Some(0));
    }

    #[test]
    fn directory_label_has_exactly_one_separator() {
        let separator = std::path::MAIN_SEPARATOR;
        let expected = format!("C:{separator}");
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
