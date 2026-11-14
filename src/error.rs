//! 业务错误类型：全部 Display 为中文，面向最终用户可读。

use thiserror::Error;

pub type Result<T> = std::result::Result<T, UwfError>;

/// 管理员 PowerShell 启用 UWF 的命令（错误文案与 D6 引导页共用同一常量）
pub const UWF_ENABLE_CMD: &str =
    "Enable-WindowsOptionalFeature -Online -FeatureName Client-UnifiedWriteFilter";

#[derive(Debug, Error)]
pub enum UwfError {
    /// UWF 功能未安装或未启用（触发 D6 引导页）
    #[cfg_attr(not(windows), allow(dead_code))]
    #[error("UWF 功能未安装或未启用。请在管理员 PowerShell 中执行：{UWF_ENABLE_CMD}")]
    NotInstalled,

    /// WMI 方法返回非零 ReturnValue（uwfmgr 风格十六进制错误码）
    #[cfg_attr(not(windows), allow(dead_code))]
    #[error("WMI 方法 {method} 调用失败，错误码: 0x{code:08X}")]
    WmiMethod { method: &'static str, code: u32 },

    /// R1：筛选器当前启用时 Overlay 类型/最大容量锁定
    #[error("筛选器当前已启用，{what}不可修改（仅预警/临界阈值可改）")]
    FilterCurrentlyEnabled { what: &'static str },

    #[error("未找到卷 {0} 对应的 UWF 实例")]
    VolumeNotFound(String),

    /// R3 两步删除的部分成功：文件已删，但 WMI 固化删除失败
    #[cfg_attr(not(windows), allow(dead_code))]
    #[error("文件 {path} 已删除，但 UWF 固化删除失败：{message}")]
    CommitDeleteAfterFileRemoved { path: String, message: String },

    #[error("无效的路径: {0}")]
    InvalidPath(String),

    #[error("文件操作失败: {0}")]
    Io(#[from] std::io::Error),

    #[cfg(windows)]
    #[error("WMI 错误: {0}")]
    Wmi(Box<wmi::WMIError>),
}

/// WMI HRESULT 是否代表 UWF 命名空间或类未安装。
#[cfg(any(windows, test))]
pub(crate) fn is_wmi_not_installed(hres: i32) -> bool {
    const WBEM_E_NOT_FOUND: i32 = 0x8004_1002_u32 as i32;
    const WBEM_E_INVALID_NAMESPACE: i32 = 0x8004_100E_u32 as i32;
    const WBEM_E_INVALID_CLASS: i32 = 0x8004_1010_u32 as i32;
    matches!(
        hres,
        WBEM_E_NOT_FOUND | WBEM_E_INVALID_NAMESPACE | WBEM_E_INVALID_CLASS
    )
}

#[cfg(windows)]
impl From<wmi::WMIError> for UwfError {
    fn from(e: wmi::WMIError) -> Self {
        match &e {
            wmi::WMIError::HResultError { hres } if is_wmi_not_installed(*hres) => {
                UwfError::NotInstalled
            }
            _ => UwfError::Wmi(Box::new(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_missing_namespace_or_class_hresult_to_not_installed() {
        assert!(is_wmi_not_installed(0x8004_1002_u32 as i32));
        assert!(is_wmi_not_installed(0x8004_100E_u32 as i32));
        assert!(is_wmi_not_installed(0x8004_1010_u32 as i32));
        assert!(!is_wmi_not_installed(0x8004_1003_u32 as i32)); // WBEM_E_ACCESS_DENIED
    }
}
