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

    /// R3 / R5 前置校验：全局筛选器当前未启用时不可执行 Overlay 提交或固化删除
    #[error("筛选器当前会话未启用，无法执行{what}（请先启用筛选器并重启后再试）")]
    FilterNotCurrentlyEnabled { what: &'static str },

    /// R3 前置校验：目标卷在当前会话未受 UWF 保护时禁止文件提交或物理删除固化
    #[error("卷 {0} 当前会话未受 UWF 保护，不可执行文件提交或固化删除操作")]
    VolumeNotProtected(String),

    #[error("未找到卷 {0} 对应的 UWF 实例")]
    VolumeNotFound(String),

    /// 同一会话同一盘符存在多个无法通过当前挂载卷消歧的 UWF 实例
    #[cfg_attr(not(windows), allow(dead_code))]
    #[error("卷 {0} 存在多个冲突的 UWF 实例且无法消歧")]
    VolumeAmbiguous(String),

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

#[cfg(any(windows, test))]
pub(crate) const WBEM_E_NOT_FOUND_CODE: u32 = 0x8004_1002;
#[cfg(any(windows, test))]
pub(crate) const WBEM_E_NOT_FOUND: i32 = WBEM_E_NOT_FOUND_CODE as i32;

/// WMI HRESULT 是否代表 UWF 命名空间或类未安装（连接命名空间或类不存在）。
///
/// 注意：`WBEM_E_NOT_FOUND` (`0x80041002`) 不应通过全局 `From<WMIError>` 泛化为未安装——
/// 在实例方法调用（如未保护卷 `GetExclusions`、叠加层外文件 `CommitFile`、不存在的排除项）
/// 中，它仅表示目标对象或记录不存在，而非 UWF 功能未安装。
#[cfg(any(windows, test))]
pub(crate) fn is_wmi_not_installed(hres: i32) -> bool {
    const WBEM_E_INVALID_NAMESPACE: i32 = 0x8004_100E_u32 as i32;
    const WBEM_E_INVALID_CLASS: i32 = 0x8004_1010_u32 as i32;
    const WBEM_E_PROVIDER_NOT_FOUND: i32 = 0x8004_1011_u32 as i32;
    matches!(
        hres,
        WBEM_E_INVALID_NAMESPACE | WBEM_E_INVALID_CLASS | WBEM_E_PROVIDER_NOT_FOUND
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
        assert!(is_wmi_not_installed(0x8004_100E_u32 as i32));
        assert!(is_wmi_not_installed(0x8004_1010_u32 as i32));
        assert!(is_wmi_not_installed(0x8004_1011_u32 as i32));
        assert!(!is_wmi_not_installed(WBEM_E_NOT_FOUND));
        assert_eq!(WBEM_E_NOT_FOUND, WBEM_E_NOT_FOUND_CODE as i32);
        assert!(!is_wmi_not_installed(0x8004_1003_u32 as i32)); // WBEM_E_ACCESS_DENIED
    }
}
