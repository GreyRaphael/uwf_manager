//! 主应用状态机：事件循环、按键路由、后端回包应用。

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::DefaultTerminal;
use ratatui::widgets::{ListState, TableState};

use crate::backend::{BackendCmd, BackendEvent, BackendKind, FullSnapshot};
use crate::model::OverlayRuntime;
use crate::ui;

/// Overlay 轮询周期（与原版 MainForm 的 2 秒后台线程一致）
const OVERLAY_POLL: Duration = Duration::from_secs(2);
/// 操作日志最大保留条数
const MAX_LOG: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Volumes,
    Exclusions,
    Commit,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Dashboard, Tab::Volumes, Tab::Exclusions, Tab::Commit];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Dashboard => "[1] 状态概览",
            Tab::Volumes => "[2] 卷保护管理",
            Tab::Exclusions => "[3] 排除项管理",
            Tab::Commit => "[4] 提交工具",
        }
    }

    fn from_index(index: usize) -> Tab {
        Tab::ALL[index % Tab::ALL.len()]
    }

    pub fn next(self) -> Tab {
        Tab::from_index(self.index() + 1)
    }

    pub fn prev(self) -> Tab {
        Tab::from_index(self.index() + Tab::ALL.len() - 1)
    }
}

/// 排除项页左右栏焦点
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclusionFocus {
    Files,
    Registry,
}

#[derive(Debug)]
pub struct StatusEntry {
    pub time: String,
    pub ok: bool,
    pub text: String,
}

pub struct App {
    pub tab: Tab,
    pub should_quit: bool,
    pub backend_kind: BackendKind,
    /// D6：UWF 功能未安装（由 `BackendEvent::NotInstalled` 置位）
    pub not_installed: bool,
    pub snapshot: FullSnapshot,
    pub overlay_runtime: Option<OverlayRuntime>,
    pub volumes_state: TableState,
    /// 排除项视图：true = 当前生效，false = 下次生效（原版默认下次）
    pub exclusion_session_current: bool,
    pub exclusion_focus: ExclusionFocus,
    pub file_exclusion_state: ListState,
    pub registry_exclusion_state: ListState,
    pub log: VecDeque<StatusEntry>,
    /// 有变更类命令在途（此时忽略后续变更键，显示 ⏳）
    pub pending: bool,
    last_overlay_tick: Instant,
    cmd_tx: Sender<BackendCmd>,
    evt_rx: Receiver<BackendEvent>,
}

impl App {
    pub fn new(
        cmd_tx: Sender<BackendCmd>,
        evt_rx: Receiver<BackendEvent>,
        backend_kind: BackendKind,
    ) -> Self {
        let mut volumes_state = TableState::default();
        volumes_state.select(Some(0));
        let mut file_exclusion_state = ListState::default();
        file_exclusion_state.select(Some(0));
        let mut registry_exclusion_state = ListState::default();
        registry_exclusion_state.select(Some(0));
        Self {
            tab: Tab::Dashboard,
            should_quit: false,
            backend_kind,
            not_installed: false,
            snapshot: FullSnapshot::default(),
            overlay_runtime: None,
            volumes_state,
            exclusion_session_current: false,
            exclusion_focus: ExclusionFocus::Files,
            file_exclusion_state,
            registry_exclusion_state,
            log: VecDeque::new(),
            pending: true, // 启动即有一次 RefreshAll 在途
            last_overlay_tick: Instant::now(),
            cmd_tx,
            evt_rx,
        }
    }

    pub fn send(&self, cmd: BackendCmd) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// 变更类命令：同刻只允许一条在途（PLAN §6.5）
    fn send_mutation(&mut self, cmd: BackendCmd) {
        if !self.pending {
            self.pending = true;
            self.send(cmd);
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        while !self.should_quit {
            self.drain_backend_events();
            terminal.draw(|frame| ui::render(frame, self))?;

            let timeout = OVERLAY_POLL.saturating_sub(self.last_overlay_tick.elapsed());
            if event::poll(timeout).context("读取终端事件失败")?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.on_key(key);
            }

            if self.last_overlay_tick.elapsed() >= OVERLAY_POLL {
                self.last_overlay_tick = Instant::now();
                self.send(BackendCmd::RefreshOverlayRuntime);
            }
        }
        self.send(BackendCmd::Quit);
        Ok(())
    }

    fn drain_backend_events(&mut self) {
        while let Ok(evt) = self.evt_rx.try_recv() {
            self.apply_event(evt);
        }
    }

    fn apply_event(&mut self, evt: BackendEvent) {
        match evt {
            BackendEvent::Snapshot(snapshot) => {
                self.overlay_runtime = snapshot.overlay_runtime;
                self.snapshot = *snapshot;
                self.pending = false;
                self.not_installed = false;
                self.clamp_selections();
                self.log(true, "已刷新 UWF 全部状态");
            }
            BackendEvent::OverlayRuntime(runtime) => {
                self.overlay_runtime = Some(runtime);
            }
            BackendEvent::OpOk(msg) => {
                self.pending = false;
                self.log(true, &msg);
            }
            BackendEvent::OpErr(msg) => {
                self.pending = false;
                self.log(false, &msg);
            }
            BackendEvent::NotInstalled => {
                self.pending = false;
                self.not_installed = true;
                self.log(false, "UWF 功能未安装或未启用");
            }
        }
    }

    fn log(&mut self, ok: bool, text: &str) {
        if self.log.len() >= MAX_LOG {
            self.log.pop_front();
        }
        self.log.push_back(StatusEntry {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            ok,
            text: text.to_string(),
        });
    }

    /// 快照更新后收敛各列表选中位置
    fn clamp_selections(&mut self) {
        let volumes_len = self.snapshot.volumes.len();
        let files_len = self.visible_file_exclusions().len();
        let registry_len = self.visible_registry_exclusions().len();
        clamp(self.volumes_state.selected_mut(), volumes_len);
        clamp(self.file_exclusion_state.selected_mut(), files_len);
        clamp(self.registry_exclusion_state.selected_mut(), registry_len);
    }

    pub fn visible_file_exclusions(&self) -> &[crate::model::FileExclusion] {
        if self.exclusion_session_current {
            &self.snapshot.file_exclusions_current
        } else {
            &self.snapshot.file_exclusions_next
        }
    }

    pub fn visible_registry_exclusions(&self) -> &[String] {
        if self.exclusion_session_current {
            &self.snapshot.registry_exclusions_current
        } else {
            &self.snapshot.registry_exclusions_next
        }
    }

    fn selected_volume(&self) -> Option<&crate::model::VolumeInfo> {
        self.volumes_state
            .selected()
            .and_then(|i| self.snapshot.volumes.get(i))
    }

    fn on_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Tab => self.tab = self.tab.next(),
            KeyCode::BackTab => self.tab = self.tab.prev(),
            KeyCode::Char(c @ '1'..='4') => {
                self.tab = Tab::from_index(c as usize - '1' as usize);
            }
            KeyCode::F(5) => self.send(BackendCmd::RefreshAll),
            _ => match self.tab {
                Tab::Dashboard => self.on_dashboard_key(key),
                Tab::Volumes => self.on_volumes_key(key),
                Tab::Exclusions => self.on_exclusions_key(key),
                Tab::Commit => {}
            },
        }
    }

    fn on_dashboard_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('e') | KeyCode::Char('E') => self.send_mutation(BackendCmd::EnableFilter),
            KeyCode::Char('d') | KeyCode::Char('D') => self.send_mutation(BackendCmd::DisableFilter),
            _ => {}
        }
    }

    fn on_volumes_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => move_selection(self.volumes_state.selected_mut(), self.snapshot.volumes.len(), -1),
            KeyCode::Down => move_selection(self.volumes_state.selected_mut(), self.snapshot.volumes.len(), 1),
            KeyCode::Char('p') | KeyCode::Char('P') => {
                if let Some(drive) = self.selected_volume().map(|v| v.drive_letter.clone()) {
                    self.send_mutation(BackendCmd::Protect(drive));
                }
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                if let Some(drive) = self.selected_volume().map(|v| v.drive_letter.clone()) {
                    self.send_mutation(BackendCmd::Unprotect(drive));
                }
            }
            _ => {}
        }
    }

    fn on_exclusions_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Left | KeyCode::Right => {
                self.exclusion_focus = match self.exclusion_focus {
                    ExclusionFocus::Files => ExclusionFocus::Registry,
                    ExclusionFocus::Registry => ExclusionFocus::Files,
                };
            }
            KeyCode::Up | KeyCode::Down => {
                let delta = if key.code == KeyCode::Up { -1 } else { 1 };
                match self.exclusion_focus {
                    ExclusionFocus::Files => {
                        let len = self.visible_file_exclusions().len();
                        move_selection(self.file_exclusion_state.selected_mut(), len, delta);
                    }
                    ExclusionFocus::Registry => {
                        let len = self.visible_registry_exclusions().len();
                        move_selection(self.registry_exclusion_state.selected_mut(), len, delta);
                    }
                }
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.exclusion_session_current = !self.exclusion_session_current;
                self.clamp_selections();
            }
            _ => {}
        }
    }
}

fn clamp(selected: &mut Option<usize>, len: usize) {
    *selected = match (*selected, len) {
        (_, 0) => None,
        (None, _) => Some(0),
        (Some(i), _) => Some(i.min(len - 1)),
    };
}

fn move_selection(selected: &mut Option<usize>, len: usize, delta: isize) {
    if len == 0 {
        *selected = None;
        return;
    }
    let len = len as isize;
    let current = selected.unwrap_or(0) as isize;
    *selected = Some(((current + delta).rem_euclid(len)) as usize);
}
