//! 后端通信协议与 `UwfBackend` 抽象特征。

use std::path::PathBuf;

use crate::error::Result;
use crate::model::*;

use super::MockUwfBackend;
#[cfg(windows)]
use super::windows;

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
    /// 后台轮询目标
    fn get_overlay_runtime(&mut self) -> Result<OverlayRuntime>;
    /// R1：筛选器"当前已启用"时必须拒绝并返回 [`UwfError::FilterCurrentlyEnabled`]
    fn set_overlay_type(&mut self, overlay_type: OverlayType) -> Result<()>;
    /// R1：同上
    fn set_overlay_max_size(&mut self, size_mb: u32) -> Result<()>;
    fn set_warning_threshold(&mut self, size_mb: u32) -> Result<()>;
    fn set_critical_threshold(&mut self, size_mb: u32) -> Result<()>;

    // ── Volume ──
    /// R2：作用于"下次会话"实例
    fn protect_volume(&mut self, drive_letter: &str) -> Result<()>;
    fn unprotect_volume(&mut self, drive_letter: &str) -> Result<()>;
    /// R3：作用于"当前会话"实例
    fn commit_file(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()>;
    /// R3：两步语义——先真实删除文件（容忍 NotFound），再调 WMI 固化删除；
    /// 作用于"下次会话"实例
    fn commit_file_deletion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()>;

    // ── Exclusion ──
    /// 返回指定会话的文件排除项及读取期间产生的非致命警告。
    fn get_file_exclusions(
        &mut self,
        current_session: bool,
    ) -> Result<(Vec<FileExclusion>, Vec<String>)>;
    /// R4：增删作用于"下次会话"实例
    fn add_file_exclusion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()>;
    fn remove_file_exclusion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()>;
    fn get_registry_exclusions(&mut self, current_session: bool) -> Result<Vec<String>>;
    fn add_registry_exclusion(&mut self, key: &str) -> Result<()>;
    fn remove_registry_exclusion(&mut self, key: &str) -> Result<()>;
    /// R5：当前会话实例；key/value 由后端线程按最后一个 `\` 拆分后传入
    fn commit_registry(&mut self, key: &str, value_name: &str) -> Result<()>;

    /// 读取当前操作系统版本名称（如 `Windows 10 Enterprise LTSC 2021 16G`）。
    fn get_os_version(&mut self) -> Option<String> {
        None
    }

    /// 采集完整状态快照（Windows 后端合并同类 WMI 查询以减少往返）。
    fn collect_snapshot(&mut self) -> Result<FullSnapshot>;
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
    /// 当前 Windows 系统版本文本
    pub os_version: Option<String>,
    /// 部分卷读取失败等非致命警告
    pub warnings: Vec<String>,
}

/// 请求关联 id：仅用于变更请求，确保只有匹配的 `Finished` / `NotInstalled` 解除在途锁。
pub type RequestId = u64;

/// UI → 后端线程消息。
#[derive(Debug)]
pub enum BackendCommand {
    RefreshAll,
    /// 暂停/恢复后台 Overlay 用量轮询（例如窗口最小化时暂停）
    SetPollingPaused(bool),
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
    SetOverlayType(OverlayType),
    SetOverlayCache {
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
    /// 导出下次会话（重启后生效）排除列表
    ExportExclusions {
        path: PathBuf,
    },
    /// 撤销下次会话排除项修改，使其恢复到与当前生效会话一致
    RevertExclusionsToCurrent,
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
    /// 启动或快照采集到的系统版本信息（UWF 未安装时也可独立展示）
    OsVersion(String),
    /// `RefreshAll` 失败（与变更请求解耦，不会误解锁 in-flight mutation）
    RefreshFailed(String),
    /// 变更请求的最终结果（只由 [`BackendCommand::Mutate`] 产出）
    Finished { id: RequestId, outcome: Outcome },
    /// UWF 功能未安装（D6 引导页；`Some(id)` 表示某条变更触发，应同步解锁）
    NotInstalled(Option<RequestId>),
    /// 后端线程意外退出（panic 等非 Quit 终止）
    Disconnected,
}

/// 后端来源标识（用于区分真实重启退出与 Mock 重启模拟）
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
            Box::new(|| Box::new(windows::WindowsWmiBackend::new())),
            BackendKind::Windows,
        );
    }
    let _ = force_mock;
    (
        Box::new(|| Box::new(MockUwfBackend::demo())),
        BackendKind::Mock,
    )
}
