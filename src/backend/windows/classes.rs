//! WMI `root\standardcimv2\embedded` 各类的 serde 映射。
//!
//! 结构体名必须即 WMI 类名（wmi crate 以类型名定位类定义/生成查询），
//! 字段名与 WMI 属性名、方法参数名完全一致。

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
    pub AvailableSpace: u32,
    pub OverlayConsumption: u32,
    pub WarningOverlayThreshold: u32,
    pub CriticalOverlayThreshold: u32,
}

#[derive(Deserialize)]
#[allow(non_snake_case, non_camel_case_types)]
pub struct UWF_OverlayConfig {
    #[serde(rename = "__Path")]
    pub path: String,
    #[allow(dead_code)]
    pub CurrentSession: bool,
    pub Type: u32,
    pub MaximumSize: u32,
}

#[derive(Deserialize)]
#[allow(non_snake_case, non_camel_case_types)]
pub struct UWF_Volume {
    #[serde(rename = "__Path")]
    pub path: String,
    #[allow(dead_code)]
    pub CurrentSession: bool,
    pub DriveLetter: String,
    /// 与原版一致：属性可能为 null，缺省视为不保护
    pub Protected: Option<bool>,
}

#[derive(Deserialize)]
#[allow(non_snake_case, non_camel_case_types)]
pub struct UWF_RegistryFilter {
    #[serde(rename = "__Path")]
    pub path: String,
    #[allow(dead_code)]
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
