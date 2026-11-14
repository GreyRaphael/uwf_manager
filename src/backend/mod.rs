//! 后端抽象层：UI 与底层 UWF 实现的隔离边界。
//!
//! 线程协议（PLAN v2 §6.5）：UI 线程发送 [`BackendCommand`]（查询、带 [`RequestId`] 的变更或退出），
//! 后端线程独占 `dyn UwfBackend` 执行后经 [`BackendEvent`] 回包。变更类命令
//! 成功后自动跟随一次全量刷新，刷新结果通过 [`Outcome`] 与动作结果一起上报。

mod mock;
#[cfg(windows)]
mod windows;

pub use mock::MockUwfBackend;
#[cfg(windows)]
pub(crate) use windows::is_elevated;

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::error::{Result, UwfError};
use crate::model::*;

/// UWF 后端接口（PLAN v2 §6.1）。业务规则 R1-R6 见各方法注释。
///
/// 全部方法 `&mut self`：Mock 需要可变状态，WMI 实现内部无状态。
/// 注意**不要求 `Send`**：`WMIConnection` 为 `!Send`（COM 套间约束，
/// PLAN D4），后端实例由工厂在后端线程内构造，Box 永不跨线程。
pub trait UwfBackend: 'static {
    // ── Filter（单实例，无会话分裂）──
    fn get_filter_state(&mut self) -> Result<FilterState>;
    /// 下次启动生效（R1 同）
    fn enable_filter(&mut self) -> Result<()>;
    fn disable_filter(&mut self) -> Result<()>;
    /// 重置全部 UWF 配置，下次启动生效
    fn reset_settings(&mut self) -> Result<()>;
    fn restart_system(&mut self) -> Result<()>;
    fn shutdown_system(&mut self) -> Result<()>;

    // ── Overlay ──
    fn get_overlay_config(&mut self, current_session: bool) -> Result<OverlayConfig>;
    /// 2 秒轮询目标
    fn get_overlay_runtime(&mut self) -> Result<OverlayRuntime>;
    /// R1：筛选器"当前已启用"时必须拒绝并返回 [`UwfError::FilterCurrentlyEnabled`]
    fn set_overlay_type(&mut self, overlay_type: OverlayType) -> Result<()>;
    /// R1：同上
    fn set_overlay_max_size(&mut self, size_mb: u32) -> Result<()>;
    fn set_warning_threshold(&mut self, size_mb: u32) -> Result<()>;
    fn set_critical_threshold(&mut self, size_mb: u32) -> Result<()>;

    // ── Volume ──
    /// R2：合并 current/next 两次查询（按盘符）
    fn list_volumes(&mut self) -> Result<Vec<VolumeInfo>>;
    /// R2：作用于"下次会话"实例
    fn protect_volume(&mut self, drive_letter: &str) -> Result<()>;
    fn unprotect_volume(&mut self, drive_letter: &str) -> Result<()>;
    /// R3：作用于"当前会话"实例
    fn commit_file(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()>;
    /// R3：两步语义——先真实删除文件（容忍 NotFound），再调 WMI 固化删除；
    /// 作用于"下次会话"实例
    fn commit_file_deletion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()>;

    // ── Exclusion ──
    fn get_file_exclusions(&mut self, current_session: bool) -> Result<Vec<FileExclusion>>;
    /// R4：增删作用于"下次会话"实例
    fn add_file_exclusion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()>;
    fn remove_file_exclusion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()>;
    fn get_registry_exclusions(&mut self, current_session: bool) -> Result<Vec<String>>;
    fn add_registry_exclusion(&mut self, key: &str) -> Result<()>;
    fn remove_registry_exclusion(&mut self, key: &str) -> Result<()>;
    /// R5：当前会话实例；key/value 由后端线程按最后一个 `\` 拆分后传入
    fn commit_registry(&mut self, key: &str, value_name: &str) -> Result<()>;

    /// P1-6：取出本次快照期间累积的非致命警告（默认无）。
    fn take_warnings(&mut self) -> Vec<String> {
        Vec::new()
    }
}

/// 全量状态快照（`RefreshAll` 回包）
#[derive(Debug, Clone, Default)]
pub struct FullSnapshot {
    pub filter_state: Option<FilterState>,
    pub overlay_config_current: Option<OverlayConfig>,
    pub overlay_config_next: Option<OverlayConfig>,
    pub overlay_runtime: Option<OverlayRuntime>,
    pub volumes: Vec<VolumeInfo>,
    pub file_exclusions_current: Vec<FileExclusion>,
    pub file_exclusions_next: Vec<FileExclusion>,
    pub registry_exclusions_current: Vec<String>,
    pub registry_exclusions_next: Vec<String>,
    /// 部分卷读取失败等非致命警告
    pub warnings: Vec<String>,
}

/// 请求关联 id：仅用于变更请求，确保只有匹配的 `Finished` / `NotInstalled` 解除在途锁。
pub type RequestId = u64;

/// UI → 后端线程消息。
#[derive(Debug)]
pub enum BackendCommand {
    RefreshAll,
    RefreshOverlayRuntime,
    Mutate {
        id: RequestId,
        cmd: BackendCmd,
    },
    /// 结束后端线程
    Quit,
}

/// UI → 后端变更命令（PLAN §6.5 业务操作）。
#[derive(Debug)]
pub enum BackendCmd {
    EnableFilter,
    DisableFilter,
    ResetSettings,
    SetOverlayConfig {
        overlay_type: OverlayType,
        max_mb: u32,
        warning_mb: u32,
        critical_mb: u32,
    },
    Protect(String),
    Unprotect(String),
    /// 绝对路径，线程内按 R6 拆分
    AddFileExclusion(String),
    RemoveFileExclusion(String),
    AddRegistryExclusion(String),
    RemoveRegistryExclusion(String),
    /// 绝对路径
    CommitFile(String),
    /// 绝对路径，含 R3 两步语义
    CommitFileDeletion(String),
    /// 注册表完整路径，线程内按 R5 拆分
    CommitRegistry(String),
    /// 逐行解析：`HK` 前缀 → 注册表排除，其余 → 文件排除
    ImportExclusions {
        path: PathBuf,
    },
    /// 导出 UI 当前视图（current_session 由 UI 传入）
    ExportExclusions {
        path: PathBuf,
        current_session: bool,
    },
    RestartSystem,
    ShutdownSystem,
}

/// 一次变更请求的最终结果。
#[derive(Debug)]
pub enum Outcome {
    /// 动作成功，且随后的状态刷新也成功
    Ok(String),
    /// 动作完成，但需要以警告级别提示（如部分导入失败）
    Warning(String),
    /// 动作成功，但状态刷新失败：两个事实都要告诉用户
    OkWithRefreshWarning { message: String, warning: String },
    /// 动作失败
    Err(String),
}

/// 后端线程 → UI 事件。
#[derive(Debug)]
pub enum BackendEvent {
    /// 全量状态快照
    Snapshot(FullSnapshot),
    /// 2 秒增量回包
    OverlayRuntime(OverlayRuntime),
    /// `RefreshAll` 失败（与变更请求解耦，不会误解锁 in-flight mutation）
    RefreshFailed(String),
    /// 变更请求的最终结果（只由 [`BackendCommand::Mutate`] 产出）
    Finished { id: RequestId, outcome: Outcome },
    /// UWF 功能未安装（D6 引导页；`Some(id)` 表示某条变更触发，应同步解锁）
    NotInstalled(Option<RequestId>),
    /// 后端线程意外退出（panic 等非 Quit 终止）
    Disconnected,
}

/// 后端来源标识（标题栏徽标）
// Windows 变体仅 Windows 构建构造；Linux 下视为保留变体
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Windows,
    Mock,
}

/// 后端工厂：在后端线程内构造后端实例。
///
/// 工厂本身可安全跨线程（`Send`），但产出的 `Box<dyn UwfBackend>` 不可——
/// WMI 连接必须在使用它的线程内创建（COM 套间）。
pub type BackendFactory = Box<dyn FnOnce() -> Box<dyn UwfBackend> + Send>;

/// 创建平台后端工厂：Windows 且未指定 `--mock` 时用 WMI 后端，其余一律 Mock。
pub fn create_backend_factory(force_mock: bool) -> (BackendFactory, BackendKind) {
    #[cfg(windows)]
    if !force_mock {
        return (
            Box::new(|| Box::new(self::windows::WindowsWmiBackend::new())),
            BackendKind::Windows,
        );
    }
    let _ = force_mock;
    (
        Box::new(|| Box::new(MockUwfBackend::demo())),
        BackendKind::Mock,
    )
}

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
    clean_quit: bool,
}

impl<E: From<BackendEvent>> Drop for DisconnectGuard<E> {
    fn drop(&mut self) {
        if !self.clean_quit {
            let _ = self.tx.send(E::from(BackendEvent::Disconnected));
        }
    }
}

/// 启动后端线程（actor）：工厂在本线程内构造 backend（WMI 连接随之在
/// 本线程建立），顺序执行命令，`Quit` 退出。若意外退出（如 panic），
/// `Drop` 哨兵立即发送 [`BackendEvent::Disconnected`] 唤醒 UI。
pub fn spawn_backend_thread<E>(
    factory: BackendFactory,
    rx: Receiver<BackendCommand>,
    tx: Sender<E>,
) -> BackendHandle
where
    E: From<BackendEvent> + Send + 'static,
{
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let join = thread::spawn(move || {
        let mut guard = DisconnectGuard {
            tx: tx.clone(),
            clean_quit: false,
        };
        let emit = |evt| {
            let _ = tx.send(E::from(evt));
        };
        let mut backend = factory();
        while let Ok(command) = rx.recv() {
            match command {
                BackendCommand::Quit => {
                    guard.clean_quit = true;
                    let _ = stop_tx.send(());
                    break;
                }
                other => handle_command(&mut *backend, other, &emit),
            }
        }
    });
    BackendHandle {
        join,
        stopped: stop_rx,
    }
}

fn handle_command(
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
        BackendCommand::RefreshOverlayRuntime => match backend.get_overlay_runtime() {
            Ok(runtime) => emit(BackendEvent::OverlayRuntime(runtime)),
            Err(UwfError::NotInstalled) => emit(BackendEvent::NotInstalled(None)),
            // 后台 2 秒轮询偶发失败静默跳过，避免覆盖底部操作反馈；用户可用 [F5] 显式诊断。
            Err(_) => {}
        },
        BackendCommand::Mutate { id, cmd } => handle_mutation(id, backend, cmd, emit),
        BackendCommand::Quit => unreachable!("Quit 由线程循环拦截"),
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
        BackendCmd::SetOverlayConfig {
            overlay_type,
            max_mb,
            warning_mb,
            critical_mb,
        } => set_overlay_config(
            id,
            backend,
            emit,
            overlay_type,
            max_mb,
            warning_mb,
            critical_mb,
        ),
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
        BackendCmd::ExportExclusions {
            path,
            current_session,
        } => export_exclusions(id, backend, emit, &path, current_session),
        BackendCmd::RestartSystem => {
            mutate_without_refresh(id, backend, emit, |b| b.restart_system(), "系统即将重启")
        }
        BackendCmd::ShutdownSystem => {
            mutate_without_refresh(id, backend, emit, |b| b.shutdown_system(), "系统即将关机")
        }
    }
}

fn try_refresh(backend: &mut dyn UwfBackend, emit: &dyn Fn(BackendEvent)) -> Result<()> {
    let snapshot = collect_snapshot(backend)?;
    emit(BackendEvent::Snapshot(snapshot));
    Ok(())
}

fn collect_snapshot(b: &mut dyn UwfBackend) -> Result<FullSnapshot> {
    Ok(FullSnapshot {
        filter_state: Some(b.get_filter_state()?),
        overlay_config_current: Some(b.get_overlay_config(true)?),
        overlay_config_next: Some(b.get_overlay_config(false)?),
        overlay_runtime: Some(b.get_overlay_runtime()?),
        volumes: b.list_volumes()?,
        file_exclusions_current: b.get_file_exclusions(true)?,
        file_exclusions_next: b.get_file_exclusions(false)?,
        registry_exclusions_current: b.get_registry_exclusions(true)?,
        registry_exclusions_next: b.get_registry_exclusions(false)?,
        warnings: b.take_warnings(),
    })
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

/// P1-17：Overlay 配置是多步 WMI 写入；中途失败时前面的步骤已经生效。
/// 失败也必须刷新，并在错误文案中列出“已应用”的步骤。
fn set_overlay_config(
    id: RequestId,
    backend: &mut dyn UwfBackend,
    emit: &dyn Fn(BackendEvent),
    overlay_type: OverlayType,
    max_mb: u32,
    warning_mb: u32,
    critical_mb: u32,
) {
    let mut applied: Vec<&'static str> = Vec::new();
    let result = (|| -> Result<String> {
        // R1 双保险：当前启用时仅应用阈值（UI 侧已锁定其余两项）
        let locked = backend.get_filter_state()?.current_enabled;
        if !locked {
            backend.set_overlay_type(overlay_type)?;
            applied.push("类型");
            backend.set_overlay_max_size(max_mb)?;
            applied.push("上限");
        }
        backend.set_warning_threshold(warning_mb)?;
        applied.push("预警阈值");
        backend.set_critical_threshold(critical_mb)?;
        applied.push("临界阈值");
        Ok(if locked {
            "Overlay 阈值已保存（类型/上限已锁定）".to_string()
        } else {
            "Overlay 配置已保存（下次启动生效）".to_string()
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
fn mutate_without_refresh(
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
fn import_exclusions(
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

/// 原子创建不覆盖既有文件的导出路径：同秒重复导出自动追加 `_1`、`_2`。
fn create_unique_export_file(path: &Path) -> io::Result<(PathBuf, std::fs::File)> {
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
fn export_exclusions(
    id: RequestId,
    backend: &mut dyn UwfBackend,
    emit: &dyn Fn(BackendEvent),
    path: &Path,
    current_session: bool,
) {
    let result = (|| -> Result<(usize, PathBuf, Vec<String>)> {
        let files = backend.get_file_exclusions(current_session)?;
        let registry = backend.get_registry_exclusions(current_session)?;
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

/// R2：合并 current/next 两次卷查询（按盘符，大小写归一）。
/// 抽成纯函数便于在非 Windows 平台覆盖 T14（next-only 卷）。
#[cfg(any(windows, test))]
pub(crate) fn merge_volume_infos(
    current: impl IntoIterator<Item = (String, bool)>,
    next: impl IntoIterator<Item = (String, bool)>,
) -> Vec<VolumeInfo> {
    let mut map: std::collections::BTreeMap<String, VolumeInfo> = std::collections::BTreeMap::new();
    for (drive, protected) in current {
        let key = drive.to_ascii_uppercase();
        map.insert(
            key,
            VolumeInfo {
                drive_letter: drive,
                current_protected: protected,
                next_protected: false,
            },
        );
    }
    for (drive, protected) in next {
        let key = drive.to_ascii_uppercase();
        let entry = map.entry(key).or_insert_with(|| VolumeInfo {
            drive_letter: drive,
            current_protected: false,
            next_protected: false,
        });
        entry.next_protected = protected;
    }
    map.into_values().collect()
}

/// R6：绝对路径 → (盘符, 相对路径)。
/// 如 `"C:\Windows\a.txt"` → `("C:", "\Windows\a.txt")`（相对路径保留前导 `\`，
/// 与原版 `FileName.Remove(0, 2)` 一致）。
///
/// 非 Windows 平台（Mock 预览）额外接受 Unix 绝对路径并映射到演示卷 `C:`。
pub fn split_absolute_path(path: &str) -> Result<(String, String)> {
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let drive = path[..2].to_ascii_uppercase();
        let rest = &path[2..];
        if rest.is_empty() {
            return Ok((drive, "\\".to_string()));
        }
        // 只接受真正的绝对路径：C:\foo 或 C:/foo。
        // C:foo 是“C 盘当前目录下的 foo”，不能静默改写成 C:\foo。
        if !(rest.starts_with('\\') || rest.starts_with('/')) {
            return Err(UwfError::InvalidPath(format!(
                "必须是绝对路径（如 C:\\Windows\\Temp）: {path}"
            )));
        }
        return Ok((drive, rest.replace('/', "\\")));
    }
    #[cfg(not(windows))]
    if let Some(rest) = path.strip_prefix('/') {
        return Ok(("C:".to_string(), format!("\\{}", rest.replace('/', "\\"))));
    }
    Err(UwfError::InvalidPath(path.into()))
}

/// R5：注册表完整路径 → (RegistryKey, ValueName)，按最后一个 `\` 拆分
/// （与原版 `UWF_RegistryFilter.CommitRegistry` 一致）。
pub fn split_registry_path(path: &str) -> Result<(String, String)> {
    match path.rsplit_once('\\') {
        Some((key, value)) if !key.is_empty() && !value.is_empty() => {
            Ok((key.to_string(), value.to_string()))
        }
        _ => Err(UwfError::InvalidPath(format!(
            "注册表路径须包含键与值名称: {path}"
        ))),
    }
}

fn send_finished(emit: &dyn Fn(BackendEvent), id: RequestId, outcome: Outcome) {
    emit(BackendEvent::Finished { id, outcome });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r6_split_absolute_path() {
        let (drive, rel) = split_absolute_path("C:\\Windows\\a.txt").unwrap();
        assert_eq!((drive.as_str(), rel.as_str()), ("C:", "\\Windows\\a.txt"));
        // 小写盘符归一化为大写
        let (drive, _) = split_absolute_path("d:\\data").unwrap();
        assert_eq!(drive, "D:");
        // C:foo 是 drive-relative，不是绝对路径，必须拒绝
        assert!(split_absolute_path("C:foo").is_err());
        // 正斜杠统一成反斜杠
        let (drive, rel) = split_absolute_path("C:/Windows/a").unwrap();
        assert_eq!((drive.as_str(), rel.as_str()), ("C:", "\\Windows\\a"));
        // 只有盘符时表示根目录
        let (drive, rel) = split_absolute_path("C:").unwrap();
        assert_eq!((drive.as_str(), rel.as_str()), ("C:", "\\"));
        // 非法盘符路径
        assert!(split_absolute_path("C::\\foo").is_err());
        // 相对路径报错
        assert!(matches!(
            split_absolute_path("Windows\\a.txt"),
            Err(UwfError::InvalidPath(_))
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn r6_unix_absolute_path_maps_to_demo_volume() {
        // 非 Windows 平台（Mock 预览）：Unix 绝对路径映射到演示卷 C:
        let (drive, rel) = split_absolute_path("/home/gewei/tmp").unwrap();
        assert_eq!((drive.as_str(), rel.as_str()), ("C:", "\\home\\gewei\\tmp"));
    }

    #[test]
    fn r5_split_registry_path() {
        let (key, value) = split_registry_path("HKLM\\SOFTWARE\\X\\MyValue").unwrap();
        assert_eq!(
            (key.as_str(), value.as_str()),
            ("HKLM\\SOFTWARE\\X", "MyValue")
        );
        // 完全无反斜杠时报错；"HKLM\\SOFTWARE" 按规则可拆为 (HKLM, SOFTWARE)，合法
        assert!(matches!(
            split_registry_path("NOSPLIT"),
            Err(UwfError::InvalidPath(_))
        ));
        assert!(split_registry_path("").is_err());
    }

    #[test]
    fn import_export_roundtrip_with_mock() {
        let dir = std::env::temp_dir().join(format!("uwf_mgr_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let import_path = dir.join("import.txt");
        std::fs::write(
            &import_path,
            "C:\\Windows\\Temp\nHKLM\\SOFTWARE\\Demo\nnot_a_path\n\nD:\\data\\cache\n",
        )
        .unwrap();

        let mut mock = MockUwfBackend::demo();
        let content = std::fs::read_to_string(&import_path).unwrap();
        let mut ok = 0;
        let mut failed = 0;
        for line in content.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let r = if line.starts_with("HK") {
                mock.add_registry_exclusion(line)
            } else {
                split_absolute_path(line)
                    .and_then(|(drive, rel)| mock.add_file_exclusion(&drive, &rel))
            };
            match r {
                Ok(()) => ok += 1,
                Err(_) => failed += 1,
            }
        }
        assert_eq!((ok, failed), (3, 1));
        // 全部写入 next 侧（R4），current 侧不受影响
        assert!(mock.get_file_exclusions(true).unwrap().is_empty());
        // 预置 1 条（与导入首行重复被去重）+ 实际新增 1 条 = 2
        assert_eq!(mock.get_file_exclusions(false).unwrap().len(), 2);
        // 预置 1 + 导入 1
        assert_eq!(mock.get_registry_exclusions(false).unwrap().len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
    fn test_sink() -> (Sender<BackendEvent>, Receiver<BackendEvent>) {
        std::sync::mpsc::channel()
    }

    #[test]
    fn power_actions_finish_without_snapshot() {
        let mut backend = MockUwfBackend::demo();
        let (tx, rx) = test_sink();
        mutate_without_refresh(
            7,
            &mut backend,
            &|e| {
                let _ = tx.send(e);
            },
            |b| b.restart_system(),
            "系统即将重启",
        );
        match rx.try_recv().unwrap() {
            BackendEvent::Finished {
                id: 7,
                outcome: Outcome::Ok(msg),
            } => assert_eq!(msg, "系统即将重启"),
            other => panic!("期望仅收到 Finished(Ok)，实际 {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "电源动作不应再发 Snapshot");
    }

    #[test]
    fn refresh_all_sends_snapshot() {
        let mut backend = MockUwfBackend::demo();
        let (tx, rx) = test_sink();
        handle_command(&mut backend, BackendCommand::RefreshAll, &|e| {
            let _ = tx.send(e);
        });
        match rx.try_recv().unwrap() {
            BackendEvent::Snapshot(snapshot) => {
                assert!(snapshot.filter_state.is_some());
            }
            other => panic!("期望 Snapshot，实际 {other:?}"),
        }
    }
    fn finished_outcomes(rx: &Receiver<BackendEvent>) -> Vec<Outcome> {
        let mut out = Vec::new();
        while let Ok(evt) = rx.try_recv() {
            if let BackendEvent::Finished { outcome, .. } = evt {
                out.push(outcome);
            }
        }
        out
    }

    #[test]
    fn import_all_failed_is_error_partial_is_warning() {
        let dir = std::env::temp_dir().join(format!("uwf_import_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let all_fail = dir.join("all_fail.txt");
        std::fs::write(&all_fail, "not_a_path\n").unwrap();
        let mut backend = MockUwfBackend::demo();
        let (tx, rx) = test_sink();
        let emit = |e| {
            let _ = tx.send(e);
        };
        import_exclusions(1, &mut backend, &emit, &all_fail);
        assert!(matches!(
            finished_outcomes(&rx).as_slice(),
            [Outcome::Err(_)]
        ));

        let partial = dir.join("partial.txt");
        std::fs::write(&partial, "C:\\Windows\\Temp\nnot_a_path\n").unwrap();
        let mut backend = MockUwfBackend::demo();
        let (tx, rx) = test_sink();
        let emit = |e| {
            let _ = tx.send(e);
        };
        import_exclusions(2, &mut backend, &emit, &partial);
        assert!(matches!(
            finished_outcomes(&rx).as_slice(),
            [Outcome::Warning(_)]
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn import_strips_utf8_bom_and_export_empty_is_zero_bytes() {
        let dir = std::env::temp_dir().join(format!("uwf_bom_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let bom = dir.join("bom.txt");
        std::fs::write(&bom, "\u{feff}C:\\Windows\\Bom\n").unwrap();
        let mut backend = MockUwfBackend::demo();
        let (tx, _rx) = test_sink();
        import_exclusions(
            1,
            &mut backend,
            &|e| {
                let _ = tx.send(e);
            },
            &bom,
        );
        assert!(
            backend
                .get_file_exclusions(false)
                .unwrap()
                .iter()
                .any(|e| e.display() == "C:\\Windows\\Bom")
        );

        let export = dir.join("empty.txt");
        let mut empty = MockUwfBackend::empty();
        let (tx, _rx) = test_sink();
        export_exclusions(
            2,
            &mut empty,
            &|e| {
                let _ = tx.send(e);
            },
            &export,
            false,
        );
        assert_eq!(std::fs::metadata(&export).unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_rejects_non_utf8_with_clear_error() {
        let dir = std::env::temp_dir().join(format!("uwf_utf16_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("utf16.txt");
        std::fs::write(&path, [0xFF, 0xFE, b'C', 0, b':', 0]).unwrap();

        let mut backend = MockUwfBackend::demo();
        let (tx, rx) = test_sink();
        import_exclusions(
            1,
            &mut backend,
            &|e| {
                let _ = tx.send(e);
            },
            &path,
        );
        let outcomes = finished_outcomes(&rx);
        assert!(matches!(
            outcomes.as_slice(),
            [Outcome::Err(msg)] if msg.contains("UTF-8")
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn volume_union_includes_next_only() {
        let merged = merge_volume_infos(
            vec![("C:".to_string(), true)],
            vec![("c:".to_string(), false), ("D:".to_string(), true)],
        );
        assert_eq!(merged.len(), 2);
        assert!(
            merged
                .iter()
                .any(|v| v.drive_letter == "C:" && v.current_protected)
        );
        assert!(
            merged
                .iter()
                .any(|v| v.drive_letter == "D:" && !v.current_protected && v.next_protected)
        );
    }

    #[test]
    fn unique_export_file_creates_non_colliding_files() {
        let dir = std::env::temp_dir().join(format!("uwf_export_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("EXCLUSIONLIST_2026_10_20_12_00_00.txt");

        let (p1, _f1) = create_unique_export_file(&base).unwrap();
        let (p2, _f2) = create_unique_export_file(&base).unwrap();
        let (p3, _f3) = create_unique_export_file(&base).unwrap();
        assert_eq!(
            p1.file_name().and_then(|s| s.to_str()),
            Some("EXCLUSIONLIST_2026_10_20_12_00_00.txt")
        );
        assert_eq!(
            p2.file_name().and_then(|s| s.to_str()),
            Some("EXCLUSIONLIST_2026_10_20_12_00_00_1.txt")
        );
        assert_eq!(
            p3.file_name().and_then(|s| s.to_str()),
            Some("EXCLUSIONLIST_2026_10_20_12_00_00_2.txt")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_supports_lowercase_hk_and_reports_first_error() {
        let dir = std::env::temp_dir().join(format!("uwf_hk_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hk.txt");
        // 首行小写 hklm，次行包含多字节汉字且非法路径，确认不 panic 且携带首个错误原因
        std::fs::write(&path, "hklm\\software\\demo\n中文非路径\n").unwrap();

        let mut backend = MockUwfBackend::demo();
        let (tx, rx) = test_sink();
        import_exclusions(
            3,
            &mut backend,
            &|e| {
                let _ = tx.send(e);
            },
            &path,
        );
        assert!(
            backend
                .get_registry_exclusions(false)
                .unwrap()
                .iter()
                .any(|k| k == "hklm\\software\\demo")
        );
        assert!(matches!(
            finished_outcomes(&rx).as_slice(),
            [Outcome::Warning(msg)] if msg.contains("首个错误") && msg.contains("中文非路径")
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct StubWarningOrNotInstalled {
        warnings: Vec<String>,
        not_installed: bool,
    }

    impl UwfBackend for StubWarningOrNotInstalled {
        fn get_filter_state(&mut self) -> Result<FilterState> {
            Err(UwfError::NotInstalled)
        }
        fn enable_filter(&mut self) -> Result<()> {
            Err(UwfError::NotInstalled)
        }
        fn disable_filter(&mut self) -> Result<()> {
            Ok(())
        }
        fn reset_settings(&mut self) -> Result<()> {
            Ok(())
        }
        fn restart_system(&mut self) -> Result<()> {
            Ok(())
        }
        fn shutdown_system(&mut self) -> Result<()> {
            Ok(())
        }
        fn get_overlay_config(&mut self, _: bool) -> Result<OverlayConfig> {
            Err(UwfError::NotInstalled)
        }
        fn get_overlay_runtime(&mut self) -> Result<OverlayRuntime> {
            Err(UwfError::NotInstalled)
        }
        fn set_overlay_type(&mut self, _: OverlayType) -> Result<()> {
            Ok(())
        }
        fn set_overlay_max_size(&mut self, _: u32) -> Result<()> {
            Ok(())
        }
        fn set_warning_threshold(&mut self, _: u32) -> Result<()> {
            Ok(())
        }
        fn set_critical_threshold(&mut self, _: u32) -> Result<()> {
            Ok(())
        }
        fn list_volumes(&mut self) -> Result<Vec<VolumeInfo>> {
            Ok(vec![])
        }
        fn protect_volume(&mut self, _: &str) -> Result<()> {
            Ok(())
        }
        fn unprotect_volume(&mut self, _: &str) -> Result<()> {
            Ok(())
        }
        fn commit_file(&mut self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn commit_file_deletion(&mut self, _: &str, _: &str) -> Result<()> {
            Err(UwfError::CommitDeleteAfterFileRemoved {
                path: "C:\\a".into(),
                message: "wmi err".into(),
            })
        }
        fn get_file_exclusions(&mut self, _: bool) -> Result<Vec<FileExclusion>> {
            if self.not_installed {
                Err(UwfError::NotInstalled)
            } else {
                Ok(vec![])
            }
        }
        fn add_file_exclusion(&mut self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn remove_file_exclusion(&mut self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn get_registry_exclusions(&mut self, _: bool) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn add_registry_exclusion(&mut self, _: &str) -> Result<()> {
            Ok(())
        }
        fn remove_registry_exclusion(&mut self, _: &str) -> Result<()> {
            Ok(())
        }
        fn commit_registry(&mut self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn take_warnings(&mut self) -> Vec<String> {
            std::mem::take(&mut self.warnings)
        }
    }

    #[test]
    fn export_drains_warnings_and_not_installed_propagates() {
        let dir = std::env::temp_dir().join(format!("uwf_warn_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let export = dir.join("out.txt");

        let mut stub = StubWarningOrNotInstalled {
            warnings: vec!["卷 D: 读取失败".into()],
            not_installed: false,
        };
        let (tx, rx) = test_sink();
        export_exclusions(
            1,
            &mut stub,
            &|e| {
                let _ = tx.send(e);
            },
            &export,
            false,
        );
        assert!(stub.warnings.is_empty(), "export 必须消费 warnings");
        assert!(matches!(
            finished_outcomes(&rx).as_slice(),
            [Outcome::Warning(msg)] if msg.contains("卷 D: 读取失败")
        ));

        let (tx, rx) = test_sink();
        let emit = |e| {
            let _ = tx.send(e);
        };
        handle_command(&mut stub, BackendCommand::RefreshAll, &emit);
        assert!(matches!(
            rx.try_recv().unwrap(),
            BackendEvent::NotInstalled(None)
        ));
        handle_command(
            &mut stub,
            BackendCommand::Mutate {
                id: 9,
                cmd: BackendCmd::EnableFilter,
            },
            &emit,
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            BackendEvent::NotInstalled(Some(9))
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
