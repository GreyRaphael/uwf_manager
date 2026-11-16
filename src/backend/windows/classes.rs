//! WMI `root\standardcimv2\embedded` 各类的 serde 映射。
//!
//! 结构体名必须即 WMI 类名（wmi crate 以类型名定位类定义/生成查询），
//! 字段名与 WMI 属性名、方法参数名完全一致。

use std::fmt;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};

// ── 查询行类型（含 __Path，供实例方法调用）──

#[derive(Deserialize)]
#[allow(non_snake_case, non_camel_case_types)]
pub struct UWF_Filter {
    #[serde(rename = "__Path")]
    pub path: String,
    pub CurrentEnabled: bool,
    pub NextEnabled: bool,
}

#[derive(Deserialize)]
#[allow(non_snake_case, non_camel_case_types)]
pub struct UWF_Overlay {
    #[serde(rename = "__Path")]
    pub path: String,
    pub OverlayConsumption: u32,
    pub WarningOverlayThreshold: u32,
    pub CriticalOverlayThreshold: u32,
}

#[derive(Deserialize)]
#[allow(non_snake_case, non_camel_case_types)]
pub struct UWF_OverlayConfig {
    #[serde(rename = "__Path")]
    pub path: String,
    pub CurrentSession: bool,
    pub Type: u32,
    pub MaximumSize: u32,
}

#[derive(Deserialize)]
#[allow(non_snake_case, non_camel_case_types)]
pub struct UWF_Volume {
    #[serde(rename = "__Path")]
    pub path: String,
    pub CurrentSession: bool,
    /// 无盘符分区（如恢复/EFI 分区）可能为 NULL 或空串
    pub DriveLetter: Option<String>,
    /// 创建下次会话实例必需的卷 GUID 名（如 `Volume{...}`）
    pub VolumeName: Option<String>,
    pub BindByDriveLetter: Option<bool>,
    /// 与原版一致：属性可能为 null，缺省视为不保护
    pub Protected: Option<bool>,
}

/// `ROOT\CIMV2` 挂载卷信息，仅在同盘符多条 `UWF_Volume` 记录冲突时按需查询。
#[derive(Deserialize)]
#[allow(non_snake_case, non_camel_case_types)]
pub struct Win32_Volume {
    pub DeviceID: Option<String>,
    pub DriveLetter: Option<String>,
}

/// 将 WMI 的 `CIM_UINT64`（可能以整数或十进制字符串返回）及空值安全解析为 `Option<u64>`。
fn deserialize_opt_u64<'de, D>(deserializer: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    struct OptU64Visitor;

    impl<'de> Visitor<'de> for OptU64Visitor {
        type Value = Option<u64>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an optional u64 integer or numeric string")
        }

        fn visit_none<E>(self) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_some<D2>(self, deserializer: D2) -> std::result::Result<Self::Value, D2::Error>
        where
            D2: Deserializer<'de>,
        {
            deserializer.deserialize_any(Self)
        }

        fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value))
        }

        fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(u64::try_from(value).ok())
        }

        fn visit_u128<E>(self, value: u128) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(u64::try_from(value).ok())
        }

        fn visit_i128<E>(self, value: i128) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(u64::try_from(value).ok())
        }

        fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(trimmed.parse::<u64>().ok())
            }
        }
    }

    deserializer.deserialize_any(OptU64Visitor)
}

/// `ROOT\CIMV2` 操作系统版本与内存信息（用于左下角系统环境面板）。
#[derive(Deserialize)]
#[allow(non_snake_case, non_camel_case_types)]
pub struct Win32_OperatingSystem {
    pub Caption: Option<String>,
    pub BuildNumber: Option<String>,
    #[serde(default, deserialize_with = "deserialize_opt_u64")]
    pub TotalVisibleMemorySize: Option<u64>,
}

/// `ROOT\CIMV2` 物理内存条容量（优先累加物理内存条字节数以对齐系统标称内存如 `16G`）。
#[derive(Deserialize)]
#[allow(non_snake_case, non_camel_case_types)]
pub struct Win32_PhysicalMemory {
    #[serde(default, deserialize_with = "deserialize_opt_u64")]
    pub Capacity: Option<u64>,
}

/// `ROOT\CIMV2` 计算机系统总物理内存（`Win32_PhysicalMemory` 不可用时的回退来源）。
#[derive(Deserialize)]
#[allow(non_snake_case, non_camel_case_types)]
pub struct Win32_ComputerSystem {
    #[serde(default, deserialize_with = "deserialize_opt_u64")]
    pub TotalPhysicalMemory: Option<u64>,
}

#[derive(Deserialize)]
#[allow(non_snake_case, non_camel_case_types)]
pub struct UWF_RegistryFilter {
    #[serde(rename = "__Path")]
    pub path: String,
    pub CurrentSession: bool,
}

// ── 方法出参 ──

/// 仅含 ReturnValue 的通用出参
#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct ReturnOnly {
    pub ReturnValue: u32,
}

/// UWF_Volume.GetExclusions 出参（嵌入对象数组，真机首验点）
#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct FileExclusionsOut {
    pub ReturnValue: u32,
    pub ExcludedFiles: Option<Vec<ExcludedFile>>,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct ExcludedFile {
    pub FileName: String,
}

/// UWF_RegistryFilter.GetExclusions 出参
#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct RegistryExclusionsOut {
    pub ReturnValue: u32,
    pub ExcludedKeys: Option<Vec<ExcludedKey>>,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct ExcludedKey {
    pub RegistryKey: String,
}

// ── 方法入参（字段名即 WMI 参数名）──

#[derive(Serialize)]
#[allow(non_snake_case)]
pub struct SizeParam {
    pub size: u32,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
pub struct TypeParam {
    pub r#type: u32,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
pub struct FileNameParam {
    pub FileName: String,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
pub struct RegistryKeyParam {
    pub RegistryKey: String,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
pub struct CommitRegistryParam {
    pub RegistryKey: String,
    pub ValueName: String,
}
