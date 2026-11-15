//! 主应用状态机与 `eframe::App` 实现：协调后端命令/回包、表单校验、模态弹窗与视图交互。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::backend::{
    BackendCmd, BackendCommand, BackendEvent, BackendKind, FullSnapshot, Outcome, RequestId,
};
use crate::model::{FileExclusion, OverlayType, VolumeInfo};
use crate::ui::components::commit_menu::CommitChoice;
use crate::ui::components::confirm::{ConfirmAction, ConfirmState};
use crate::ui::components::input_dialog::{BrowseKind, InputAction, InputState};
use crate::ui::file_dialog::{
    FileDialogKind, FileDialogService, FileDialogTarget, RfdFileDialogService,
};
use crate::ui::theme;

/// Overlay 运行时用量后台刷新周期（2 秒）
const OVERLAY_POLL: Duration = Duration::from_secs(2);
/// 底层状态无事可做时的最大周期唤醒间隔（兼顾 2s 轮询与低空转）
const IDLE_REPAINT: Duration = Duration::from_millis(500);
/// 操作日志抽屉最大保留条数
const MAX_LOG_ENTRIES: usize = 50;

/// 写入缓存设置表单状态
#[derive(Debug, Default)]
pub struct CacheForm {
    pub max_size_text: String,
    pub warning_text: String,
    pub critical_text: String,
    /// 是否有未保存编辑；为 true 时自动刷新不覆盖用户输入
    pub dirty: bool,
    pub error: Option<String>,
}

impl CacheForm {
    pub fn sync_from_snapshot(&mut self, snapshot: &FullSnapshot) {
        self.max_size_text = snapshot
            .overlay_config_next
            .map(|c| c.max_size_mb.to_string())
            .unwrap_or_default();
        if let Some(rt) = snapshot.overlay_runtime {
            self.warning_text = rt.warning_threshold_mb.to_string();
            self.critical_text = rt.critical_threshold_mb.to_string();
        } else {
            self.warning_text.clear();
            self.critical_text.clear();
        }
        self.dirty = false;
        self.error = None;
    }

    pub fn on_edited(&mut self) {
        self.dirty = true;
        self.error = None;
    }

    pub fn validate(&self, snapshot: &FullSnapshot) -> Result<(u32, u32, u32), String> {
        let parse = |s: &str, what: &str| -> Result<u32, String> {
            s.trim()
                .parse::<u32>()
                .map_err(|_| format!("{what}须为 0-4294967295 的整数"))
        };
        let locked = is_overlay_locked(snapshot);
        let max_mb = if locked {
            snapshot
                .overlay_config_next
                .map(|c| c.max_size_mb)
                .ok_or_else(|| "配置尚未加载完成，无法保存".to_string())?
        } else {
            parse(&self.max_size_text, "最大容量")?
        };
        let warning_mb = parse(&self.warning_text, "警告阈值")?;
        let critical_mb = parse(&self.critical_text, "严重阈值")?;

        if max_mb == 0 {
            return Err("最大容量必须大于 0 MB".to_string());
        }
        if warning_mb >= critical_mb {
            return Err("警告阈值必须小于严重阈值".to_string());
        }
        if critical_mb > max_mb {
            return Err("严重阈值不能大于最大容量".to_string());
        }
        Ok((max_mb, warning_mb, critical_mb))
    }
}

/// R1：筛选器当前会话已启用时，缓存模式与最大容量锁定
pub fn is_overlay_locked(snapshot: &FullSnapshot) -> bool {
    snapshot.filter_state.is_some_and(|f| f.current_enabled)
}

/// 排除列表类型展示过滤
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExclusionFilter {
    #[default]
    All,
    Files,
    Registry,
}

/// 统一排除列表条目（文件/目录排除 或 注册表排除）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExclusionItem {
    File(FileExclusion),
    Registry(String),
}

impl ExclusionItem {
    pub fn display(&self) -> String {
        match self {
            Self::File(f) => f.display(),
            Self::Registry(k) => k.clone(),
        }
    }

    pub fn is_registry(&self) -> bool {
        matches!(self, Self::Registry(_))
    }
}

/// 当前打开的模态窗口
pub enum Modal {
    CommitMenu,
    Input(InputState),
    Confirm(ConfirmState),
    Help,
}

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

#[derive(Debug, Clone)]
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

pub struct UwfApp {
    pub backend_kind: BackendKind,
    pub phase: Phase,
    pub snapshot: FullSnapshot,
    pub cache: CacheForm,
    pub selected_volume_drive: Option<String>,
    /// 排除列表视图：true = 当前生效（只读），false = 重启后生效（默认，可编辑）
    pub exclusion_session_current: bool,
    pub exclusion_filter: ExclusionFilter,
    /// 跨刷新/切换视图保持选中身份的稳定 key
    pub selected_exclusion_key: Option<ExclusionItem>,
    pub modal: Option<Modal>,
    pub logs: VecDeque<StatusEntry>,
    pub show_log_drawer: bool,
    /// 当前在途变更请求 id；仅匹配回包可解除锁定
    pub mutation: Option<RequestId>,
    pending_cache_save: Option<RequestId>,
    power_action: Option<(RequestId, PowerAction)>,
    next_request_id: RequestId,
    last_overlay_tick: Instant,
    pub should_quit: bool,
    force_close_confirmed: bool,
    cmd_tx: Sender<BackendCommand>,
    event_rx: Receiver<BackendEvent>,
    file_dialog: Box<dyn FileDialogService>,
    active_file_dialog: Option<(FileDialogTarget, Receiver<Option<PathBuf>>)>,
}

impl UwfApp {
    pub fn new(
        cmd_tx: Sender<BackendCommand>,
        event_rx: Receiver<BackendEvent>,
        backend_kind: BackendKind,
    ) -> Self {
        Self::new_with_file_dialog(
            cmd_tx,
            event_rx,
            backend_kind,
            Box::new(RfdFileDialogService),
        )
    }

    pub fn new_with_file_dialog(
        cmd_tx: Sender<BackendCommand>,
        event_rx: Receiver<BackendEvent>,
        backend_kind: BackendKind,
        file_dialog: Box<dyn FileDialogService>,
    ) -> Self {
        Self {
            backend_kind,
            phase: Phase::Loading,
            snapshot: FullSnapshot::default(),
            cache: CacheForm::default(),
            selected_volume_drive: None,
            exclusion_session_current: false,
            exclusion_filter: ExclusionFilter::All,
            selected_exclusion_key: None,
            modal: None,
            logs: VecDeque::with_capacity(MAX_LOG_ENTRIES),
            show_log_drawer: false,
            mutation: None,
            pending_cache_save: None,
            power_action: None,
            next_request_id: 1,
            last_overlay_tick: Instant::now(),
            should_quit: false,
            force_close_confirmed: false,
            cmd_tx,
            event_rx,
            file_dialog,
            active_file_dialog: None,
        }
    }

    pub fn is_busy(&self) -> bool {
        self.phase == Phase::Loading || self.is_pending()
    }

    pub fn is_pending(&self) -> bool {
        self.mutation.is_some()
    }

    pub fn is_file_dialog_open(&self) -> bool {
        self.active_file_dialog.is_some()
    }

    pub fn last_log(&self) -> Option<&StatusEntry> {
        self.logs.back()
    }

    pub fn push_ok(&mut self, msg: impl AsRef<str>) {
        self.log(true, msg.as_ref());
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

    pub fn send_overlay_refresh(&mut self) {
        if self.phase != Phase::Ready || self.is_pending() {
            return;
        }
        if !self.send(BackendCommand::RefreshOverlayRuntime) {
            self.mark_backend_lost("后端线程已退出，无法刷新缓存监控");
        }
    }

    pub fn begin_mutation(&mut self, cmd: BackendCmd) -> Option<RequestId> {
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

    // ── 后端与原生对话框事件排空 ──

    pub fn drain_events(&mut self) {
        while let Ok(evt) = self.event_rx.try_recv() {
            self.apply_event(evt);
        }
    }

    fn poll_file_dialog(&mut self) {
        let Some((target, rx)) = self.active_file_dialog.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Some(path)) => match target {
                FileDialogTarget::InputFill => {
                    if let Some(Modal::Input(state)) = &mut self.modal {
                        state.set_browsed_path(&path);
                    }
                }
                FileDialogTarget::ImportExclusions => {
                    let _ = self.begin_mutation(BackendCmd::ImportExclusions { path });
                }
                FileDialogTarget::ExportExclusions => {
                    let _ = self.begin_mutation(BackendCmd::ExportExclusions {
                        path,
                        current_session: self.exclusion_session_current,
                    });
                }
            },
            Ok(None) => {}
            Err(TryRecvError::Empty) => {
                self.active_file_dialog = Some((target, rx));
            }
            Err(TryRecvError::Disconnected) => {}
        }
    }

    pub fn apply_event(&mut self, evt: BackendEvent) {
        match evt {
            BackendEvent::Snapshot(snapshot) => {
                let first_ready = self.phase != Phase::Ready;
                self.snapshot = snapshot;
                self.phase = Phase::Ready;
                if first_ready || !self.cache.dirty {
                    self.cache.sync_from_snapshot(&self.snapshot);
                }
                self.reconcile_selections();
                self.log(true, "已刷新 UWF 全部状态");
                if !self.snapshot.warnings.is_empty() {
                    let combined = self.snapshot.warnings.join("；");
                    self.log_warn(&combined);
                }
            }
            BackendEvent::OverlayRuntime(runtime) => {
                self.snapshot.overlay_runtime = Some(runtime);
                if !self.cache.dirty {
                    self.cache.warning_text = runtime.warning_threshold_mb.to_string();
                    self.cache.critical_text = runtime.critical_threshold_mb.to_string();
                }
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
        if self.logs.len() >= MAX_LOG_ENTRIES {
            self.logs.pop_front();
        }
        self.logs.push_back(StatusEntry {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            level,
            text: text.to_string(),
        });
    }

    // ── 选择状态与身份保持 ──

    fn reconcile_selections(&mut self) {
        // 卷选中：若原选盘符仍在列表则保持，否则默认选第一行
        let still_exists = self.selected_volume_drive.as_ref().is_some_and(|sel| {
            self.snapshot
                .volumes
                .iter()
                .any(|v| v.drive_letter.eq_ignore_ascii_case(sel))
        });
        if !still_exists {
            self.selected_volume_drive = self
                .snapshot
                .volumes
                .first()
                .map(|v| v.drive_letter.clone());
        }

        // 排除项选中：仅在非空可见区且原 key 消失时回退首个，保留跨空视图切换的身份记忆
        let visible = self.visible_exclusions();
        if let Some(key) = &self.selected_exclusion_key {
            if !visible.is_empty() && !visible.contains(key) && !self.exclusion_session_current {
                self.selected_exclusion_key = visible.into_iter().next();
            }
        } else if !visible.is_empty() {
            self.selected_exclusion_key = visible.into_iter().next();
        }
    }

    pub fn selected_volume(&self) -> Option<&VolumeInfo> {
        let drive = self.selected_volume_drive.as_ref()?;
        self.snapshot
            .volumes
            .iter()
            .find(|v| v.drive_letter.eq_ignore_ascii_case(drive))
    }

    pub fn select_volume(&mut self, drive_letter: String) {
        self.selected_volume_drive = Some(drive_letter);
    }

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

    /// 供 UI 渲染使用：按 [`ExclusionFilter`] 过滤，并标明是否已存在于当前会话。
    pub fn filtered_exclusions(&self) -> Vec<(ExclusionItem, bool)> {
        self.visible_exclusions()
            .into_iter()
            .filter(|item| match self.exclusion_filter {
                ExclusionFilter::All => true,
                ExclusionFilter::Files => !item.is_registry(),
                ExclusionFilter::Registry => item.is_registry(),
            })
            .map(|item| {
                let in_current = match &item {
                    ExclusionItem::File(f) => self.snapshot.file_exclusions_current.contains(f),
                    ExclusionItem::Registry(k) => {
                        self.snapshot.registry_exclusions_current.contains(k)
                    }
                };
                (item, in_current)
            })
            .collect()
    }

    pub fn select_exclusion(&mut self, item: ExclusionItem) {
        self.selected_exclusion_key = Some(item);
    }

    pub fn selected_exclusion(&self) -> Option<ExclusionItem> {
        let key = self.selected_exclusion_key.as_ref()?;
        self.filtered_exclusions()
            .into_iter()
            .find(|(item, _)| item == key)
            .map(|(item, _)| item)
    }

    pub fn set_exclusion_session(&mut self, current: bool) {
        self.exclusion_session_current = current;
        self.reconcile_selections();
    }

    pub fn set_exclusion_filter(&mut self, filter: ExclusionFilter) {
        self.exclusion_filter = filter;
    }

    // ── 业务操作派发 ──

    pub fn toggle_write_filter(&mut self) {
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

    pub fn set_overlay_type(&mut self, next_type: OverlayType) {
        if is_overlay_locked(&self.snapshot) {
            self.log(false, "筛选器当前已启用，需禁用并重启后才可修改缓存模式");
            return;
        }
        if self
            .snapshot
            .overlay_config_next
            .is_some_and(|c| c.overlay_type == next_type)
        {
            return;
        }
        let _ = self.begin_mutation(BackendCmd::SetOverlayType(next_type));
    }

    pub fn submit_cache_settings(&mut self) {
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

    pub fn reset_cache_form(&mut self) {
        self.cache.sync_from_snapshot(&self.snapshot);
    }

    pub fn protect_selected(&mut self, protect: bool) {
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

    pub fn open_add_file_exclusion(&mut self, folder: bool) {
        if !self.ensure_mutable_exclusions() {
            return;
        }
        let (title, hint, browse) = if folder {
            (
                "添加文件夹排除",
                "输入文件夹绝对路径（如 C:\\Windows\\Temp），或点击右侧选择目录",
                BrowseKind::Folder,
            )
        } else {
            (
                "添加文件排除",
                "输入文件绝对路径（如 C:\\data\\app.log），或点击右侧浏览文件",
                BrowseKind::File,
            )
        };
        self.open_action_modal(Modal::Input(InputState::new(
            title,
            "目标绝对路径",
            hint,
            InputAction::AddFileExclusion,
            browse,
        )));
    }

    pub fn open_add_registry_exclusion(&mut self) {
        if !self.ensure_mutable_exclusions() {
            return;
        }
        self.open_action_modal(Modal::Input(InputState::new(
            "添加注册表排除",
            "注册表键路径",
            "输入完整注册表路径（如 HKLM\\SOFTWARE\\Example）",
            InputAction::AddRegistryExclusion,
            BrowseKind::None,
        )));
    }

    pub fn delete_selected_exclusion(&mut self) {
        if !self.ensure_mutable_exclusions() {
            return;
        }
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

    pub fn open_import_dialog(&mut self, ctx: &egui::Context) {
        if !self.ensure_mutable_exclusions() || self.phase != Phase::Ready || self.is_busy() {
            return;
        }
        self.launch_file_dialog(
            FileDialogTarget::ImportExclusions,
            FileDialogKind::PickImportTxt {
                title: "选择要导入的排除列表 (.txt)".to_string(),
            },
            ctx,
        );
    }

    pub fn open_export_dialog(&mut self, ctx: &egui::Context) {
        if self.phase != Phase::Ready || self.is_busy() {
            return;
        }
        let default_name = chrono::Local::now()
            .format("EXCLUSIONLIST_%Y_%m_%d_%H_%M_%S.txt")
            .to_string();
        self.launch_file_dialog(
            FileDialogTarget::ExportExclusions,
            FileDialogKind::SaveExportTxt {
                title: "导出排除列表为文本文件".to_string(),
                default_name,
            },
            ctx,
        );
    }

    pub fn open_input_browse(&mut self, kind: BrowseKind, ctx: &egui::Context) {
        let dialog_kind = match kind {
            BrowseKind::File => FileDialogKind::PickFile {
                title: "选择目标文件".to_string(),
            },
            BrowseKind::Folder => FileDialogKind::PickFolder {
                title: "选择目标文件夹".to_string(),
            },
            BrowseKind::None => return,
        };
        self.launch_file_dialog(FileDialogTarget::InputFill, dialog_kind, ctx);
    }

    fn launch_file_dialog(
        &mut self,
        target: FileDialogTarget,
        kind: FileDialogKind,
        ctx: &egui::Context,
    ) {
        if self.active_file_dialog.is_some() {
            return;
        }
        let rx = self.file_dialog.open(kind, ctx);
        self.active_file_dialog = Some((target, rx));
    }

    pub fn open_commit_menu(&mut self) {
        self.open_action_modal(Modal::CommitMenu);
    }

    pub fn open_reset_confirm(&mut self) {
        self.open_action_modal(Modal::Confirm(ConfirmState::new(
            "重置配置确认",
            "将重置全部 UWF 配置（写入过滤 / 缓存 / 分区保护 / 排除项），\n下次重启后恢复默认值。确定继续？",
            ConfirmAction::ResetSettings,
        )));
    }

    pub fn open_restart_confirm(&mut self) {
        self.open_action_modal(Modal::Confirm(ConfirmState::new(
            "重启系统确认",
            "系统将立即重启，未保存的工作将丢失。\n确定继续？",
            ConfirmAction::Restart,
        )));
    }

    pub fn open_shutdown_confirm(&mut self) {
        self.open_action_modal(Modal::Confirm(ConfirmState::new(
            "关机确认",
            "系统将立即关机，未保存的工作将丢失。\n确定继续？",
            ConfirmAction::Shutdown,
        )));
    }

    pub fn open_help(&mut self) {
        self.modal = Some(Modal::Help);
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
            self.log(false, "当前生效视图只读，请切换到「重启后生效 (可编辑)」");
            false
        } else {
            true
        }
    }

    // ── 弹窗回调 ──

    pub fn execute_commit_choice(&mut self, choice: CommitChoice) {
        match choice {
            CommitChoice::File => {
                self.open_action_modal(Modal::Input(InputState::new(
                    "提交文件修改",
                    "受保护卷中的文件绝对路径",
                    "将目标文件当前会话的更改立即固化到物理磁盘（如 C:\\data\\config.ini）",
                    InputAction::CommitFile,
                    BrowseKind::File,
                )));
            }
            CommitChoice::FileDeletion => {
                self.open_action_modal(Modal::Input(InputState::new(
                    "删除并固化文件（第 1 / 2 步）",
                    "待删除的文件绝对路径",
                    "先物理删除目标文件，随后向 UWF 固化删除（下一步需二次确认）",
                    InputAction::CommitFileDeletion,
                    BrowseKind::File,
                )));
            }
            CommitChoice::Registry => {
                self.open_action_modal(Modal::Input(InputState::new(
                    "提交注册表键值",
                    "注册表完整路径（含值名）",
                    "格式为 键路径\\值名称（如 HKLM\\SOFTWARE\\Example\\ValueName）",
                    InputAction::CommitRegistry,
                    BrowseKind::None,
                )));
            }
        }
    }

    pub fn execute_input(&mut self, action: InputAction, text: String) {
        match action {
            InputAction::AddFileExclusion => {
                self.exclusion_filter = match self.exclusion_filter {
                    ExclusionFilter::Registry => ExclusionFilter::All,
                    other => other,
                };
                let _ = self.begin_mutation(BackendCmd::AddFileExclusion(text));
            }
            InputAction::AddRegistryExclusion => {
                self.exclusion_filter = match self.exclusion_filter {
                    ExclusionFilter::Files => ExclusionFilter::All,
                    other => other,
                };
                let _ = self.begin_mutation(BackendCmd::AddRegistryExclusion(text));
            }
            InputAction::CommitFile => {
                let _ = self.begin_mutation(BackendCmd::CommitFile(text));
            }
            InputAction::CommitFileDeletion => {
                let path = PathBuf::from(&text);
                self.modal = Some(Modal::Confirm(ConfirmState::new(
                    "删除并固化文件确认（第 2 / 2 步）",
                    format!(
                        "警告：即将立即物理删除文件：\n{}\n并固化该删除，计算机重启后不会恢复。\n\n确定继续？",
                        path.display()
                    ),
                    ConfirmAction::CommitFileDeletion(path),
                )));
            }
            InputAction::CommitRegistry => {
                let _ = self.begin_mutation(BackendCmd::CommitRegistry(text));
            }
        }
    }

    pub fn execute_confirm(&mut self, action: ConfirmAction) {
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
            ConfirmAction::Quit => {
                self.force_close_confirmed = true;
                self.should_quit = true;
            }
        }
    }
}

impl eframe::App for UwfApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG_CANVAS.to_normalized_gamma_f32()
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.poll_file_dialog();

        if self.last_overlay_tick.elapsed() >= OVERLAY_POLL {
            self.last_overlay_tick = Instant::now();
            self.send_overlay_refresh();
        }

        if self.phase == Phase::Ready {
            ctx.request_repaint_after(IDLE_REPAINT);
        }

        if ctx.input(|i| i.viewport().close_requested())
            && self.is_pending()
            && !self.force_close_confirmed
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.modal = Some(Modal::Confirm(ConfirmState::new(
                "退出确认",
                "当前仍有 UWF 后台操作正在执行，直接退出可能导致状态不同步。\n是否确认强制退出？",
                ConfirmAction::Quit,
            )));
        }

        if self.should_quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        crate::ui::render(ui, self);
    }

    fn on_exit(&mut self) {
        self.send_quit();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{FilterState, OverlayConfig, OverlayRuntime};
    use egui_kittest::{Harness, kittest::Queryable as _};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone, Default)]
    struct StubFileDialog {
        replies: Rc<RefCell<VecDeque<Option<PathBuf>>>>,
    }

    impl StubFileDialog {
        fn with_replies(items: impl IntoIterator<Item = Option<PathBuf>>) -> Self {
            Self {
                replies: Rc::new(RefCell::new(items.into_iter().collect())),
            }
        }
    }

    impl FileDialogService for StubFileDialog {
        fn open(
            &mut self,
            _kind: FileDialogKind,
            ctx: &egui::Context,
        ) -> Receiver<Option<PathBuf>> {
            let (tx, rx) = std::sync::mpsc::channel();
            let reply = self.replies.borrow_mut().pop_front().flatten();
            let _ = tx.send(reply);
            ctx.request_repaint();
            rx
        }
    }

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

    fn test_app(kind: BackendKind) -> (UwfApp, Receiver<BackendCommand>) {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (_, event_rx) = std::sync::mpsc::channel();
        (
            UwfApp::new_with_file_dialog(
                cmd_tx,
                event_rx,
                kind,
                Box::new(StubFileDialog::default()),
            ),
            cmd_rx,
        )
    }

    fn ready_app(kind: BackendKind) -> (UwfApp, Receiver<BackendCommand>) {
        let (mut app, cmd_rx) = test_app(kind);
        app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
        (app, cmd_rx)
    }

    fn recv_cmd(rx: &Receiver<BackendCommand>) -> BackendCmd {
        match rx.try_recv().expect("期望收到后端命令") {
            BackendCommand::Mutate { cmd, .. } => cmd,
            other => panic!("期望 Mutate 命令，实际 {other:?}"),
        }
    }

    fn build_gui_harness<'a>(
        kind: BackendKind,
        dialog: StubFileDialog,
        snapshot: Option<FullSnapshot>,
    ) -> (
        Harness<'a, UwfApp>,
        Sender<BackendEvent>,
        Receiver<BackendCommand>,
    ) {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        if let Some(snap) = snapshot {
            let _ = event_tx.send(BackendEvent::Snapshot(snap));
        }
        let harness = Harness::builder()
            .with_size([1060.0, 740.0])
            .allow_missing_glyphs()
            .build_eframe(move |cc| {
                theme::apply_theme(&cc.egui_ctx);
                UwfApp::new_with_file_dialog(cmd_tx, event_rx, kind, Box::new(dialog))
            });
        (harness, event_tx, cmd_rx)
    }

    #[test]
    fn not_installed_event_shows_guide_and_snapshot_recovers() {
        let (mut app, _cmd_rx) = test_app(BackendKind::Mock);
        assert!(app.is_busy());
        app.apply_event(BackendEvent::NotInstalled(None));
        assert_eq!(app.phase, Phase::NotInstalled);
        assert!(!app.is_busy());
        assert!(app.last_log().is_some_and(|e| e.level == LogLevel::Err));
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
        app.toggle_write_filter();
        let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
            panic!("期望 Mutate 命令");
        };
        assert!(matches!(cmd, BackendCmd::DisableFilter));
        assert_eq!(app.mutation, Some(id));

        app.protect_selected(false);
        assert!(cmd_rx.try_recv().is_err());

        app.apply_event(BackendEvent::Finished {
            id,
            outcome: Outcome::Ok("ok".into()),
        });
        assert!(!app.is_pending());
        app.protect_selected(false);
        assert!(matches!(recv_cmd(&cmd_rx), BackendCmd::Unprotect(_)));
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
    fn basic_settings_toggles_write_filter_and_respects_r1_lock() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.toggle_write_filter();
        let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
            panic!("期望 Mutate");
        };
        assert!(matches!(cmd, BackendCmd::DisableFilter));
        app.apply_event(BackendEvent::Finished {
            id,
            outcome: Outcome::Ok("ok".into()),
        });

        // current_enabled == true 时 R1 锁定切换缓存模式
        app.set_overlay_type(OverlayType::Disk);
        assert!(cmd_rx.try_recv().is_err());

        // 解锁后允许切换缓存模式
        let mut snap = demo_snapshot();
        snap.filter_state = Some(FilterState {
            current_enabled: false,
            next_enabled: false,
        });
        app.apply_event(BackendEvent::Snapshot(snap));
        app.set_overlay_type(OverlayType::Disk);
        assert!(matches!(
            recv_cmd(&cmd_rx),
            BackendCmd::SetOverlayType(OverlayType::Disk)
        ));
    }

    #[test]
    fn cache_form_validation_dirty_flag_and_reset() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.cache.warning_text = "2000".into();
        app.cache.on_edited();
        app.submit_cache_settings();
        assert!(cmd_rx.try_recv().is_err());
        assert!(
            app.cache
                .error
                .as_deref()
                .is_some_and(|e| e.contains("必须小于"))
        );

        // dirty = true 时后台刷新不冲掉正在编辑的文本
        app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
        assert_eq!(app.cache.warning_text, "2000");

        // 还原恢复到快照值
        app.reset_cache_form();
        assert!(!app.cache.dirty);
        assert_eq!(app.cache.warning_text, "1536");

        // 合法修改后提交
        app.cache.warning_text = "1530".into();
        app.cache.on_edited();
        app.submit_cache_settings();
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
    fn selection_identity_and_session_toggle_preserve_key() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        let mut snap = demo_snapshot();
        snap.file_exclusions_next = vec![
            FileExclusion::new("C:".into(), "\\A".into()),
            FileExclusion::new("C:".into(), "\\B".into()),
        ];
        app.apply_event(BackendEvent::Snapshot(snap));

        let b_item = ExclusionItem::File(FileExclusion::new("C:".into(), "\\B".into()));
        app.select_exclusion(b_item.clone());
        assert_eq!(app.selected_exclusion(), Some(b_item.clone()));

        // 切到只读当前视图（空），不允许新增/删除；切回后必须保持选中 \B
        app.set_exclusion_session(true);
        assert!(app.selected_exclusion().is_none());
        app.delete_selected_exclusion();
        assert!(cmd_rx.try_recv().is_err());

        app.set_exclusion_session(false);
        assert_eq!(app.selected_exclusion(), Some(b_item));
        app.delete_selected_exclusion();
        match recv_cmd(&cmd_rx) {
            BackendCmd::RemoveFileExclusion(path) => assert_eq!(path, "C:\\B"),
            other => panic!("期望 RemoveFileExclusion，实际 {other:?}"),
        }
    }

    #[test]
    fn restart_flow_mock_stays_windows_quits() {
        let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
        app.execute_confirm(ConfirmAction::Restart);
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
            app.last_log()
                .is_some_and(|e| e.text.contains("（模拟）已重启"))
        );

        let (mut app, cmd_rx) = ready_app(BackendKind::Windows);
        app.execute_confirm(ConfirmAction::Restart);
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
    fn gui_dashboard_renders_and_dispatches_filter_volume_and_cache_actions() {
        let (mut harness, event_tx, cmd_rx) = build_gui_harness(
            BackendKind::Mock,
            StubFileDialog::default(),
            Some(demo_snapshot()),
        );

        // 切换筛选器（执行期间顶部 Spinner 持续重绘，使用 run_steps 推进一帧）
        harness.get_by_label("停用筛选器").click();
        harness.run_steps(2);
        let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
            panic!("期望 Mutate 命令");
        };
        assert!(matches!(cmd, BackendCmd::DisableFilter));
        let _ = event_tx.send(BackendEvent::Finished {
            id,
            outcome: Outcome::Ok("ok".into()),
        });
        harness.run();

        // 选中 D: 行并点击保护
        harness.get_by_label_contains("D:").click();
        harness.run();
        harness.get_by_label("保护选中分区").click();
        harness.run_steps(2);
        match recv_cmd(&cmd_rx) {
            BackendCmd::Protect(drive) => assert_eq!(drive, "D:"),
            other => panic!("期望 Protect(D:)，实际 {other:?}"),
        }
    }

    #[test]
    fn gui_cache_form_validation_and_save() {
        let (mut harness, _event_tx, cmd_rx) = build_gui_harness(
            BackendKind::Mock,
            StubFileDialog::default(),
            Some(demo_snapshot()),
        );

        {
            let warn_input = harness.get_by_label("警告阈值");
            warn_input.click();
        }
        harness.state_mut().cache.warning_text = "2000".into();
        harness.state_mut().cache.on_edited();
        harness.run();

        harness.get_by_label("应用缓存设置").click();
        harness.run();
        assert!(cmd_rx.try_recv().is_err());
        assert!(
            harness
                .query_all_by_label_contains("警告阈值必须小于严重阈值")
                .next()
                .is_some()
        );

        harness.get_by_label("还原").click();
        harness.run();
        assert_eq!(harness.state().cache.warning_text, "1536");

        harness.state_mut().cache.warning_text = "1530".into();
        harness.state_mut().cache.on_edited();
        harness.run();
        harness.get_by_label("应用缓存设置").click();
        harness.run_steps(2);
        match recv_cmd(&cmd_rx) {
            BackendCmd::SetOverlayCache {
                max_mb,
                warning_mb,
                critical_mb,
            } => assert_eq!((max_mb, warning_mb, critical_mb), (2048, 1530, 1843)),
            other => panic!("期望 SetOverlayCache，实际 {other:?}"),
        }
    }

    #[test]
    fn gui_folder_exclusion_with_stub_file_dialog() {
        let stub = StubFileDialog::with_replies([Some(PathBuf::from("D:\\TempWork"))]);
        let (mut harness, _event_tx, cmd_rx) =
            build_gui_harness(BackendKind::Mock, stub, Some(demo_snapshot()));

        harness.get_by_label("+ 文件夹").click();
        harness.run();
        harness.get_by_label("选择目录…").click();
        harness.run();
        harness.get_by_label("确认").click();
        harness.run_steps(2);

        match recv_cmd(&cmd_rx) {
            BackendCmd::AddFileExclusion(path) => assert_eq!(path, "D:\\TempWork"),
            other => panic!("期望 AddFileExclusion，实际 {other:?}"),
        }
    }

    #[test]
    fn gui_commit_file_deletion_enforces_two_step_confirm() {
        let stub = StubFileDialog::with_replies([Some(PathBuf::from("C:\\data\\a.txt"))]);
        let (mut harness, _event_tx, cmd_rx) =
            build_gui_harness(BackendKind::Mock, stub, Some(demo_snapshot()));

        harness.get_by_label("提交操作…").click();
        harness.run();
        harness.get_by_label("选择待删文件…").click();
        harness.run();
        harness.get_by_label("浏览文件…").click();
        harness.run();
        harness.get_by_label("下一步").click();
        harness.run();
        // 第一步仅切到二次确认，尚未发命令
        assert!(cmd_rx.try_recv().is_err());

        harness.get_by_label("确认删除并固化").click();
        harness.run_steps(2);
        match recv_cmd(&cmd_rx) {
            BackendCmd::CommitFileDeletion(path) => assert_eq!(path, "C:\\data\\a.txt"),
            other => panic!("期望 CommitFileDeletion，实际 {other:?}"),
        }
    }

    #[test]
    fn gui_not_installed_and_log_drawer() {
        let (mut harness, event_tx, cmd_rx) =
            build_gui_harness(BackendKind::Mock, StubFileDialog::default(), None);

        let _ = event_tx.send(BackendEvent::NotInstalled(None));
        harness.run();
        harness.get_by_label("重新检测").click();
        harness.run_steps(2);
        assert!(matches!(cmd_rx.try_recv(), Ok(BackendCommand::RefreshAll)));

        let _ = event_tx.send(BackendEvent::Snapshot(demo_snapshot()));
        harness.run();
        harness.get_by_label_contains("操作日志").click();
        harness.run();
        assert!(harness.state().show_log_drawer);
        assert!(harness.query_by_label("复制全部").is_some());
    }
}
