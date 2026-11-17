//! 后端工作线程与命令执行循环。

use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::error::{Result, UwfError};
use crate::model::OverlayRuntime;

use super::logic::{
    ExclusionRevertPlan, plan_exclusion_revert, split_absolute_path, split_registry_path,
};
use super::protocol::{
    BackendCmd, BackendCommand, BackendEvent, BackendFactory, Outcome, RequestId, UwfBackend,
};

/// 后端线程句柄：停止 ack + JoinHandle。
///
/// `Quit` 后线程先发 ack 再退出；主线程最多等 `timeout`，避免 WMI 调用
/// 挂死时无条件 `join` 把 UI 卡住。
pub struct BackendHandle {
    join: JoinHandle<()>,
    stopped: Receiver<()>,
}

impl BackendHandle {
    /// 等待后端确认停止；收到 ack 才 join，超时则放弃 join（进程退出会回收线程）。
    pub fn wait(self, timeout: Duration) {
        if self.stopped.recv_timeout(timeout).is_ok() {
            let _ = self.join.join();
        }
    }
}

struct DisconnectGuard<E: From<BackendEvent>> {
    tx: Sender<E>,
    wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    clean_quit: bool,
}

impl<E: From<BackendEvent>> Drop for DisconnectGuard<E> {
    fn drop(&mut self) {
        if !self.clean_quit {
            let _ = self.tx.send(E::from(BackendEvent::Disconnected));
            (self.wake)();
        }
    }
}

/// Overlay 运行时用量后台轮询周期（2 秒）
const OVERLAY_POLL: Duration = Duration::from_secs(2);

#[derive(Default)]
struct PollState {
    ready: bool,
    filter_enabled: bool,
    paused: bool,
    last_runtime: Option<OverlayRuntime>,
    next_poll_at: Option<Instant>,
}

impl PollState {
    fn should_poll(&self) -> bool {
        self.ready && self.filter_enabled && !self.paused
    }

    fn observe_event(&mut self, evt: &BackendEvent, poll_interval: Duration) {
        match evt {
            BackendEvent::Snapshot(snap) => {
                self.ready = true;
                self.filter_enabled = snap.filter_state.is_some_and(|fs| fs.current_enabled);
                self.last_runtime = snap.overlay_runtime;
                self.next_poll_at = self.should_poll().then(|| Instant::now() + poll_interval);
            }
            BackendEvent::OverlayRuntime(runtime) => {
                self.last_runtime = Some(*runtime);
            }
            BackendEvent::NotInstalled(_) => {
                self.ready = false;
                self.filter_enabled = false;
                self.last_runtime = None;
                self.next_poll_at = None;
            }
            _ => {}
        }
    }

    fn set_paused(&mut self, paused: bool) {
        let was_paused = self.paused;
        self.paused = paused;
        if !self.should_poll() {
            self.next_poll_at = None;
        } else if was_paused && !paused {
            // 恢复窗口后立即校准一次，仅在数值变化时唤醒 UI
            self.next_poll_at = Some(Instant::now());
        }
    }
}

/// 启动带唤醒回调的后端线程（actor）：工厂在本线程内构造 backend（WMI 连接随之在
/// 本线程建立），顺序执行命令并在筛选器开启且窗口未最小化时在后台执行 2 秒轮询，
/// 仅当用量变化时推送事件并唤醒 UI，`Quit` 退出。若意外退出（如 panic），`Drop` 哨兵立即发送
/// [`BackendEvent::Disconnected`] 并唤醒 UI。
pub fn spawn_backend_thread_with_wake<E, W>(
    factory: BackendFactory,
    rx: Receiver<BackendCommand>,
    tx: Sender<E>,
    wake: W,
) -> BackendHandle
where
    E: From<BackendEvent> + Send + 'static,
    W: Fn() + Send + Sync + 'static,
{
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let join = thread::spawn(move || {
        let mut guard = DisconnectGuard {
            tx: tx.clone(),
            wake: Arc::clone(&wake),
            clean_quit: false,
        };
        let mut backend = factory();
        let clean_quit = run_worker_loop(&mut *backend, &rx, &tx, &wake, OVERLAY_POLL);
        if clean_quit {
            guard.clean_quit = true;
            let _ = stop_tx.send(());
        }
    });
    BackendHandle {
        join,
        stopped: stop_rx,
    }
}

pub(super) fn run_worker_loop<E>(
    backend: &mut dyn UwfBackend,
    rx: &Receiver<BackendCommand>,
    tx: &Sender<E>,
    wake: &Arc<dyn Fn() + Send + Sync>,
    poll_interval: Duration,
) -> bool
where
    E: From<BackendEvent>,
{
    let state = RefCell::new(PollState::default());
    let emit = |evt: BackendEvent| {
        state.borrow_mut().observe_event(&evt, poll_interval);
        let _ = tx.send(E::from(evt));
        (wake)();
    };
    if let Some(ver) = backend.get_os_version() {
        emit(BackendEvent::OsVersion(ver));
    }

    loop {
        let next_poll_at = state.borrow().next_poll_at;
        let command = match next_poll_at {
            Some(deadline) => {
                let wait = deadline.saturating_duration_since(Instant::now());
                match rx.recv_timeout(wait) {
                    Ok(cmd) => cmd,
                    Err(RecvTimeoutError::Timeout) => {
                        poll_overlay_once(backend, &state, &emit);
                        let mut st = state.borrow_mut();
                        st.next_poll_at = st.should_poll().then(|| Instant::now() + poll_interval);
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return false,
                }
            }
            None => match rx.recv() {
                Ok(cmd) => cmd,
                Err(_) => return false,
            },
        };

        match command {
            BackendCommand::Quit => return true,
            BackendCommand::SetPollingPaused(paused) => {
                state.borrow_mut().set_paused(paused);
            }
            other => handle_command(backend, other, &emit),
        }
    }
}

fn poll_overlay_once(
    backend: &mut dyn UwfBackend,
    state: &RefCell<PollState>,
    emit: &dyn Fn(BackendEvent),
) {
    if !state.borrow().should_poll() {
        return;
    }
    match backend.get_overlay_runtime() {
        Ok(runtime) => {
            let changed = state.borrow().last_runtime != Some(runtime);
            if changed {
                emit(BackendEvent::OverlayRuntime(runtime));
            }
        }
        Err(UwfError::NotInstalled) => emit(BackendEvent::NotInstalled(None)),
        // 后台 2 秒轮询偶发失败静默跳过，避免覆盖底部操作反馈；用户可用 [F5] 显式诊断。
        Err(_) => {}
    }
}

pub(super) fn handle_command(
    backend: &mut dyn UwfBackend,
    command: BackendCommand,
    emit: &dyn Fn(BackendEvent),
) {
    match command {
        BackendCommand::RefreshAll => match try_refresh(backend, emit) {
            Ok(()) => {}
            Err(UwfError::NotInstalled) => emit(BackendEvent::NotInstalled(None)),
            Err(e) => emit(BackendEvent::RefreshFailed(e.to_string())),
        },
        BackendCommand::Mutate { id, cmd } => handle_mutation(id, backend, cmd, emit),
        BackendCommand::SetPollingPaused(_) | BackendCommand::Quit => {
            unreachable!("SetPollingPaused 与 Quit 由线程循环拦截")
        }
    }
}

fn handle_mutation(
    id: RequestId,
    backend: &mut dyn UwfBackend,
    cmd: BackendCmd,
    emit: &dyn Fn(BackendEvent),
) {
    match cmd {
        BackendCmd::EnableFilter => mutate(
            id,
            backend,
            emit,
            |b| b.enable_filter(),
            "UWF 将在下次系统重启后启用",
        ),
        BackendCmd::DisableFilter => mutate(
            id,
            backend,
            emit,
            |b| b.disable_filter(),
            "UWF 将在下次系统重启后禁用",
        ),
        BackendCmd::ResetSettings => mutate(
            id,
            backend,
            emit,
            |b| b.reset_settings(),
            "UWF 配置将在下次重启后重置为默认值",
        ),
        BackendCmd::SetOverlayType(overlay_type) => mutate(
            id,
            backend,
            emit,
            |b| b.set_overlay_type(overlay_type),
            format!("覆盖类型已设为 {}（下次启动生效）", overlay_type.label()),
        ),
        BackendCmd::SetOverlayCache {
            max_mb,
            warning_mb,
            critical_mb,
        } => set_overlay_cache(id, backend, emit, max_mb, warning_mb, critical_mb),
        BackendCmd::Protect(drive) => mutate(
            id,
            backend,
            emit,
            |b| b.protect_volume(&drive),
            format!("{drive} 保护成功（下次启动生效）"),
        ),
        BackendCmd::Unprotect(drive) => mutate(
            id,
            backend,
            emit,
            |b| b.unprotect_volume(&drive),
            format!("{drive} 已取消保护（下次启动生效）"),
        ),
        BackendCmd::AddFileExclusion(path) => mutate_with(id, backend, emit, |b| {
            let (drive, rel) = split_absolute_path(&path)?;
            b.add_file_exclusion(&drive, &rel)?;
            Ok(format!("已添加文件排除: {path}"))
        }),
        BackendCmd::RemoveFileExclusion(path) => mutate_with(id, backend, emit, |b| {
            let (drive, rel) = split_absolute_path(&path)?;
            b.remove_file_exclusion(&drive, &rel)?;
            Ok(format!("已删除文件排除: {path}"))
        }),
        BackendCmd::AddRegistryExclusion(key) => mutate(
            id,
            backend,
            emit,
            |b| b.add_registry_exclusion(&key),
            format!("已添加注册表排除: {key}"),
        ),
        BackendCmd::RemoveRegistryExclusion(key) => mutate(
            id,
            backend,
            emit,
            |b| b.remove_registry_exclusion(&key),
            format!("已删除注册表排除: {key}"),
        ),
        BackendCmd::CommitFile(path) => mutate_with(id, backend, emit, |b| {
            let (drive, rel) = split_absolute_path(&path)?;
            b.commit_file(&drive, &rel)?;
            Ok(format!("{path} 提交成功"))
        }),
        BackendCmd::CommitFileDeletion(path) => mutate_with(id, backend, emit, |b| {
            let (drive, rel) = split_absolute_path(&path)?;
            b.commit_file_deletion(&drive, &rel)?;
            Ok(format!("{path} 删除提交成功"))
        }),
        BackendCmd::CommitRegistry(full_path) => mutate_with(id, backend, emit, |b| {
            let (key, value) = split_registry_path(&full_path)?;
            b.commit_registry(&key, &value)?;
            Ok(format!("注册表 {full_path} 提交成功"))
        }),
        BackendCmd::ImportExclusions { path } => import_exclusions(id, backend, emit, &path),
        BackendCmd::ExportExclusions { path } => export_exclusions(id, backend, emit, &path),
        BackendCmd::RevertExclusionsToCurrent => {
            revert_exclusions_to_current(id, backend, emit);
        }
        BackendCmd::RestartSystem => {
            mutate_without_refresh(id, backend, emit, |b| b.restart_system(), "系统即将重启")
        }
        BackendCmd::ShutdownSystem => {
            mutate_without_refresh(id, backend, emit, |b| b.shutdown_system(), "系统即将关机")
        }
    }
}

fn try_refresh(backend: &mut dyn UwfBackend, emit: &dyn Fn(BackendEvent)) -> Result<()> {
    let snapshot = backend.collect_snapshot()?;
    emit(BackendEvent::Snapshot(snapshot));
    Ok(())
}

/// 变更类命令统一出口：失败回 `Finished(Err)` 或 `NotInstalled(Some(id))`；
/// 成功先全量刷新，再回 `Finished::Ok` 或 `Finished::OkWithRefreshWarning`——
/// 动作成功与刷新失败是两个事实，不能互相覆盖。
fn mutate(
    id: RequestId,
    backend: &mut dyn UwfBackend,
    emit: &dyn Fn(BackendEvent),
    action: impl FnOnce(&mut dyn UwfBackend) -> Result<()>,
    ok_msg: impl Into<String>,
) {
    mutate_with(id, backend, emit, |b| {
        action(b)?;
        Ok(ok_msg.into())
    });
}

fn finish_with_refresh(
    id: RequestId,
    backend: &mut dyn UwfBackend,
    emit: &dyn Fn(BackendEvent),
    outcome: Outcome,
) {
    let final_outcome = match try_refresh(backend, emit) {
        Ok(()) => outcome,
        Err(e) => {
            let warn_text = match e {
                UwfError::NotInstalled => "状态刷新失败：UWF 功能未安装或未启用".into(),
                other => format!("状态刷新失败：{other}"),
            };
            match outcome {
                Outcome::Ok(message) => Outcome::OkWithRefreshWarning {
                    message,
                    warning: warn_text,
                },
                Outcome::Warning(msg) => Outcome::Warning(format!("{msg}；{warn_text}")),
                other => other,
            }
        }
    };
    send_finished(emit, id, final_outcome);
}

fn send_mutation_err(emit: &dyn Fn(BackendEvent), id: RequestId, err: UwfError) {
    match err {
        UwfError::NotInstalled => emit(BackendEvent::NotInstalled(Some(id))),
        other => send_finished(emit, id, Outcome::Err(other.to_string())),
    }
}

/// P1-17：缓存配置是多步 WMI 写入；中途失败时前面的步骤已经生效。
/// 失败也必须刷新，并在错误文案中列出“已应用”的步骤。
fn set_overlay_cache(
    id: RequestId,
    backend: &mut dyn UwfBackend,
    emit: &dyn Fn(BackendEvent),
    max_mb: u32,
    warning_mb: u32,
    critical_mb: u32,
) {
    let mut applied: Vec<&'static str> = Vec::new();
    let result = (|| -> Result<String> {
        // R1 双保险：当前启用时仅应用阈值（UI 侧已锁定最大缓存）
        let locked = backend.get_filter_state()?.current_enabled;
        if !locked {
            backend.set_overlay_max_size(max_mb)?;
            applied.push("最大缓存");
        }
        backend.set_warning_threshold(warning_mb)?;
        applied.push("警告阈值");
        backend.set_critical_threshold(critical_mb)?;
        applied.push("严重阈值");
        Ok(if locked {
            "缓存阈值已保存（最大缓存已锁定）".to_string()
        } else {
            "缓存配置已保存（下次启动生效）".to_string()
        })
    })();

    match result {
        Ok(msg) => finish_with_refresh(id, backend, emit, Outcome::Ok(msg)),
        Err(e) if applied.is_empty() && matches!(e, UwfError::NotInstalled) => {
            send_mutation_err(emit, id, e);
        }
        Err(e) => {
            // 让 UI 看到实际状态，而不是停留在旧的 Overlay 值。
            let refresh_warning = try_refresh(backend, emit).err();
            let mut msg = if applied.is_empty() {
                e.to_string()
            } else {
                format!("{e}；已应用: {}", applied.join("、"))
            };
            if let Some(w) = refresh_warning {
                msg.push_str(&format!("；状态刷新失败：{w}"));
            }
            send_finished(emit, id, Outcome::Err(msg));
        }
    }
}

/// 电源动作出口：动作成功后**不做全量刷新**。
///
/// 重启/关机发出后系统立即进入关机流程，再发起整轮 WMI 查询可能卡住，
/// 延迟终端恢复；动作是否成功只由 ReturnValue 决定。
pub(super) fn mutate_without_refresh(
    id: RequestId,
    backend: &mut dyn UwfBackend,
    emit: &dyn Fn(BackendEvent),
    action: impl FnOnce(&mut dyn UwfBackend) -> Result<()>,
    ok_msg: impl Into<String>,
) {
    match action(backend) {
        Ok(()) => send_finished(emit, id, Outcome::Ok(ok_msg.into())),
        Err(e) => send_mutation_err(emit, id, e),
    }
}

/// 同 [`mutate`]，但成功文案由 action 返回（适用于需先解析/校验的命令）。
fn mutate_with(
    id: RequestId,
    backend: &mut dyn UwfBackend,
    emit: &dyn Fn(BackendEvent),
    action: impl FnOnce(&mut dyn UwfBackend) -> Result<String>,
) {
    match action(backend) {
        Ok(msg) => finish_with_refresh(id, backend, emit, Outcome::Ok(msg)),
        Err(e) => send_mutation_err(emit, id, e),
    }
}

/// 导入排除列表（原版 button_import_list_Click 语义）：
/// 仅支持 UTF-8（含可选 UTF-8 BOM）；`HK` 开头（不区分大小写）→ 注册表排除，其余 → 文件排除。
pub(super) fn import_exclusions(
    id: RequestId,
    backend: &mut dyn UwfBackend,
    emit: &dyn Fn(BackendEvent),
    path: &Path,
) {
    let result = (|| -> Result<(usize, usize, Option<String>)> {
        // 明确只支持 UTF-8；非 UTF-8 给出可操作的中文错误，而不是底层 IO 报错。
        let bytes = std::fs::read(path)?;
        let content = String::from_utf8(bytes)
            .map_err(|_| UwfError::InvalidPath("导入文件须为 UTF-8 编码".into()))?;
        let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
        let mut ok = 0;
        let mut failed = 0;
        let mut first_err = None;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let r = if line.get(..2).is_some_and(|p| p.eq_ignore_ascii_case("HK")) {
                backend.add_registry_exclusion(line)
            } else {
                split_absolute_path(line)
                    .and_then(|(drive, rel)| backend.add_file_exclusion(&drive, &rel))
            };
            match r {
                Ok(()) => ok += 1,
                Err(e) => {
                    failed += 1;
                    if first_err.is_none() {
                        first_err = Some(e.to_string());
                    }
                }
            }
        }
        Ok((ok, failed, first_err))
    })();
    match result {
        Ok((ok, failed, first_err)) => {
            let err_detail = first_err
                .map(|e| format!("（首个错误：{e}）"))
                .unwrap_or_default();
            let base = match (ok, failed) {
                (0, 0) => Outcome::Ok("导入完成：文件为空".to_string()),
                (0, n) => Outcome::Err(format!("导入失败：{n} 项均未成功{err_detail}")),
                (n, 0) => Outcome::Ok(format!("导入完成：成功 {n} 项")),
                (n, m) => {
                    Outcome::Warning(format!("导入完成：成功 {n} 项，失败 {m} 项{err_detail}"))
                }
            };
            finish_with_refresh(id, backend, emit, base);
        }
        Err(e) => send_mutation_err(emit, id, e),
    }
}

/// 将“重启后生效”排除项恢复为与“当前生效”一致（先移除多余项，再补回被删项）。
fn revert_exclusions_to_current(
    id: RequestId,
    backend: &mut dyn UwfBackend,
    emit: &dyn Fn(BackendEvent),
) {
    let plan = (|| -> Result<ExclusionRevertPlan> {
        let files_current = backend.get_file_exclusions(true)?;
        let files_next = backend.get_file_exclusions(false)?;
        let regs_current = backend.get_registry_exclusions(true)?;
        let regs_next = backend.get_registry_exclusions(false)?;
        Ok(plan_exclusion_revert(
            &files_current,
            &files_next,
            &regs_current,
            &regs_next,
        ))
    })();

    let plan = match plan {
        Ok(p) => p,
        Err(e) => {
            send_mutation_err(emit, id, e);
            return;
        }
    };

    if plan.is_empty() {
        finish_with_refresh(
            id,
            backend,
            emit,
            Outcome::Ok("重启后排除项已与当前生效一致，无需撤销".to_string()),
        );
        return;
    }

    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut first_err: Option<String> = None;

    let mut track = |res: Result<()>| match res {
        Ok(()) => ok += 1,
        Err(e) => {
            failed += 1;
            if first_err.is_none() {
                first_err = Some(e.to_string());
            }
        }
    };

    for file in &plan.files_to_remove {
        track(backend.remove_file_exclusion(&file.drive_letter, &file.file_rel_path));
    }
    for key in &plan.regs_to_remove {
        track(backend.remove_registry_exclusion(key));
    }
    for file in &plan.files_to_add {
        track(backend.add_file_exclusion(&file.drive_letter, &file.file_rel_path));
    }
    for key in &plan.regs_to_add {
        track(backend.add_registry_exclusion(key));
    }

    let err_detail = first_err
        .map(|e| format!("（首个错误：{e}）"))
        .unwrap_or_default();
    let outcome = if failed == 0 {
        Outcome::Ok(format!(
            "已撤销排除项修改（移除 {} 项，恢复 {} 项），重启后列表已与当前生效一致",
            plan.remove_count(),
            plan.add_count()
        ))
    } else if ok == 0 {
        Outcome::Err(format!(
            "撤销排除项修改失败：{failed} 项均未成功{err_detail}"
        ))
    } else {
        Outcome::Warning(format!(
            "已部分撤销排除项修改：成功 {ok} 项，失败 {failed} 项{err_detail}"
        ))
    };
    finish_with_refresh(id, backend, emit, outcome);
}

/// 原子创建不覆盖既有文件的导出路径：同秒重复导出自动追加 `_1`、`_2`。
pub(super) fn create_unique_export_file(path: &Path) -> io::Result<(PathBuf, std::fs::File)> {
    let mut candidate = path.to_path_buf();
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("EXCLUSIONLIST");
    let ext = path.extension().and_then(|s| s.to_str());
    let mut suffix = 0usize;
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                suffix += 1;
                let name = match ext {
                    Some(ext) => format!("{stem}_{suffix}.{ext}"),
                    None => format!("{stem}_{suffix}"),
                };
                candidate = path.with_file_name(name);
            }
            Err(e) => return Err(e),
        }
    }
}

/// 导出排除列表（原版 button_save_exclusionlist_Click 语义）：
/// 文件排除（绝对路径形式）在前，注册表排除在后，每行一条。
pub(super) fn export_exclusions(
    id: RequestId,
    backend: &mut dyn UwfBackend,
    emit: &dyn Fn(BackendEvent),
    path: &Path,
) {
    let result = (|| -> Result<(usize, PathBuf, Vec<String>)> {
        // 排除列表与配置保持一致，统一导出下次会话（重启后生效）的持久规则
        let files = backend.get_file_exclusions(false)?;
        let registry = backend.get_registry_exclusions(false)?;
        let warnings = backend.take_warnings();
        let mut lines: Vec<String> = files.iter().map(|e| e.display()).collect();
        lines.extend(registry);
        // 空列表导出 0 字节，不写一个只有换行符的文件。
        let body = if lines.is_empty() {
            String::new()
        } else {
            lines.join("\n") + "\n"
        };
        let (actual_path, mut file) = create_unique_export_file(path)?;
        file.write_all(body.as_bytes())?;
        Ok((lines.len(), actual_path, warnings))
    })();
    match result {
        Ok((count, actual_path, warnings)) => {
            let outcome = if warnings.is_empty() {
                Outcome::Ok(format!("已导出 {count} 项至 {}", actual_path.display()))
            } else {
                Outcome::Warning(format!(
                    "已导出 {count} 项至 {}（部分卷读取异常：{}）",
                    actual_path.display(),
                    warnings.join("；")
                ))
            };
            send_finished(emit, id, outcome);
        }
        Err(e) => send_mutation_err(emit, id, e),
    }
}

fn send_finished(emit: &dyn Fn(BackendEvent), id: RequestId, outcome: Outcome) {
    emit(BackendEvent::Finished { id, outcome });
}
