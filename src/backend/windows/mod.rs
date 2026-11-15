//! Windows 原生 WMI 后端（wmi crate，全 serde 化，无 unsafe）。
//!
//! COM 线程模型（PLAN D4）：`WMIConnection` 为 `!Send`，仅在后端线程内
//! 懒连接并独占使用。连接失败或核心查询为空映射为
//! [`UwfError::NotInstalled`]，经 `BackendEvent::NotInstalled` 驱动 D6 引导页。
//!
//! 业务规则落位（对照 PLAN §6.3 映射表）：
//! - R1：set_overlay_type/max_size 先查当前筛选器状态，启用则拒绝
//! - R2：protect/unprotect 作用于**下次会话**实例（若缺失则由当前会话自动注册）；
//!   读取时对同会话同盘符多实例按当前挂载卷消歧
//! - R3：commit_file → 当前会话实例；commit_file_deletion → 先删真实文件
//!   （容忍 NotFound）再调下次会话实例
//! - R4：排除项增删作用于下次会话实例（新增缺失时自动注册），查询支持双视图
//! - R5：注册表提交作用于当前会话实例（路径拆分在后端线程已完成）

mod classes;
mod elevate;

pub(crate) use elevate::is_elevated;

use std::collections::HashMap;

use serde::Serialize;
use serde::de::DeserializeOwned;
use wmi::WMIConnection;

use crate::error::{Result, UwfError};
use crate::model::*;
use classes::*;

use super::{
    RawVolumeInstance, ResolvedVolume, UwfBackend, find_session_volume, normalize_drive_letter,
    resolve_volumes, should_query_volume_exclusions,
};

/// UWF WMI 命名空间
const NAMESPACE: &str = "root\\standardcimv2\\embedded";

/// WQL 布尔字面量（显式大写，与原版 C# 插值结果一致）
fn session_kw(current_session: bool) -> &'static str {
    if current_session { "TRUE" } else { "FALSE" }
}

pub struct WindowsWmiBackend {
    con: Option<WMIConnection>,
    /// 非致命读取警告（如单卷 GetExclusions 失败），由 collect_snapshot / export_exclusions 取走
    warnings: Vec<String>,
    /// 缓存的操作系统版本文本（`None` 表示尚未查询）
    cached_os_version: Option<Option<String>>,
}

impl WindowsWmiBackend {
    /// 懒连接：首次调用时才初始化 COM 并连接命名空间（保证 COM 初始化
    /// 发生在后端线程内）
    pub fn new() -> Self {
        Self {
            con: None,
            warnings: Vec::new(),
            cached_os_version: None,
        }
    }

    fn con(&mut self) -> Result<&WMIConnection> {
        if self.con.is_none() {
            // 连接嵌入式命名空间失败（包括部分系统返回的 WBEM_E_NOT_FOUND）统一视为 UWF 未安装
            let conn = WMIConnection::with_namespace_path(NAMESPACE).map_err(|e| match &e {
                wmi::WMIError::HResultError { hres } if *hres == crate::error::WBEM_E_NOT_FOUND => {
                    UwfError::NotInstalled
                }
                _ => e.into(),
            })?;
            self.con = Some(conn);
        }
        Ok(self.con.as_ref().expect("connected above"))
    }

    fn query<T: DeserializeOwned>(&mut self) -> Result<Vec<T>> {
        match self.con()?.query() {
            Ok(rows) => Ok(rows),
            Err(e) => {
                self.con = None;
                Err(e.into())
            }
        }
    }

    fn raw_query<T: DeserializeOwned>(&mut self, wql: &str) -> Result<Vec<T>> {
        match self.con()?.raw_query(wql) {
            Ok(rows) => Ok(rows),
            Err(e) => {
                self.con = None;
                Err(e.into())
            }
        }
    }

    /// 调用实例方法（serde 化入参/出参）
    fn exec<Class, Out>(
        &mut self,
        object_path: &str,
        method: &'static str,
        in_params: impl Serialize,
    ) -> Result<Out>
    where
        Class: DeserializeOwned,
        Out: DeserializeOwned,
    {
        match self
            .con()?
            .exec_instance_method::<Class, Out>(object_path, method, in_params)
        {
            Ok(out) => Ok(out),
            Err(wmi::WMIError::HResultError { hres }) if hres == crate::error::WBEM_E_NOT_FOUND => {
                // 目标对象或叠加层记录不存在，并非 UWF 未安装，也无需重置健康连接
                Err(UwfError::WmiMethod {
                    method,
                    code: crate::error::WBEM_E_NOT_FOUND_CODE,
                })
            }
            Err(e) => {
                self.con = None;
                Err(e.into())
            }
        }
    }

    /// ReturnValue != 0 → UwfError::WmiMethod（uwfmgr 风格十六进制错误码）
    fn ensure_ok(method: &'static str, out: ReturnOnly) -> Result<()> {
        if out.ReturnValue == 0 {
            Ok(())
        } else {
            Err(UwfError::WmiMethod {
                method,
                code: out.ReturnValue,
            })
        }
    }

    // ── 实例获取 ──

    /// UWF_Filter 单例；查询失败或为空 → UWF 功能未安装
    fn filter_instance(&mut self) -> Result<UWF_Filter> {
        let rows: Vec<UWF_Filter> = self.query()?;
        rows.into_iter().next().ok_or(UwfError::NotInstalled)
    }

    fn overlay_instance(&mut self) -> Result<UWF_Overlay> {
        let rows: Vec<UWF_Overlay> = self.query()?;
        rows.into_iter().next().ok_or(UwfError::NotInstalled)
    }

    fn overlay_config_instance(&mut self, current_session: bool) -> Result<UWF_OverlayConfig> {
        let wql = format!(
            "SELECT * FROM UWF_OverlayConfig WHERE CurrentSession = {}",
            session_kw(current_session)
        );
        let rows: Vec<UWF_OverlayConfig> = self.raw_query(&wql)?;
        rows.into_iter().next().ok_or(UwfError::NotInstalled)
    }

    /// 单次全量读取 `UWF_Volume`，跳过无效盘符实例并消歧同会话同盘符冲突。
    fn read_volumes(&mut self) -> Result<Vec<ResolvedVolume>> {
        let rows: Vec<UWF_Volume> = self.raw_query("SELECT * FROM UWF_Volume")?;
        let raw_instances = rows
            .into_iter()
            .map(|v| RawVolumeInstance {
                path: v.path,
                current_session: v.CurrentSession,
                drive_letter: v.DriveLetter,
                volume_name: v.VolumeName,
                bind_by_drive_letter: v.BindByDriveLetter.unwrap_or(false),
                protected: v.Protected.unwrap_or(false),
            })
            .collect();
        let mut mounted_by_drive: Option<HashMap<String, String>> = None;
        resolve_volumes(raw_instances, |drive| {
            if mounted_by_drive.is_none() {
                mounted_by_drive = Some(Self::mounted_volumes_by_drive()?);
            }
            Ok(mounted_by_drive
                .as_ref()
                .and_then(|map| map.get(drive))
                .cloned())
        })
    }

    /// 仅在同会话出现重复盘符时查询 `ROOT\CIMV2` 中的 `Win32_Volume` 建立 `盘符 → DeviceID` 映射。
    fn mounted_volumes_by_drive() -> Result<HashMap<String, String>> {
        let cimv2 = WMIConnection::new()?;
        let rows: Vec<Win32_Volume> = cimv2.raw_query(
            "SELECT DeviceID, DriveLetter FROM Win32_Volume WHERE DriveLetter IS NOT NULL",
        )?;
        let mut map = HashMap::new();
        for row in rows {
            if let (Some(drive), Some(device_id)) = (
                row.DriveLetter.as_deref().and_then(normalize_drive_letter),
                row.DeviceID,
            ) {
                map.insert(drive, device_id);
            }
        }
        Ok(map)
    }

    /// 查询 `ROOT\CIMV2` 中的 `Win32_OperatingSystem` 以展示系统版本与 UWF 兼容性提示。
    fn query_os_version() -> Option<String> {
        let cimv2 = WMIConnection::new().ok()?;
        let rows: Vec<Win32_OperatingSystem> = cimv2
            .raw_query("SELECT Caption, BuildNumber, OSArchitecture FROM Win32_OperatingSystem")
            .ok()?;
        let os = rows.into_iter().next()?;
        let caption = os.Caption.as_deref().unwrap_or("").trim();
        if caption.is_empty() {
            return None;
        }
        Some(super::format_os_version(
            caption,
            os.BuildNumber.as_deref(),
            os.OSArchitecture.as_deref(),
        ))
    }

    /// 按会话过滤已消歧的卷实例。
    fn volumes_for(&mut self, current_session: bool) -> Result<Vec<ResolvedVolume>> {
        Ok(self
            .read_volumes()?
            .into_iter()
            .filter(|v| v.current_session == current_session)
            .collect())
    }

    /// 按盘符定位指定会话的卷实例路径。
    fn find_volume_path(&mut self, current_session: bool, drive_letter: &str) -> Result<String> {
        let volumes = self.read_volumes()?;
        find_session_volume(&volumes, current_session, drive_letter)
            .map(|v| v.path.clone())
            .ok_or_else(|| UwfError::VolumeNotFound(drive_letter.into()))
    }

    /// 定位下次会话卷实例路径；若仅当前会话存在该卷（未曾被 UWF 配置过），
    /// 则先向 WMI 写入 `CurrentSession = FALSE` 实例再回读真实 `__Path`。
    fn ensure_next_volume_path(&mut self, drive_letter: &str) -> Result<String> {
        let volumes = self.read_volumes()?;
        if let Some(next) = find_session_volume(&volumes, false, drive_letter) {
            return Ok(next.path.clone());
        }
        let current = find_session_volume(&volumes, true, drive_letter)
            .ok_or_else(|| UwfError::VolumeNotFound(drive_letter.into()))?;
        if current.volume_name.is_empty() {
            return Err(UwfError::VolumeNotFound(drive_letter.into()));
        }
        let drive = current.drive_letter.clone();
        let volume_name = current.volume_name.clone();

        self.put_next_volume(&drive, &volume_name)?;

        let refreshed = self.read_volumes()?;
        find_session_volume(&refreshed, false, &drive)
            .map(|v| v.path.clone())
            .ok_or_else(|| UwfError::VolumeNotFound(drive_letter.into()))
    }

    fn put_next_volume(&mut self, drive_letter: &str, volume_name: &str) -> Result<()> {
        const WBEM_E_ALREADY_EXISTS: i32 = 0x8004_1009_u32 as i32;
        let con = self.con()?;
        match Self::spawn_and_put_next_volume(con, drive_letter, volume_name) {
            Ok(()) => Ok(()),
            Err(wmi::WMIError::HResultError { hres }) if hres == WBEM_E_ALREADY_EXISTS => Ok(()),
            Err(e) => {
                self.con = None;
                Err(e.into())
            }
        }
    }

    fn spawn_and_put_next_volume(
        con: &WMIConnection,
        drive_letter: &str,
        volume_name: &str,
    ) -> wmi::WMIResult<()> {
        let class_obj = con.get_object("UWF_Volume")?;
        let inst = class_obj.spawn_instance()?;
        inst.put_property("CurrentSession", false)?;
        inst.put_property("DriveLetter", drive_letter)?;
        inst.put_property("VolumeName", volume_name)?;
        con.put_instance(&inst)
    }

    fn registry_filter_path(&mut self, current_session: bool) -> Result<String> {
        let wql = format!(
            "SELECT * FROM UWF_RegistryFilter WHERE CurrentSession = {}",
            session_kw(current_session)
        );
        let rows: Vec<UWF_RegistryFilter> = self.raw_query(&wql)?;
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
        // ReturnValue 校验：非零时不要假装重启已发出。
        // 注：Windows 真机上该调用是否能在重启前返回非零尚未验证（P0-5 定级说明）。
        let path = self.filter_instance()?.path;
        let out: ReturnOnly = self.exec::<UWF_Filter, _>(&path, "RestartSystem", ())?;
        Self::ensure_ok("RestartSystem", out)
    }

    fn shutdown_system(&mut self) -> Result<()> {
        let path = self.filter_instance()?.path;
        let out: ReturnOnly = self.exec::<UWF_Filter, _>(&path, "ShutdownSystem", ())?;
        Self::ensure_ok("ShutdownSystem", out)
    }

    // ── Overlay ──

    fn get_overlay_config(&mut self, current_session: bool) -> Result<OverlayConfig> {
        let c = self.overlay_config_instance(current_session)?;
        Ok(OverlayConfig {
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
            TypeParam {
                r#type: overlay_type.as_u32(),
            },
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
        let out: ReturnOnly =
            self.exec::<UWF_Overlay, _>(&path, "SetWarningThreshold", SizeParam { size: size_mb })?;
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
        // R2 / PLAN：单次全量消歧查询后按会话合并，next-only 卷与未注册的 current-only 卷均可见。
        let volumes = self.read_volumes()?;
        let current = volumes
            .iter()
            .filter(|v| v.current_session)
            .map(|v| (v.drive_letter.clone(), v.protected));
        let next = volumes
            .iter()
            .filter(|v| !v.current_session)
            .map(|v| (v.drive_letter.clone(), v.protected));
        Ok(super::merge_volume_infos(current, next))
    }

    fn protect_volume(&mut self, drive_letter: &str) -> Result<()> {
        // R2：作用于下次会话实例；未曾保护过的卷先按当前会话 GUID 自动创建下次会话实例
        let path = self.ensure_next_volume_path(drive_letter)?;
        let out: ReturnOnly = self.exec::<UWF_Volume, _>(&path, "Protect", ())?;
        Self::ensure_ok("Protect", out)
    }

    fn unprotect_volume(&mut self, drive_letter: &str) -> Result<()> {
        let path = self.ensure_next_volume_path(drive_letter)?;
        let out: ReturnOnly = self.exec::<UWF_Volume, _>(&path, "Unprotect", ())?;
        Self::ensure_ok("Unprotect", out)
    }

    fn commit_file(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()> {
        // R3 前置保护校验 + 获取当前会话实例路径
        let volumes = self.read_volumes()?;
        let current = find_session_volume(&volumes, true, drive_letter)
            .ok_or_else(|| UwfError::VolumeNotFound(drive_letter.into()))?;
        if !current.protected {
            return Err(UwfError::VolumeNotProtected(current.drive_letter.clone()));
        }
        let path = current.path.clone();
        let out: ReturnOnly = self.exec::<UWF_Volume, _>(
            &path,
            "CommitFile",
            FileNameParam {
                FileName: file_rel_path.to_string(),
            },
        )?;
        Self::ensure_ok("CommitFile", out)
    }

    fn commit_file_deletion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()> {
        // R3 第一步：必须在触碰物理文件前校验当前会话受保护状态与下次会话实例存在性
        let volumes = self.read_volumes()?;
        match find_session_volume(&volumes, true, drive_letter) {
            Some(current) if current.protected => {}
            Some(current) => {
                return Err(UwfError::VolumeNotProtected(current.drive_letter.clone()));
            }
            None if find_session_volume(&volumes, false, drive_letter).is_some() => {
                return Err(UwfError::VolumeNotProtected(drive_letter.into()));
            }
            None => return Err(UwfError::VolumeNotFound(drive_letter.into())),
        }
        let path = find_session_volume(&volumes, false, drive_letter)
            .map(|v| v.path.clone())
            .ok_or_else(|| UwfError::VolumeNotFound(drive_letter.into()))?;

        // R3 第二步：真实删除文件（写入 Overlay；已不存在则容忍）
        let abs_path = format!("{drive_letter}{file_rel_path}");
        match std::fs::remove_file(&abs_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }

        // R3 第三步：下次会话实例固化删除（重启后依然生效）。
        // 走到这里文件已经删除；失败必须明确告知“文件已删除、固化失败”。
        let out: ReturnOnly = self
            .exec::<UWF_Volume, _>(
                &path,
                "CommitFileDeletion",
                FileNameParam {
                    FileName: file_rel_path.to_string(),
                },
            )
            .map_err(|e| UwfError::CommitDeleteAfterFileRemoved {
                path: abs_path.clone(),
                message: e.to_string(),
            })?;
        Self::ensure_ok("CommitFileDeletion", out).map_err(|e| {
            UwfError::CommitDeleteAfterFileRemoved {
                path: abs_path,
                message: e.to_string(),
            }
        })
    }

    // ── Exclusion ──

    fn get_file_exclusions(&mut self, current_session: bool) -> Result<Vec<FileExclusion>> {
        let mut items = Vec::new();
        for volume in self.volumes_for(current_session)? {
            // 当前会话未受保护的卷未加载过滤驱动，调用 GetExclusions 会报 0x80041002
            if !should_query_volume_exclusions(current_session, volume.protected) {
                continue;
            }
            // P1-6：单卷失败继续读其他卷，但必须留下警告，不能静默显示为空。
            let out =
                match self.exec::<UWF_Volume, FileExclusionsOut>(&volume.path, "GetExclusions", ())
                {
                    Ok(out) if out.ReturnValue == 0 => out,
                    Ok(out) => {
                        self.warnings.push(format!(
                            "卷 {} 读取排除项失败: ReturnValue=0x{:08X}",
                            volume.drive_letter, out.ReturnValue
                        ));
                        continue;
                    }
                    Err(e) => {
                        self.warnings
                            .push(format!("卷 {} 读取排除项失败: {e}", volume.drive_letter));
                        continue;
                    }
                };
            for file in out.ExcludedFiles.unwrap_or_default() {
                items.push(FileExclusion::new(
                    volume.drive_letter.clone(),
                    file.FileName,
                ));
            }
        }
        items.sort();
        Ok(items)
    }

    fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    fn add_file_exclusion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()> {
        // R4：作用于下次会话实例；若卷尚未注册下次会话则先自动创建
        let path = self.ensure_next_volume_path(drive_letter)?;
        let out: ReturnOnly = self.exec::<UWF_Volume, _>(
            &path,
            "AddExclusion",
            FileNameParam {
                FileName: file_rel_path.to_string(),
            },
        )?;
        Self::ensure_ok("AddExclusion", out)
    }

    fn remove_file_exclusion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()> {
        let path = self.find_volume_path(false, drive_letter)?;
        let out: ReturnOnly = self.exec::<UWF_Volume, _>(
            &path,
            "RemoveExclusion",
            FileNameParam {
                FileName: file_rel_path.to_string(),
            },
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
        let mut keys: Vec<String> = out
            .ExcludedKeys
            .unwrap_or_default()
            .into_iter()
            .map(|k| k.RegistryKey)
            .collect();
        keys.sort();
        Ok(keys)
    }

    fn add_registry_exclusion(&mut self, key: &str) -> Result<()> {
        let path = self.registry_filter_path(false)?;
        let out: ReturnOnly = self.exec::<UWF_RegistryFilter, _>(
            &path,
            "AddExclusion",
            RegistryKeyParam {
                RegistryKey: key.to_string(),
            },
        )?;
        Self::ensure_ok("AddExclusion", out)
    }

    fn remove_registry_exclusion(&mut self, key: &str) -> Result<()> {
        let path = self.registry_filter_path(false)?;
        let out: ReturnOnly = self.exec::<UWF_RegistryFilter, _>(
            &path,
            "RemoveExclusion",
            RegistryKeyParam {
                RegistryKey: key.to_string(),
            },
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

    fn get_os_version(&mut self) -> Option<String> {
        if let Some(cached) = &self.cached_os_version {
            return cached.clone();
        }
        let queried = Self::query_os_version();
        self.cached_os_version = Some(queried.clone());
        queried
    }
}
