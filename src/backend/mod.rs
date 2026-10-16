//! 后端抽象层：UI 与底层 UWF 实现的隔离边界。
//!
//! 线程协议（PLAN v2 §6.5）：UI 线程发送 [`BackendCmd`]，后端线程独占
//! `dyn UwfBackend` 执行后经 [`BackendEvent`] 回包。变更类命令成功后
//! 自动跟随一次全量刷新（先刷新后 OpOk，日志末条保留动作文案）。

mod mock;
#[cfg(windows)]
mod windows;

pub use mock::MockUwfBackend;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::thread::{self, JoinHandle};

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

/// UI → 后端线程命令（PLAN §6.5 完整协议）
#[derive(Debug)]
pub enum BackendCmd {
    RefreshAll,
    RefreshOverlayRuntime,
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
    ImportExclusions { path: PathBuf },
    /// 导出 UI 当前视图（current_session 由 UI 传入）
    ExportExclusions { path: PathBuf, current_session: bool },
    RestartSystem,
    ShutdownSystem,
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

/// 启动后端线程（actor）：工厂在本线程内构造 backend（WMI 连接随之在
/// 本线程建立），顺序执行命令，`Quit` 退出。
pub fn spawn_backend_thread(
    factory: BackendFactory,
    rx: Receiver<BackendCmd>,
    tx: Sender<BackendEvent>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut backend = factory();
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
        BackendCmd::ResetSettings => {
            mutate(backend, tx, |b| b.reset_settings(), "UWF 配置将在下次重启后重置为默认值")
        }
        BackendCmd::SetOverlayConfig { overlay_type, max_mb, warning_mb, critical_mb } => {
            mutate_with(backend, tx, |b| {
                // R1 双保险：当前启用时仅应用阈值（UI 侧已锁定其余两项）
                let locked = b.get_filter_state()?.current_enabled;
                if !locked {
                    b.set_overlay_type(overlay_type)?;
                    b.set_overlay_max_size(max_mb)?;
                }
                b.set_warning_threshold(warning_mb)?;
                b.set_critical_threshold(critical_mb)?;
                Ok(if locked {
                    "Overlay 阈值已保存（类型/上限已锁定）".to_string()
                } else {
                    "Overlay 配置已保存（下次启动生效）".to_string()
                })
            })
        }
        BackendCmd::Protect(drive) => {
            mutate(backend, tx, |b| b.protect_volume(&drive), format!("{drive} 保护成功（下次启动生效）"))
        }
        BackendCmd::Unprotect(drive) => {
            mutate(backend, tx, |b| b.unprotect_volume(&drive), format!("{drive} 已取消保护（下次启动生效）"))
        }
        BackendCmd::AddFileExclusion(path) => {
            mutate_with(backend, tx, |b| {
                let (drive, rel) = split_absolute_path(&path)?;
                b.add_file_exclusion(&drive, &rel)?;
                Ok(format!("已添加文件排除: {path}"))
            })
        }
        BackendCmd::RemoveFileExclusion(path) => {
            mutate_with(backend, tx, |b| {
                let (drive, rel) = split_absolute_path(&path)?;
                b.remove_file_exclusion(&drive, &rel)?;
                Ok(format!("已删除文件排除: {path}"))
            })
        }
        BackendCmd::AddRegistryExclusion(key) => {
            mutate(backend, tx, |b| b.add_registry_exclusion(&key), format!("已添加注册表排除: {key}"))
        }
        BackendCmd::RemoveRegistryExclusion(key) => {
            mutate(backend, tx, |b| b.remove_registry_exclusion(&key), format!("已删除注册表排除: {key}"))
        }
        BackendCmd::CommitFile(path) => {
            mutate_with(backend, tx, |b| {
                let (drive, rel) = split_absolute_path(&path)?;
                b.commit_file(&drive, &rel)?;
                Ok(format!("{path} 提交成功"))
            })
        }
        BackendCmd::CommitFileDeletion(path) => {
            mutate_with(backend, tx, |b| {
                let (drive, rel) = split_absolute_path(&path)?;
                b.commit_file_deletion(&drive, &rel)?;
                Ok(format!("{path} 删除提交成功"))
            })
        }
        BackendCmd::CommitRegistry(full_path) => {
            mutate_with(backend, tx, |b| {
                let (key, value) = split_registry_path(&full_path)?;
                b.commit_registry(&key, &value)?;
                Ok(format!("注册表 {full_path} 提交成功"))
            })
        }
        BackendCmd::ImportExclusions { path } => import_exclusions(backend, tx, &path),
        BackendCmd::ExportExclusions { path, current_session } => {
            export_exclusions(backend, tx, &path, current_session)
        }
        BackendCmd::RestartSystem => {
            mutate(backend, tx, |b| b.restart_system(), "系统即将重启")
        }
        BackendCmd::ShutdownSystem => {
            mutate(backend, tx, |b| b.shutdown_system(), "系统即将关机")
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
/// 回包按序应用后日志末条保留动作文案（如 "C: 保护成功"）而非刷新提示。
fn mutate(
    backend: &mut dyn UwfBackend,
    tx: &Sender<BackendEvent>,
    action: impl FnOnce(&mut dyn UwfBackend) -> Result<()>,
    ok_msg: impl Into<String>,
) {
    mutate_with(backend, tx, |b| {
        action(b)?;
        Ok(ok_msg.into())
    });
}

/// 同 [`mutate`]，但成功文案由 action 返回（适用于需先解析/校验的命令）。
fn mutate_with(
    backend: &mut dyn UwfBackend,
    tx: &Sender<BackendEvent>,
    action: impl FnOnce(&mut dyn UwfBackend) -> Result<String>,
) {
    match action(backend) {
        Ok(msg) => {
            refresh_all(backend, tx);
            send(tx, BackendEvent::OpOk(msg));
        }
        Err(e) => send_err(tx, e),
    }
}

/// 导入排除列表（原版 button_import_list_Click 语义）：
/// 逐行解析，`HK` 前缀 → 注册表排除，其余 → 文件排除；统计成败。
fn import_exclusions(backend: &mut dyn UwfBackend, tx: &Sender<BackendEvent>, path: &PathBuf) {
    let result = (|| -> Result<(usize, usize)> {
        let content = std::fs::read_to_string(path)?;
        let mut ok = 0;
        let mut failed = 0;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let r = if line.starts_with("HK") {
                backend.add_registry_exclusion(line)
            } else {
                split_absolute_path(line)
                    .and_then(|(drive, rel)| backend.add_file_exclusion(&drive, &rel))
            };
            match r {
                Ok(()) => ok += 1,
                Err(_) => failed += 1,
            }
        }
        Ok((ok, failed))
    })();
    match result {
        Ok((ok, failed)) => {
            let msg = if failed == 0 {
                format!("导入完成：成功 {ok} 项")
            } else {
                format!("导入完成：成功 {ok} 项，失败 {failed} 项")
            };
            refresh_all(backend, tx);
            send(tx, BackendEvent::OpOk(msg));
        }
        Err(e) => send_err(tx, e),
    }
}

/// 导出排除列表（原版 button_save_exclusionlist_Click 语义）：
/// 文件排除（绝对路径形式）在前，注册表排除在后，每行一条。
fn export_exclusions(
    backend: &mut dyn UwfBackend,
    tx: &Sender<BackendEvent>,
    path: &PathBuf,
    current_session: bool,
) {
    let result = (|| -> Result<usize> {
        let files = backend.get_file_exclusions(current_session)?;
        let registry = backend.get_registry_exclusions(current_session)?;
        let mut lines: Vec<String> = files.iter().map(|e| e.display()).collect();
        lines.extend(registry);
        std::fs::write(path, lines.join("\n") + "\n")?;
        Ok(lines.len())
    })();
    match result {
        Ok(count) => send(tx, BackendEvent::OpOk(format!("已导出 {count} 项至 {}", path.display()))),
        Err(e) => send_err(tx, e),
    }
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
        let rel = &path[2..];
        let rel = if rel.is_empty() {
            "\\".to_string()
        } else if rel.starts_with('\\') {
            rel.to_string()
        } else {
            format!("\\{rel}")
        };
        return Ok((drive, rel));
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
        _ => Err(UwfError::InvalidPath(format!("注册表路径须包含键与值名称: {path}"))),
    }
}

fn send(tx: &Sender<BackendEvent>, evt: BackendEvent) {
    let _ = tx.send(evt);
}

fn send_err(tx: &Sender<BackendEvent>, e: UwfError) {
    send(tx, BackendEvent::OpErr(e.to_string()));
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
        // 缺前导反斜杠时补齐
        let (_, rel) = split_absolute_path("C:foo").unwrap();
        assert_eq!(rel, "\\foo");
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
        assert_eq!((key.as_str(), value.as_str()), ("HKLM\\SOFTWARE\\X", "MyValue"));
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
}
