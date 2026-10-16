//! Windows 原生 WMI 后端（wmi crate，全 serde 化，无 unsafe）。
//!
//! COM 线程模型（PLAN D4）：`WMIConnection` 为 `!Send`，仅在后端线程内
//! 懒连接并独占使用。连接失败或核心查询为空映射为
//! [`UwfError::NotInstalled`]，经 `BackendEvent::NotInstalled` 驱动 D6 引导页。
//!
//! 业务规则落位（对照 PLAN §6.3 映射表）：
//! - R1：set_overlay_type/max_size 先查当前筛选器状态，启用则拒绝
//! - R2：protect/unprotect 作用于**下次会话**实例；list_volumes 双查询合并
//! - R3：commit_file → 当前会话实例；commit_file_deletion → 先删真实文件
//!   （容忍 NotFound）再调下次会话实例
//! - R4：排除项增删作用于下次会话实例，查询支持双视图
//! - R5：注册表提交作用于当前会话实例（路径拆分在后端线程已完成）

mod classes;

use serde::Serialize;
use serde::de::DeserializeOwned;
use wmi::WMIConnection;

use classes::*;
use crate::error::{Result, UwfError};
use crate::model::*;

use super::UwfBackend;

/// UWF WMI 命名空间
const NAMESPACE: &str = "root\\standardcimv2\\embedded";

/// WQL 布尔字面量（显式大写，与原版 C# 插值结果一致）
fn session_kw(current_session: bool) -> &'static str {
    if current_session { "TRUE" } else { "FALSE" }
}

pub struct WindowsWmiBackend {
    con: Option<WMIConnection>,
}

impl WindowsWmiBackend {
    /// 懒连接：首次调用时才初始化 COM 并连接命名空间（保证 COM 初始化
    /// 发生在后端线程内）
    pub fn new() -> Self {
        Self { con: None }
    }

    fn con(&mut self) -> Result<&WMIConnection> {
        if self.con.is_none() {
            // 命名空间不存在（UWF 功能未安装）时 ConnectServer 返回错误
            self.con = Some(
                WMIConnection::with_namespace_path(NAMESPACE)
                    .map_err(|_| UwfError::NotInstalled)?,
            );
        }
        Ok(self.con.as_ref().expect("connected above"))
    }

    /// 调用实例方法（serde 化入参/出参）
    fn exec<Class, Out>(
        &mut self,
        object_path: &str,
        method: &str,
        in_params: impl Serialize,
    ) -> Result<Out>
    where
        Class: DeserializeOwned,
        Out: DeserializeOwned,
    {
        Ok(self
            .con()?
            .exec_instance_method::<Class, Out>(object_path, method, in_params)?)
    }

    /// ReturnValue != 0 → UwfError::WmiMethod（uwfmgr 风格十六进制错误码）
    fn ensure_ok(method: &'static str, out: ReturnOnly) -> Result<()> {
        if out.ReturnValue == 0 {
            Ok(())
        } else {
            Err(UwfError::WmiMethod { method, code: out.ReturnValue })
        }
    }

    // ── 实例获取 ──

    /// UWF_Filter 单例；查询失败或为空 → UWF 功能未安装
    fn filter_instance(&mut self) -> Result<UWF_Filter> {
        let rows: Vec<UWF_Filter> = self
            .con()?
            .query()
            .map_err(|_| UwfError::NotInstalled)?;
        rows.into_iter().next().ok_or(UwfError::NotInstalled)
    }

    fn overlay_instance(&mut self) -> Result<UWF_Overlay> {
        let rows: Vec<UWF_Overlay> = self
            .con()?
            .query()
            .map_err(|_| UwfError::NotInstalled)?;
        rows.into_iter().next().ok_or(UwfError::NotInstalled)
    }

    fn overlay_config_instance(&mut self, current_session: bool) -> Result<UWF_OverlayConfig> {
        let wql = format!(
            "SELECT * FROM UWF_OverlayConfig WHERE CurrentSession = {}",
            session_kw(current_session)
        );
        let rows: Vec<UWF_OverlayConfig> = self.con()?.raw_query(&wql)?;
        rows.into_iter().next().ok_or(UwfError::NotInstalled)
    }

    /// 按会话查询卷实例（跳过空盘符实例，与原版一致）
    fn volumes_for(&mut self, current_session: bool) -> Result<Vec<UWF_Volume>> {
        let wql = format!(
            "SELECT * FROM UWF_Volume WHERE CurrentSession = {}",
            session_kw(current_session)
        );
        let rows: Vec<UWF_Volume> = self.con()?.raw_query(&wql)?;
        Ok(rows
            .into_iter()
            .filter(|v| !v.DriveLetter.is_empty())
            .collect())
    }

    /// 按盘符定位指定会话的卷实例路径
    fn find_volume_path(&mut self, current_session: bool, drive_letter: &str) -> Result<String> {
        self.volumes_for(current_session)?
            .into_iter()
            .find(|v| v.DriveLetter.eq_ignore_ascii_case(drive_letter))
            .map(|v| v.path)
            .ok_or_else(|| UwfError::VolumeNotFound(drive_letter.into()))
    }

    fn registry_filter_path(&mut self, current_session: bool) -> Result<String> {
        let wql = format!(
            "SELECT * FROM UWF_RegistryFilter WHERE CurrentSession = {}",
            session_kw(current_session)
        );
        let rows: Vec<UWF_RegistryFilter> = self.con()?.raw_query(&wql)?;
        rows.into_iter()
            .next()
            .map(|r| r.path)
            .ok_or(UwfError::NotInstalled)
    }

    fn filter_method_no_params(&mut self, method: &'static str) -> Result<()> {
        let path = self.filter_instance()?.path;
        let out: ReturnOnly = self.exec::<UWF_Filter, _>(&path, method, ())?;
        Self::ensure_ok(method, out)
    }

    /// R1：筛选器当前启用时 Overlay 类型/上限锁定
    fn ensure_overlay_editable(&mut self, what: &'static str) -> Result<()> {
        if self.filter_instance()?.CurrentEnabled {
            return Err(UwfError::FilterCurrentlyEnabled { what });
        }
        Ok(())
    }
}

impl UwfBackend for WindowsWmiBackend {
    // ── Filter ──

    fn get_filter_state(&mut self) -> Result<FilterState> {
        let f = self.filter_instance()?;
        Ok(FilterState {
            current_enabled: f.CurrentEnabled,
            next_enabled: f.NextEnabled,
        })
    }

    fn enable_filter(&mut self) -> Result<()> {
        self.filter_method_no_params("Enable")
    }

    fn disable_filter(&mut self) -> Result<()> {
        self.filter_method_no_params("Disable")
    }

    fn reset_settings(&mut self) -> Result<()> {
        self.filter_method_no_params("ResetSettings")
    }

    fn restart_system(&mut self) -> Result<()> {
        // 与原版一致：不校验 ReturnValue（调用发出后系统即进入重启流程）
        let path = self.filter_instance()?.path;
        let _: ReturnOnly = self.exec::<UWF_Filter, _>(&path, "RestartSystem", ())?;
        Ok(())
    }

    fn shutdown_system(&mut self) -> Result<()> {
        let path = self.filter_instance()?.path;
        let _: ReturnOnly = self.exec::<UWF_Filter, _>(&path, "ShutdownSystem", ())?;
        Ok(())
    }

    // ── Overlay ──

    fn get_overlay_config(&mut self, current_session: bool) -> Result<OverlayConfig> {
        let c = self.overlay_config_instance(current_session)?;
        Ok(OverlayConfig {
            current_session,
            overlay_type: OverlayType::from_u32(c.Type),
            max_size_mb: c.MaximumSize,
        })
    }

    fn get_overlay_runtime(&mut self) -> Result<OverlayRuntime> {
        let o = self.overlay_instance()?;
        Ok(OverlayRuntime {
            available_space_mb: o.AvailableSpace,
            consumption_mb: o.OverlayConsumption,
            warning_threshold_mb: o.WarningOverlayThreshold,
            critical_threshold_mb: o.CriticalOverlayThreshold,
        })
    }

    fn set_overlay_type(&mut self, overlay_type: OverlayType) -> Result<()> {
        self.ensure_overlay_editable("Overlay 类型")?;
        // 配置写入下次会话实例
        let path = self.overlay_config_instance(false)?.path;
        let out: ReturnOnly = self.exec::<UWF_OverlayConfig, _>(
            &path,
            "SetType",
            TypeParam { r#type: overlay_type.as_u32() },
        )?;
        Self::ensure_ok("SetType", out)
    }

    fn set_overlay_max_size(&mut self, size_mb: u32) -> Result<()> {
        self.ensure_overlay_editable("Overlay 最大容量")?;
        let path = self.overlay_config_instance(false)?.path;
        let out: ReturnOnly = self.exec::<UWF_OverlayConfig, _>(
            &path,
            "SetMaximumSize",
            SizeParam { size: size_mb },
        )?;
        Self::ensure_ok("SetMaximumSize", out)
    }

    fn set_warning_threshold(&mut self, size_mb: u32) -> Result<()> {
        let path = self.overlay_instance()?.path;
        let out: ReturnOnly = self.exec::<UWF_Overlay, _>(
            &path,
            "SetWarningThreshold",
            SizeParam { size: size_mb },
        )?;
        Self::ensure_ok("SetWarningThreshold", out)
    }

    fn set_critical_threshold(&mut self, size_mb: u32) -> Result<()> {
        let path = self.overlay_instance()?.path;
        let out: ReturnOnly = self.exec::<UWF_Overlay, _>(
            &path,
            "SetCriticalThreshold",
            SizeParam { size: size_mb },
        )?;
        Self::ensure_ok("SetCriticalThreshold", out)
    }

    // ── Volume ──

    fn list_volumes(&mut self) -> Result<Vec<VolumeInfo>> {
        let current = self.volumes_for(true)?;
        let next = self.volumes_for(false)?;
        // 与原版一致：以当前会话盘符集为准，下次查询缺失视为不保护
        let mut infos: Vec<VolumeInfo> = current
            .iter()
            .map(|c| {
                let next_protected = next
                    .iter()
                    .find(|n| n.DriveLetter.eq_ignore_ascii_case(&c.DriveLetter))
                    .and_then(|n| n.Protected)
                    .unwrap_or(false);
                VolumeInfo {
                    drive_letter: c.DriveLetter.clone(),
                    current_protected: c.Protected.unwrap_or(false),
                    next_protected,
                }
            })
            .collect();
        infos.sort_by(|a, b| a.drive_letter.cmp(&b.drive_letter));
        Ok(infos)
    }

    fn protect_volume(&mut self, drive_letter: &str) -> Result<()> {
        // R2：作用于下次会话实例（取代原版改 CurrentSession 属性再 Put 的 hack）
        let path = self.find_volume_path(false, drive_letter)?;
        let out: ReturnOnly = self.exec::<UWF_Volume, _>(&path, "Protect", ())?;
        Self::ensure_ok("Protect", out)
    }

    fn unprotect_volume(&mut self, drive_letter: &str) -> Result<()> {
        let path = self.find_volume_path(false, drive_letter)?;
        let out: ReturnOnly = self.exec::<UWF_Volume, _>(&path, "Unprotect", ())?;
        Self::ensure_ok("Unprotect", out)
    }

    fn commit_file(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()> {
        // R3：当前会话实例
        let path = self.find_volume_path(true, drive_letter)?;
        let out: ReturnOnly = self.exec::<UWF_Volume, _>(
            &path,
            "CommitFile",
            FileNameParam { FileName: file_rel_path.to_string() },
        )?;
        Self::ensure_ok("CommitFile", out)
    }

    fn commit_file_deletion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()> {
        // R3 第一步：真实删除文件（写入 Overlay；已不存在则容忍）
        let abs_path = format!("{drive_letter}{file_rel_path}");
        match std::fs::remove_file(&abs_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        // R3 第二步：下次会话实例固化删除（重启后依然生效）
        let path = self.find_volume_path(false, drive_letter)?;
        let out: ReturnOnly = self.exec::<UWF_Volume, _>(
            &path,
            "CommitFileDeletion",
            FileNameParam { FileName: file_rel_path.to_string() },
        )?;
        Self::ensure_ok("CommitFileDeletion", out)
    }

    // ── Exclusion ──

    fn get_file_exclusions(&mut self, current_session: bool) -> Result<Vec<FileExclusion>> {
        let mut items = Vec::new();
        for volume in self.volumes_for(current_session)? {
            // 与原版一致：单卷 GetExclusions 失败跳过该卷继续
            let Ok(out) = self
                .exec::<UWF_Volume, FileExclusionsOut>(&volume.path, "GetExclusions", ())
            else {
                continue;
            };
            if out.ReturnValue != 0 {
                continue;
            }
            for file in out.ExcludedFiles.unwrap_or_default() {
                items.push(FileExclusion::new(volume.DriveLetter.clone(), file.FileName));
            }
        }
        items.sort();
        Ok(items)
    }

    fn add_file_exclusion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()> {
        // R4：作用于下次会话实例
        let path = self.find_volume_path(false, drive_letter)?;
        let out: ReturnOnly = self.exec::<UWF_Volume, _>(
            &path,
            "AddExclusion",
            FileNameParam { FileName: file_rel_path.to_string() },
        )?;
        Self::ensure_ok("AddExclusion", out)
    }

    fn remove_file_exclusion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()> {
        let path = self.find_volume_path(false, drive_letter)?;
        let out: ReturnOnly = self.exec::<UWF_Volume, _>(
            &path,
            "RemoveExclusion",
            FileNameParam { FileName: file_rel_path.to_string() },
        )?;
        Self::ensure_ok("RemoveExclusion", out)
    }

    fn get_registry_exclusions(&mut self, current_session: bool) -> Result<Vec<String>> {
        let path = self.registry_filter_path(current_session)?;
        let out: RegistryExclusionsOut =
            self.exec::<UWF_RegistryFilter, _>(&path, "GetExclusions", ())?;
        if out.ReturnValue != 0 {
            return Err(UwfError::WmiMethod {
                method: "GetExclusions",
                code: out.ReturnValue,
            });
        }
        Ok(out
            .ExcludedKeys
            .unwrap_or_default()
            .into_iter()
            .map(|k| k.RegistryKey)
            .collect())
    }

    fn add_registry_exclusion(&mut self, key: &str) -> Result<()> {
        let path = self.registry_filter_path(false)?;
        let out: ReturnOnly = self.exec::<UWF_RegistryFilter, _>(
            &path,
            "AddExclusion",
            RegistryKeyParam { RegistryKey: key.to_string() },
        )?;
        Self::ensure_ok("AddExclusion", out)
    }

    fn remove_registry_exclusion(&mut self, key: &str) -> Result<()> {
        let path = self.registry_filter_path(false)?;
        let out: ReturnOnly = self.exec::<UWF_RegistryFilter, _>(
            &path,
            "RemoveExclusion",
            RegistryKeyParam { RegistryKey: key.to_string() },
        )?;
        Self::ensure_ok("RemoveExclusion", out)
    }

    fn commit_registry(&mut self, key: &str, value_name: &str) -> Result<()> {
        // R5：当前会话实例
        let path = self.registry_filter_path(true)?;
        let out: ReturnOnly = self.exec::<UWF_RegistryFilter, _>(
            &path,
            "CommitRegistry",
            CommitRegistryParam {
                RegistryKey: key.to_string(),
                ValueName: value_name.to_string(),
            },
        )?;
        Self::ensure_ok("CommitRegistry", out)
    }
}
