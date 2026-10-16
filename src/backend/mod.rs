//! 后端抽象层：UI 与底层 UWF 实现的隔离边界。
//!
//! 线程协议（PLAN v2 §6.5）：UI 线程发送 [`BackendCmd`]，后端线程独占
//! `dyn UwfBackend` 执行后经 [`BackendEvent`] 回包。变更类命令成功后
//! 自动跟随一次全量刷新，保证 UI 状态一致。

mod mock;

pub use mock::MockUwfBackend;

use std::sync::mpsc::{Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::error::{Result, UwfError};
use crate::model::*;

/// UWF 后端接口（PLAN v2 §6.1）。业务规则 R1-R6 见各方法注释。
///
/// 全部方法 `&mut self`：Mock 需要可变状态，WMI 实现内部无状态。
// 阶段 1 先落完整接口，阶段 2 UI 接线全部方法后移除该 allow
#[allow(dead_code)]
pub trait UwfBackend: Send + 'static {
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
}

/// UI → 后端线程命令
#[derive(Debug)]
pub enum BackendCmd {
    RefreshAll,
    RefreshOverlayRuntime,
    EnableFilter,
    DisableFilter,
    Protect(String),
    Unprotect(String),
    /// 结束后端线程
    Quit,
}

/// 后端线程 → UI 事件
#[derive(Debug)]
pub enum BackendEvent {
    Snapshot(Box<FullSnapshot>),
    /// 2 秒增量回包
    OverlayRuntime(OverlayRuntime),
    /// 操作成功（Toast 文案）
    OpOk(String),
    /// 操作失败（Toast 文案，含错误码）
    OpErr(String),
    /// UWF 功能未安装（D6 引导页）
    NotInstalled,
}

/// 后端来源标识（标题栏徽标）
// `Windows` 变体在阶段 3 接入 WMI 后端后移除该 allow
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Windows,
    Mock,
}

/// 创建平台后端。
///
/// 阶段 1：全平台返回 Mock；阶段 3 接入后，Windows 且未指定 `--mock` 时
/// 返回 `WindowsWmiBackend`。
pub fn create_backend(force_mock: bool) -> (Box<dyn UwfBackend>, BackendKind) {
    let _ = force_mock;
    (Box::new(MockUwfBackend::demo()), BackendKind::Mock)
}

/// 启动后端线程（actor）：独占 backend，顺序执行命令，`Quit` 退出。
pub fn spawn_backend_thread(
    backend: Box<dyn UwfBackend>,
    rx: Receiver<BackendCmd>,
    tx: Sender<BackendEvent>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut backend = backend;
        while let Ok(cmd) = rx.recv() {
            if matches!(cmd, BackendCmd::Quit) {
                break;
            }
            handle_cmd(&mut *backend, cmd, &tx);
        }
    })
}

fn handle_cmd(backend: &mut dyn UwfBackend, cmd: BackendCmd, tx: &Sender<BackendEvent>) {
    match cmd {
        BackendCmd::RefreshAll => refresh_all(backend, tx),
        BackendCmd::RefreshOverlayRuntime => match backend.get_overlay_runtime() {
            Ok(runtime) => send(tx, BackendEvent::OverlayRuntime(runtime)),
            Err(e) => send_err(tx, e),
        },
        BackendCmd::EnableFilter => {
            mutate(backend, tx, |b| b.enable_filter(), "UWF 将在下次系统重启后启用")
        }
        BackendCmd::DisableFilter => {
            mutate(backend, tx, |b| b.disable_filter(), "UWF 将在下次系统重启后禁用")
        }
        BackendCmd::Protect(drive) => {
            mutate(backend, tx, |b| b.protect_volume(&drive), format!("{drive} 保护成功（下次启动生效）"))
        }
        BackendCmd::Unprotect(drive) => {
            mutate(backend, tx, |b| b.unprotect_volume(&drive), format!("{drive} 已取消保护（下次启动生效）"))
        }
        BackendCmd::Quit => unreachable!("Quit 由线程循环拦截"),
    }
}

fn refresh_all(backend: &mut dyn UwfBackend, tx: &Sender<BackendEvent>) {
    match collect_snapshot(backend) {
        Ok(snapshot) => send(tx, BackendEvent::Snapshot(Box::new(snapshot))),
        Err(UwfError::NotInstalled) => send(tx, BackendEvent::NotInstalled),
        Err(e) => send_err(tx, e),
    }
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
    })
}

/// 变更类命令统一出口：失败回 `OpErr`；成功先全量刷新再回 `OpOk`——
/// 回包按序应用后日志末条保留动作文案（如"C: 保护成功"）而非刷新提示。
fn mutate(
    backend: &mut dyn UwfBackend,
    tx: &Sender<BackendEvent>,
    action: impl FnOnce(&mut dyn UwfBackend) -> Result<()>,
    ok_msg: impl Into<String>,
) {
    match action(backend) {
        Ok(()) => {
            refresh_all(backend, tx);
            send(tx, BackendEvent::OpOk(ok_msg.into()));
        }
        Err(e) => send_err(tx, e),
    }
}

fn send(tx: &Sender<BackendEvent>, evt: BackendEvent) {
    let _ = tx.send(evt);
}

fn send_err(tx: &Sender<BackendEvent>, e: UwfError) {
    send(tx, BackendEvent::OpErr(e.to_string()));
}
