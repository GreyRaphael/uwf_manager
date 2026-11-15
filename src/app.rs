//! 主应用状态机：单屏四区布局控制、事件循环、键盘/鼠标路由、后端回包应用。

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::DefaultTerminal;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{ListState, TableState};

use crate::backend::{
    BackendCmd, BackendCommand, BackendEvent, BackendKind, FullSnapshot, Outcome, RequestId,
};
use crate::model::{FileExclusion, OverlayType};
use crate::ui::components::commit_menu::{CommitChoice, CommitMenuOutcome, CommitMenuState};
use crate::ui::components::confirm::{ConfirmAction, ConfirmOutcome, ConfirmState};
use crate::ui::components::file_picker::{
    FilePickerState, PickerAction, PickerMode, PickerOutcome,
};
use crate::ui::components::help::HelpState;
use crate::ui::components::input_dialog::{InputAction, InputOutcome, InputState};

/// UI 线程统一事件：终端输入与后端回包走同一个 channel，
/// 后端回包可立即唤醒 `recv_timeout`，不再依赖短轮询。
pub enum UiEvent {
    Terminal(Event),
    Backend(BackendEvent),
}

impl From<BackendEvent> for UiEvent {
    fn from(evt: BackendEvent) -> Self {
        Self::Backend(evt)
    }
}

/// Overlay 轮询周期（与原版 2 秒后台刷新一致）
const OVERLAY_POLL: Duration = Duration::from_secs(2);

/// 主界面四个交互区块（按 Tab 顺序轮转）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPanel {
    BasicSettings,
    CacheSettings,
    Volumes,
    Exclusions,
}

impl FocusPanel {
    pub const ALL: [FocusPanel; 4] = [
        FocusPanel::BasicSettings,
        FocusPanel::CacheSettings,
        FocusPanel::Volumes,
        FocusPanel::Exclusions,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// 1. 基本设置区聚焦项
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasicField {
    WriteFilter,
    OverlayType,
}

/// 2. 缓存设置区聚焦项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheField {
    MaxSize,
    #[default]
    WarningThreshold,
    CriticalThreshold,
    ConfirmButton,
}

impl CacheField {
    const ALL: [CacheField; 4] = [
        CacheField::MaxSize,
        CacheField::WarningThreshold,
        CacheField::CriticalThreshold,
        CacheField::ConfirmButton,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|f| *f == self).unwrap_or(1)
    }
}

/// 简洁数字输入框（支持光标移动与插入删除）
#[derive(Debug, Clone, Default)]
pub struct NumericField {
    pub text: String,
    pub cursor: usize,
}

impl NumericField {
    pub fn set(&mut self, value: impl Into<String>) {
        self.text = value.into();
        self.cursor = self.text.len();
    }

    pub fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                false
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.text.len());
                false
            }
            KeyCode::Home => {
                self.cursor = 0;
                false
            }
            KeyCode::End => {
                self.cursor = self.text.len();
                false
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.text.remove(self.cursor);
                    true
                } else {
                    false
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.text.len() {
                    self.text.remove(self.cursor);
                    true
                } else {
                    false
                }
            }
            KeyCode::Char(c)
                if c.is_ascii_digit()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && self.text.len() < 10 =>
            {
                self.text.insert(self.cursor, c);
                self.cursor += 1;
                true
            }
            _ => false,
        }
    }
}

/// 2. 缓存设置表单状态
#[derive(Debug, Default)]
pub struct CacheForm {
    pub field: CacheField,
    pub max_size: NumericField,
    pub warning: NumericField,
    pub critical: NumericField,
    /// 是否有未保存编辑；为 true 时自动刷新不覆盖用户输入
    pub dirty: bool,
    pub error: Option<String>,
}

impl CacheForm {
    pub fn sync_from_snapshot(&mut self, snapshot: &FullSnapshot) {
        if let Some(cfg) = snapshot.overlay_config_next {
            self.max_size.set(cfg.max_size_mb.to_string());
        } else {
            self.max_size.set("");
        }
        if let Some(rt) = snapshot.overlay_runtime {
            self.warning.set(rt.warning_threshold_mb.to_string());
            self.critical.set(rt.critical_threshold_mb.to_string());
        } else {
            self.warning.set("");
            self.critical.set("");
        }
        self.dirty = false;
        self.error = None;
        self.ensure_unlocked_field(is_overlay_locked(snapshot));
    }

    pub fn ensure_unlocked_field(&mut self, locked: bool) {
        if locked && self.field == CacheField::MaxSize {
            self.field = CacheField::WarningThreshold;
        }
    }

    pub fn move_field(&mut self, delta: isize, locked: bool) {
        let mut idx = self.field.index() as isize;
        for _ in 0..CacheField::ALL.len() {
            idx = (idx + delta).rem_euclid(CacheField::ALL.len() as isize);
            let candidate = CacheField::ALL[idx as usize];
            if !(locked && candidate == CacheField::MaxSize) {
                self.field = candidate;
                return;
            }
        }
    }

    pub fn active_input_mut(&mut self, locked: bool) -> Option<&mut NumericField> {
        match self.field {
            CacheField::MaxSize if !locked => Some(&mut self.max_size),
            CacheField::WarningThreshold => Some(&mut self.warning),
            CacheField::CriticalThreshold => Some(&mut self.critical),
            _ => None,
        }
    }

    pub fn validate(&self, snapshot: &FullSnapshot) -> Result<(u32, u32, u32), String> {
        let parse = |s: &str, what: &str| -> Result<u32, String> {
            s.parse::<u32>()
                .map_err(|_| format!("{what}须为 0-4294967295 的整数"))
        };
        let locked = is_overlay_locked(snapshot);
        let max_mb = if locked {
            snapshot
                .overlay_config_next
                .map(|c| c.max_size_mb)
                .ok_or_else(|| "配置尚未加载完成，无法保存".to_string())?
        } else {
            parse(&self.max_size.text, "最大缓存")?
        };
        let warning_mb = parse(&self.warning.text, "警告阈值")?;
        let critical_mb = parse(&self.critical.text, "严重阈值")?;

        if max_mb == 0 {
            return Err("最大缓存必须大于 0".to_string());
        }
        if warning_mb >= critical_mb {
            return Err("警告阈值必须小于严重阈值".to_string());
        }
        if critical_mb > max_mb {
            return Err("严重阈值不能大于最大缓存".to_string());
        }
        Ok((max_mb, warning_mb, critical_mb))
    }
}

/// R1：筛选器当前启用时，覆盖类型与最大缓存锁定
pub fn is_overlay_locked(snapshot: &FullSnapshot) -> bool {
    snapshot
        .filter_state
        .map(|f| f.current_enabled)
        .unwrap_or(false)
}

/// 统一排除列表条目（文件/文件夹排除 或 注册表排除）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExclusionItem {
    File(FileExclusion),
    Registry(String),
}

impl ExclusionItem {
    pub fn display(&self) -> String {
        match self {
            ExclusionItem::File(f) => f.display(),
            ExclusionItem::Registry(k) => k.clone(),
        }
    }

    pub fn is_registry(&self) -> bool {
        matches!(self, ExclusionItem::Registry(_))
    }
}

/// 渲染时记录的可点击/可滚动区域（供鼠标精准交互）
#[derive(Debug, Default, Clone)]
pub struct UiRegions {
    // 顶部栏按钮
    pub commit_btn: Rect,
    pub reset_btn: Rect,
    pub refresh_btn: Rect,
    pub help_btn: Rect,
    pub quit_btn: Rect,

    // 1. 基本设置
    pub panel_basic: Rect,
    pub filter_toggle: Rect,
    pub overlay_type_toggle: Rect,

    // 2. 缓存设置
    pub panel_cache: Rect,
    pub max_size_input: Rect,
    pub warning_input: Rect,
    pub critical_input: Rect,
    pub cache_confirm_btn: Rect,

    // 3. 保护分区
    pub panel_volumes: Rect,
    pub volume_rows: Rect,
    pub protect_btn: Rect,
    pub unprotect_btn: Rect,

    // 4. 排除列表 + 底部动作按钮
    pub panel_exclusions: Rect,
    pub exclusion_rows: Rect,
    pub excl_add_file_btn: Rect,
    pub excl_add_reg_btn: Rect,
    pub excl_delete_btn: Rect,
    pub excl_view_btn: Rect,
    pub import_btn: Rect,
    pub export_btn: Rect,
    pub restart_btn: Rect,
    pub shutdown_btn: Rect,

    // 未安装引导页按钮
    pub ni_retry_btn: Rect,
    pub ni_quit_btn: Rect,
}

/// 当前打开的模态弹窗
pub enum Modal {
    Input(InputState),
    FilePicker(FilePickerState),
    Confirm(ConfirmState),
    CommitMenu(CommitMenuState),
    Help(HelpState),
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

/// 应用生命周期阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Loading,
    LoadFailed,
    NotInstalled,
    Ready,
    BackendLost,
}

pub struct App {
    pub focus: FocusPanel,
    pub basic_field: BasicField,
    pub cache: CacheForm,
    pub should_quit: bool,
    pub backend_kind: BackendKind,
    pub phase: Phase,
    pub snapshot: FullSnapshot,
    pub volumes_state: TableState,
    /// 排除列表视图：true = 当前生效（只读），false = 重启后生效（默认，可编辑）
    pub exclusion_session_current: bool,
    pub exclusion_state: ListState,
    /// 刷新/排序后用于恢复选中身份的稳定 key
    selected_exclusion_key: Option<ExclusionItem>,
    pub modal: Option<Modal>,
    pub last_log: Option<StatusEntry>,
    pub regions: UiRegions,
    /// 当前在途的变更请求；只有匹配 id 的 Finished / NotInstalled 才能清除
    pub mutation: Option<RequestId>,
    /// 记录提交中的缓存保存请求 id，成功后清除 dirty 并同步表单
    pending_cache_save: Option<RequestId>,
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
        let mut exclusion_state = ListState::default();
        exclusion_state.select(Some(0));
        Self {
            focus: FocusPanel::BasicSettings,
            basic_field: BasicField::WriteFilter,
            cache: CacheForm::default(),
            should_quit: false,
            backend_kind,
            phase: Phase::Loading,
            snapshot: FullSnapshot::default(),
            volumes_state,
            exclusion_session_current: false,
            exclusion_state,
            selected_exclusion_key: None,
            last_log: None,
            regions: UiRegions::default(),
            mutation: None,
            pending_cache_save: None,
            power_action: None,
            next_request_id: 1,
            last_overlay_tick: Instant::now(),
            modal: None,
            cmd_tx,
            ui_rx,
        }
    }

    pub fn is_busy(&self) -> bool {
        self.phase == Phase::Loading || self.is_pending()
    }

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

    fn mark_backend_lost(&mut self, message: &str) {
        let first = self.phase != Phase::BackendLost;
        self.phase = Phase::BackendLost;
        self.modal = None;
        self.mutation = None;
        self.pending_cache_save = None;
        self.power_action = None;
        if first {
            self.log(false, message);
        }
    }

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
            self.mark_backend_lost("后端线程已退出，无法刷新缓存监控");
        }
    }

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
                    while let Ok(evt) = self.ui_rx.try_recv() {
                        self.handle_ui_event(evt);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }

            if self.last_overlay_tick.elapsed() >= OVERLAY_POLL {
                self.last_overlay_tick = Instant::now();
                self.send_overlay_refresh();
            }
        }
        Ok(())
    }

    pub(crate) fn handle_ui_event(&mut self, evt: UiEvent) {
        match evt {
            UiEvent::Terminal(Event::Key(key))
                if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
            {
                self.on_key(key)
            }
            UiEvent::Terminal(Event::Mouse(mouse)) => self.on_mouse(mouse),
            UiEvent::Terminal(_) => {}
            UiEvent::Backend(evt) => self.apply_event(evt),
        }
    }

    fn apply_event(&mut self, evt: BackendEvent) {
        match evt {
            BackendEvent::Snapshot(snapshot) => {
                let first_ready = self.phase != Phase::Ready;
                self.snapshot = snapshot;
                self.phase = Phase::Ready;
                if first_ready || !self.cache.dirty {
                    self.cache.sync_from_snapshot(&self.snapshot);
                } else {
                    self.cache
                        .ensure_unlocked_field(is_overlay_locked(&self.snapshot));
                }
                self.clamp_selections();
                self.restore_selection();
                self.log(true, "已刷新 UWF 全部状态");
                if !self.snapshot.warnings.is_empty() {
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
                let is_cache_save = self.pending_cache_save == Some(id);
                if is_cache_save {
                    self.pending_cache_save = None;
                }
                match outcome {
                    Outcome::Ok(msg) => {
                        if is_cache_save {
                            self.cache.sync_from_snapshot(&self.snapshot);
                        }
                        self.log(true, &msg);
                        self.on_power_action_ok(id);
                    }
                    Outcome::OkWithRefreshWarning { message, warning } => {
                        if is_cache_save {
                            self.cache.sync_from_snapshot(&self.snapshot);
                        }
                        self.log(true, &message);
                        self.log_warn(&warning);
                        self.on_power_action_ok(id);
                    }
                    Outcome::Warning(msg) => {
                        self.log_warn(&msg);
                    }
                    Outcome::Err(msg) => {
                        if is_cache_save {
                            self.cache.error = Some(msg.clone());
                        }
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
                    if self.pending_cache_save == Some(req_id) {
                        self.pending_cache_save = None;
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
                self.cache.dirty = false;
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

    /// 当前排除列表视图下的全部条目（文件在前、注册表在后）
    pub fn visible_exclusions(&self) -> Vec<ExclusionItem> {
        let (files, regs) = if self.exclusion_session_current {
            (
                &self.snapshot.file_exclusions_current,
                &self.snapshot.registry_exclusions_current,
            )
        } else {
            (
                &self.snapshot.file_exclusions_next,
                &self.snapshot.registry_exclusions_next,
            )
        };
        let mut items = Vec::with_capacity(files.len() + regs.len());
        items.extend(files.iter().cloned().map(ExclusionItem::File));
        items.extend(regs.iter().cloned().map(ExclusionItem::Registry));
        items
    }

    pub fn selected_exclusion(&self) -> Option<ExclusionItem> {
        let items = self.visible_exclusions();
        self.exclusion_state
            .selected()
            .and_then(|i| items.get(i).cloned())
    }

    fn remember_selection(&mut self) {
        self.selected_exclusion_key = self.selected_exclusion();
    }

    fn restore_selection(&mut self) {
        let items = self.visible_exclusions();
        let hit = self
            .selected_exclusion_key
            .as_ref()
            .and_then(|key| items.iter().position(|item| item == key));
        match hit {
            Some(i) => self.exclusion_state.select(Some(i)),
            None => self.selected_exclusion_key = self.selected_exclusion(),
        }
    }

    fn clamp_selections(&mut self) {
        let volumes_len = self.snapshot.volumes.len();
        let exclusions_len = self.visible_exclusions().len();
        clamp(self.volumes_state.selected_mut(), volumes_len);
        clamp(self.exclusion_state.selected_mut(), exclusions_len);
    }

    fn selected_volume(&self) -> Option<&crate::model::VolumeInfo> {
        self.volumes_state
            .selected()
            .and_then(|i| self.snapshot.volumes.get(i))
    }

    // ── 键盘路由 ──

    fn on_key(&mut self, key: KeyEvent) {
        if self.modal.is_some() {
            self.on_modal_key(key);
            return;
        }
        match self.phase {
            Phase::NotInstalled => match key.code {
                KeyCode::F(5) => self.refresh_all(),
                KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => self.request_quit(),
                KeyCode::Char('?') | KeyCode::F(1) => {
                    self.modal = Some(Modal::Help(HelpState::default()))
                }
                _ => {}
            },
            Phase::BackendLost => match key.code {
                KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => self.should_quit = true,
                KeyCode::Char('?') | KeyCode::F(1) => {
                    self.modal = Some(Modal::Help(HelpState::default()))
                }
                _ => self.log(false, "后端线程已退出，请重启应用"),
            },
            Phase::Loading | Phase::LoadFailed => match key.code {
                KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => self.request_quit(),
                KeyCode::Tab => self.cycle_focus(true),
                KeyCode::BackTab => self.cycle_focus(false),
                KeyCode::Char('?') | KeyCode::F(1) => {
                    self.modal = Some(Modal::Help(HelpState::default()))
                }
                KeyCode::F(5) => self.refresh_all(),
                _ => {}
            },
            Phase::Ready => self.on_ready_key(key),
        }
    }

    fn on_ready_key(&mut self, key: KeyEvent) {
        // Alt+1..4 直达分区
        if key.modifiers.contains(KeyModifiers::ALT)
            && let KeyCode::Char(c @ '1'..='4') = key.code
        {
            self.focus = FocusPanel::ALL[c as usize - '1' as usize];
            self.on_focus_entered();
            return;
        }

        match key.code {
            KeyCode::Tab => self.cycle_focus(true),
            KeyCode::BackTab => self.cycle_focus(false),
            KeyCode::F(1) | KeyCode::Char('?') => {
                self.modal = Some(Modal::Help(HelpState::default()))
            }
            KeyCode::F(5) => self.refresh_all(),
            KeyCode::F(9) => self.open_reset_confirm(),
            KeyCode::F(10) => self.open_restart_confirm(),
            KeyCode::F(11) => self.open_shutdown_confirm(),
            KeyCode::Char('c') | KeyCode::Char('C') => self.open_commit_menu(),
            KeyCode::Char('i') | KeyCode::Char('I') => self.open_import_picker(),
            KeyCode::Char('x') | KeyCode::Char('X') => self.export_exclusions(),
            KeyCode::Char('q') | KeyCode::Char('Q') => self.request_quit(),
            KeyCode::Esc => {
                if self.focus == FocusPanel::CacheSettings && self.cache.dirty {
                    self.cache.sync_from_snapshot(&self.snapshot);
                    self.log_warn("已放弃未保存的缓存设置修改");
                } else {
                    self.request_quit();
                }
            }
            _ => match self.focus {
                FocusPanel::BasicSettings => self.on_basic_key(key),
                FocusPanel::CacheSettings => self.on_cache_key(key),
                FocusPanel::Volumes => self.on_volumes_key(key),
                FocusPanel::Exclusions => self.on_exclusions_key(key),
            },
        }
    }

    fn cycle_focus(&mut self, forward: bool) {
        self.focus = if forward {
            self.focus.next()
        } else {
            self.focus.prev()
        };
        self.on_focus_entered();
    }

    fn on_focus_entered(&mut self) {
        if self.focus == FocusPanel::CacheSettings {
            self.cache
                .ensure_unlocked_field(is_overlay_locked(&self.snapshot));
        }
    }

    fn on_basic_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Down => {
                self.basic_field = match self.basic_field {
                    BasicField::WriteFilter => BasicField::OverlayType,
                    BasicField::OverlayType => BasicField::WriteFilter,
                };
            }
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => {
                match self.basic_field {
                    BasicField::WriteFilter => self.toggle_write_filter(),
                    BasicField::OverlayType => self.toggle_overlay_type(),
                }
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                let _ = self.begin_mutation(BackendCmd::EnableFilter);
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                let _ = self.begin_mutation(BackendCmd::DisableFilter);
            }
            _ => {}
        }
    }

    fn on_cache_key(&mut self, key: KeyEvent) {
        let locked = is_overlay_locked(&self.snapshot);
        match key.code {
            KeyCode::Up => self.cache.move_field(-1, locked),
            KeyCode::Down => self.cache.move_field(1, locked),
            KeyCode::Enter => self.submit_cache_settings(),
            KeyCode::Char(' ') if self.cache.field == CacheField::ConfirmButton => {
                self.submit_cache_settings()
            }
            _ => {
                if let Some(field) = self.cache.active_input_mut(locked) {
                    let changed = field.on_key(key);
                    if changed {
                        self.cache.dirty = true;
                        self.cache.error = None;
                    }
                }
            }
        }
    }

    fn on_volumes_key(&mut self, key: KeyEvent) {
        let len = self.snapshot.volumes.len();
        match key.code {
            KeyCode::Up => move_selection(self.volumes_state.selected_mut(), len, -1),
            KeyCode::Down => move_selection(self.volumes_state.selected_mut(), len, 1),
            KeyCode::Home => select_edge(self.volumes_state.selected_mut(), len, false),
            KeyCode::End => select_edge(self.volumes_state.selected_mut(), len, true),
            KeyCode::Char('p') | KeyCode::Char('P') => self.protect_selected(true),
            KeyCode::Char('u') | KeyCode::Char('U') => self.protect_selected(false),
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(vol) = self.selected_volume() {
                    let target = !vol.next_protected;
                    self.protect_selected(target);
                }
            }
            _ => {}
        }
    }

    fn on_exclusions_key(&mut self, key: KeyEvent) {
        let len = self.visible_exclusions().len();
        match key.code {
            KeyCode::Up => {
                move_selection(self.exclusion_state.selected_mut(), len, -1);
                self.remember_selection();
            }
            KeyCode::Down => {
                move_selection(self.exclusion_state.selected_mut(), len, 1);
                self.remember_selection();
            }
            KeyCode::Home => {
                select_edge(self.exclusion_state.selected_mut(), len, false);
                self.remember_selection();
            }
            KeyCode::End => {
                select_edge(self.exclusion_state.selected_mut(), len, true);
                self.remember_selection();
            }
            KeyCode::Char('s') | KeyCode::Char('S') => self.toggle_exclusion_view(),
            KeyCode::Char('a') | KeyCode::Char('A') => self.open_add_file_exclusion(),
            KeyCode::Char('r') | KeyCode::Char('R') => self.open_add_registry_exclusion(),
            KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Delete
                if self.ensure_mutable_exclusions() =>
            {
                self.delete_selected_exclusion();
            }
            _ => {}
        }
    }

    // ── 鼠标路由 ──

    fn on_mouse(&mut self, mouse: MouseEvent) {
        if let Some(modal) = self.modal.take() {
            self.on_modal_mouse(modal, mouse);
            return;
        }
        if self.phase == Phase::NotInstalled {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                let pos = Position::new(mouse.column, mouse.row);
                if self.regions.ni_retry_btn.contains(pos) {
                    self.refresh_all();
                } else if self.regions.ni_quit_btn.contains(pos) {
                    self.request_quit();
                }
            }
            return;
        }
        if self.phase == Phase::BackendLost {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                let pos = Position::new(mouse.column, mouse.row);
                if self.regions.quit_btn.contains(pos) {
                    self.should_quit = true;
                } else if self.regions.help_btn.contains(pos) {
                    self.modal = Some(Modal::Help(HelpState::default()));
                }
            }
            return;
        }

        let pos = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollUp if self.phase == Phase::Ready => {
                if self.regions.panel_volumes.contains(pos) {
                    self.focus = FocusPanel::Volumes;
                    let len = self.snapshot.volumes.len();
                    move_selection(self.volumes_state.selected_mut(), len, -1);
                } else if self.regions.panel_exclusions.contains(pos) {
                    self.focus = FocusPanel::Exclusions;
                    let len = self.visible_exclusions().len();
                    move_selection(self.exclusion_state.selected_mut(), len, -1);
                    self.remember_selection();
                }
            }
            MouseEventKind::ScrollDown if self.phase == Phase::Ready => {
                if self.regions.panel_volumes.contains(pos) {
                    self.focus = FocusPanel::Volumes;
                    let len = self.snapshot.volumes.len();
                    move_selection(self.volumes_state.selected_mut(), len, 1);
                } else if self.regions.panel_exclusions.contains(pos) {
                    self.focus = FocusPanel::Exclusions;
                    let len = self.visible_exclusions().len();
                    move_selection(self.exclusion_state.selected_mut(), len, 1);
                    self.remember_selection();
                }
            }
            MouseEventKind::Down(MouseButton::Left) => self.on_left_click(pos),
            _ => {}
        }
    }

    fn on_left_click(&mut self, pos: Position) {
        // 顶部通用按钮在 Loading / LoadFailed / Ready 均可用
        if self.regions.refresh_btn.contains(pos) {
            self.refresh_all();
            return;
        }
        if self.regions.help_btn.contains(pos) {
            self.modal = Some(Modal::Help(HelpState::default()));
            return;
        }
        if self.regions.quit_btn.contains(pos) {
            self.request_quit();
            return;
        }
        if self.phase != Phase::Ready {
            return;
        }

        if self.regions.commit_btn.contains(pos) {
            self.open_commit_menu();
        } else if self.regions.reset_btn.contains(pos) {
            self.open_reset_confirm();
        } else if self.regions.filter_toggle.contains(pos) {
            self.focus = FocusPanel::BasicSettings;
            self.basic_field = BasicField::WriteFilter;
            self.toggle_write_filter();
        } else if self.regions.overlay_type_toggle.contains(pos) {
            self.focus = FocusPanel::BasicSettings;
            self.basic_field = BasicField::OverlayType;
            self.toggle_overlay_type();
        } else if self.regions.max_size_input.contains(pos) {
            self.focus = FocusPanel::CacheSettings;
            if is_overlay_locked(&self.snapshot) {
                self.log(false, "筛选器当前已启用，最大缓存不可修改");
            } else {
                self.cache.field = CacheField::MaxSize;
                self.cache.max_size.move_end();
            }
        } else if self.regions.warning_input.contains(pos) {
            self.focus = FocusPanel::CacheSettings;
            self.cache.field = CacheField::WarningThreshold;
            self.cache.warning.move_end();
        } else if self.regions.critical_input.contains(pos) {
            self.focus = FocusPanel::CacheSettings;
            self.cache.field = CacheField::CriticalThreshold;
            self.cache.critical.move_end();
        } else if self.regions.cache_confirm_btn.contains(pos) {
            self.focus = FocusPanel::CacheSettings;
            self.cache.field = CacheField::ConfirmButton;
            self.submit_cache_settings();
        } else if self.regions.volume_rows.contains(pos) {
            self.focus = FocusPanel::Volumes;
            let row = (pos.y - self.regions.volume_rows.y) as usize + self.volumes_state.offset();
            if row < self.snapshot.volumes.len() {
                self.volumes_state.select(Some(row));
            }
        } else if self.regions.protect_btn.contains(pos) {
            self.focus = FocusPanel::Volumes;
            self.protect_selected(true);
        } else if self.regions.unprotect_btn.contains(pos) {
            self.focus = FocusPanel::Volumes;
            self.protect_selected(false);
        } else if self.regions.exclusion_rows.contains(pos) {
            self.focus = FocusPanel::Exclusions;
            let items = self.visible_exclusions();
            let row =
                (pos.y - self.regions.exclusion_rows.y) as usize + self.exclusion_state.offset();
            if row < items.len() {
                self.exclusion_state.select(Some(row));
                self.remember_selection();
            }
        } else if self.regions.excl_add_file_btn.contains(pos) {
            self.focus = FocusPanel::Exclusions;
            self.open_add_file_exclusion();
        } else if self.regions.excl_add_reg_btn.contains(pos) {
            self.focus = FocusPanel::Exclusions;
            self.open_add_registry_exclusion();
        } else if self.regions.excl_delete_btn.contains(pos) {
            self.focus = FocusPanel::Exclusions;
            if self.ensure_mutable_exclusions() {
                self.delete_selected_exclusion();
            }
        } else if self.regions.excl_view_btn.contains(pos) {
            self.focus = FocusPanel::Exclusions;
            self.toggle_exclusion_view();
        } else if self.regions.import_btn.contains(pos) {
            self.focus = FocusPanel::Exclusions;
            self.open_import_picker();
        } else if self.regions.export_btn.contains(pos) {
            self.focus = FocusPanel::Exclusions;
            self.export_exclusions();
        } else if self.regions.restart_btn.contains(pos) {
            self.open_restart_confirm();
        } else if self.regions.shutdown_btn.contains(pos) {
            self.open_shutdown_confirm();
        } else if self.regions.panel_basic.contains(pos) {
            self.focus = FocusPanel::BasicSettings;
        } else if self.regions.panel_cache.contains(pos) {
            self.focus = FocusPanel::CacheSettings;
            self.on_focus_entered();
        } else if self.regions.panel_volumes.contains(pos) {
            self.focus = FocusPanel::Volumes;
        } else if self.regions.panel_exclusions.contains(pos) {
            self.focus = FocusPanel::Exclusions;
        }
    }

    // ── 业务动作分派 ──

    fn toggle_write_filter(&mut self) {
        let Some(state) = self.snapshot.filter_state else {
            return;
        };
        let cmd = if state.next_enabled {
            BackendCmd::DisableFilter
        } else {
            BackendCmd::EnableFilter
        };
        let _ = self.begin_mutation(cmd);
    }

    fn toggle_overlay_type(&mut self) {
        if is_overlay_locked(&self.snapshot) {
            self.log(false, "筛选器当前已启用，需禁用并重启后才可修改覆盖类型");
            return;
        }
        let Some(cfg) = self.snapshot.overlay_config_next else {
            return;
        };
        let next_type = match cfg.overlay_type {
            OverlayType::Ram => OverlayType::Disk,
            OverlayType::Disk => OverlayType::Ram,
        };
        let _ = self.begin_mutation(BackendCmd::SetOverlayType(next_type));
    }

    fn submit_cache_settings(&mut self) {
        match self.cache.validate(&self.snapshot) {
            Ok((max_mb, warning_mb, critical_mb)) => {
                self.cache.error = None;
                if let Some(id) = self.begin_mutation(BackendCmd::SetOverlayCache {
                    max_mb,
                    warning_mb,
                    critical_mb,
                }) {
                    self.pending_cache_save = Some(id);
                }
            }
            Err(msg) => {
                self.cache.error = Some(msg.clone());
                self.log(false, &msg);
            }
        }
    }

    fn protect_selected(&mut self, protect: bool) {
        let Some(drive) = self.selected_volume().map(|v| v.drive_letter.clone()) else {
            return;
        };
        let cmd = if protect {
            BackendCmd::Protect(drive)
        } else {
            BackendCmd::Unprotect(drive)
        };
        let _ = self.begin_mutation(cmd);
    }

    fn toggle_exclusion_view(&mut self) {
        self.exclusion_session_current = !self.exclusion_session_current;
        self.clamp_selections();
        if !self.exclusion_session_current {
            self.restore_selection();
        }
    }

    fn open_add_file_exclusion(&mut self) {
        if !self.ensure_mutable_exclusions() {
            return;
        }
        self.open_action_modal(Modal::Input(InputState::new(
            "添加文件/目录排除",
            "输入绝对路径（如 C:\\Windows\\Temp），或按 [F2] / 点击浏览选择",
            InputAction::AddFileExclusion,
            true,
        )));
    }

    fn open_add_registry_exclusion(&mut self) {
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

    fn open_import_picker(&mut self) {
        if !self.ensure_mutable_exclusions() {
            return;
        }
        self.open_action_modal(Modal::FilePicker(FilePickerState::new(
            PickerMode::File,
            PickerAction::ImportExclusions,
        )));
    }

    fn open_commit_menu(&mut self) {
        self.open_action_modal(Modal::CommitMenu(CommitMenuState::new()));
    }

    fn open_reset_confirm(&mut self) {
        self.open_action_modal(Modal::Confirm(ConfirmState::new(
            "重置配置确认",
            "将重置全部 UWF 配置（写入过滤 / 缓存 / 分区保护 / 排除项），\n下次重启后恢复默认值。确定继续？",
            ConfirmAction::ResetSettings,
        )));
    }

    fn open_restart_confirm(&mut self) {
        self.open_action_modal(Modal::Confirm(ConfirmState::new(
            "重启系统确认",
            "系统将立即重启，未保存的工作将丢失。\n确定继续？",
            ConfirmAction::Restart,
        )));
    }

    fn open_shutdown_confirm(&mut self) {
        self.open_action_modal(Modal::Confirm(ConfirmState::new(
            "关机确认",
            "系统将立即关机，未保存的工作将丢失。\n确定继续？",
            ConfirmAction::Shutdown,
        )));
    }

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

    fn ensure_mutable_exclusions(&mut self) -> bool {
        if self.exclusion_session_current {
            self.log(
                false,
                "当前生效视图只读，请按 [S] 或点击视图按钮切换到重启后视图",
            );
            false
        } else {
            true
        }
    }

    // ── 模态按键/鼠标与结果执行 ──

    fn on_modal_key(&mut self, key: KeyEvent) {
        let Some(modal) = self.modal.take() else {
            return;
        };
        match modal {
            Modal::Help(state) => {
                if !state.on_key(key) {
                    self.modal = Some(Modal::Help(state));
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
            Modal::CommitMenu(mut state) => match state.on_key(key) {
                CommitMenuOutcome::Stay => self.modal = Some(Modal::CommitMenu(state)),
                CommitMenuOutcome::Cancel => {}
                CommitMenuOutcome::Select(choice) => self.execute_commit_choice(choice),
            },
        }
    }

    fn on_modal_mouse(&mut self, modal: Modal, mouse: MouseEvent) {
        match modal {
            Modal::Help(state) => {
                if !state.on_mouse(mouse) {
                    self.modal = Some(Modal::Help(state));
                }
            }
            Modal::Confirm(mut state) => match state.on_mouse(mouse) {
                ConfirmOutcome::Stay => self.modal = Some(Modal::Confirm(state)),
                ConfirmOutcome::Cancel => {}
                ConfirmOutcome::Confirm(action) => self.execute_confirm(action),
            },
            Modal::Input(mut state) => match state.on_mouse(mouse) {
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
            Modal::FilePicker(mut state) => match state.on_mouse(mouse) {
                PickerOutcome::Stay => self.modal = Some(Modal::FilePicker(state)),
                PickerOutcome::Cancel => {}
                PickerOutcome::Pick(action, path) => self.execute_pick(action, path),
            },
            Modal::CommitMenu(mut state) => match state.on_mouse(mouse) {
                CommitMenuOutcome::Stay => self.modal = Some(Modal::CommitMenu(state)),
                CommitMenuOutcome::Cancel => {}
                CommitMenuOutcome::Select(choice) => self.execute_commit_choice(choice),
            },
        }
    }

    fn execute_commit_choice(&mut self, choice: CommitChoice) {
        match choice {
            CommitChoice::File => {
                self.open_action_modal(Modal::FilePicker(FilePickerState::new(
                    PickerMode::File,
                    PickerAction::CommitFile,
                )));
            }
            CommitChoice::FileDeletion => {
                self.open_action_modal(Modal::FilePicker(FilePickerState::new(
                    PickerMode::File,
                    PickerAction::CommitFileDeletion,
                )));
            }
            CommitChoice::Registry => {
                self.open_action_modal(Modal::Input(InputState::new(
                    "注册表提交",
                    "输入注册表完整路径（如 HKLM\\SOFTWARE\\Example\\ValueName）",
                    InputAction::CommitRegistry,
                    false,
                )));
            }
        }
    }

    fn execute_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::ResetSettings => {
                if let Some(id) = self.begin_mutation(BackendCmd::ResetSettings) {
                    self.pending_cache_save = Some(id);
                }
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
        match self.selected_exclusion() {
            Some(ExclusionItem::File(f)) => {
                let _ = self.begin_mutation(BackendCmd::RemoveFileExclusion(f.display()));
            }
            Some(ExclusionItem::Registry(k)) => {
                let _ = self.begin_mutation(BackendCmd::RemoveRegistryExclusion(k));
            }
            None => {}
        }
    }

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
    use crate::model::{
        FileExclusion, FilterState, OverlayConfig, OverlayRuntime, OverlayType, VolumeInfo,
    };
    use crossterm::event;

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

    pub(crate) fn ready_app(kind: BackendKind) -> (App, Receiver<BackendCommand>) {
        let (mut app, cmd_rx) = test_app(kind);
        app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
        (app, cmd_rx)
    }

    #[test]
    fn focus_panel_navigation_wraps() {
        assert_eq!(FocusPanel::BasicSettings.next(), FocusPanel::CacheSettings);
        assert_eq!(FocusPanel::Exclusions.next(), FocusPanel::BasicSettings);
        assert_eq!(FocusPanel::BasicSettings.prev(), FocusPanel::Exclusions);
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
        assert_eq!(sel, Some(2));
        move_selection(&mut sel, 3, 1);
        assert_eq!(sel, Some(0));
        let mut sel = Some(0);
        move_selection(&mut sel, 0, 1);
        assert_eq!(sel, None);
    }

    #[test]
    fn not_installed_event_shows_guide_and_snapshot_recovers() {
        let (mut app, _cmd_rx) = test_app(BackendKind::Mock);
        assert!(app.is_busy());
        app.apply_event(BackendEvent::NotInstalled(None));
        assert_eq!(app.phase, Phase::NotInstalled);
        assert!(!app.is_busy());
        assert!(
            app.last_log
                .as_ref()
                .is_some_and(|e| e.level == LogLevel::Err)
        );
        app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
        assert_eq!(app.phase, Phase::Ready);
    }

    #[test]
    fn initial_load_error_and_retry_failure_transition_cleanly() {
        let (mut app, cmd_rx) = test_app(BackendKind::Mock);
        app.apply_event(BackendEvent::RefreshFailed("temporary WMI failure".into()));
        assert!(!app.is_busy());
        assert_eq!(app.phase, Phase::LoadFailed);

        app.refresh_all();
        assert!(matches!(cmd_rx.try_recv(), Ok(BackendCommand::RefreshAll)));
        assert_eq!(app.phase, Phase::Loading);
        app.apply_event(BackendEvent::RefreshFailed("second failure".into()));
        assert_eq!(app.phase, Phase::LoadFailed);

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

        app.apply_event(BackendEvent::RefreshFailed("刷新失败".into()));
        assert_eq!(app.mutation, Some(mutation));

        app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
        assert_eq!(app.mutation, Some(mutation));

        app.apply_event(BackendEvent::NotInstalled(None));
        assert_eq!(app.mutation, Some(mutation));

        app.apply_event(BackendEvent::NotInstalled(Some(mutation)));
        assert_eq!(app.mutation, None);

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
        app.on_key(key(KeyCode::Char('e')));
        let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
            panic!("期望 Mutate 命令");
        };
        assert!(matches!(cmd, BackendCmd::EnableFilter));
        assert_eq!(app.mutation, Some(id));

        app.on_key(key(KeyCode::Char('d')));
        assert!(cmd_rx.try_recv().is_err());

        app.apply_event(BackendEvent::Finished {
            id,
            outcome: Outcome::Ok("ok".into()),
        });
        assert!(!app.is_pending());
        app.on_key(key(KeyCode::Char('d')));
        assert!(matches!(recv_cmd(&cmd_rx), BackendCmd::DisableFilter));
    }

    #[test]
    fn power_action_not_armed_when_mutation_is_pending() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        let first = app.begin_mutation(BackendCmd::EnableFilter).unwrap();
        let _ = cmd_rx.try_recv().unwrap();

        app.execute_confirm(ConfirmAction::Restart);
        assert!(cmd_rx.try_recv().is_err());
        assert!(app.power_action.is_none());

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
            KeyCode::Char('c'),
            KeyCode::Char('e'),
            KeyCode::Char('a'),
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
        assert!(cmd_rx.try_recv().is_err());

        app.apply_event(BackendEvent::NotInstalled(None));
        app.send_overlay_refresh();
        assert!(cmd_rx.try_recv().is_err());

        app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
        app.send_overlay_refresh();
        assert!(matches!(
            cmd_rx.try_recv(),
            Ok(BackendCommand::RefreshOverlayRuntime)
        ));

        app.apply_event(BackendEvent::Disconnected);
        app.send_overlay_refresh();
        assert!(cmd_rx.try_recv().is_err());
    }

    #[test]
    fn global_keys_focus_help_refresh_quit() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.focus, FocusPanel::CacheSettings);
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(app.focus, FocusPanel::BasicSettings);
        app.on_key(key(KeyCode::Char('?')));
        assert!(matches!(app.modal, Some(Modal::Help(_))));
        app.on_key(key(KeyCode::Esc));
        assert!(app.modal.is_none());
        app.on_key(key(KeyCode::F(5)));
        assert!(matches!(cmd_rx.try_recv(), Ok(BackendCommand::RefreshAll)));
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn basic_settings_toggles_write_filter_and_respects_r1_lock() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        // 写入过滤默认启用 -> Enter 发送 DisableFilter
        app.on_key(key(KeyCode::Enter));
        let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
            panic!("期望 Mutate");
        };
        assert!(matches!(cmd, BackendCmd::DisableFilter));
        app.apply_event(BackendEvent::Finished {
            id,
            outcome: Outcome::Ok("ok".into()),
        });

        // 切到覆盖类型：current_enabled = true 时 R1 锁定
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.basic_field, BasicField::OverlayType);
        app.on_key(key(KeyCode::Enter));
        assert!(cmd_rx.try_recv().is_err());

        // 解锁后（current_enabled = false）允许切换覆盖类型
        let mut snap = demo_snapshot();
        snap.filter_state = Some(FilterState {
            current_enabled: false,
            next_enabled: false,
        });
        app.apply_event(BackendEvent::Snapshot(snap));
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(
            recv_cmd(&cmd_rx),
            BackendCmd::SetOverlayType(OverlayType::Disk)
        ));
    }

    #[test]
    fn cache_form_validation_dirty_flag_and_esc_discard() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.focus = FocusPanel::CacheSettings;
        app.on_focus_entered();
        // current_enabled == true 时聚焦落在警告阈值
        assert_eq!(app.cache.field, CacheField::WarningThreshold);

        // 输入非法阈值（警告 >= 严重）
        app.cache.warning.set("2000");
        app.cache.dirty = true;
        app.on_key(key(KeyCode::Enter));
        assert!(cmd_rx.try_recv().is_err());
        assert!(
            app.cache
                .error
                .as_deref()
                .is_some_and(|e| e.contains("必须小于"))
        );

        // 正在编辑时收到后台刷新，不冲掉输入内容
        app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
        assert_eq!(app.cache.warning.text, "2000");

        // Esc 放弃未保存修改，而不是直接退出
        app.on_key(key(KeyCode::Esc));
        assert!(!app.should_quit);
        assert!(!app.cache.dirty);
        assert_eq!(app.cache.warning.text, "1536");

        // 再次合法编辑并 Enter 提交
        app.on_key(key(KeyCode::Backspace));
        app.on_key(key(KeyCode::Char('0'))); // 1530
        assert!(app.cache.dirty);
        app.on_key(key(KeyCode::Enter));
        match recv_cmd(&cmd_rx) {
            BackendCmd::SetOverlayCache {
                max_mb,
                warning_mb,
                critical_mb,
            } => {
                assert_eq!((max_mb, warning_mb, critical_mb), (2048, 1530, 1843));
            }
            other => panic!("期望 SetOverlayCache，实际 {other:?}"),
        }
    }

    #[test]
    fn volumes_protect_selected_drive() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.focus = FocusPanel::Volumes;
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
        app.focus = FocusPanel::Exclusions;
        assert!(!app.exclusion_session_current);
        app.on_key(key(KeyCode::Char('s')));
        assert!(app.exclusion_session_current);
        app.on_key(key(KeyCode::Char('s')));

        // 统一列表中第 0 项为文件排除，第 1 项为注册表排除
        app.on_key(key(KeyCode::Char('d')));
        let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
            panic!("期望 Mutate 命令");
        };
        match cmd {
            BackendCmd::RemoveFileExclusion(path) => assert_eq!(path, "C:\\Windows\\Temp"),
            other => panic!("期望 RemoveFileExclusion，实际 {other:?}"),
        }
        app.apply_event(BackendEvent::Finished {
            id,
            outcome: Outcome::Ok("ok".into()),
        });

        // 下移选中注册表项并删除
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Char('d')));
        match recv_cmd(&cmd_rx) {
            BackendCmd::RemoveRegistryExclusion(key) => {
                assert_eq!(key, "HKLM\\SOFTWARE\\UwfManagerDemo")
            }
            other => panic!("期望 RemoveRegistryExclusion，实际 {other:?}"),
        }
    }

    #[test]
    fn current_session_view_is_read_only() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.focus = FocusPanel::Exclusions;
        app.on_key(key(KeyCode::Char('s')));
        assert!(app.exclusion_session_current);

        for code in [
            KeyCode::Char('a'),
            KeyCode::Char('r'),
            KeyCode::Char('d'),
            KeyCode::Char('i'),
        ] {
            app.on_key(key(code));
            assert!(app.modal.is_none());
            assert!(cmd_rx.try_recv().is_err());
        }

        app.on_key(key(KeyCode::Char('s')));
        app.on_key(key(KeyCode::Char('d')));
        assert!(matches!(
            recv_cmd(&cmd_rx),
            BackendCmd::RemoveFileExclusion(_)
        ));
    }

    #[test]
    fn commit_menu_registry_input_flow() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.on_key(key(KeyCode::Char('c')));
        assert!(matches!(app.modal, Some(Modal::CommitMenu(_))));
        app.on_key(key(KeyCode::Char('3')));
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
    fn restart_flow_mock_stays_windows_quits() {
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
    fn selection_identity_survives_reorder_and_session_toggle() {
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        assert_eq!(
            app.selected_exclusion_key,
            Some(ExclusionItem::File(FileExclusion::new(
                "C:".into(),
                "\\Windows\\Temp".into()
            )))
        );
        app.focus = FocusPanel::Exclusions;
        let mut snap = demo_snapshot();
        snap.file_exclusions_next = vec![
            FileExclusion::new("C:".into(), "\\A".into()),
            FileExclusion::new("C:".into(), "\\B".into()),
        ];
        app.apply_event(BackendEvent::Snapshot(snap));
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.exclusion_state.selected(), Some(1));

        // 切到空当前视图再切回，必须保持选中 \B
        app.on_key(key(KeyCode::Char('s')));
        assert_eq!(app.exclusion_state.selected(), None);
        app.on_key(key(KeyCode::Char('s')));
        assert_eq!(app.exclusion_state.selected(), Some(1));
        assert_eq!(
            app.selected_exclusion_key,
            Some(ExclusionItem::File(FileExclusion::new(
                "C:".into(),
                "\\B".into()
            )))
        );
    }

    #[test]
    fn backend_disconnect_marks_lost_closes_modal_and_blocks_mutations() {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (_ui_tx, ui_rx) = std::sync::mpsc::channel();
        let mut app = App::new(cmd_tx, ui_rx, BackendKind::Mock);
        app.modal = Some(Modal::Help(HelpState::default()));
        assert!(app.is_busy());

        app.apply_event(BackendEvent::Disconnected);
        assert_eq!(app.phase, Phase::BackendLost);
        assert!(app.modal.is_none());
        assert!(!app.is_busy());

        app.on_key(key(KeyCode::Char('e')));
        assert!(cmd_rx.try_recv().is_err());
        app.on_key(key(KeyCode::Esc));
        assert!(app.should_quit);
    }

    #[test]
    fn repeat_events_advance_selection_and_release_is_ignored() {
        let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
        app.focus = FocusPanel::Volumes;
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
}
