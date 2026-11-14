//! 主应用状态机：事件循环、按键路由、模态管理、后端回包应用。

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::DefaultTerminal;
use ratatui::widgets::{ListState, TableState};

use crate::backend::{
    BackendCmd, BackendCommand, BackendEvent, BackendKind, FullSnapshot, Outcome, RequestId,
};
#[cfg(test)]
use crate::model::OverlayRuntime;
use crate::ui::components::confirm::{ConfirmAction, ConfirmOutcome, ConfirmState};
use crate::ui::components::file_picker::{
    FilePickerState, PickerAction, PickerMode, PickerOutcome,
};
use crate::ui::components::input_dialog::{InputAction, InputOutcome, InputState};
use crate::ui::components::overlay_settings::{OverlayOutcome, OverlaySettingsState};

/// UI 线程统一事件：终端输入与后端回包走同一个 channel，
/// 后端回包可立即唤醒 `recv_timeout`，不再依赖 50ms 短轮询。
pub enum UiEvent {
    Terminal(Event),
    Backend(BackendEvent),
}

impl From<BackendEvent> for UiEvent {
    fn from(evt: BackendEvent) -> Self {
        Self::Backend(evt)
    }
}

/// Overlay 轮询周期（与原版 MainForm 的 2 秒后台线程一致）
const OVERLAY_POLL: Duration = Duration::from_secs(2);

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
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
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

/// 当前打开的模态弹窗
pub enum Modal {
    OverlaySettings(OverlaySettingsState),
    Input(InputState),
    FilePicker(FilePickerState),
    Confirm(ConfirmState),
    Help,
}

/// 已确认、等待后端回包的电源动作（决定回包后是退出还是继续演示）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerAction {
    Restart,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Ok,
    Warn,
    Err,
}

#[derive(Debug)]
pub struct StatusEntry {
    pub time: String,
    pub level: LogLevel,
    pub text: String,
}

/// 应用生命周期阶段（单源表达，避免多个布尔状态出现非法组合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 启动加载中，或失败/未安装状态下按 `[F5]` 重新检测中（尚无可用快照）
    Loading,
    /// 无有效快照时的加载失败（可按 `[F5]` 重试）
    LoadFailed,
    /// D6：UWF 功能未安装或未启用
    NotInstalled,
    /// 已持有至少一份有效快照，正常工作态
    Ready,
    /// 后端线程已退出（channel 断开或 panic），仅允许退出或查看帮助
    BackendLost,
}

pub struct App {
    pub tab: Tab,
    pub should_quit: bool,
    pub backend_kind: BackendKind,
    pub phase: Phase,
    pub snapshot: FullSnapshot,
    pub volumes_state: TableState,
    /// 排除项视图：true = 当前生效，false = 下次生效（原版默认下次）
    pub exclusion_session_current: bool,
    pub exclusion_focus: ExclusionFocus,
    pub file_exclusion_state: ListState,
    pub registry_exclusion_state: ListState,
    /// 刷新/排序后用于恢复选中身份的稳定 key
    selected_file_key: Option<(String, String)>,
    selected_registry_key: Option<String>,
    pub modal: Option<Modal>,
    pub last_log: Option<StatusEntry>,
    /// 当前在途的变更请求；只有匹配 id 的 Finished / NotInstalled 才能清除
    pub mutation: Option<RequestId>,
    /// 电源动作绑定到具体请求 id，避免被无关回包误触发
    power_action: Option<(RequestId, PowerAction)>,
    next_request_id: RequestId,
    last_overlay_tick: Instant,
    cmd_tx: Sender<BackendCommand>,
    ui_rx: Receiver<UiEvent>,
}

impl App {
    pub fn new(
        cmd_tx: Sender<BackendCommand>,
        ui_rx: Receiver<UiEvent>,
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
            phase: Phase::Loading,
            snapshot: FullSnapshot::default(),
            volumes_state,
            exclusion_session_current: false,
            exclusion_focus: ExclusionFocus::Files,
            file_exclusion_state,
            registry_exclusion_state,
            selected_file_key: None,
            selected_registry_key: None,
            last_log: None,
            mutation: None,
            power_action: None,
            next_request_id: 1,
            last_overlay_tick: Instant::now(),
            modal: None,
            cmd_tx,
            ui_rx,
        }
    }

    /// 是否应显示“执行中”：初始/重试加载中或有变更在途。
    pub fn is_busy(&self) -> bool {
        self.phase == Phase::Loading || self.is_pending()
    }

    /// 是否有变更在途（退出确认、变更键拦截只看这个）。
    pub fn is_pending(&self) -> bool {
        self.mutation.is_some()
    }

    fn next_request_id(&mut self) -> RequestId {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        id
    }

    fn send(&self, command: BackendCommand) -> bool {
        self.cmd_tx.send(command).is_ok()
    }

    /// 后端线程不可用：进入错误态、关闭可能覆盖提示的弹窗并清除在途锁。
    fn mark_backend_lost(&mut self, message: &str) {
        let first = self.phase != Phase::BackendLost;
        self.phase = Phase::BackendLost;
        self.modal = None;
        self.mutation = None;
        self.power_action = None;
        if first {
            self.log(false, message);
        }
    }

    /// F5 / 启动首次加载 / 未安装重检：查询不占用 mutation 锁。
    pub fn refresh_all(&mut self) {
        if matches!(self.phase, Phase::LoadFailed | Phase::NotInstalled) {
            self.phase = Phase::Loading;
        }
        if !self.send(BackendCommand::RefreshAll) {
            self.mark_backend_lost("后端线程已退出，无法刷新");
        }
    }

    fn send_overlay_refresh(&mut self) {
        if self.phase != Phase::Ready || self.is_pending() {
            return;
        }
        if !self.send(BackendCommand::RefreshOverlayRuntime) {
            self.mark_backend_lost("后端线程已退出，无法刷新 Overlay 状态");
        }
    }

    /// 变更类命令：同刻只允许一条在途（PLAN §6.5）。
    /// 返回 id 仅当命令真正发送成功；调用方据此才 arm 电源动作。
    fn begin_mutation(&mut self, cmd: BackendCmd) -> Option<RequestId> {
        if self.phase == Phase::BackendLost {
            self.log(false, "后端线程已退出，请重启应用");
            return None;
        }
        if self.phase != Phase::Ready {
            return None;
        }
        if self.mutation.is_some() {
            self.log(false, "有操作正在执行，请稍后再试");
            return None;
        }
        let id = self.next_request_id();
        if !self.send(BackendCommand::Mutate { id, cmd }) {
            self.mark_backend_lost("后端线程已退出，操作未发送");
            return None;
        }
        self.mutation = Some(id);
        Some(id)
    }

    /// 通知后端线程退出；main 在 `run` 返回后调用（正常/错误路径都会停线程）。
    pub fn send_quit(&mut self) {
        let _ = self.send(BackendCommand::Quit);
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        while !self.should_quit {
            if let Some(Modal::FilePicker(picker)) = &mut self.modal {
                picker.poll();
            }
            terminal.draw(|frame| crate::ui::render(frame, self))?;

            let mut timeout = OVERLAY_POLL.saturating_sub(self.last_overlay_tick.elapsed());
            if let Some(Modal::FilePicker(picker)) = &self.modal
                && picker.is_polling()
            {
                timeout = timeout.min(Duration::from_millis(50));
            }
            match self.ui_rx.recv_timeout(timeout) {
                Ok(evt) => {
                    self.handle_ui_event(evt);
                    // 一次处理当前所有已就绪事件，避免后端回包积压。
                    while let Ok(evt) = self.ui_rx.try_recv() {
                        self.handle_ui_event(evt);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                // 仅当后端与终端读取线程均已退出时通道才会闭合，直接退出避免空转。
                Err(RecvTimeoutError::Disconnected) => break,
            }

            if self.last_overlay_tick.elapsed() >= OVERLAY_POLL {
                self.last_overlay_tick = Instant::now();
                self.send_overlay_refresh();
            }
        }
        Ok(())
    }

    fn handle_ui_event(&mut self, evt: UiEvent) {
        match evt {
            UiEvent::Terminal(Event::Key(key))
                if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
            {
                self.on_key(key)
            }
            UiEvent::Terminal(_) => {}
            UiEvent::Backend(evt) => self.apply_event(evt),
        }
    }

    fn apply_event(&mut self, evt: BackendEvent) {
        match evt {
            BackendEvent::Snapshot(snapshot) => {
                self.snapshot = snapshot;
                self.phase = Phase::Ready;
                self.clamp_selections();
                self.restore_selection();
                self.log(true, "已刷新 UWF 全部状态");
                if !self.snapshot.warnings.is_empty() {
                    // 单行日志聚合全部卷读取警告，避免前序警告被覆盖。
                    let combined = self.snapshot.warnings.join("；");
                    self.log_warn(&combined);
                }
            }
            BackendEvent::OverlayRuntime(runtime) => {
                self.snapshot.overlay_runtime = Some(runtime);
            }
            BackendEvent::RefreshFailed(msg) => {
                self.log(false, &msg);
                if self.phase == Phase::Loading {
                    self.phase = Phase::LoadFailed;
                }
            }
            BackendEvent::Finished { id, outcome } => {
                if self.mutation == Some(id) {
                    self.mutation = None;
                }
                match outcome {
                    Outcome::Ok(msg) => {
                        self.log(true, &msg);
                        self.on_power_action_ok(id);
                    }
                    Outcome::OkWithRefreshWarning { message, warning } => {
                        self.log(true, &message);
                        self.log_warn(&warning);
                        self.on_power_action_ok(id);
                    }
                    Outcome::Warning(msg) => {
                        self.log_warn(&msg);
                    }
                    Outcome::Err(msg) => {
                        self.log(false, &msg);
                        if self
                            .power_action
                            .is_some_and(|(power_id, _)| power_id == id)
                        {
                            self.power_action = None;
                        }
                    }
                }
            }
            BackendEvent::NotInstalled(id) => {
                if let Some(req_id) = id {
                    if self.mutation == Some(req_id) {
                        self.mutation = None;
                    }
                    if self
                        .power_action
                        .is_some_and(|(power_id, _)| power_id == req_id)
                    {
                        self.power_action = None;
                    }
                }
                self.phase = Phase::NotInstalled;
                self.modal = None;
                self.log(false, "UWF 功能未安装或未启用");
            }
            BackendEvent::Disconnected => {
                self.mark_backend_lost("后端线程已退出，请重启应用");
            }
        }
    }

    /// 电源动作回包处理：只有绑定到该请求 id 的成功回包才允许退出/模拟重启。
    fn on_power_action_ok(&mut self, id: RequestId) {
        let Some((power_id, power)) = self.power_action else {
            return;
        };
        if power_id != id {
            return;
        }
        self.power_action = None;
        match (power, self.backend_kind) {
            (PowerAction::Restart, BackendKind::Mock) => {
                self.log(true, "（模拟）已重启：下次会话配置已生效");
                // Mock 的重启是把 next 应用为 current；刷新一次让界面展示生效结果。
                self.refresh_all();
            }
            _ => self.should_quit = true,
        }
    }

    fn log(&mut self, ok: bool, text: &str) {
        self.push_log(if ok { LogLevel::Ok } else { LogLevel::Err }, text);
    }

    fn log_warn(&mut self, text: &str) {
        self.push_log(LogLevel::Warn, text);
    }

    fn push_log(&mut self, level: LogLevel, text: &str) {
        self.last_log = Some(StatusEntry {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            level,
            text: text.to_string(),
        });
    }

    /// 当前文件排除项的稳定 key（盘符 + 相对路径）。
    fn current_file_key(&self) -> Option<(String, String)> {
        self.file_exclusion_state
            .selected()
            .and_then(|i| self.visible_file_exclusions().get(i))
            .map(|e| (e.drive_letter.clone(), e.file_rel_path.clone()))
    }

    /// 当前注册表排除项的稳定 key。
    fn current_registry_key(&self) -> Option<String> {
        self.registry_exclusion_state
            .selected()
            .and_then(|i| self.visible_registry_exclusions().get(i))
            .cloned()
    }

    /// 记录当前选中项的稳定 key。
    fn remember_selection(&mut self) {
        self.selected_file_key = self.current_file_key();
        self.selected_registry_key = self.current_registry_key();
    }

    /// 快照/排序变化后按 key 恢复选中。
    /// key 尚未建立（首次快照）或对应项已消失（被删除）时，重新锚定到
    /// clamp 后的选中项，避免后续插入项时选中索引无声漂移。
    fn restore_selection(&mut self) {
        let file_hit = self.selected_file_key.clone().and_then(|(drive, rel)| {
            self.visible_file_exclusions()
                .iter()
                .position(|e| e.drive_letter == drive && e.file_rel_path == rel)
        });
        match file_hit {
            Some(i) => self.file_exclusion_state.select(Some(i)),
            None => self.selected_file_key = self.current_file_key(),
        }

        let registry_hit = self.selected_registry_key.clone().and_then(|key| {
            self.visible_registry_exclusions()
                .iter()
                .position(|k| k == &key)
        });
        match registry_hit {
            Some(i) => self.registry_exclusion_state.select(Some(i)),
            None => self.selected_registry_key = self.current_registry_key(),
        }
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

    // ── 按键路由 ──

    fn on_key(&mut self, key: KeyEvent) {
        if self.modal.is_some() {
            self.on_modal_key(key);
            return;
        }
        match self.phase {
            // D6：未安装引导页只允许重新检测、退出、帮助，其余管理键全部隔离。
            Phase::NotInstalled => match key.code {
                KeyCode::F(5) => self.refresh_all(),
                KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => self.request_quit(),
                KeyCode::Char('?') => self.modal = Some(Modal::Help),
                _ => {}
            },
            Phase::BackendLost => match key.code {
                KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => self.should_quit = true,
                KeyCode::Char('?') => self.modal = Some(Modal::Help),
                _ => self.log(false, "后端线程已退出，请重启应用"),
            },
            Phase::Loading | Phase::LoadFailed => match key.code {
                KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => self.request_quit(),
                KeyCode::Tab => self.tab = self.tab.next(),
                KeyCode::BackTab => self.tab = self.tab.prev(),
                KeyCode::Char(c @ '1'..='4') => {
                    self.tab = Tab::from_index(c as usize - '1' as usize);
                }
                KeyCode::Char('?') => self.modal = Some(Modal::Help),
                KeyCode::F(5) => self.refresh_all(),
                _ => {}
            },
            Phase::Ready => match key.code {
                KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => self.request_quit(),
                KeyCode::Tab => self.tab = self.tab.next(),
                KeyCode::BackTab => self.tab = self.tab.prev(),
                KeyCode::Char(c @ '1'..='4') => {
                    self.tab = Tab::from_index(c as usize - '1' as usize);
                }
                KeyCode::Char('?') => self.modal = Some(Modal::Help),
                KeyCode::F(5) => self.refresh_all(),
                _ => match self.tab {
                    Tab::Dashboard => self.on_dashboard_key(key),
                    Tab::Volumes => self.on_volumes_key(key),
                    Tab::Exclusions => self.on_exclusions_key(key),
                    Tab::Commit => self.on_commit_key(key),
                },
            },
        }
    }

    /// 退出请求：变更在途时二次确认，避免强杀正在执行的 WMI 调用。
    fn request_quit(&mut self) {
        if self.is_pending() {
            self.modal = Some(Modal::Confirm(ConfirmState::new(
                "退出确认",
                "有操作正在执行，退出可能中断 WMI 调用。确定退出？",
                ConfirmAction::Quit,
            )));
        } else {
            self.should_quit = true;
        }
    }

    /// 在途或未就绪时拦截打开修改类弹窗，避免用户输完长路径后才被拒绝并丢失输入。
    fn open_action_modal(&mut self, modal: Modal) {
        if self.phase != Phase::Ready {
            return;
        }
        if self.is_pending() {
            self.log(false, "有操作正在执行，请稍后再试");
            return;
        }
        self.modal = Some(modal);
    }

    fn on_dashboard_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('e') | KeyCode::Char('E') => {
                let _ = self.begin_mutation(BackendCmd::EnableFilter);
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                let _ = self.begin_mutation(BackendCmd::DisableFilter);
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.open_action_modal(Modal::Confirm(ConfirmState::new(
                    "重置配置确认",
                    "将重置全部 UWF 配置（筛选器 / Overlay / 卷保护 / 排除项），\n下次重启后恢复默认值。确定继续？",
                    ConfirmAction::ResetSettings,
                )));
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.open_action_modal(Modal::OverlaySettings(
                    OverlaySettingsState::from_snapshot(&self.snapshot),
                ));
            }
            KeyCode::F(10) => {
                self.open_action_modal(Modal::Confirm(ConfirmState::new(
                    "重启系统确认",
                    "系统将立即重启，未保存的工作将丢失。\n确定继续？",
                    ConfirmAction::Restart,
                )));
            }
            KeyCode::F(11) => {
                self.open_action_modal(Modal::Confirm(ConfirmState::new(
                    "关机确认",
                    "系统将立即关机，未保存的工作将丢失。\n确定继续？",
                    ConfirmAction::Shutdown,
                )));
            }
            _ => {}
        }
    }

    fn on_volumes_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => {
                let len = self.snapshot.volumes.len();
                move_selection(self.volumes_state.selected_mut(), len, -1);
            }
            KeyCode::Down => {
                let len = self.snapshot.volumes.len();
                move_selection(self.volumes_state.selected_mut(), len, 1);
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                if let Some(drive) = self.selected_volume().map(|v| v.drive_letter.clone()) {
                    let _ = self.begin_mutation(BackendCmd::Protect(drive));
                }
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                if let Some(drive) = self.selected_volume().map(|v| v.drive_letter.clone()) {
                    let _ = self.begin_mutation(BackendCmd::Unprotect(drive));
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
                self.remember_selection();
            }
            KeyCode::Home | KeyCode::End => {
                let end = key.code == KeyCode::End;
                match self.exclusion_focus {
                    ExclusionFocus::Files => {
                        let len = self.visible_file_exclusions().len();
                        select_edge(self.file_exclusion_state.selected_mut(), len, end);
                    }
                    ExclusionFocus::Registry => {
                        let len = self.visible_registry_exclusions().len();
                        select_edge(self.registry_exclusion_state.selected_mut(), len, end);
                    }
                }
                self.remember_selection();
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.exclusion_session_current = !self.exclusion_session_current;
                self.clamp_selections();
                if !self.exclusion_session_current {
                    self.restore_selection();
                }
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                if !self.ensure_mutable_exclusions() {
                    return;
                }
                self.open_action_modal(Modal::Input(InputState::new(
                    "添加文件排除",
                    "输入绝对路径（如 C:\\Windows\\Temp），或按 [F2] 打开文件选择器",
                    InputAction::AddFileExclusion,
                    true,
                )));
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if !self.ensure_mutable_exclusions() {
                    return;
                }
                self.open_action_modal(Modal::Input(InputState::new(
                    "添加注册表排除",
                    "输入注册表键路径（如 HKLM\\SOFTWARE\\Example）",
                    InputAction::AddRegistryExclusion,
                    false,
                )));
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                if self.ensure_mutable_exclusions() {
                    self.delete_selected_exclusion();
                }
            }
            KeyCode::Char('i') | KeyCode::Char('I') => {
                if !self.ensure_mutable_exclusions() {
                    return;
                }
                self.open_action_modal(Modal::FilePicker(FilePickerState::new(
                    PickerMode::File,
                    PickerAction::ImportExclusions,
                )));
            }
            KeyCode::Char('x') | KeyCode::Char('X') => self.export_exclusions(),
            _ => {}
        }
    }

    /// P1-2：当前生效视图只读；增删/导入必须先切到下次生效视图。
    fn ensure_mutable_exclusions(&mut self) -> bool {
        if self.exclusion_session_current {
            self.log(false, "当前生效视图只读，请按 [S] 切换到下次生效视图");
            false
        } else {
            true
        }
    }

    fn on_commit_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('f') | KeyCode::Char('F') => {
                self.open_action_modal(Modal::FilePicker(FilePickerState::new(
                    PickerMode::File,
                    PickerAction::CommitFile,
                )));
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                self.open_action_modal(Modal::FilePicker(FilePickerState::new(
                    PickerMode::File,
                    PickerAction::CommitFileDeletion,
                )));
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.open_action_modal(Modal::Input(InputState::new(
                    "注册表提交",
                    "输入注册表完整路径（如 HKLM\\SOFTWARE\\Example\\ValueName）",
                    InputAction::CommitRegistry,
                    false,
                )));
            }
            _ => {}
        }
    }

    // ── 模态按键与结果执行 ──

    fn on_modal_key(&mut self, key: KeyEvent) {
        let Some(modal) = self.modal.take() else {
            return;
        };
        match modal {
            Modal::Help => {
                if !matches!(
                    key.code,
                    KeyCode::Esc
                        | KeyCode::Enter
                        | KeyCode::Char('q')
                        | KeyCode::Char('Q')
                        | KeyCode::Char('?')
                ) {
                    self.modal = Some(Modal::Help);
                }
            }
            Modal::Confirm(mut state) => match state.on_key(key) {
                ConfirmOutcome::Stay => self.modal = Some(Modal::Confirm(state)),
                ConfirmOutcome::Cancel => {}
                ConfirmOutcome::Confirm(action) => self.execute_confirm(action),
            },
            Modal::Input(mut state) => match state.on_key(key) {
                InputOutcome::Stay => self.modal = Some(Modal::Input(state)),
                InputOutcome::Cancel => {}
                InputOutcome::Submit(action, text) => self.execute_input(action, text),
                InputOutcome::Browse => {
                    self.modal = Some(Modal::FilePicker(FilePickerState::new(
                        PickerMode::Any,
                        PickerAction::AddFileExclusion,
                    )));
                }
            },
            Modal::FilePicker(mut state) => match state.on_key(key) {
                PickerOutcome::Stay => self.modal = Some(Modal::FilePicker(state)),
                PickerOutcome::Cancel => {}
                PickerOutcome::Pick(action, path) => self.execute_pick(action, path),
            },
            Modal::OverlaySettings(mut state) => match state.on_key(key) {
                OverlayOutcome::Stay => self.modal = Some(Modal::OverlaySettings(state)),
                OverlayOutcome::Cancel => {}
                OverlayOutcome::Submit {
                    overlay_type,
                    max_mb,
                    warning_mb,
                    critical_mb,
                } => {
                    let _ = self.begin_mutation(BackendCmd::SetOverlayConfig {
                        overlay_type,
                        max_mb,
                        warning_mb,
                        critical_mb,
                    });
                }
            },
        }
    }

    fn execute_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::ResetSettings => {
                let _ = self.begin_mutation(BackendCmd::ResetSettings);
            }
            ConfirmAction::Restart => {
                if let Some(id) = self.begin_mutation(BackendCmd::RestartSystem) {
                    self.power_action = Some((id, PowerAction::Restart));
                }
            }
            ConfirmAction::Shutdown => {
                if let Some(id) = self.begin_mutation(BackendCmd::ShutdownSystem) {
                    self.power_action = Some((id, PowerAction::Shutdown));
                }
            }
            ConfirmAction::CommitFileDeletion(path) => {
                let _ = self.begin_mutation(BackendCmd::CommitFileDeletion(
                    path.to_string_lossy().into_owned(),
                ));
            }
            ConfirmAction::Quit => self.should_quit = true,
        }
    }

    fn execute_input(&mut self, action: InputAction, text: String) {
        match action {
            InputAction::AddFileExclusion => {
                let _ = self.begin_mutation(BackendCmd::AddFileExclusion(text));
            }
            InputAction::AddRegistryExclusion => {
                let _ = self.begin_mutation(BackendCmd::AddRegistryExclusion(text));
            }
            InputAction::CommitRegistry => {
                let _ = self.begin_mutation(BackendCmd::CommitRegistry(text));
            }
        }
    }

    fn execute_pick(&mut self, action: PickerAction, path: PathBuf) {
        let path_str = path.to_string_lossy().into_owned();
        match action {
            PickerAction::CommitFile => {
                let _ = self.begin_mutation(BackendCmd::CommitFile(path_str));
            }
            PickerAction::AddFileExclusion => {
                let _ = self.begin_mutation(BackendCmd::AddFileExclusion(path_str));
            }
            PickerAction::ImportExclusions => {
                let _ = self.begin_mutation(BackendCmd::ImportExclusions { path });
            }
            PickerAction::CommitFileDeletion => {
                // R3：两步语义（先删文件再固化），必须确认
                self.modal = Some(Modal::Confirm(ConfirmState::new(
                    "删除提交确认",
                    format!(
                        "将立即删除文件:\n{}\n并固化该删除（重启后依然生效）。确定继续？",
                        path.display()
                    ),
                    ConfirmAction::CommitFileDeletion(path),
                )));
            }
        }
    }

    fn delete_selected_exclusion(&mut self) {
        match self.exclusion_focus {
            ExclusionFocus::Files => {
                let item = self
                    .file_exclusion_state
                    .selected()
                    .and_then(|i| self.visible_file_exclusions().get(i))
                    .map(|e| e.display());
                if let Some(item) = item {
                    let _ = self.begin_mutation(BackendCmd::RemoveFileExclusion(item));
                }
            }
            ExclusionFocus::Registry => {
                let item = self
                    .registry_exclusion_state
                    .selected()
                    .and_then(|i| self.visible_registry_exclusions().get(i))
                    .cloned();
                if let Some(item) = item {
                    let _ = self.begin_mutation(BackendCmd::RemoveRegistryExclusion(item));
                }
            }
        }
    }

    /// 导出当前视图排除列表：文件名格式与原版一致
    /// （EXCLUSIONLIST_yyyy_MM_dd_HH_mm_ss.txt，防重名自动编号由后端线程原子创建）
    fn export_exclusions(&mut self) {
        let filename = chrono::Local::now()
            .format("EXCLUSIONLIST_%Y_%m_%d_%H_%M_%S.txt")
            .to_string();
        let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let path = dir.join(&filename);
        let _ = self.begin_mutation(BackendCmd::ExportExclusions {
            path,
            current_session: self.exclusion_session_current,
        });
    }
}

fn clamp(selected: &mut Option<usize>, len: usize) {
    *selected = match (*selected, len) {
        (_, 0) => None,
        (None, _) => Some(0),
        (Some(i), _) => Some(i.min(len - 1)),
    };
}

/// Home/End：选中首项或末项；空列表清空选中。
fn select_edge(selected: &mut Option<usize>, len: usize, end: bool) {
    *selected = if len == 0 {
        None
    } else if end {
        Some(len - 1)
    } else {
        Some(0)
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{FileExclusion, FilterState, OverlayConfig, OverlayType, VolumeInfo};
    use crossterm::event;

    /// 与 `MockUwfBackend::demo()` 对齐的快照数据（供 app/ui 测试共用）
    pub(crate) fn demo_snapshot() -> FullSnapshot {
        FullSnapshot {
            filter_state: Some(FilterState {
                current_enabled: true,
                next_enabled: true,
            }),
            overlay_config_current: Some(OverlayConfig {
                overlay_type: OverlayType::Ram,
                max_size_mb: 2048,
            }),
            overlay_config_next: Some(OverlayConfig {
                overlay_type: OverlayType::Ram,
                max_size_mb: 2048,
            }),
            overlay_runtime: Some(OverlayRuntime {
                available_space_mb: 1536,
                consumption_mb: 512,
                warning_threshold_mb: 1536,
                critical_threshold_mb: 1843,
            }),
            volumes: vec![
                VolumeInfo {
                    drive_letter: "C:".into(),
                    current_protected: true,
                    next_protected: true,
                },
                VolumeInfo {
                    drive_letter: "D:".into(),
                    current_protected: false,
                    next_protected: false,
                },
            ],
            file_exclusions_current: vec![],
            file_exclusions_next: vec![FileExclusion::new("C:".into(), "\\Windows\\Temp".into())],
            registry_exclusions_current: vec![],
            registry_exclusions_next: vec!["HKLM\\SOFTWARE\\UwfManagerDemo".into()],
            warnings: vec![],
        }
    }

    fn test_app(kind: BackendKind) -> (App, Receiver<BackendCommand>) {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (_, ui_rx) = std::sync::mpsc::channel();
        (App::new(cmd_tx, ui_rx, kind), cmd_rx)
    }

    fn recv_cmd(rx: &Receiver<BackendCommand>) -> BackendCmd {
        match rx.try_recv().expect("期望收到后端命令") {
            BackendCommand::Mutate { cmd, .. } => cmd,
            other => panic!("期望 Mutate 命令，实际 {other:?}"),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, event::KeyModifiers::NONE)
    }

    /// 注入启动快照，进入 Ready 阶段。
    fn ready_app(kind: BackendKind) -> (App, Receiver<BackendCommand>) {
        let (mut app, cmd_rx) = test_app(kind);
        app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
        (app, cmd_rx)
    }

    #[test]
    fn tab_navigation_wraps() {
        assert_eq!(Tab::Dashboard.next(), Tab::Volumes);
        assert_eq!(Tab::Commit.next(), Tab::Dashboard);
        assert_eq!(Tab::Dashboard.prev(), Tab::Commit);
        assert_eq!(Tab::from_index(5), Tab::Volumes); // 取模回绕
    }

    #[test]
    fn tab_index_matches_all_order() {
        for (i, tab) in Tab::ALL.iter().enumerate() {
            assert_eq!(tab.index(), i);
        }
    }

    #[test]
    fn selection_helpers_clamp_and_wrap() {
        let mut sel = Some(5);
        clamp(&mut sel, 3);
        assert_eq!(sel, Some(2));
        let mut sel = None;
        clamp(&mut sel, 3);
        assert_eq!(sel, Some(0));
        let mut sel = Some(0);
        clamp(&mut sel, 0);
        assert_eq!(sel, None);
        let mut sel = Some(0);
        move_selection(&mut sel, 3, -1);
        assert_eq!(sel, Some(2)); // 顶部上移回绕到底部
        move_selection(&mut sel, 3, 1);
        assert_eq!(sel, Some(0)); // 底部下移回绕到顶部
        let mut sel = Some(0);
        move_selection(&mut sel, 0, 1);
        assert_eq!(sel, None); // 空列表无选中
    }

    #[test]
    fn not_installed_event_shows_guide_and_snapshot_recovers() {
        let (mut app, _cmd_rx) = test_app(BackendKind::Mock);
        assert!(app.is_busy()); // 启动即 Loading
        app.apply_event(BackendEvent::NotInstalled(None));
        assert_eq!(app.phase, Phase::NotInstalled);
        assert!(!app.is_busy());
        assert!(
            app.last_log
                .as_ref()
                .is_some_and(|e| e.level == LogLevel::Err)
        );
        // 重新检测成功（Snapshot）后自动恢复
        app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
        assert_eq!(app.phase, Phase::Ready);
    }

    #[test]
    fn initial_load_error_and_retry_failure_transition_cleanly() {
        let (mut app, cmd_rx) = test_app(BackendKind::Mock);
        app.apply_event(BackendEvent::RefreshFailed("temporary WMI failure".into()));
        assert!(!app.is_busy(), "初始查询失败也必须结束加载状态");
        assert_eq!(app.phase, Phase::LoadFailed);

        // F5 重试进入 Loading，若再次失败必须回到 LoadFailed，不能卡死在加载中。
        app.refresh_all();
        assert!(matches!(cmd_rx.try_recv(), Ok(BackendCommand::RefreshAll)));
        assert_eq!(app.phase, Phase::Loading);
        app.apply_event(BackendEvent::RefreshFailed("second failure".into()));
        assert_eq!(app.phase, Phase::LoadFailed);

        // 重试成功后恢复 Ready，之后 Ready 下 F5 失败不破坏当前快照显示。
        app.refresh_all();
        app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
        assert_eq!(app.phase, Phase::Ready);
        assert!(!app.is_busy());
        app.refresh_all();
        app.apply_event(BackendEvent::RefreshFailed("transient".into()));
        assert_eq!(app.phase, Phase::Ready);
    }

    #[test]
    fn query_events_do_not_clear_mutation() {
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        let mutation = app.begin_mutation(BackendCmd::EnableFilter).unwrap();

        // 刷新失败或快照回包均不能解锁变更
        app.apply_event(BackendEvent::RefreshFailed("刷新失败".into()));
        assert_eq!(app.mutation, Some(mutation));

        app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
        assert_eq!(app.mutation, Some(mutation));

        // 无 id 的 NotInstalled 只置引导页，不误解锁
        app.apply_event(BackendEvent::NotInstalled(None));
        assert_eq!(app.mutation, Some(mutation));

        // 带匹配 id 的 NotInstalled 应同步解锁
        app.apply_event(BackendEvent::NotInstalled(Some(mutation)));
        assert_eq!(app.mutation, None);

        // 重回 Ready 验证正常 Finished 解锁
        app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
        let mutation = app.begin_mutation(BackendCmd::EnableFilter).unwrap();
        app.apply_event(BackendEvent::Finished {
            id: mutation,
            outcome: Outcome::Ok("ok".into()),
        });
        assert_eq!(app.mutation, None);
    }

    #[test]
    fn mutation_gated_while_pending() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.on_dashboard_key(key(KeyCode::Char('e')));
        let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
            panic!("期望 Mutate 命令");
        };
        assert!(matches!(cmd, BackendCmd::EnableFilter));
        assert_eq!(app.mutation, Some(id));

        // 在途期间第二条变更被忽略（PLAN §6.5 单变更在途）
        app.on_dashboard_key(key(KeyCode::Char('d')));
        assert!(cmd_rx.try_recv().is_err());

        // 匹配 id 的完成事件解除锁
        app.apply_event(BackendEvent::Finished {
            id,
            outcome: Outcome::Ok("ok".into()),
        });
        assert!(!app.is_pending());
        app.on_dashboard_key(key(KeyCode::Char('d')));
        assert!(matches!(recv_cmd(&cmd_rx), BackendCmd::DisableFilter));
    }

    #[test]
    fn power_action_not_armed_when_mutation_is_pending() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        let first = app.begin_mutation(BackendCmd::EnableFilter).unwrap();
        let _ = cmd_rx.try_recv().unwrap(); // 消费 EnableFilter

        app.execute_confirm(ConfirmAction::Restart);
        assert!(cmd_rx.try_recv().is_err()); // 没有发出 RestartSystem
        assert!(app.power_action.is_none()); // 也没有 arm 电源动作

        // 前一条变更成功，不应触发退出/模拟重启
        app.apply_event(BackendEvent::Finished {
            id: first,
            outcome: Outcome::Ok("UWF 将在下次系统重启后启用".into()),
        });
        assert!(!app.should_quit);
    }

    #[test]
    fn quit_requires_confirm_when_mutation_pending() {
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        let _ = app.begin_mutation(BackendCmd::EnableFilter).unwrap();
        app.on_key(key(KeyCode::Esc));
        assert!(!app.should_quit);
        assert!(matches!(app.modal, Some(Modal::Confirm(_))));
        app.on_key(key(KeyCode::Enter));
        assert!(app.should_quit);
    }

    #[test]
    fn not_installed_blocks_management_keys() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.apply_event(BackendEvent::NotInstalled(None));
        for code in [
            KeyCode::F(10),
            KeyCode::Char('e'),
            KeyCode::Char('s'),
            KeyCode::Char('a'),
            KeyCode::Char('d'),
            KeyCode::Char('p'),
        ] {
            app.on_key(key(code));
            assert!(app.modal.is_none(), "未安装页不应打开管理弹窗: {code:?}");
        }
        assert!(cmd_rx.try_recv().is_err());
    }

    #[test]
    fn not_installed_and_backend_lost_skip_overlay_poll() {
        let (mut app, cmd_rx) = test_app(BackendKind::Mock);
        app.send_overlay_refresh();
        assert!(cmd_rx.try_recv().is_err(), "Loading 阶段不应轮询 Overlay");

        app.apply_event(BackendEvent::NotInstalled(None));
        app.send_overlay_refresh();
        assert!(cmd_rx.try_recv().is_err(), "未安装阶段不应轮询 Overlay");

        app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
        app.send_overlay_refresh();
        assert!(matches!(
            cmd_rx.try_recv(),
            Ok(BackendCommand::RefreshOverlayRuntime)
        ));

        app.apply_event(BackendEvent::Disconnected);
        app.send_overlay_refresh();
        assert!(cmd_rx.try_recv().is_err(), "断线后不应轮询 Overlay");
    }

    #[test]
    fn global_keys_tab_help_refresh_quit() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.on_key(key(KeyCode::Char('2')));
        assert_eq!(app.tab, Tab::Volumes);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.tab, Tab::Exclusions);
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(app.tab, Tab::Volumes);
        app.on_key(key(KeyCode::Char('?')));
        assert!(matches!(app.modal, Some(Modal::Help)));
        app.on_key(key(KeyCode::Esc));
        assert!(app.modal.is_none());
        app.on_key(key(KeyCode::F(5)));
        assert!(matches!(cmd_rx.try_recv(), Ok(BackendCommand::RefreshAll)));
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn volumes_protect_selected_drive() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.tab = Tab::Volumes;
        // 下移选中 D:（索引 1）
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Char('p')));
        match recv_cmd(&cmd_rx) {
            BackendCmd::Protect(drive) => assert_eq!(drive, "D:"),
            other => panic!("期望 Protect，实际 {other:?}"),
        }
    }

    #[test]
    fn exclusions_toggle_session_and_delete_selected() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.tab = Tab::Exclusions;
        assert!(!app.exclusion_session_current); // 默认下次生效视图
        app.on_key(key(KeyCode::Char('s')));
        assert!(app.exclusion_session_current);
        app.on_key(key(KeyCode::Char('s')));

        // 焦点文件栏，删除选中 → RemoveFileExclusion（display 形式绝对路径）
        app.on_key(key(KeyCode::Char('d')));
        let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
            panic!("期望 Mutate 命令");
        };
        match cmd {
            BackendCmd::RemoveFileExclusion(path) => {
                assert_eq!(path, "C:\\Windows\\Temp")
            }
            other => panic!("期望 RemoveFileExclusion，实际 {other:?}"),
        }
        app.apply_event(BackendEvent::Finished {
            id,
            outcome: Outcome::Ok("ok".into()),
        });

        // 切到注册表栏删除
        app.on_key(key(KeyCode::Right));
        app.on_key(key(KeyCode::Char('d')));
        match recv_cmd(&cmd_rx) {
            BackendCmd::RemoveRegistryExclusion(key) => {
                assert_eq!(key, "HKLM\\SOFTWARE\\UwfManagerDemo")
            }
            other => panic!("期望 RemoveRegistryExclusion，实际 {other:?}"),
        }
    }

    #[test]
    fn exclusions_home_end_move_selection() {
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        app.tab = Tab::Exclusions;
        let mut snapshot = demo_snapshot();
        snapshot.file_exclusions_next = vec![
            FileExclusion::new("C:".into(), "\\A".into()),
            FileExclusion::new("C:".into(), "\\B".into()),
        ];
        app.apply_event(BackendEvent::Snapshot(snapshot));

        app.on_key(key(KeyCode::End));
        assert_eq!(app.file_exclusion_state.selected(), Some(1));
        assert_eq!(app.selected_file_key, Some(("C:".into(), "\\B".into())));
        app.on_key(key(KeyCode::Home));
        assert_eq!(app.file_exclusion_state.selected(), Some(0));
        assert_eq!(app.selected_file_key, Some(("C:".into(), "\\A".into())));
    }

    #[test]
    fn restart_flow_mock_stays_windows_quits() {
        // Mock：重启不回真机，留在界面展示 next→current 生效
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.on_key(key(KeyCode::F(10)));
        assert!(matches!(app.modal, Some(Modal::Confirm(_))));
        app.on_key(key(KeyCode::Enter));
        assert!(app.modal.is_none());
        let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
            panic!("期望 Mutate 命令");
        };
        assert!(matches!(cmd, BackendCmd::RestartSystem));
        app.apply_event(BackendEvent::Finished {
            id,
            outcome: Outcome::Ok("系统即将重启".into()),
        });
        assert!(!app.should_quit);
        assert!(
            app.last_log
                .as_ref()
                .is_some_and(|e| e.text.contains("（模拟）已重启"))
        );

        // Windows：真实重启，退出 TUI 让位
        let (mut app, cmd_rx) = ready_app(BackendKind::Windows);
        app.on_key(key(KeyCode::F(10)));
        app.on_key(key(KeyCode::Enter));
        let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
            panic!("期望 Mutate 命令");
        };
        assert!(matches!(cmd, BackendCmd::RestartSystem));
        app.apply_event(BackendEvent::Finished {
            id,
            outcome: Outcome::Ok("系统即将重启".into()),
        });
        assert!(app.should_quit);
    }

    #[test]
    fn commit_registry_input_flow() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.tab = Tab::Commit;
        app.on_key(key(KeyCode::Char('r')));
        assert!(matches!(app.modal, Some(Modal::Input(_))));
        for c in "HKLM\\SOFTWARE\\X\\Val".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Enter));
        assert!(app.modal.is_none());
        match recv_cmd(&cmd_rx) {
            BackendCmd::CommitRegistry(path) => assert_eq!(path, "HKLM\\SOFTWARE\\X\\Val"),
            other => panic!("期望 CommitRegistry，实际 {other:?}"),
        }
    }

    #[test]
    fn overlay_settings_locked_when_filter_enabled() {
        // demo 快照筛选器当前启用 → R1 锁定类型/上限，选中行落到阈值
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        app.on_dashboard_key(key(KeyCode::Char('s')));
        let Some(Modal::OverlaySettings(state)) = &app.modal else {
            panic!("期望 OverlaySettings 弹窗");
        };
        assert!(state.lock_type_size);
        assert_eq!(state.selected, 2);

        // 当前禁用 → 不锁定
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        let mut snapshot = demo_snapshot();
        snapshot.filter_state = Some(FilterState {
            current_enabled: false,
            next_enabled: false,
        });
        app.apply_event(BackendEvent::Snapshot(snapshot));
        app.on_dashboard_key(key(KeyCode::Char('s')));
        let Some(Modal::OverlaySettings(state)) = &app.modal else {
            panic!("期望 OverlaySettings 弹窗");
        };
        assert!(!state.lock_type_size);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn commit_file_deletion_requires_confirm() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.execute_pick(
            PickerAction::CommitFileDeletion,
            PathBuf::from("C:\\data\\a.txt"),
        );
        assert!(matches!(app.modal, Some(Modal::Confirm(_))));
        app.on_key(key(KeyCode::Enter));
        match recv_cmd(&cmd_rx) {
            BackendCmd::CommitFileDeletion(path) => assert_eq!(path, "C:\\data\\a.txt"),
            other => panic!("期望 CommitFileDeletion，实际 {other:?}"),
        }
    }
    #[test]
    fn refresh_warning_is_visible_and_clears_pending() {
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        let id = app.begin_mutation(BackendCmd::EnableFilter).unwrap();
        app.apply_event(BackendEvent::Finished {
            id,
            outcome: Outcome::OkWithRefreshWarning {
                message: "UWF 将在下次系统重启后启用".into(),
                warning: "操作已成功，但状态刷新失败：boom".into(),
            },
        });
        assert!(!app.is_pending());
        assert!(
            app.last_log
                .as_ref()
                .is_some_and(|e| e.text.contains("状态刷新失败"))
        );
    }
    #[test]
    fn current_session_view_is_read_only() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.tab = Tab::Exclusions;
        app.on_key(key(KeyCode::Char('s'))); // 当前生效视图
        assert!(app.exclusion_session_current);

        for code in [
            KeyCode::Char('a'),
            KeyCode::Char('r'),
            KeyCode::Char('d'),
            KeyCode::Char('i'),
        ] {
            app.on_key(key(code));
            assert!(app.modal.is_none(), "当前视图不应打开编辑弹窗: {code:?}");
            assert!(
                cmd_rx.try_recv().is_err(),
                "当前视图不应发管理命令: {code:?}"
            );
        }

        // 切回下次生效视图后恢复可编辑
        app.on_key(key(KeyCode::Char('s')));
        app.on_key(key(KeyCode::Char('d')));
        assert!(matches!(
            recv_cmd(&cmd_rx),
            BackendCmd::RemoveFileExclusion(_)
        ));
    }

    #[test]
    fn snapshot_warnings_are_merged_without_losing_first() {
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        let mut snapshot = demo_snapshot();
        snapshot.warnings = vec![
            "卷 C: 读取排除项失败: e1".into(),
            "卷 D: 读取排除项失败: e2".into(),
        ];
        app.apply_event(BackendEvent::Snapshot(snapshot));
        let entry = app.last_log.as_ref().expect("应记录合并后的警告");
        assert_eq!(entry.level, LogLevel::Warn);
        assert!(entry.text.contains("卷 C:"));
        assert!(entry.text.contains("卷 D:"));
    }

    #[test]
    fn overlay_runtime_event_updates_snapshot_single_source() {
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        let runtime = OverlayRuntime {
            available_space_mb: 100,
            consumption_mb: 1024,
            warning_threshold_mb: 1536,
            critical_threshold_mb: 1843,
        };
        app.apply_event(BackendEvent::OverlayRuntime(runtime));
        assert_eq!(app.snapshot.overlay_runtime, Some(runtime));
    }
    #[test]
    fn backend_disconnect_marks_lost_closes_modal_and_blocks_mutations() {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (_ui_tx, ui_rx) = std::sync::mpsc::channel();
        let mut app = App::new(cmd_tx, ui_rx, BackendKind::Mock);
        app.modal = Some(Modal::Help);
        assert!(app.is_busy()); // 启动加载在途

        app.apply_event(BackendEvent::Disconnected);
        assert_eq!(app.phase, Phase::BackendLost);
        assert!(app.modal.is_none(), "断线必须关闭遗留弹窗");
        assert!(!app.is_busy()); // 不会永久 pending

        app.on_key(key(KeyCode::Char('e')));
        assert!(cmd_rx.try_recv().is_err());
        app.on_key(key(KeyCode::Esc));
        assert!(app.should_quit);
    }
    #[test]
    fn selection_reanchors_after_selected_item_removed() {
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);

        // 选中项被删除：选择收敛到相邻项，并把 key 重新锚定到该项。
        let mut snapshot = demo_snapshot();
        snapshot.file_exclusions_next = vec![
            FileExclusion::new("C:".into(), "\\A".into()),
            FileExclusion::new("C:".into(), "\\B".into()),
        ];
        snapshot.registry_exclusions_next = vec!["HKLM\\SOFTWARE\\A".into()];
        app.apply_event(BackendEvent::Snapshot(snapshot));
        assert_eq!(app.file_exclusion_state.selected(), Some(0));
        assert_eq!(app.selected_file_key, Some(("C:".into(), "\\A".into())));
        assert_eq!(app.registry_exclusion_state.selected(), Some(0));
        assert_eq!(app.selected_registry_key, Some("HKLM\\SOFTWARE\\A".into()));

        // 后续插入排序更靠前的项：必须继续跟随 \A（index 1），不能漂移到 \0。
        let mut snapshot = demo_snapshot();
        snapshot.file_exclusions_next = vec![
            FileExclusion::new("C:".into(), "\\0".into()),
            FileExclusion::new("C:".into(), "\\A".into()),
            FileExclusion::new("C:".into(), "\\B".into()),
        ];
        snapshot.registry_exclusions_next =
            vec!["HKLM\\SOFTWARE\\0".into(), "HKLM\\SOFTWARE\\A".into()];
        app.apply_event(BackendEvent::Snapshot(snapshot));
        assert_eq!(app.file_exclusion_state.selected(), Some(1));
        assert_eq!(app.selected_file_key, Some(("C:".into(), "\\A".into())));
        assert_eq!(app.registry_exclusion_state.selected(), Some(1));
        assert_eq!(app.selected_registry_key, Some("HKLM\\SOFTWARE\\A".into()));
    }

    #[test]
    fn default_selection_identity_survives_reorder() {
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        // 不手工预填 key：首份快照必须为默认选中的文件和注册表项建立身份。
        assert_eq!(
            app.selected_file_key,
            Some(("C:".into(), "\\Windows\\Temp".into()))
        );
        assert_eq!(
            app.selected_registry_key,
            Some("HKLM\\SOFTWARE\\UwfManagerDemo".into())
        );

        let mut snapshot = demo_snapshot();
        snapshot.file_exclusions_next = vec![
            FileExclusion::new("C:".into(), "\\A".into()),
            FileExclusion::new("C:".into(), "\\Windows\\Temp".into()),
        ];
        snapshot.registry_exclusions_next = vec![
            "HKLM\\SOFTWARE\\A".into(),
            "HKLM\\SOFTWARE\\UwfManagerDemo".into(),
        ];
        app.apply_event(BackendEvent::Snapshot(snapshot));

        assert_eq!(app.file_exclusion_state.selected(), Some(1));
        assert_eq!(app.registry_exclusion_state.selected(), Some(1));
        assert_eq!(
            app.selected_file_key,
            Some(("C:".into(), "\\Windows\\Temp".into()))
        );
        assert_eq!(
            app.selected_registry_key,
            Some("HKLM\\SOFTWARE\\UwfManagerDemo".into())
        );
    }
    #[test]
    fn exclusions_s_toggle_preserves_selected_key_across_empty_current() {
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        let mut snapshot = demo_snapshot();
        snapshot.file_exclusions_next = vec![
            FileExclusion::new("C:".into(), "\\A".into()),
            FileExclusion::new("C:".into(), "\\B".into()),
        ];
        snapshot.file_exclusions_current = vec![];
        app.apply_event(BackendEvent::Snapshot(snapshot));
        app.tab = Tab::Exclusions;
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.file_exclusion_state.selected(), Some(1));

        // 切到空的当前视图再切回，必须回到 \B 而不是重置到索引 0
        app.on_key(key(KeyCode::Char('s')));
        assert_eq!(app.file_exclusion_state.selected(), None);
        app.on_key(key(KeyCode::Char('s')));
        assert_eq!(app.file_exclusion_state.selected(), Some(1));
        assert_eq!(app.selected_file_key, Some(("C:".into(), "\\B".into())));
    }

    #[test]
    fn repeat_events_advance_selection_and_release_is_ignored() {
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        app.tab = Tab::Volumes;
        app.handle_ui_event(UiEvent::Terminal(Event::Key(KeyEvent::new_with_kind(
            KeyCode::Down,
            event::KeyModifiers::NONE,
            KeyEventKind::Repeat,
        ))));
        assert_eq!(app.volumes_state.selected(), Some(1));

        app.handle_ui_event(UiEvent::Terminal(Event::Key(KeyEvent::new_with_kind(
            KeyCode::Down,
            event::KeyModifiers::NONE,
            KeyEventKind::Release,
        ))));
        assert_eq!(app.volumes_state.selected(), Some(1));
    }

    #[test]
    fn help_closes_with_upper_q_and_enter() {
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        for close in [KeyCode::Char('Q'), KeyCode::Enter] {
            app.on_key(key(KeyCode::Char('?')));
            assert!(matches!(app.modal, Some(Modal::Help)));
            app.on_key(key(close));
            assert!(app.modal.is_none());
        }
    }
}
