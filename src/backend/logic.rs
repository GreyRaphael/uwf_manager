//! 卷消歧、版本支持度、路径拆分与排除项回退计划等纯逻辑。

use crate::error::{Result, UwfError};
use crate::model::*;

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

/// 原始 `UWF_Volume` WMI 行（Windows 查询后直接解码到此结构供纯逻辑消歧）。
#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawVolumeInstance {
    pub path: String,
    pub current_session: bool,
    pub drive_letter: Option<String>,
    pub volume_name: Option<String>,
    pub bind_by_drive_letter: bool,
    pub protected: bool,
}

/// 经筛选与盘符消歧后的有效 `UWF_Volume` 实例。
#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedVolume {
    pub path: String,
    pub current_session: bool,
    pub drive_letter: String,
    pub volume_name: String,
    pub bind_by_drive_letter: bool,
    pub protected: bool,
}

/// 将 `Win32_Volume.DeviceID`（如 `\\?\Volume{GUID}\`）归一为 `Volume{GUID}`，
/// 以匹配 `UWF_Volume.VolumeName`。
#[cfg(any(windows, test))]
pub(crate) fn extract_volume_guid_name(device_id: &str) -> Option<String> {
    let trimmed = device_id.trim();
    let without_prefix = trimmed.strip_prefix(r"\\?\").unwrap_or(trimmed);
    let guid = without_prefix.trim_end_matches('\\');
    (!guid.is_empty()).then(|| guid.to_string())
}

/// 将盘符文本（如 `"d:"`、`"D:\\"`）归一为大写 `"X:"`；空串或无效值返回 `None`。
#[cfg(any(windows, test))]
pub(crate) fn normalize_drive_letter(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches(['\\', '/']);
    let bytes = trimmed.as_bytes();
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        Some(trimmed.to_ascii_uppercase())
    } else {
        None
    }
}

/// 判断是否应对该卷实例调用 `UWF_Volume.GetExclusions`。
///
/// 当前会话中未受保护的卷未挂载 UWF 过滤驱动，直接调用 `GetExclusions` 会返回
/// `WBEM_E_NOT_FOUND (0x80041002)`；而下次会话即使尚未启用保护也可能已预配置排除项，
/// 因此仅跳过 `current_session && !protected`。
#[cfg(any(windows, test))]
pub(crate) fn should_query_volume_exclusions(current_session: bool, protected: bool) -> bool {
    !current_session || protected
}

/// 在已消歧的卷集合中按 `(current_session, drive_letter)` 查找实例。
#[cfg(any(windows, test))]
pub(crate) fn find_session_volume<'a>(
    volumes: &'a [ResolvedVolume],
    current_session: bool,
    drive_letter: &str,
) -> Option<&'a ResolvedVolume> {
    let target = normalize_drive_letter(drive_letter)?;
    volumes
        .iter()
        .find(|v| v.current_session == current_session && v.drive_letter == target)
}

/// 过滤无效盘符行，并在同一会话同一盘符存在多条 GUID 实例时按当前挂载卷消歧。
///
/// 仅当某 `(current_session, drive_letter)` 桶出现 >= 2 条实例时才按需调用
/// `mounted_guid(drive_letter)`：
/// - `CurrentSession = TRUE`：仅保留与当前挂载 GUID 一致的实例（剔除陈旧拔插盘符记录）；
/// - `CurrentSession = FALSE`：保留 `BindByDriveLetter = true` 或匹配当前挂载 GUID 的实例；
/// - 同一桶内若仍有 >= 2 条适用实例（或完全相同的 `(session, drive, guid)` 重复）返回 [`UwfError::VolumeAmbiguous`]。
#[cfg(any(windows, test))]
pub(crate) fn resolve_volumes<F>(
    raw_rows: Vec<RawVolumeInstance>,
    mut mounted_guid: F,
) -> Result<Vec<ResolvedVolume>>
where
    F: FnMut(&str) -> Result<Option<String>>,
{
    let mut volumes = Vec::with_capacity(raw_rows.len());
    let mut seen_identities = std::collections::BTreeSet::new();
    let mut buckets: std::collections::BTreeMap<(bool, String), Vec<usize>> =
        std::collections::BTreeMap::new();

    for raw in raw_rows {
        let Some(drive_letter) = raw.drive_letter.as_deref().and_then(normalize_drive_letter)
        else {
            continue;
        };
        let volume_name = raw
            .volume_name
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if !volume_name.is_empty()
            && !seen_identities.insert((
                raw.current_session,
                drive_letter.clone(),
                volume_name.to_ascii_uppercase(),
            ))
        {
            return Err(UwfError::VolumeAmbiguous(drive_letter));
        }
        let idx = volumes.len();
        buckets
            .entry((raw.current_session, drive_letter.clone()))
            .or_default()
            .push(idx);
        volumes.push(ResolvedVolume {
            path: raw.path,
            current_session: raw.current_session,
            drive_letter,
            volume_name,
            bind_by_drive_letter: raw.bind_by_drive_letter,
            protected: raw.protected,
        });
    }

    let mut selected = vec![true; volumes.len()];
    let mut mount_cache: std::collections::BTreeMap<String, Option<String>> =
        std::collections::BTreeMap::new();

    for ((current_session, drive_letter), indices) in buckets {
        if indices.len() <= 1 {
            continue;
        }
        let mounted = match mount_cache.get(&drive_letter) {
            Some(cached) => cached.clone(),
            None => {
                let resolved = mounted_guid(&drive_letter)?
                    .as_deref()
                    .and_then(extract_volume_guid_name);
                mount_cache.insert(drive_letter.clone(), resolved.clone());
                resolved
            }
        };
        let mut matched = 0usize;
        for idx in indices {
            let row = &volumes[idx];
            let matches_mount = mounted.as_deref().is_some_and(|guid| {
                !row.volume_name.is_empty() && guid.eq_ignore_ascii_case(&row.volume_name)
            });
            let applies = (!current_session && row.bind_by_drive_letter) || matches_mount;
            selected[idx] = applies;
            if applies {
                matched += 1;
                if matched > 1 {
                    return Err(UwfError::VolumeAmbiguous(drive_letter));
                }
            }
        }
    }

    Ok(volumes
        .into_iter()
        .zip(selected)
        .filter_map(|(v, keep)| keep.then_some(v))
        .collect())
}

/// 校验当前会话全局筛选器是否已启用，方可执行 Overlay 提交或固化操作。
pub fn validate_filter_commit_ready(filter_enabled: bool, what: &'static str) -> Result<()> {
    if filter_enabled {
        Ok(())
    } else {
        Err(UwfError::FilterNotCurrentlyEnabled { what })
    }
}

/// 校验目标盘符是否已在当前会话受 UWF 保护，防止对未保护分区误执行文件提交或物理删除。
pub fn validate_volume_commit_ready(
    volumes: &[VolumeInfo],
    filter_enabled: bool,
    drive_letter: &str,
    what: &'static str,
) -> Result<()> {
    validate_filter_commit_ready(filter_enabled, what)?;
    match volumes
        .iter()
        .find(|v| v.drive_letter.eq_ignore_ascii_case(drive_letter))
    {
        None => Err(UwfError::VolumeNotFound(drive_letter.to_string())),
        Some(v) if !v.current_protected => {
            Err(UwfError::VolumeNotProtected(v.drive_letter.clone()))
        }
        Some(_) => Ok(()),
    }
}

/// Windows 版本对 UWF 的支持级别（仅基于版本名称作友好提示，不阻止用户操作）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditionSupport {
    /// 企业版 / 教育版 / IoT Enterprise
    Supported,
    /// 非企业版/教育版（如家庭版 / 专业版等）
    Unsupported,
    /// 未检测到或无法识别
    Unknown,
}

/// 将物理内存字节数格式化为紧凑的容量后缀（如 `16G`）。
#[cfg_attr(not(windows), allow(dead_code))]
pub fn format_memory_gib(bytes: u64) -> Option<String> {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;
    if bytes == 0 {
        return None;
    }
    if bytes >= GIB / 2 {
        let gib = bytes.saturating_add(GIB / 2) / GIB;
        Some(format!("{gib}G"))
    } else {
        let mib = (bytes.saturating_add(MIB / 2) / MIB).max(1);
        Some(format!("{mib}M"))
    }
}

/// 规范化系统版本与物理内存文本，例如 `Windows 10 Enterprise LTSC 2021 16G`。
#[cfg_attr(not(windows), allow(dead_code))]
pub fn format_os_version(caption: &str, build: Option<&str>, memory_bytes: Option<u64>) -> String {
    let mut base = caption
        .trim()
        .strip_prefix("Microsoft ")
        .unwrap_or(caption.trim())
        .to_string();
    let build = build.map(str::trim).filter(|s| !s.is_empty());

    // 注册表 ProductName 在 Windows 11 上仍常保留 "Windows 10 ..."，结合 Build 号（>= 22000）修正显示
    if build
        .and_then(|b| b.parse::<u32>().ok())
        .is_some_and(|b| b >= 22000)
        && let Some(rest) = base.strip_prefix("Windows 10")
    {
        base = format!("Windows 11{rest}");
    }

    // Windows LTSC 的 Caption 常省略年份，结合 Build 号补齐以便一目了然辨识
    if base.contains("LTSC") {
        let ltsc_year = match build {
            Some("14393") if !base.contains("2016") => Some("2016"),
            Some("17763") if !base.contains("2019") => Some("2019"),
            Some("19044") if !base.contains("2021") => Some("2021"),
            Some("26100") if !base.contains("2024") => Some("2024"),
            _ => None,
        };
        if let Some(year) = ltsc_year {
            base.push(' ');
            base.push_str(year);
        }
    }

    if let Some(mem) = memory_bytes.and_then(format_memory_gib) {
        if base.is_empty() {
            mem
        } else {
            format!("{base} {mem}")
        }
    } else {
        base
    }
}

/// 检查系统版本文本是否为官方支持 UWF 的 Windows 企业版 / 教育版。
pub fn check_windows_edition_support(os_version: &str) -> EditionSupport {
    let trimmed = os_version.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.is_empty() || lower.contains("未知") {
        return EditionSupport::Unknown;
    }
    if !lower.contains("server")
        && (lower.contains("enterprise")
            || trimmed.contains("企业版")
            || lower.contains("education")
            || trimmed.contains("教育版"))
    {
        EditionSupport::Supported
    } else {
        EditionSupport::Unsupported
    }
}

/// 将“重启后生效”排除项恢复为与“当前生效”一致的差异计划。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExclusionRevertPlan {
    pub files_to_remove: Vec<FileExclusion>,
    pub files_to_add: Vec<FileExclusion>,
    pub regs_to_remove: Vec<String>,
    pub regs_to_add: Vec<String>,
}

impl ExclusionRevertPlan {
    pub fn remove_count(&self) -> usize {
        self.files_to_remove.len() + self.regs_to_remove.len()
    }

    pub fn add_count(&self) -> usize {
        self.files_to_add.len() + self.regs_to_add.len()
    }

    pub fn is_empty(&self) -> bool {
        self.remove_count() == 0 && self.add_count() == 0
    }
}

/// 计算把 `next` 对齐回 `current` 需要移除与补回的排除项列表（忽略大小写去重比较）。
pub fn plan_exclusion_revert(
    files_current: &[FileExclusion],
    files_next: &[FileExclusion],
    regs_current: &[String],
    regs_next: &[String],
) -> ExclusionRevertPlan {
    let files_to_remove = files_next
        .iter()
        .filter(|next| {
            !files_current
                .iter()
                .any(|cur| cur.matches_ignore_case(next))
        })
        .cloned()
        .collect();
    let files_to_add = files_current
        .iter()
        .filter(|cur| !files_next.iter().any(|next| next.matches_ignore_case(cur)))
        .cloned()
        .collect();
    let regs_to_remove = regs_next
        .iter()
        .filter(|next| {
            !regs_current
                .iter()
                .any(|cur| cur.eq_ignore_ascii_case(next))
        })
        .cloned()
        .collect();
    let regs_to_add = regs_current
        .iter()
        .filter(|cur| !regs_next.iter().any(|next| next.eq_ignore_ascii_case(cur)))
        .cloned()
        .collect();
    ExclusionRevertPlan {
        files_to_remove,
        files_to_add,
        regs_to_remove,
        regs_to_add,
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
