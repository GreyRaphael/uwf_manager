//! 主应用状态机与 `eframe::App` 实现：协调后端命令/回包、表单校验、模态弹窗与视图交互。

mod forms;
#[cfg(test)]
mod tests;

pub use forms::{
    CacheForm, ExclusionItem, ExclusionRow, LogLevel, Modal, Phase, StatusEntry, is_overlay_locked,
};
use forms::{CachePendingAction, PowerAction, format_revert_confirm_message};

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::backend::{
    BackendCmd, BackendCommand, BackendEvent, BackendKind, ExclusionRevertPlan, FullSnapshot,
    Outcome, RequestId, plan_exclusion_revert, split_absolute_path, split_registry_path,
    validate_filter_commit_ready, validate_volume_commit_ready,
};
use crate::model::OverlayType;
use crate::ui::components::commit_menu::CommitChoice;
use crate::ui::components::confirm::{ConfirmAction, ConfirmState};
use crate::ui::components::input_dialog::{BrowseKind, InputAction, InputState};
use crate::ui::file_dialog::{
    FileDialogKind, FileDialogService, FileDialogTarget, RfdFileDialogService,
};
use crate::ui::theme;

/// Overlay 运行时用量后台刷新周期（2 秒）
const OVERLAY_POLL: Duration = Duration::from_secs(2);
/// 钳位字形纹理图集单边最大尺寸，避免显卡驱动返回的 16384/32768 上限导致图集占满数 MB 甚至十数 MB
const MAX_FONT_ATLAS_SIDE: usize = 2048;
/// 操作日志弹窗最大保留条数
const MAX_LOG_ENTRIES: usize = 50;

pub struct UwfApp {
    pub backend_kind: BackendKind,
    pub phase: Phase,
    pub snapshot: FullSnapshot,
    pub cache: CacheForm,
    pub os_version: Option<String>,
    pub modal: Option<Modal>,
    pub logs: VecDeque<StatusEntry>,
    /// 当前在途变更请求 id；仅匹配回包可解除锁定
    pub mutation: Option<RequestId>,
    pending_cache_action: Option<CachePendingAction>,
    power_action: Option<(RequestId, PowerAction)>,
    next_request_id: RequestId,
    last_overlay_tick: Instant,
    pub should_quit: bool,
    force_close_confirmed: bool,
    cmd_tx: Sender<BackendCommand>,
    event_rx: Receiver<BackendEvent>,
    file_dialog: Box<dyn FileDialogService>,
    active_file_dialog: Option<(FileDialogTarget, Receiver<Option<PathBuf>>)>,
    cached_exclusion_rows: Vec<ExclusionRow>,
    cached_revert_plan: ExclusionRevertPlan,
    cached_reboot_reasons: Vec<&'static str>,
    #[cfg(windows)]
    was_minimized: bool,
    #[cfg(windows)]
    initial_trim_done: bool,
    #[cfg(windows)]
    startup_trim_countdown: Option<u8>,
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
            os_version: None,
            modal: None,
            logs: VecDeque::with_capacity(MAX_LOG_ENTRIES),
            mutation: None,
            pending_cache_action: None,
            power_action: None,
            next_request_id: 1,
            last_overlay_tick: Instant::now(),
            should_quit: false,
            force_close_confirmed: false,
            cmd_tx,
            event_rx,
            file_dialog,
            active_file_dialog: None,
            cached_exclusion_rows: Vec::new(),
            cached_revert_plan: ExclusionRevertPlan::default(),
            cached_reboot_reasons: Vec::new(),
            #[cfg(windows)]
            was_minimized: false,
            #[cfg(windows)]
            initial_trim_done: false,
            #[cfg(windows)]
            startup_trim_countdown: None,
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
        self.pending_cache_action = None;
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
                    let _ = self.begin_mutation(BackendCmd::ExportExclusions { path });
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
                if let Some(ver) = &snapshot.os_version {
                    self.os_version = Some(ver.clone());
                }
                self.snapshot = snapshot;
                self.rebuild_snapshot_cache();
                self.phase = Phase::Ready;
                #[cfg(windows)]
                if !self.initial_trim_done {
                    self.initial_trim_done = true;
                    self.startup_trim_countdown = Some(2);
                }
                if first_ready || !self.cache.dirty {
                    self.cache.sync_from_snapshot(&self.snapshot);
                }
                self.log(true, "已刷新 UWF 全部状态");
                if !self.snapshot.warnings.is_empty() {
                    let combined = self.snapshot.warnings.join("；");
                    self.log_warn(&combined);
                }
            }
            BackendEvent::OsVersion(ver) => {
                self.os_version = Some(ver);
            }
            BackendEvent::OverlayRuntime(runtime) => {
                self.snapshot.overlay_runtime = Some(runtime);
                if !self.cache.dirty {
                    self.cache.sync_thresholds_from_runtime(&runtime);
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
                let cache_action = self
                    .pending_cache_action
                    .filter(|act| act.request_id() == id);
                if cache_action.is_some() {
                    self.pending_cache_action = None;
                }
                match outcome {
                    Outcome::Ok(msg) => {
                        if cache_action.is_some() {
                            self.cache.sync_from_snapshot(&self.snapshot);
                        }
                        self.log(true, &msg);
                        self.on_power_action_ok(id);
                    }
                    Outcome::OkWithRefreshWarning { message, warning } => {
                        if cache_action.is_some() {
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
                        if matches!(cache_action, Some(CachePendingAction::Save(_))) {
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
                    if self
                        .pending_cache_action
                        .is_some_and(|act| act.request_id() == req_id)
                    {
                        self.pending_cache_action = None;
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
                #[cfg(windows)]
                if !self.initial_trim_done {
                    self.initial_trim_done = true;
                    self.startup_trim_countdown = Some(2);
                }
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

    // ── 排除项与变更汇总缓存（仅快照到达时重算，避免 UI 每帧克隆/拼接） ──

    fn rebuild_snapshot_cache(&mut self) {
        let mut rows = Vec::with_capacity(
            self.snapshot.file_exclusions_next.len() + self.snapshot.registry_exclusions_next.len(),
        );
        for f in &self.snapshot.file_exclusions_next {
            let in_current = self
                .snapshot
                .file_exclusions_current
                .iter()
                .any(|cur| cur.matches_ignore_case(f));
            let item = ExclusionItem::File(f.clone());
            let display = item.display();
            rows.push(ExclusionRow {
                item,
                display,
                in_current,
            });
        }
        for k in &self.snapshot.registry_exclusions_next {
            let in_current = self
                .snapshot
                .registry_exclusions_current
                .iter()
                .any(|cur| cur.eq_ignore_ascii_case(k));
            let item = ExclusionItem::Registry(k.clone());
            let display = item.display();
            rows.push(ExclusionRow {
                item,
                display,
                in_current,
            });
        }
        self.cached_exclusion_rows = rows;

        self.cached_revert_plan = plan_exclusion_revert(
            &self.snapshot.file_exclusions_current,
            &self.snapshot.file_exclusions_next,
            &self.snapshot.registry_exclusions_current,
            &self.snapshot.registry_exclusions_next,
        );

        let mut reasons = Vec::new();
        if self
            .snapshot
            .filter_state
            .is_some_and(|fs| fs.current_enabled != fs.next_enabled)
        {
            reasons.push("保护总开关");
        }
        if let (Some(cur), Some(next)) = (
            self.snapshot.overlay_config_current,
            self.snapshot.overlay_config_next,
        ) && (cur.overlay_type != next.overlay_type || cur.max_size_mb != next.max_size_mb)
        {
            reasons.push("保护缓存设置");
        }
        if self
            .snapshot
            .volumes
            .iter()
            .any(|v| v.current_protected != v.next_protected)
        {
            reasons.push("磁盘分区保护");
        }
        if !self.cached_revert_plan.is_empty() {
            reasons.push("排除项规则");
        }
        self.cached_reboot_reasons = reasons;
    }

    /// 供 UI 渲染使用：返回重启后排除项列表，并标明是否已在当前会话生效。
    pub fn exclusion_rows(&self) -> &[ExclusionRow] {
        &self.cached_exclusion_rows
    }

    /// 计算将“重启后生效”排除项恢复成与当前生效状态一致所需的操作计划。
    pub fn exclusion_revert_plan(&self) -> &ExclusionRevertPlan {
        &self.cached_revert_plan
    }

    // ── 业务操作派发 ──

    /// 汇总当前已配置但尚未生效（需重启计算机）的变更分类，供顶栏引导条展示。
    pub fn pending_reboot_reasons(&self) -> &[&'static str] {
        &self.cached_reboot_reasons
    }

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
                    self.pending_cache_action = Some(CachePendingAction::Save(id));
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

    pub fn set_volume_protected(&mut self, drive_letter: &str, protect: bool) {
        let cmd = if protect {
            BackendCmd::Protect(drive_letter.to_string())
        } else {
            BackendCmd::Unprotect(drive_letter.to_string())
        };
        let _ = self.begin_mutation(cmd);
    }

    pub fn open_add_file_exclusion(&mut self, folder: bool) {
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
        self.open_action_modal(Modal::Input(InputState::new(
            "添加注册表排除",
            "注册表键路径",
            "输入完整注册表路径（如 HKLM\\SOFTWARE\\Example）",
            InputAction::AddRegistryExclusion,
            BrowseKind::None,
        )));
    }

    pub fn remove_exclusion(&mut self, item: &ExclusionItem) {
        match item {
            ExclusionItem::File(f) => {
                let _ = self.begin_mutation(BackendCmd::RemoveFileExclusion(f.display()));
            }
            ExclusionItem::Registry(k) => {
                let _ = self.begin_mutation(BackendCmd::RemoveRegistryExclusion(k.clone()));
            }
        }
    }

    pub fn open_import_dialog(&mut self, ctx: &egui::Context) {
        if self.phase != Phase::Ready || self.is_busy() {
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

    pub fn open_logs(&mut self) {
        self.modal = Some(Modal::Logs);
    }

    pub fn open_revert_exclusions_confirm(&mut self) {
        let plan = self.exclusion_revert_plan();
        if plan.is_empty() {
            return;
        }
        let message = format_revert_confirm_message(plan);
        self.open_action_modal(Modal::Confirm(ConfirmState::new(
            "撤销排除项修改确认",
            message,
            ConfirmAction::RevertExclusions,
        )));
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

    // ── 弹窗回调 ──

    pub fn execute_commit_choice(&mut self, choice: CommitChoice) {
        match choice {
            CommitChoice::File => {
                self.open_action_modal(Modal::Input(InputState::new(
                    "强制保存文件修改",
                    "受保护分区中的文件绝对路径",
                    "立即将目标文件当前的修改强制写入真实硬盘，重启后不还原（如 C:\\data\\config.ini）",
                    InputAction::CommitFile,
                    BrowseKind::File,
                )));
            }
            CommitChoice::FileDeletion => {
                self.open_action_modal(Modal::Input(InputState::new(
                    "永久删除文件（第 1 / 2 步）",
                    "待永久删除的文件绝对路径",
                    "立即彻底删除目标文件并同步记录，重启后不会自动恢复（下一步需二次确认）",
                    InputAction::CommitFileDeletion,
                    BrowseKind::File,
                )));
            }
            CommitChoice::Registry => {
                self.open_action_modal(Modal::Input(InputState::new(
                    "强制保存注册表键值",
                    "注册表完整路径（含值名）",
                    "格式为 键路径\\值名称（如 HKLM\\SOFTWARE\\Example\\ValueName）",
                    InputAction::CommitRegistry,
                    BrowseKind::None,
                )));
            }
        }
    }

    /// 对“提交固化”类输入做前置规则校验：筛选器须当前启用，且文件操作目标卷在当前会话必须已受保护。
    pub fn validate_input_preflight(
        &self,
        action: InputAction,
        text: &str,
    ) -> std::result::Result<(), String> {
        let Some(fs) = self.snapshot.filter_state else {
            return Ok(());
        };
        match action {
            InputAction::AddFileExclusion | InputAction::AddRegistryExclusion => Ok(()),
            InputAction::CommitFile | InputAction::CommitFileDeletion => {
                let what = if action == InputAction::CommitFile {
                    "文件提交"
                } else {
                    "文件删除固化"
                };
                let (drive, _) = split_absolute_path(text).map_err(|e| e.to_string())?;
                validate_volume_commit_ready(
                    &self.snapshot.volumes,
                    fs.current_enabled,
                    &drive,
                    what,
                )
                .map_err(|e| e.to_string())
            }
            InputAction::CommitRegistry => {
                validate_filter_commit_ready(fs.current_enabled, "注册表提交")
                    .map_err(|e| e.to_string())?;
                split_registry_path(text)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
        }
    }

    pub fn execute_input(&mut self, action: InputAction, text: String) {
        if let Err(err) = self.validate_input_preflight(action, &text) {
            self.log(false, &err);
            return;
        }
        match action {
            InputAction::AddFileExclusion => {
                let _ = self.begin_mutation(BackendCmd::AddFileExclusion(text));
            }
            InputAction::AddRegistryExclusion => {
                let _ = self.begin_mutation(BackendCmd::AddRegistryExclusion(text));
            }
            InputAction::CommitFile => {
                let _ = self.begin_mutation(BackendCmd::CommitFile(text));
            }
            InputAction::CommitFileDeletion => {
                let path = PathBuf::from(&text);
                self.modal = Some(Modal::Confirm(ConfirmState::new(
                    "永久删除文件确认（第 2 / 2 步）",
                    format!(
                        "警告：即将立即彻底删除文件：\n{}\n且计算机重启后不会自动恢复。\n\n确定继续？",
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
                    self.pending_cache_action = Some(CachePendingAction::ResetSync(id));
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
            ConfirmAction::RevertExclusions => {
                let _ = self.begin_mutation(BackendCmd::RevertExclusionsToCurrent);
            }
            ConfirmAction::Quit => {
                self.force_close_confirmed = true;
                self.should_quit = true;
            }
        }
    }
    pub(crate) fn tick_overlay_poll(&mut self, ctx: &egui::Context, minimized: bool) {
        if self.phase != Phase::Ready || minimized {
            return;
        }
        let elapsed = self.last_overlay_tick.elapsed();
        if elapsed >= OVERLAY_POLL {
            self.last_overlay_tick = Instant::now();
            self.send_overlay_refresh();
            ctx.request_repaint_after(OVERLAY_POLL);
        } else {
            ctx.request_repaint_after(OVERLAY_POLL - elapsed);
        }
    }
}

impl eframe::App for UwfApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG_CANVAS.to_normalized_gamma_f32()
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(side) = raw_input.max_texture_side.as_mut() {
            *side = (*side).min(MAX_FONT_ATLAS_SIDE);
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.poll_file_dialog();

        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));

        #[cfg(windows)]
        {
            if minimized && !self.was_minimized {
                crate::win_platform::trim_working_set();
            }
            self.was_minimized = minimized;

            if let Some(rem) = self.startup_trim_countdown.as_mut() {
                if *rem == 0 {
                    self.startup_trim_countdown = None;
                    crate::win_platform::trim_working_set();
                } else {
                    *rem -= 1;
                    ctx.request_repaint();
                }
            }
        }

        self.tick_overlay_poll(ctx, minimized);

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

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.send_quit();
    }
}
